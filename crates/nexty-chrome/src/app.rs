//! winit + wgpu 呈现：窗口、输入转译与 Pixmap 上屏。
//!
//! 本模块是无法在无头环境自动化测试的薄胶水层：窗口创建、surface 配置、
//! 纹理上传与 blit 呈现。所有可测试逻辑（管线、渲染线程、UI 状态机）
//! 都在其余模块并有单测覆盖。
//!
//! 呈现路径：页面 + 地址栏 → Scene → [`RenderThread`]（隔离线程 +
//! `catch_unwind`）→ Pixmap → `wgpu` 纹理 → 全屏 `textureLoad` blit →
//! surface present。像素 1:1 上屏，无缩放采样。

use std::sync::Arc;
use std::time::Duration;

use nexty_layout::Fragment;
use nexty_network::{Method, NetworkFetcher, ReqwestFetcher};
use nexty_paint::{Pixmap, Scene, Size};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::history::History;
use crate::pipeline::{self, Page};
use crate::render::RenderThread;
use crate::ui::{AddressBar, BAR_HEIGHT, BarCommand, UiEvent};

/// 滚轮一格（LineDelta 1.0）对应的滚动距离，px。
const WHEEL_LINE_PX: f32 = 40.0;
/// 滚动条轨道宽度，px。
const SCROLLBAR_WIDTH: f32 = 8.0;
/// 滚动条 thumb 最小高度，px。
const SCROLLBAR_MIN_THUMB: f32 = 24.0;

/// 最大滚动量：内容总高（页面 + 工具带）超出视口的部分，下限 0。
fn max_scroll(content_height: f32, bar_height: f32, viewport_height: f32) -> f32 {
    (content_height + bar_height - viewport_height).max(0.0)
}

/// 把滚动量限制到 `[0, max]`。
fn clamp_scroll(scroll: f32, max: f32) -> f32 {
    scroll.clamp(0.0, max)
}

/// 滚动条 thumb 矩形（视口坐标）：页面不溢出时 `None`。
///
/// thumb 高 = 轨道高 × 视口占比，不低于 [`SCROLLBAR_MIN_THUMB`]；y 按
/// 滚动比例落在轨道内。
fn scrollbar_thumb(
    scroll: f32,
    viewport_height: f32,
    content_height: f32,
    bar_height: f32,
    track_width: f32,
) -> Option<nexty_paint::Rect> {
    let total = content_height + bar_height;
    if total <= viewport_height {
        return None;
    }
    let track = viewport_height - bar_height;
    let thumb_height = (track * viewport_height / total).max(SCROLLBAR_MIN_THUMB);
    let progress = scroll / (total - viewport_height);
    Some(nexty_paint::Rect {
        x: 0.0,
        y: bar_height + (track - thumb_height) * progress,
        width: track_width,
        height: thumb_height,
    })
}

/// 应用错误。
#[derive(Debug)]
pub enum AppError {
    /// 窗口或事件循环初始化失败。
    Window(String),
    /// GPU / surface 初始化失败。
    Gpu(String),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::Window(message) => write!(f, "window error: {message}"),
            AppError::Gpu(message) => write!(f, "gpu error: {message}"),
        }
    }
}

impl std::error::Error for AppError {}

/// 后台抓取完成后经事件代理送回的载荷。
enum BackgroundEvent {
    /// 页面（含子资源：外链 CSS + 解码图片）就绪。
    PageLoaded {
        url: String,
        /// 页面构建结果；错误携带用户可读信息。
        result: Result<Page, String>,
    },
}

/// 启动浏览器窗口。阻塞直至窗口关闭。
///
/// # Errors
///
/// 事件循环或 GPU 初始化失败时返回 [`AppError`]。
pub fn run() -> Result<(), AppError> {
    let event_loop = EventLoop::<BackgroundEvent>::with_user_event()
        .build()
        .map_err(|error| AppError::Window(error.to_string()))?;
    let proxy = event_loop.create_proxy();
    let mut app = BrowserApp::new(proxy);
    event_loop
        .run_app(&mut app)
        .map_err(|error| AppError::Window(error.to_string()))
}

/// wgpu 资源集合（surface + blit 管线）。
struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_format: wgpu::TextureFormat,
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline_rgba: wgpu::RenderPipeline,
    pipeline_bgra: wgpu::RenderPipeline,
    texture: Option<(wgpu::Texture, [u32; 2])>,
}

/// 浏览器应用状态。
struct BrowserApp {
    proxy: EventLoopProxy<BackgroundEvent>,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    renderer: Option<RenderThread>,
    /// 网络接驳缝：后端可整体替换（reqwest 实现或其他 `NetworkFetcher`）。
    fetcher: Option<Arc<dyn NetworkFetcher>>,
    shaper: nexty_text::ParleyTextShaper,
    bar: AddressBar,
    history: History,
    /// Alt 修饰键按住状态（后退/前进快捷键）。
    alt_down: bool,
    page: Option<Page>,
    /// 当前页面布局好的根片段（滚动时按偏移重建显示列表）。
    root: Option<Fragment>,
    /// 根片段内容总高，px（滚动条与 clamp 的依据）。
    content_height: f32,
    /// 页面滚动量，px ∈ [0, max_scroll]。
    scroll_y: f32,
    cursor: (f32, f32),
}

impl BrowserApp {
    fn new(proxy: EventLoopProxy<BackgroundEvent>) -> Self {
        Self {
            proxy,
            window: None,
            gpu: None,
            renderer: None,
            fetcher: None,
            shaper: nexty_text::ParleyTextShaper::new(),
            bar: AddressBar::new("about:blank"),
            history: History::new("about:blank"),
            alt_down: false,
            page: None,
            root: None,
            content_height: 0.0,
            scroll_y: 0.0,
            cursor: (0.0, 0.0),
        }
    }

    /// 窗口创建后的初始化：GPU、渲染线程、默认页。
    fn initialize(&mut self, window: Arc<Window>) {
        // 窗口句柄必须先存下来：redraw / resize / 事件处理都依赖它，
        // 缺失会让 redraw 直接 return，窗口永远停在未呈现的空白 surface。
        self.window = Some(window.clone());
        let gpu = match Gpu::new(window.clone()) {
            Ok(gpu) => gpu,
            Err(message) => {
                eprintln!("gpu init failed: {message}");
                return;
            }
        };
        self.gpu = Some(gpu);
        self.renderer = Some(RenderThread::spawn(Arc::new(
            nexty_paint::VelloCpuRasterizer::new(),
        )));
        // 启动路径用 fail-fast：TLS 后端不可用时浏览器本就无法工作，
        // 带着半初始化的状态继续跑不如立刻退出（渲染路径才需要降级）。
        // 整请求超时 30s：真实站点挂起时不无限占住加载状态。
        self.fetcher = Some(Arc::new(
            ReqwestFetcher::with_timeout(Duration::from_secs(30)).expect("network fetcher"),
        ));
        self.show_blank();
        window.request_redraw();
    }

    /// 显示空白页（`about:blank` 等内部 scheme 走这里，不经网络）。
    fn show_blank(&mut self) {
        self.bar.set_url("about:blank");
        self.set_page(pipeline::load_page("", ""), true);
    }

    /// 把已解析页面按当前视口布局。
    ///
    /// `reset_scroll` 为 false 时保留滚动量（resize 重排场景），但按新
    /// 视口重新 clamp。
    fn set_page(&mut self, page: Page, reset_scroll: bool) {
        let (viewport_width, viewport_height) = self
            .window
            .as_ref()
            .map(|window| {
                let size = window.inner_size();
                (size.width as f32, size.height as f32)
            })
            .unwrap_or((800.0, 600.0));
        let root = pipeline::layout_page(&page, &self.shaper, viewport_width);
        self.content_height = root
            .as_ref()
            .map(|root| root.border_box.height)
            .unwrap_or(0.0);
        self.root = root;
        self.page = Some(page);
        self.scroll_y = if reset_scroll {
            0.0
        } else {
            clamp_scroll(
                self.scroll_y,
                max_scroll(self.content_height, BAR_HEIGHT, viewport_height),
            )
        };
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// 滚动 `delta` px（正值向下看更多内容），并按当前视口 clamp。
    fn scroll_by(&mut self, delta: f32) {
        let Some(window) = &self.window else {
            return;
        };
        let viewport_height = window.inner_size().height as f32;
        let max = max_scroll(self.content_height, BAR_HEIGHT, viewport_height);
        self.scroll_y = clamp_scroll(self.scroll_y + delta, max);
        window.request_redraw();
    }

    /// 页面区域点击：命中测试 → 最近 `<a href>` → 相对地址归一 → 导航。
    ///
    /// 未命中链接（点空白/非锚元素）时不动。
    fn open_link_at(&mut self, x: f32, y: f32) {
        let page_y = y - BAR_HEIGHT + self.scroll_y;
        let target = self
            .root
            .as_ref()
            .and_then(|root| pipeline::hit_test(root, x, page_y))
            .and_then(|node| {
                self.page
                    .as_ref()
                    .and_then(|page| pipeline::link_target(page, node))
            })
            .and_then(|href| nexty_network::resolve(self.history.current(), &href).ok());
        if let Some(url) = target {
            self.navigate(&url);
        }
    }

    /// 导航：入历史栈并加载。
    fn navigate(&mut self, url: &str) {
        self.history.push(url.to_owned());
        self.sync_history_flags();
        self.load(url);
    }

    /// 后退/前进（历史栈搬运后按目标 URL 加载，不再入栈）。
    fn go_back(&mut self) {
        if let Some(url) = self.history.go_back() {
            self.sync_history_flags();
            self.load(&url);
        }
    }

    fn go_forward(&mut self) {
        if let Some(url) = self.history.go_forward() {
            self.sync_history_flags();
            self.load(&url);
        }
    }

    /// 刷新：重载当前条目，不产生新历史。
    fn reload(&mut self) {
        let url = self.history.current().to_owned();
        self.load(&url);
    }

    /// 把历史栈可用性同步到工具带按钮。
    fn sync_history_flags(&mut self) {
        self.bar
            .set_history(self.history.can_back(), self.history.can_forward());
    }

    /// 加载：更新地址栏并后台抓取（不改历史栈）。
    fn load(&mut self, url: &str) {
        self.bar.set_url(url.to_owned());
        self.bar.set_loading(true);

        // 内部 scheme（about:blank 等）不由网络抓取，直接生成本地空白页。
        // 交给 NetworkFetcher 会被协议白名单判为 InvalidUrl，白屏。
        if !url.starts_with("http://") && !url.starts_with("https://") {
            self.show_blank();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        let Some(fetcher) = self.fetcher.clone() else {
            return;
        };
        let Some(proxy) = Some(self.proxy.clone()) else {
            return;
        };
        let url = url.to_owned();
        let target = url.clone();
        std::thread::Builder::new()
            .name("nexty-fetch".to_owned())
            .spawn(move || {
                let request = nexty_network::Request {
                    url: target.clone(),
                    method: Method::Get,
                    // 不少真实站点对无 UA 请求直接 403/429
                    headers: vec![(
                        "User-Agent".to_owned(),
                        format!("Nexty/{}", env!("CARGO_PKG_VERSION")),
                    )],
                };
                // 全部加载在后台线程完成：文档抓取 → 解析 → 子资源并发抓取
                // → 级联 → 页面就绪。主线程只做布局与呈现。
                let result = match fetcher.fetch(&request) {
                    Ok(response) if (200..300).contains(&response.status) => {
                        match String::from_utf8(response.body) {
                            Ok(html) => {
                                let document = nexty_html::parse_document(
                                    &html,
                                    nexty_html::ParseOptions::default(),
                                );
                                let resources = crate::resources::fetch_subresources(
                                    fetcher.as_ref(),
                                    &url,
                                    &document,
                                );
                                Ok(pipeline::build_page(
                                    document,
                                    resources.stylesheets,
                                    resources.images,
                                ))
                            }
                            Err(error) => Err(error.to_string()),
                        }
                    }
                    Ok(response) => Err(format!("HTTP {}", response.status)),
                    Err(error) => Err(format!("{error:?}")),
                };
                let _ = proxy.send_event(BackgroundEvent::PageLoaded { url, result });
            })
            .expect("spawn fetch thread");
    }

    /// 抓取完成：重建页面与显示列表。
    fn page_loaded(&mut self, url: String, result: Result<Page, String>) {
        self.bar.set_loading(false);
        match result {
            Ok(page) => {
                let title = pipeline::document_title(&page.document);
                self.set_page(page, true);
                self.bar.set_url(url);
                if let Some(window) = &self.window {
                    match title {
                        Some(title) => window.set_title(&format!("{title} - Nexty")),
                        None => window.set_title("Nexty"),
                    }
                }
            }
            Err(error) => {
                eprintln!("load {url} failed: {error}");
                // 失败也要出错误页：只打日志会让画面停在上一帧或空白，
                // 用户看不到任何反馈。
                let html = format!(
                    "<body style=\"font-family: sans-serif; padding: 24px\">\
                     <h1 style=\"color: #c5221f\">无法加载此页</h1>\
                     <p>{url}</p>\
                     <p style=\"color: #5f6368\">{error}</p></body>"
                );
                self.set_page(pipeline::load_page(&html, ""), true);
            }
        }
    }

    /// 一帧：合成显示列表并渲染上屏。
    fn redraw(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let Some(gpu) = &mut self.gpu else {
            return;
        };
        let size = window.inner_size();
        let viewport = Size {
            width: size.width,
            height: size.height,
        };
        if viewport.width == 0 || viewport.height == 0 {
            return;
        }
        let (width, height) = (viewport.width as f32, viewport.height as f32);

        // 页面按滚动偏移重建显示列表（内容从工具带下方开始）
        let mut scene = match (&self.page, &self.root) {
            (Some(page), Some(root)) => {
                pipeline::build_scene_at(page, root, 0.0, BAR_HEIGHT - self.scroll_y)
            }
            _ => Scene::default(),
        };
        // 画布底色：pipeline 的画布保持透明，直接 blit 会把透明像素交给
        // surface（alpha_mode Auto），在部分后端表现为黑屏/花屏。先铺不透明白底。
        scene.commands.insert(
            0,
            nexty_paint::Command::FillRect {
                rect: nexty_paint::Rect {
                    x: 0.0,
                    y: 0.0,
                    width,
                    height,
                },
                color: nexty_paint::Color::opaque(0xff, 0xff, 0xff),
            },
        );
        // 滚动条：仅指示（不可拖，本轮偏差）
        if let Some(thumb) = scrollbar_thumb(
            self.scroll_y,
            height,
            self.content_height,
            BAR_HEIGHT,
            SCROLLBAR_WIDTH,
        ) {
            scene.commands.push(nexty_paint::Command::FillRect {
                rect: nexty_paint::Rect {
                    x: width - SCROLLBAR_WIDTH,
                    y: BAR_HEIGHT,
                    width: SCROLLBAR_WIDTH,
                    height: height - BAR_HEIGHT,
                },
                color: nexty_paint::Color::opaque(0xe2, 0xe2, 0xe2),
            });
            scene.commands.push(nexty_paint::Command::FillRect {
                rect: nexty_paint::Rect {
                    x: width - SCROLLBAR_WIDTH,
                    ..thumb
                },
                color: nexty_paint::Color::opaque(0xb4, 0xb4, 0xb4),
            });
        }
        self.bar.draw(&self.shaper, &mut scene, width);

        let Some(renderer) = &self.renderer else {
            return;
        };
        match renderer.render(&scene, viewport) {
            Ok(pixmap) => gpu.present(&pixmap, window),
            Err(error) => {
                // 渲染失败不拖垮主进程：清屏为灰色并上报
                eprintln!("render failed: {error}");
                gpu.clear(window, [0.6, 0.6, 0.6, 1.0]);
            }
        }
    }
}

impl ApplicationHandler<BackgroundEvent> for BrowserApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title("Nexty")
            .with_inner_size(winit::dpi::LogicalSize::new(1000.0, 640.0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("window creation failed: {error}");
                event_loop.exit();
                return;
            }
        };
        self.initialize(window);
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: BackgroundEvent) {
        let BackgroundEvent::PageLoaded { url, result } = event;
        self.page_loaded(url, result);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                }
                // 按新视口宽重排（保留滚动量，set_page 内重新 clamp）
                if let Some(page) = self.page.take() {
                    self.set_page(page, false);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // 滚轮向上（y > 0）→ 减小滚动量
                let amount = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * WHEEL_LINE_PX,
                    MouseScrollDelta::PixelDelta(position) => position.y as f32,
                };
                self.scroll_by(-amount);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let (x, y) = self.cursor;
                let action = self.bar.handle(UiEvent::Click { x, y });
                match action.command {
                    Some(BarCommand::Back) => self.go_back(),
                    Some(BarCommand::Forward) => self.go_forward(),
                    Some(BarCommand::Reload) => self.reload(),
                    None => {
                        if let Some(url) = action.navigate {
                            self.navigate(&url);
                        } else if y > BAR_HEIGHT {
                            // 页面区域：链接命中导航
                            self.open_link_at(x, y);
                        }
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.alt_down = modifiers.state().alt_key();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                // Alt+方向键：后退/前进（优先于地址栏输入路由）
                if self.alt_down {
                    match event.logical_key {
                        Key::Named(NamedKey::ArrowLeft) => {
                            self.go_back();
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                            return;
                        }
                        Key::Named(NamedKey::ArrowRight) => {
                            self.go_forward();
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                            return;
                        }
                        _ => {}
                    }
                }
                // 地址栏未聚焦时，翻页/滚动键作用于页面
                if !self.bar.is_focused() {
                    let viewport_height = self
                        .window
                        .as_ref()
                        .map(|window| window.inner_size().height as f32)
                        .unwrap_or(0.0);
                    let page_step = viewport_height * 0.9;
                    match event.logical_key {
                        Key::Named(NamedKey::PageDown) => {
                            self.scroll_by(page_step);
                        }
                        Key::Named(NamedKey::PageUp) => {
                            self.scroll_by(-page_step);
                        }
                        Key::Named(NamedKey::ArrowDown) => {
                            self.scroll_by(WHEEL_LINE_PX);
                        }
                        Key::Named(NamedKey::ArrowUp) => {
                            self.scroll_by(-WHEEL_LINE_PX);
                        }
                        Key::Named(NamedKey::Home) => {
                            self.scroll_by(-self.scroll_y);
                        }
                        Key::Named(NamedKey::End) => {
                            let max = max_scroll(self.content_height, BAR_HEIGHT, viewport_height);
                            self.scroll_by(max - self.scroll_y);
                        }
                        _ => {}
                    }
                }
                if let Some(text) = &event.text {
                    for character in text.chars() {
                        let _ = self.bar.handle(UiEvent::Character(character));
                    }
                }
                match event.logical_key {
                    Key::Named(NamedKey::Backspace) => {
                        let _ = self.bar.handle(UiEvent::Backspace);
                    }
                    Key::Named(NamedKey::Enter) => {
                        let action = self.bar.handle(UiEvent::Submit);
                        if let Some(url) = action.navigate {
                            self.navigate(&url);
                        }
                    }
                    _ => {}
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

impl Gpu {
    /// 初始化 wgpu 资源并配置 surface。
    fn new(window: Arc<Window>) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(window.clone())
            .map_err(|error| error.to_string())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .map_err(|error| error.to_string())?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("nexty-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        }))
        .map_err(|error| error.to_string())?;

        let capabilities = surface.get_capabilities(&adapter);
        // 优先 Rgba8Unorm（与 Pixmap 字节序一致），否则 Bgra8Unorm + 着色器换色
        let surface_format = if capabilities
            .formats
            .contains(&wgpu::TextureFormat::Rgba8Unorm)
        {
            wgpu::TextureFormat::Rgba8Unorm
        } else {
            capabilities.formats[0]
        };
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("nexty-blit"),
            source: wgpu::ShaderSource::Wgsl(blit_shader().into()),
        });
        let bind_group_layout = blit_bind_group_layout(&device);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("nexty-blit-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        // 两个变体：直出 RGBA / 换色 BGRA
        let make_pipeline = |entry: &'static str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("nexty-blit-pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline_rgba = make_pipeline("fs_main");
        let pipeline_bgra = make_pipeline("fs_main_bgra");

        Ok(Self {
            device,
            queue,
            surface,
            surface_format,
            bind_group_layout,
            pipeline_rgba,
            pipeline_bgra,
            texture: None,
        })
    }

    /// 视口尺寸变化：重配 surface 并丢弃纹理。
    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: self.surface_format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        self.surface.configure(&self.device, &config);
        self.texture = None;
    }

    /// 上传 Pixmap 并 blit 到 surface。
    fn present(&mut self, pixmap: &Pixmap, _window: &Window) {
        let [width, height] = [pixmap.size.width, pixmap.size.height];
        if width == 0 || height == 0 {
            return;
        }
        // 尺寸变化时重建纹理
        let texture = match &self.texture {
            Some((texture, size)) if *size == [width, height] => texture.clone(),
            _ => {
                let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("nexty-page"),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let texture_clone = texture.clone();
                self.texture = Some((texture, [width, height]));
                texture_clone
            }
        };
        // WebGPU 要求 bytes_per_row 为 256 的倍数。Pixmap 是紧凑行优先
        // （4 * width），宽度非 64 的倍数时（如 800 → 3200）不满足对齐，
        // write_texture 会校验失败并 panic 主进程。故先把行尾padding 到
        // 256 的倍数再上传。
        let unpadded = 4 * width;
        let padded = unpadded.div_ceil(256) * 256;
        if padded == unpadded {
            self.upload_aligned(&texture, &pixmap.data, unpadded, width, height);
        } else {
            let staging = pad_rows(&pixmap.data, width, height, padded);
            self.upload_aligned(&texture, &staging, padded, width, height);
        }

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = self
            .device
            .create_sampler(&wgpu::SamplerDescriptor::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nexty-blit-bind"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        // get_current_texture 返回枚举：成功/亚优取帧，其余跳帧
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            status @ (wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Outdated) => {
                eprintln!("surface skipped: {status:?}");
                return;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                eprintln!("surface acquire validation error");
                return;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                eprintln!("surface lost");
                return;
            }
        };
        let frame_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("nexty-frame"),
            });
        let pipeline = if self.surface_format == wgpu::TextureFormat::Bgra8Unorm {
            &self.pipeline_bgra
        } else {
            &self.pipeline_rgba
        };
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nexty-present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &frame_view,
                    resolve_target: None,
                    ops: wgpu::Operations::default(),
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        frame.present();
    }

    /// 按给定的行跨度把像素上传到纹理。
    ///
    /// `bytes_per_row` 必须是 256 的倍数（WebGPU 硬要求），调用方负责
    /// padding。见 [`pad_rows`]。
    fn upload_aligned(
        &self,
        texture: &wgpu::Texture,
        data: &[u8],
        bytes_per_row: u32,
        width: u32,
        height: u32,
    ) {
        self.queue.write_texture(
            texture.as_image_copy(),
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// 渲染失败时的兜底清屏。
    fn clear(&mut self, _window: &Window, color: [f64; 4]) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            _ => return,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("nexty-clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: color[0],
                            g: color[1],
                            b: color[2],
                            a: color[3],
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        self.queue.submit([encoder.finish()]);
        frame.present();
    }
}

/// 把紧凑行优先的像素数据按行padding 到 256 字节对齐。
///
/// WebGPU 的 `write_texture` 要求 `bytes_per_row` 为 256 的倍数，而 Pixmap
/// 的行跨度是 `4 * width`。本函数为每行尾部补零，使行跨度变为
/// `padded`。padding 字节的内容无关紧要（着色器只读实际宽度内的像素），
/// 但必须是**每行独立补齐**而非只在末尾补一次。
fn pad_rows(data: &[u8], width: u32, height: u32, padded: u32) -> Vec<u8> {
    let unpadded = 4 * width;
    let mut out = Vec::with_capacity(padded as usize * height as usize);
    for y in 0..height as usize {
        let start = y * unpadded as usize;
        let end = start + unpadded as usize;
        // 数据不足时补零而非 panic：Pixmap 契约保证长度，但越界不该崩主进程
        if start < data.len() {
            let available = (end - start).min(data.len() - start);
            out.extend_from_slice(&data[start..start + available]);
            out.resize(out.len() + (unpadded as usize - available), 0);
        } else {
            out.resize(out.len() + unpadded as usize, 0);
        }
        // 行尾补到 256 对齐
        out.resize(y * padded as usize + padded as usize, 0);
    }
    out
}

fn blit_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("nexty-blit-bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                count: None,
            },
        ],
    })
}

/// 全屏三角形顶点（NDC）。
///
/// 必须是**大**三角形：斜边要越过 `(1.0, 1.0)`，整体包住 NDC 方形
/// `[-1, 1]²`。用刚好等于方形的三角形会切掉对角，屏幕上表现为一块斜切的
/// 未绘制区域（该区域保留上次内容或呈黑色）。
const FULLSCREEN_TRIANGLE: [[f32; 2]; 3] = [[-1.0, -1.0], [3.0, -1.0], [-1.0, 3.0]];

/// blit 着色器源码。
///
/// 顶点位置由 [`FULLSCREEN_TRIANGLE`] 生成，避免手写坐标时把大三角形
/// 缩成小三角形。片元用 `textureLoad` 像素直读（1:1，不采样），坐标按
/// 纹理尺寸 clamp，越界时取边缘像素而不是返回未定义值。
fn blit_shader() -> String {
    let [a, b, c] = FULLSCREEN_TRIANGLE;
    format!(
        "
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {{
    var positions = array<vec2<f32>, 3>(
        vec2<f32>({}, {}),
        vec2<f32>({}, {}),
        vec2<f32>({}, {}),
    );
    return vec4<f32>(positions[index], 0.0, 1.0);
}}

@group(0) @binding(0) var page_texture: texture_2d<f32>;
@group(0) @binding(1) var page_sampler: sampler;

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {{
    let dims = vec2<i32>(textureDimensions(page_texture, 0));
    let coords = clamp(vec2<i32>(position.xy), vec2<i32>(0), dims - vec2<i32>(1));
    return textureLoad(page_texture, coords, 0);
}}

@fragment
fn fs_main_bgra(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {{
    let dims = vec2<i32>(textureDimensions(page_texture, 0));
    let coords = clamp(vec2<i32>(position.xy), vec2<i32>(0), dims - vec2<i32>(1));
    let color = textureLoad(page_texture, coords, 0);
    return vec4<f32>(color.b, color.g, color.r, color.a);
}}
",
        a[0], a[1], b[0], b[1], c[0], c[1],
    )
}

#[cfg(test)]
mod tests {
    use super::{
        BAR_HEIGHT, FULLSCREEN_TRIANGLE, clamp_scroll, max_scroll, pad_rows, scrollbar_thumb,
    };

    /// 最大滚动量：不足视口为 0，超出为差值（含工具带高度）。
    #[test]
    fn max_scroll_is_overflow_above_viewport() {
        assert_eq!(max_scroll(0.0, BAR_HEIGHT, 600.0), 0.0);
        assert_eq!(max_scroll(564.0, BAR_HEIGHT, 600.0), 0.0, "恰好填满视口");
        assert_eq!(max_scroll(1064.0, BAR_HEIGHT, 600.0), 500.0);
        assert_eq!(max_scroll(2000.0, BAR_HEIGHT, 100.0), 1936.0);
    }

    /// 滚动量 clamp 到 [0, max]。
    #[test]
    fn clamp_scroll_bounds() {
        assert_eq!(clamp_scroll(-50.0, 100.0), 0.0);
        assert_eq!(clamp_scroll(42.0, 100.0), 42.0);
        assert_eq!(clamp_scroll(150.0, 100.0), 100.0);
        assert_eq!(clamp_scroll(5.0, 0.0), 0.0, "无溢出时滚动量归零");
    }

    /// thumb 几何：零溢出无 thumb；半溢出 thumb 占轨道一半（受最小高度约束）。
    #[test]
    fn scrollbar_thumb_geometry() {
        // 不溢出：None
        assert!(scrollbar_thumb(0.0, 600.0, 100.0, BAR_HEIGHT, 8.0).is_none());
        assert!(scrollbar_thumb(0.0, 600.0, 564.0, BAR_HEIGHT, 8.0).is_none());

        // 溢出 500（总高 1100）：thumb 高 = 564 × 600/1100 ≈ 307.6（> 24 下限），
        // 顶部对齐轨道顶
        let thumb = scrollbar_thumb(0.0, 600.0, 1064.0, BAR_HEIGHT, 8.0).expect("thumb");
        assert_eq!(thumb.width, 8.0);
        let expected_height = (600.0 - BAR_HEIGHT) * 600.0 / (1064.0 + BAR_HEIGHT);
        assert!((thumb.height - expected_height).abs() < 0.01);
        assert!(
            (thumb.y - BAR_HEIGHT).abs() < 0.01,
            "scroll=0 时 thumb 贴轨道顶"
        );

        // 滚到底：thumb 贴轨道底
        let thumb = scrollbar_thumb(500.0, 600.0, 1064.0, BAR_HEIGHT, 8.0).expect("thumb");
        assert!((thumb.y + thumb.height - 600.0).abs() < 0.01);

        // 超长文档：thumb 触发 24px 下限
        let thumb = scrollbar_thumb(0.0, 600.0, 100000.0, BAR_HEIGHT, 8.0).expect("thumb");
        assert_eq!(thumb.height, 24.0);
        assert!((thumb.y - BAR_HEIGHT).abs() < 0.01);
    }

    /// 点是否在三角形内（叉积符号一致法）。
    fn in_triangle(points: [[f32; 2]; 3], p: [f32; 2]) -> bool {
        let sign = |a: [f32; 2], b: [f32; 2]| {
            (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
        };
        let [p0, p1, p2] = points;
        let (d0, d1, d2) = (sign(p0, p1), sign(p1, p2), sign(p2, p0));
        let has_neg = d0 < 0.0 || d1 < 0.0 || d2 < 0.0;
        let has_pos = d0 > 0.0 || d1 > 0.0 || d2 > 0.0;
        !(has_neg && has_pos)
    }

    /// 全屏三角形必须覆盖 NDC 方形的四个角——漏掉任一角就是斜切黑块。
    #[test]
    fn fullscreen_triangle_covers_all_ndc_corners() {
        for corner in [[-1.0, -1.0], [1.0, -1.0], [-1.0, 1.0], [1.0, 1.0]] {
            assert!(
                in_triangle(FULLSCREEN_TRIANGLE, corner),
                "三角形未覆盖 NDC 角{corner:?}，会把该区域漏成未绘制"
            );
        }
    }

    /// 三角形还须覆盖 NDC 中心与各边中点，确保没有撕裂带。
    #[test]
    fn fullscreen_triangle_covers_center_and_edge_midpoints() {
        for point in [[0.0, 0.0], [0.0, -1.0], [0.0, 1.0], [-1.0, 0.0], [1.0, 0.0]] {
            assert!(
                in_triangle(FULLSCREEN_TRIANGLE, point),
                "三角形未覆盖 {point:?}"
            );
        }
    }

    /// 每行 padding 后长度必须是 256 的倍数，且像素内容不变。
    #[test]
    fn pad_rows_aligns_each_row_to_256_bytes() {
        for width in [1u32, 63, 64, 100, 200, 800, 960, 1000, 1366] {
            let height = 3u32;
            let unpadded = 4 * width;
            let padded = unpadded.div_ceil(256) * 256;
            // 关键前提：padded 必须是 256 的倍数（否则修复无意义）
            assert_eq!(
                padded % 256,
                0,
                "width={width} 的 padded 行跨度应为 256倍数"
            );

            // 构造每行首字节递增的像素，便于验证行序不错乱
            let mut data = vec![0u8; (unpadded * height) as usize];
            for y in 0..height as usize {
                for x in 0..width as usize {
                    let i = y * unpadded as usize + x * 4;
                    data[i] = (y * 10) as u8;
                    data[i + 1] = x as u8;
                }
            }

            let padded_data = pad_rows(&data, width, height, padded);
            assert_eq!(
                padded_data.len(),
                (padded * height) as usize,
                "width={width} 的 padding 后总长度"
            );

            // 逐行校验：本行像素必须与原始一致，下一行起点正确
            for y in 0..height as usize {
                let dst = y * padded as usize;
                for x in 0..width as usize {
                    assert_eq!(
                        padded_data[dst + x * 4],
                        (y * 10) as u8,
                        "width={width} 行{y} 列{x} 的 R通道"
                    );
                    assert_eq!(
                        padded_data[dst + x * 4 + 1],
                        x as u8,
                        "width={width} 行{y} 列{x} 的 G 通道"
                    );
                }
                // 行尾padding 应为 0
                if padded > unpadded {
                    assert!(
                        padded_data[dst + unpadded as usize..dst + padded as usize]
                            .iter()
                            .all(|byte| *byte == 0),
                        "width={width} 行{y} 的 padding 应为 0"
                    );
                }
            }
        }
    }

    #[test]
    fn pad_rows_tolerates_short_input() {
        // 数据长度不足：补零而非 panic
        let padded = pad_rows(&[1, 2, 3], 800, 3, 3328);
        assert_eq!(padded.len(), 3328 * 3);
    }
}
