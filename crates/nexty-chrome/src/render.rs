//! 渲染隔离线程：光栅化在独立线程执行，panic 由 `catch_unwind` 兜底。
//!
//! 对应 AGENTS.md 硬规则「渲染隔离」：vello 系后端对未支持特性是 panic
//! 而非返回错误，渲染失败不得拖垮主进程。本模块保证：
//! 1. 光栅化只发生在专用线程（主线程只做请求/应答）；
//! 2. 单次渲染 panic 被捕获并转为 [`RenderError::Panicked`]，
//!    线程存活、后续请求照常服务；
//! 3. 渲染线程持有的 Rasterizer 由调用方注入（CPU 后端或未来的 GPU 适配）。

use std::sync::Arc;
use std::thread::JoinHandle;

use nexty_paint::{Pixmap, RasterError, Rasterizer, Scene, Size};

/// 渲染错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    /// 光栅后端返回的错误。
    Raster(RasterError),
    /// 光栅过程 panic（payload 已转为字符串），本次渲染失败但线程存活。
    Panicked(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::Raster(error) => write!(f, "raster error: {error:?}"),
            RenderError::Panicked(message) => write!(f, "renderer panicked: {message}"),
        }
    }
}

impl std::error::Error for RenderError {}

type Job = (
    Scene,
    Size,
    std::sync::mpsc::Sender<Result<Pixmap, RenderError>>,
);

/// 渲染线程句柄。Drop 时等待线程退出。
pub struct RenderThread {
    sender: Option<std::sync::mpsc::Sender<Job>>,
    handle: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for RenderThread {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderThread").finish()
    }
}

impl RenderThread {
    /// 启动渲染线程。
    ///
    /// # Panics
    ///
    /// 线程启动失败时 panic（主进程随后的渲染请求会得到通道错误）。
    #[must_use]
    pub fn spawn(rasterizer: Arc<dyn Rasterizer + Send + Sync>) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel::<Job>();
        let handle = std::thread::Builder::new()
            .name("nexty-render".to_owned())
            .spawn(move || {
                while let Ok((scene, size, reply)) = receiver.recv() {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        rasterizer.rasterize(&scene, size)
                    }))
                    .map_err(panic_message)
                    .and_then(|result| result.map_err(RenderError::Raster));
                    let _ = reply.send(result);
                }
            })
            .expect("spawn render thread");
        Self {
            sender: Some(sender),
            handle: Some(handle),
        }
    }

    /// 请求一帧光栅化，阻塞等待结果。
    ///
    /// # Errors
    ///
    /// 光栅失败或渲染线程 panic 时返回 [`RenderError`]；通道断裂
    /// （线程意外退出）也归入 [`RenderError::Panicked`]。
    pub fn render(&self, scene: &Scene, size: Size) -> Result<Pixmap, RenderError> {
        let (reply_sender, reply_receiver) = std::sync::mpsc::channel();
        self.sender
            .as_ref()
            .expect("render thread sender")
            .send((scene.clone(), size, reply_sender))
            .map_err(|_| RenderError::Panicked("render thread terminated".to_owned()))?;
        reply_receiver
            .recv()
            .map_err(|_| RenderError::Panicked("render thread terminated".to_owned()))?
    }
}

impl Drop for RenderThread {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 从 panic payload 提取可读信息。
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> RenderError {
    let message = if let Some(text) = payload.downcast_ref::<&'static str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "unknown panic".to_owned()
    };
    RenderError::Panicked(message)
}
