//! winit + wgpu 呈现：窗口、输入转译与 Pixmap 上屏。
//!
//! 本模块的 winit/GPU 胶水是无法在无头环境自动化测试的薄层：窗口创建、
//! surface 配置、纹理上传与 blit 呈现。所有可测试逻辑（管线、渲染线程、
//! UI 状态机）都在其余模块并有单测覆盖；交互事件接线与生命周期派发集中在
//! 子模块 [`events`]（纯函数部分有无头单测，K1 T3）。
//!
//! 呈现路径：页面 + 地址栏 → Scene → [`RenderThread`]（隔离线程 +
//! `catch_unwind`）→ Pixmap → `wgpu` 纹理 → 全屏 `textureLoad` blit →
//! surface present。像素 1:1 上屏，无缩放采样。

use std::sync::Arc;
use std::time::{Duration, Instant};

use nexty_dom::{DeltaMode, NodeId};
use nexty_layout::{Fragment, TextMap};
use nexty_network::{CachedFetcher, NetworkFetcher, ReqwestFetcher};
use nexty_paint::{Pixmap, Scene, Size};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::history::History;
use crate::pipeline::{self, HoverKind, Page};
use crate::render::{RenderError, RenderThread};
use crate::scrollbar::{self, ScrollGeometry, Scrollbar, ScrollbarHit};
use crate::selection::{self, Selection};
use crate::ui::{self, AddressBar, BAR_HEIGHT, BarCommand, CaretDirection, UiEvent};

mod events;

/// 滚轮一格（LineDelta 1.0）对应的滚动距离，px。
const WHEEL_LINE_PX: f32 = 40.0;

/// 双击判定窗口。
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

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
    /// 抓取数据面（HTML + 子资源字节/像素）就绪。载荷装箱（与单元变体
    /// 并列时避免枚举整体膨胀）。
    PageLoaded(Box<PageLoadedPayload>),
    /// 字体字节就绪（K4 T3）：注册进会话注册表，有新注册触发恰好一轮
    /// 重排 + 重绘。
    FontsReady(Box<FontsReadyPayload>),
    /// 渲染线程完成一份光栅作业（异步呈现：主线程发请求即返回，不阻塞
    /// 等待——巨页帧可达数十秒，阻塞会让窗口进入系统「无响应」状态并
    /// 触发 WM_PAINT/WM_SIZE 重绘风暴）。
    FrameReady,
}

/// [`BackgroundEvent::FontsReady`] 的载荷（K4 T3）：抓取成功的字体字节 +
/// 描述符。`base_url` 是发起抓取的页面文档 base（302 落点即最终 URL；
/// 跨页保护：页面已离开时只入库，不重排当前页）。
struct FontsReadyPayload {
    base_url: String,
    fonts: Vec<crate::resources::FetchedFont>,
}

/// [`BackgroundEvent::PageLoaded`] 的载荷。
///
/// K1 起载荷只携带全 `Send` 数据面（[`crate::resources::FetchedPageData`]）：
/// `Document` 含监听器注册表（!Send）只能在主线程构建——后台线程只投
/// 信号与字节，不触碰 Document。K3 T5 起后台编排同步完成（解析 + settle）
/// 后事件才到达，生命周期门控在到达点即满足，不再携带中途信号字段。
struct PageLoadedPayload {
    url: String,
    /// 文档 base：响应最终 URL（重定向落点）；错误路径与请求 URL 同值
    /// （错误页无子资源，不参与查表）。
    base_url: String,
    /// 抓取产物；错误携带用户可读信息。
    result: Result<crate::resources::FetchedDocument, String>,
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
    /// 行对齐 staging 缓冲（M6）：键 (宽, 高, padded 步幅) 命中时复用
    /// 分配，不每帧重建。
    staging_cache: Option<((u32, u32, u32), Vec<u8>)>,
}

/// 页面左键按下手势（链接目标在按下时解析，释放且未拖动才导航；
/// 命中节点存入 `Press`，供 mousedown/mouseup/click 派发作目标）。
struct Press {
    x: f32,
    y: f32,
    link: Option<String>,
    /// mousedown 时命中测试的节点（无命中为 None，事件退回文档根）。
    node: Option<NodeId>,
}

/// 浏览器应用状态。
struct BrowserApp {
    proxy: EventLoopProxy<BackgroundEvent>,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    renderer: Option<RenderThread>,
    /// 网络接驳缝：后端可整体替换（reqwest 实现或其他 `NetworkFetcher`）。
    fetcher: Option<Arc<dyn NetworkFetcher>>,
    /// @font-face 字体编排会话（K4 T3）：会话级注册表 + 已注册 URL 集。
    fonts: crate::resources::FontOrchestrator,
    shaper: nexty_text::ParleyTextShaper,
    bar: AddressBar,
    history: History,
    /// DOM 宏任务队列（K1）：生命周期事件经 post_task 派发、
    /// `run_pending_tasks` 排空（FIFO，派发中 post 不重入）。
    dom_loop: nexty_dom::EventLoop,
    /// 滚动条交互状态（悬停 / 拖拽）。
    scrollbar: Scrollbar,
    /// Alt 修饰键按住状态（后退/前进快捷键）。
    alt_down: bool,
    /// Ctrl 修饰键按住状态（快捷键映射）。
    ctrl_down: bool,
    /// Shift 修饰键按住状态（地址栏扩选）。
    shift_down: bool,
    page: Option<Page>,
    /// 当前页面布局好的根片段（滚动时按偏移重建显示列表）。
    root: Option<Fragment>,
    /// 当前布局的文本地图（选区命中/矩形/复制文本）。
    text_map: Option<TextMap>,
    /// 页面文本选区（端点按文本节点 + 源偏移保存，重排后重新解析）。
    selection: Selection,
    /// 页面左键按下手势：释放时判定「点击链接」还是「拖拽选择」。
    press: Option<Press>,
    /// 上一次点击的时间与位置（双击选词）。
    last_click: Option<(Instant, f32, f32)>,
    /// 根片段内容总高，px（滚动条与 clamp 的依据）。
    content_height: f32,
    /// 页面滚动量，px ∈ [0, max_scroll]。
    scroll_y: f32,
    cursor: (f32, f32),
    /// 当前系统光标种类（仅在变化时调用 `set_cursor`）。
    hover: HoverKind,
    /// 页面坐标系显示列表缓存（M6）：滚动复用，导航/重排/选区失效。
    scene_cache: pipeline::SceneCache,
    /// 页面代次：每次重排（导航 / resize 后 relayout）+1，作缓存键。
    page_generation: u64,
    /// resize 合帧器（M6）：连发 Resized 折叠到下一次重绘。
    resize_coalescer: pipeline::ResizeCoalescer,
    /// 最近一次 Resized 的物理尺寸（去重：Windows 对无响应窗口的 ghost
    /// 恢复会重发同尺寸 WM_SIZE，逐事件全管线会把长帧变成重排循环）。
    last_resize: Option<(u32, u32)>,
    /// 渲染结果通道（异步呈现）：渲染线程完成后经 [`BackgroundEvent::FrameReady`]
    /// 唤醒主线程收取。
    render_results: Option<std::sync::mpsc::Receiver<Result<Pixmap, RenderError>>>,
    /// 渲染结果的发送端（每次请求随作业发一份克隆给渲染线程）。
    render_reply: Option<std::sync::mpsc::Sender<Result<Pixmap, RenderError>>>,
    /// 是否有一份渲染作业在途（期间到来的重绘请求只置脏，出结果后补一帧）。
    render_in_flight: bool,
    /// 在途期间有交互请求了新帧：出结果并上屏后再排一帧取最新状态。
    frame_dirty: bool,
}

impl BrowserApp {
    fn new(proxy: EventLoopProxy<BackgroundEvent>) -> Self {
        // 字体注册表先于整形器创建：整形器经 with_registry 接线（K4 T3），
        // 此后 register 即生效、无需重建（nexty-text T2 语义）。
        let fonts = crate::resources::FontOrchestrator::new();
        let shaper = nexty_text::ParleyTextShaper::with_registry(fonts.registry());
        Self {
            proxy,
            window: None,
            gpu: None,
            renderer: None,
            fetcher: None,
            fonts,
            shaper,
            bar: AddressBar::new("about:blank"),
            history: History::new("about:blank"),
            dom_loop: nexty_dom::EventLoop::new(),
            scrollbar: Scrollbar::new(),
            alt_down: false,
            ctrl_down: false,
            shift_down: false,
            page: None,
            root: None,
            text_map: None,
            selection: Selection::new(),
            press: None,
            last_click: None,
            content_height: 0.0,
            scroll_y: 0.0,
            cursor: (0.0, 0.0),
            hover: HoverKind::Default,
            scene_cache: pipeline::SceneCache::new(),
            page_generation: 0,
            resize_coalescer: pipeline::ResizeCoalescer::default(),
            last_resize: None,
            render_results: None,
            render_reply: None,
            render_in_flight: false,
            frame_dirty: false,
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
        // 渲染结果经通道回主线程，线程完成时以事件代理唤醒事件循环
        //（异步呈现，巨页帧不阻塞主线程）
        let (render_reply, render_receiver) = std::sync::mpsc::channel();
        self.render_results = Some(render_receiver);
        self.render_reply = Some(render_reply);
        let proxy = self.proxy.clone();
        self.renderer = Some(RenderThread::spawn(
            Arc::new(nexty_paint::VelloCpuRasterizer::new()),
            Arc::new(move || {
                let _ = proxy.send_event(BackgroundEvent::FrameReady);
            }),
        ));
        // 启动路径用 fail-fast：TLS 后端不可用时浏览器本就无法工作，
        // 带着半初始化的状态继续跑不如立刻退出（渲染路径才需要降级）。
        // 整请求超时 30s：真实站点挂起时不无限占住加载状态。
        // 抓取栈接 HTTP 内存缓存（K3 T4/T5）：主文档与子资源共用同一实例，
        // 同 URL 二次导航 / 重复子资源直接命中缓存，不再打网络。
        self.fetcher = Some(Arc::new(CachedFetcher::new(
            ReqwestFetcher::with_timeout(Duration::from_secs(30)).expect("network fetcher"),
        )));
        self.show_blank();
        window.request_redraw();
    }

    /// 显示空白页（`about:blank` 等内部 scheme 走这里，不经网络）。
    fn show_blank(&mut self) {
        self.bar.set_url("about:blank");
        self.set_page(pipeline::load_page("", ""), true);
        // 内部页生命周期：主线程本地解析完成、零子资源编排 = 立即 settle
        self.dispatch_page_lifecycle(pipeline::PendingLifecycle {
            dom_content_loaded: true,
            load: true,
        });
    }

    /// 当前视口与内容对应的滚动几何；窗口未创建时为 `None`。
    fn geometry(&self) -> Option<ScrollGeometry> {
        let window = self.window.as_ref()?;
        let size = window.inner_size();
        Some(ScrollGeometry {
            viewport_width: size.width as f32,
            viewport_height: size.height as f32,
            content_height: self.content_height,
            bar_height: BAR_HEIGHT,
        })
    }

    /// 直接设置滚动量并请求重绘。
    fn set_scroll(&mut self, scroll: f32) {
        self.scroll_y = scroll;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// 把已解析页面按当前视口级联并布局。
    ///
    /// 每次都重级联：视口单位（vw/vh）在 computed 阶段折算，resize 后
    /// 必须取新视口。`reset_scroll` 为 false 时保留滚动量（resize 重排
    /// 场景），但按新视口与内容高重新 clamp。
    fn set_page(&mut self, page: Page, reset_scroll: bool) {
        self.root = None;
        self.text_map = None;
        self.page = Some(page);
        // 导航后的第一帧是全新布局：缓存作废由代次推进表达
        self.relayout();
        if reset_scroll {
            self.scroll_y = 0.0;
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// 按当前视口重级联 + 重排当前页面（M6：resize 合帧后每帧至多一次）。
    ///
    /// 由 [`Self::set_page`]（导航）与 `RedrawRequested`（resize 合帧）共用：
    /// 视口单位折算、文本地图重建、选区端点重解析、滚动 clamp、代次推进
    /// （使场景缓存失效）都在这里。
    fn relayout(&mut self) {
        let (viewport_width, viewport_height) = self
            .window
            .as_ref()
            .map(|window| {
                let size = window.inner_size();
                (size.width as f32, size.height as f32)
            })
            .unwrap_or((800.0, 600.0));
        let Some(mut page) = self.page.take() else {
            return;
        };
        pipeline::recascade(
            &mut page,
            nexty_css::MediaViewport {
                width: viewport_width,
                height: viewport_height,
            },
        );
        let root = pipeline::layout_page(&page, &self.shaper, viewport_width);
        self.content_height = root
            .as_ref()
            .map(|root| root.border_box.height)
            .unwrap_or(0.0);
        self.root = root;
        self.page = Some(page);
        // 文本地图随布局重建；已有选区按新布局夹取端点（resize 后保持选中）。
        self.text_map = self.root.as_ref().map(TextMap::new);
        if self.selection.positions().is_some() {
            match &self.text_map {
                Some(map) => {
                    self.selection.resolve(map);
                }
                None => self.selection.clear(),
            }
        }
        // 新布局结果：页面坐标系场景整体重建（缓存失效）
        self.page_generation = self.page_generation.wrapping_add(1);
        if let Some(geometry) = self.geometry() {
            self.scroll_y = scrollbar::clamp_scroll(self.scroll_y, geometry.max_scroll());
        }
    }

    /// 滚动 `delta` px（正值向下看更多内容），并按当前视口 clamp。
    fn scroll_by(&mut self, delta: f32) {
        let Some(geometry) = self.geometry() else {
            return;
        };
        let target = scrollbar::clamp_scroll(self.scroll_y + delta, geometry.max_scroll());
        self.set_scroll(target);
    }

    /// 按光标位置更新系统光标形状（仅在种类变化时调用 `set_cursor`）。
    ///
    /// 拖拽滚动条期间不覆盖（拖拽是明确的指代动作）。
    fn update_cursor_icon(&mut self) {
        if self.scrollbar.is_dragging() {
            return;
        }
        let (x, y) = self.cursor;
        let viewport_width = self
            .window
            .as_ref()
            .map_or(0.0, |window| window.inner_size().width as f32);
        let kind = if y <= BAR_HEIGHT {
            // 工具带：输入框范围内是 I 形，其余默认
            let input = ui::input_rect(viewport_width);
            if x >= input.x && x <= input.x + input.width {
                HoverKind::Text
            } else {
                HoverKind::Default
            }
        } else {
            match (&self.page, &self.root) {
                (Some(page), Some(root)) => {
                    // 窗口坐标 → 页面坐标（与命中测试/导航同一换算）
                    pipeline::hover_kind(page, root, x, y - BAR_HEIGHT + self.scroll_y)
                }
                _ => HoverKind::Default,
            }
        };
        if kind != self.hover {
            self.hover = kind;
            if let Some(window) = &self.window {
                window.set_cursor(cursor_icon(kind));
            }
        }
    }

    /// 页面区域左键按下：滚动条优先（拖拽 thumb / 轨道翻页），其次开始选择
    /// 手势（点击链接在释放且未拖动时导航）。
    fn press_page(&mut self, x: f32, y: f32) {
        let Some(geometry) = self.geometry() else {
            return;
        };
        match self.scrollbar.hit(&geometry, self.scroll_y, x, y) {
            ScrollbarHit::None => self.begin_page_gesture(x, y),
            _ => {
                self.press = None;
                if let Some(scroll) = self.scrollbar.on_press(&geometry, self.scroll_y, x, y) {
                    self.set_scroll(scrollbar::clamp_scroll(scroll, geometry.max_scroll()));
                }
            }
        }
    }

    /// 开始一次页面点击/选择手势。
    fn begin_page_gesture(&mut self, x: f32, y: f32) {
        let page_y = y - BAR_HEIGHT + self.scroll_y;
        // 单次命中测试同时供事件目标与链接解析（hit_test_ex 结果 → 事件节点）
        let hit = self
            .root
            .as_ref()
            .and_then(|root| pipeline::hit_test_ex(root, x, page_y));
        let node = hit.map(|hit| hit.node);
        let link = hit
            .and_then(|hit| {
                self.page
                    .as_ref()
                    .and_then(|page| pipeline::link_target(page, hit.node))
            })
            .and_then(|href| nexty_network::resolve(self.history.current(), &href).ok());
        // mousedown 先派发后默认：默认动作 = 开始选择手势（含地址栏失焦，
        // 同浏览器 preventDefault mousedown 阻止焦点转移）。取消则手势不开始。
        if !self.dispatch_page_mouse(node, "mousedown", x, y, 1) {
            return;
        }
        // 页面点击让地址栏失焦：之后 Ctrl+A / Ctrl+C 作用于页面。
        self.bar.blur();
        let position = self.text_map.as_ref().and_then(|map| map.hit(x, page_y));
        let now = Instant::now();
        let double_click = self.last_click.is_some_and(|(time, last_x, last_y)| {
            now.duration_since(time) <= DOUBLE_CLICK
                && (x - last_x).abs() <= selection::CLICK_SLOP
                && (y - last_y).abs() <= selection::CLICK_SLOP
        });
        self.last_click = Some((now, x, y));
        match position {
            Some(position) => {
                if double_click
                    && let Some((from, to)) = self
                        .text_map
                        .as_ref()
                        .and_then(|map| map.word_range(position))
                {
                    self.selection.select(from, to);
                } else {
                    self.selection.begin(position, self.shift_down);
                }
            }
            None => self.selection.clear(),
        }
        self.press = Some(Press { x, y, link, node });
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// 导航：入历史栈并加载。
    ///
    /// 地址栏输入与页面链接共用入口。地址栏输入在这里做 URL 规范化：无
    /// scheme 补 `http://`（HTTP-first，决策见
    /// `docs/decisions/2026-10-31-addressbar-url.md` 与
    /// [`crate::resources::normalize_address_bar_input`]）；链接 href 已由
    /// `resolve` 按文档 base 绝对化，规范化对其无操作。历史栈存规范化后
    /// 的 URL：后退/前进重放的即实际导航目标。
    fn navigate(&mut self, url: &str) {
        let url = crate::resources::normalize_address_bar_input(url);
        self.history.push(url.clone());
        self.sync_history_flags();
        self.load(&url);
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
        // 导航丢弃旧页面的选区与按下手势（新页面的节点 id 从 arena 重新分配）。
        self.selection.clear();
        self.press = None;
        self.text_map = None;
        self.last_click = None;
        self.bar.set_url(url.to_owned());
        self.bar.set_loading(true);

        // 内部 scheme（about:blank 等）不经网络，直接生成本地空白页。
        // 其余 URL（含补全 scheme 后仍非法的输入）交给抓取路径：协议白名单
        // 判 InvalidUrl → 错误页给出反馈，不再静默白屏。
        if url.starts_with("about:") {
            self.bar.set_loading(false);
            self.show_blank();
            if let Some(window) = &self.window {
                window.set_title("Nexty");
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
        let target = url.to_owned();
        std::thread::Builder::new()
            .name("nexty-fetch".to_owned())
            .spawn(move || {
                // 全部抓取在后台线程完成：文档抓取 → 解码 → 规划解析 →
                // 子资源并发抓取与解码（resources::fetch_document 同步编排），
                // 产出全 Send 数据面（K1 起 Document 含监听器注册表 !Send，
                // 只能在主线程构建——主线程按同一 HTML 重建正式文档）。
                // 错误串收敛为「错误类别 + 状态码」（见 fetch_document）。
                let result = crate::resources::fetch_document(fetcher.as_ref(), &target);
                let base_url = match &result {
                    Ok(fetched) => fetched.base_url.clone(),
                    Err(_) => target.clone(),
                };
                let _ =
                    proxy.send_event(BackgroundEvent::PageLoaded(Box::new(PageLoadedPayload {
                        url: target,
                        base_url,
                        result,
                    })));
            })
            .expect("spawn fetch thread");
    }

    /// 抓取完成：重建页面与显示列表，并派发生命周期事件。
    fn page_loaded(&mut self, payload: PageLoadedPayload) {
        self.bar.set_loading(false);
        // 生命周期门控：后台编排同步完成（解析 + settle）后事件才到达；
        // 错误页在主线程本地合成解析——两条路径的 DOMContentLoaded 与 load
        // 门控均已满足，经 EventLoop post_task 按 FIFO 顺序派发。
        let pending = pipeline::PendingLifecycle {
            dom_content_loaded: true,
            load: true,
        };
        match payload.result {
            Ok(fetched) => {
                // 正式文档在主线程构建（Document 含监听器注册表 !Send）：
                // 按同一 HTML 重建，并装配后台线程抓取的子资源数据面。
                // 查表 base 与后台抓取一致：响应最终 URL（重定向落点）。
                let document = nexty_html::parse_document(
                    &fetched.data.html,
                    nexty_html::ParseOptions::default(),
                );
                let resources = crate::resources::assemble_page_data(
                    &document,
                    &payload.base_url,
                    &fetched.data,
                );
                // @font-face 字体源收集与后台抓取（K4 T3）：不在 load 门控
                // 内，首帧先用系统字体渲染；字节就绪注册后经 FontsReady
                // 触发恰好一轮重排（font-display swap 简化）。
                self.spawn_font_fetch(&resources.stylesheets, &payload.base_url);
                let page = pipeline::build_page(document, resources.stylesheets, resources.images);
                let title = pipeline::document_title(&page.document);
                self.set_page(page, true);
                // 302 落点（WHATWG 会话历史）：条目与地址栏改为最终 URL——
                // 页内相对链接的解析基准（history.current()）随之落在落点，
                // 而非重定向前的请求 URL；无重定向时 base_url == url，不变。
                if payload.base_url != payload.url {
                    self.history.replace_current(payload.base_url.clone());
                }
                self.bar.set_url(payload.base_url);
                if let Some(window) = &self.window {
                    match title {
                        Some(title) => window.set_title(&format!("{title} - Nexty")),
                        None => window.set_title("Nexty"),
                    }
                }
            }
            Err(error) => {
                eprintln!("load {} failed: {error}", payload.url);
                // 失败也要出错误页：只打日志会让画面停在上一帧或空白，
                // 用户看不到任何反馈。
                let html = error_page_html(&payload.url, &error);
                self.set_page(pipeline::load_page(&html, ""), true);
                if let Some(window) = &self.window {
                    window.set_title("加载失败 - Nexty");
                }
            }
        }
        // 生命周期派发经 EventLoop post_task（DOMContentLoaded → load，FIFO）
        self.dispatch_page_lifecycle(pending);
    }

    /// 收集 @font-face 源并在后台线程抓取字体字节（K4 T3）。
    ///
    /// 无待抓取源（无 @font-face / 全部会话已注册 / woff2 提示跳过）时
    /// 不起线程：已注册字体的重复导航零抓取、零重排。
    fn spawn_font_fetch(&mut self, stylesheets: &[nexty_css::Stylesheet], base_url: &str) {
        let requests = self.fonts.collect(stylesheets, base_url);
        if requests.is_empty() {
            return;
        }
        let Some(fetcher) = self.fetcher.clone() else {
            return;
        };
        let proxy = self.proxy.clone();
        let base_url = base_url.to_owned();
        std::thread::Builder::new()
            .name("nexty-fonts".to_owned())
            .spawn(move || {
                // 抓取降级语义与主文档子资源一致（404 / 超预算 / 传输错误
                // 缺席）；全空则无事可做，不回送事件。
                let fonts = crate::resources::fetch_fonts(fetcher.as_ref(), &requests);
                if fonts.is_empty() {
                    return;
                }
                let _ =
                    proxy.send_event(BackgroundEvent::FontsReady(Box::new(FontsReadyPayload {
                        base_url,
                        fonts,
                    })));
            })
            .expect("spawn font fetch thread");
    }

    /// 字体字节就绪：注册进会话注册表；有新注册时重级联 + 重排 + 重绘
    /// **恰好一轮**（K4 T3）。已接线的整形器自动命中注册字体（nexty-text
    /// T2）；chunk 分析缓存随新片段树重建，场景缓存随代次推进失效。载荷
    /// 属于已离开的页面时只入库（会话缓存跨页有效），不重排。
    fn fonts_ready(&mut self, payload: FontsReadyPayload) {
        let registration = self.fonts.register(payload.fonts);
        if registration.rejected > 0 {
            eprintln!(
                "font registration rejected {} source(s); fallback to system fonts",
                registration.rejected
            );
        }
        if registration.registered > 0 && payload.base_url == self.history.current() {
            self.relayout();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    /// 一帧：合成显示列表并发给渲染线程（异步，不等待）。
    ///
    /// 巨页帧的光栅可达数十秒：主线程若阻塞等待会进入系统「无响应」状态，
    /// winit 0.30 把 WM_PAINT 直接映射为 RedrawRequested，配合系统的区域
    /// 失效就形成重绘风暴——因此光栅在渲染线程执行，完成后经
    /// [`BackgroundEvent::FrameReady`] 唤醒上屏。在途期间的请求只置
    /// `frame_dirty`，出结果补一帧取最新状态（帧合流）。
    fn redraw(&mut self) {
        if self.window.is_none() {
            return;
        }
        // 在途合流：不叠加作业，出结果后补一帧
        if self.render_in_flight {
            self.frame_dirty = true;
            return;
        }
        // M6 合帧：折叠的 resize 在这里执行一次重排（每帧至多一次）
        if self.resize_coalescer.take() && self.page.is_some() {
            self.relayout();
        }
        let window = self.window.as_ref().expect("window checked");
        let size = window.inner_size();
        let viewport = Size {
            width: size.width,
            height: size.height,
        };
        if viewport.width == 0 || viewport.height == 0 {
            return;
        }
        let (width, height) = (viewport.width as f32, viewport.height as f32);

        // 滚动几何与滚动条状态先取好：下面 `&mut self.gpu` 的可变借用期间
        // 不能再调用 `&self` 方法
        let geometry = ScrollGeometry {
            viewport_width: width,
            viewport_height: height,
            content_height: self.content_height,
            bar_height: BAR_HEIGHT,
        };
        let scrollbar_hot = self.scrollbar.is_hovered() || self.scrollbar.is_dragging();
        let thumb = geometry.thumb(self.scroll_y);
        let track = geometry.track();

        // 页面坐标系场景缓存（M6）：滚动只平移 origin，不重建显示列表；
        // 高亮集合变化（选区）由键失配自然失效重建
        let highlights = match &self.text_map {
            Some(map) if !self.selection.is_empty() => self.selection.rects(map),
            _ => Vec::new(),
        };
        // 画布底色：CSS 2.2 §14.2 html/body 背景传播到画布——html 携带背景
        // 用 html，否则 body，都无回落白；视口坐标系，不随滚动平移。画布
        // 必须不透明：透明像素直接 blit 给 surface（alpha_mode Auto）在部分
        // 后端表现为黑屏/花屏。被传播元素的自身背景由 emit 抑制（搬移语义）。
        let mut scene = match (&self.page, &self.root) {
            (Some(page), Some(root)) => {
                let page_scene = self.scene_cache.scene_for(
                    self.page_generation,
                    page,
                    root,
                    width,
                    &highlights,
                );
                let mut scene = page_scene.translated(0.0, BAR_HEIGHT - self.scroll_y);
                for (index, command) in pipeline::canvas_background_commands(page, width, height)
                    .into_iter()
                    .enumerate()
                {
                    scene.commands.insert(index, command);
                }
                scene
            }
            _ => {
                // 无页面（启动空窗）：无从传播，固定白底。
                let mut scene = Scene::default();
                scene.commands.push(nexty_paint::Command::FillRect {
                    rect: nexty_paint::Rect {
                        x: 0.0,
                        y: 0.0,
                        width,
                        height,
                    },
                    color: nexty_paint::Color::opaque(0xff, 0xff, 0xff),
                });
                scene
            }
        };
        // 滚动条：轨道 + thumb，可拖拽（悬停/拖拽时加深）
        if let (Some(track), Some(thumb)) = (track, thumb) {
            scene.commands.push(nexty_paint::Command::FillRect {
                rect: track,
                color: nexty_paint::Color::opaque(0xe2, 0xe2, 0xe2),
            });
            let thumb_color = if self.scrollbar.is_dragging() {
                nexty_paint::Color::opaque(0x8a, 0x8a, 0x8a)
            } else if scrollbar_hot {
                nexty_paint::Color::opaque(0x9e, 0x9e, 0x9e)
            } else {
                nexty_paint::Color::opaque(0xb4, 0xb4, 0xb4)
            };
            scene.commands.push(nexty_paint::Command::FillRect {
                rect: thumb,
                color: thumb_color,
            });
        }
        self.bar.draw(&self.shaper, &mut scene, width);

        let Some(gpu) = &mut self.gpu else {
            return;
        };
        let Some(renderer) = &self.renderer else {
            return;
        };
        let Some(reply) = self.render_reply.as_ref() else {
            return;
        };
        self.render_in_flight = true;
        if let Err(scene) = renderer.request(scene, viewport, reply.clone()) {
            // 渲染线程已终止：就地降级为灰屏，不留在途状态
            self.render_in_flight = false;
            eprintln!("render thread terminated");
            gpu.clear(window, [0.6, 0.6, 0.6, 1.0]);
            drop(scene);
        }
    }

    /// 收取渲染线程完成的光栅结果并上屏（[`BackgroundEvent::FrameReady`]）。
    ///
    /// 通道内可能积压多份结果（快速交互连发）：逐份排空、只上屏最后一份
    /// ——中间帧的画面立即被更新的覆盖，present 只发生一次。
    fn present_finished_frames(&mut self) {
        let Some(results) = &self.render_results else {
            return;
        };
        let mut latest: Option<Result<Pixmap, RenderError>> = None;
        while let Ok(result) = results.try_recv() {
            latest = Some(result);
        }
        self.render_in_flight = false;
        let Some(result) = latest else {
            return;
        };
        let Some(window) = &self.window else {
            return;
        };
        let Some(gpu) = &mut self.gpu else {
            return;
        };
        match result {
            Ok(pixmap) => gpu.present(&pixmap, window),
            Err(error) => {
                // 渲染失败不拖垮主进程：清屏为灰色并上报
                eprintln!("render failed: {error}");
                gpu.clear(window, [0.6, 0.6, 0.6, 1.0]);
            }
        }
        if self.frame_dirty {
            self.frame_dirty = false;
            window.request_redraw();
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
        match event {
            BackgroundEvent::PageLoaded(payload) => self.page_loaded(*payload),
            // 字体字节就绪：注册 + 恰一轮重排（K4 T3）
            BackgroundEvent::FontsReady(payload) => self.fonts_ready(*payload),
            // 渲染线程完成：上屏最新一帧，按需补排帧
            BackgroundEvent::FrameReady => self.present_finished_frames(),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::Resized(size) => {
                // 同尺寸去重：长帧阻塞后 Windows 的 ghost 恢复会重发同尺寸
                // WM_SIZE；不滤掉就变成「帧 → 重排 → 帧」的无限循环
                if self.last_resize == Some((size.width, size.height)) {
                    return;
                }
                self.last_resize = Some((size.width, size.height));
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                }
                // M6 合帧：只置脏 + 请求重绘，重级联/重排合并到下一次
                // RedrawRequested 执行一次（拖动边缘连发事件时管线每帧
                // 至多跑一遍）
                self.resize_coalescer.on_resized();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
                // 拖拽中：thumb 跟随光标移动
                if self.scrollbar.is_dragging()
                    && let Some(geometry) = self.geometry()
                    && let Some(scroll) =
                        self.scrollbar
                            .on_drag(&geometry, self.cursor.0, self.cursor.1)
                {
                    self.set_scroll(scrollbar::clamp_scroll(scroll, geometry.max_scroll()));
                }
                // 拖拽选择：光标离开行盒时按最近文本吸附（拖到页尾仍扩展）。
                if self.selection.is_dragging()
                    && self.cursor.1 > BAR_HEIGHT
                    && let Some(map) = &self.text_map
                {
                    let page_y = self.cursor.1 - BAR_HEIGHT + self.scroll_y;
                    if let Some(position) = map
                        .hit(self.cursor.0, page_y)
                        .or_else(|| map.nearest(self.cursor.0, page_y))
                    {
                        self.selection.extend_to(position);
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                }
                // 悬停态变化只影响滚动条配色，需要一帧重绘
                if let Some(geometry) = self.geometry()
                    && self.scrollbar.on_hover(
                        &geometry,
                        self.scroll_y,
                        self.cursor.0,
                        self.cursor.1,
                    )
                    && let Some(window) = &self.window
                {
                    window.request_redraw();
                }
                self.update_cursor_icon();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // 滚轮先派发后默认：WheelEvent（正值 deltaY = 页面向下滚，
                // 与 winit 相反号）未取消才滚动，滚动量换算沿用现语义。
                let (mode, amount, wheel_delta) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (
                        DeltaMode::Line,
                        y * WHEEL_LINE_PX,
                        (-f64::from(x), -f64::from(y)),
                    ),
                    MouseScrollDelta::PixelDelta(position) => (
                        DeltaMode::Pixel,
                        position.y as f32,
                        (-position.x, -position.y),
                    ),
                };
                if self.dispatch_page_wheel(wheel_delta, mode) {
                    self.scroll_by(-amount);
                }
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
                            self.press_page(x, y);
                        }
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                self.scrollbar.on_release();
                if let Some(press) = self.press.take() {
                    let dragged = !selection::is_click((press.x, press.y), self.cursor);
                    // mouseup 先派发后默认；selection.finish 与滚动条
                    // on_release 一样是 chrome 侧手势状态卫生（无论取消都
                    // 执行，避免状态机卡在拖拽态）。
                    let _ = self.dispatch_page_mouse(press.node, "mouseup", press.x, press.y, 0);
                    self.selection.finish();
                    // 释放且未拖动才算点击：click 仅此时派发（现语义），
                    // 默认动作 = 链接导航，被取消则跳过（先派发后默认）。
                    if !dragged
                        && self.dispatch_page_mouse(press.node, "click", press.x, press.y, 0)
                        && let Some(url) = press.link
                    {
                        self.navigate(&url);
                    }
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                let state = modifiers.state();
                self.alt_down = state.alt_key();
                self.ctrl_down = state.control_key();
                self.shift_down = state.shift_key();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let modifiers = self.modifiers();
                let key = ui_key(&event.logical_key);
                // 浏览器级快捷键优先于一切（Alt+方向键、Ctrl+L/R/A、F5、Esc）。
                // 「先派发后默认」：地址栏聚焦时键属 chrome UI（不进页面），
                // 直接执行；未聚焦时先派发可取消 keydown，未取消才执行。
                if let Some(key) = key
                    && let Some(shortcut) = ui::shortcut(key, modifiers)
                {
                    let allowed = if self.bar.is_focused() {
                        true
                    } else {
                        let (key_name, code) = events::dom_key_names(key);
                        self.dispatch_page_key(&key_name, &code, event.repeat)
                    };
                    if allowed {
                        self.apply_shortcut(shortcut);
                    }
                    return;
                }
                if self.bar.is_focused() {
                    // 地址栏聚焦：方向键/Home/End 作用于光标（Shift 扩选）
                    // —— chrome UI 键，不派发到页面
                    if let Some(direction) = key.and_then(caret_direction) {
                        let _ = self.bar.handle(UiEvent::MoveCaret {
                            direction,
                            extend: modifiers.shift,
                        });
                    }
                } else {
                    // 未聚焦：翻页/滚动键作用于页面——先派发后默认
                    let page_step = self
                        .geometry()
                        .map_or(0.0, |geometry| geometry.page_step() * 0.9);
                    match event.logical_key {
                        Key::Named(NamedKey::PageDown) => {
                            self.dispatch_key_scroll("PageDown", page_step);
                        }
                        Key::Named(NamedKey::PageUp) => {
                            self.dispatch_key_scroll("PageUp", -page_step);
                        }
                        Key::Named(NamedKey::ArrowDown) => {
                            self.dispatch_key_scroll("ArrowDown", WHEEL_LINE_PX);
                        }
                        Key::Named(NamedKey::ArrowUp) => {
                            self.dispatch_key_scroll("ArrowUp", -WHEEL_LINE_PX);
                        }
                        Key::Named(NamedKey::Home) => {
                            let delta = -self.scroll_y;
                            self.dispatch_key_scroll("Home", delta);
                        }
                        Key::Named(NamedKey::End) => {
                            if let Some(geometry) = self.geometry() {
                                let delta = geometry.max_scroll() - self.scroll_y;
                                self.dispatch_key_scroll("End", delta);
                            }
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
                    Key::Named(NamedKey::Delete) => {
                        let _ = self.bar.handle(UiEvent::Delete);
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

/// 执行浏览器级快捷键命令。
impl BrowserApp {
    fn apply_shortcut(&mut self, shortcut: ui::Shortcut) {
        match shortcut {
            ui::Shortcut::Back => self.go_back(),
            ui::Shortcut::Forward => self.go_forward(),
            ui::Shortcut::Reload => self.reload(),
            ui::Shortcut::FocusAddressBar => {
                let _ = self.bar.handle(UiEvent::FocusAndSelect);
            }
            ui::Shortcut::SelectAll => {
                if self.bar.is_focused() {
                    let _ = self.bar.handle(UiEvent::SelectAll);
                } else if let Some((from, to)) = self.text_map.as_ref().and_then(TextMap::all) {
                    self.selection.select(from, to);
                }
            }
            ui::Shortcut::Copy => {
                // 地址栏聚焦时不处理（地址栏复制不在本轮范围）。
                if !self.bar.is_focused()
                    && let Some(map) = &self.text_map
                {
                    let text = self.selection.text(map);
                    if !text.is_empty()
                        && let Err(error) = selection::write_clipboard(&text)
                    {
                        eprintln!("clipboard write failed: {error}");
                    }
                }
            }
            ui::Shortcut::Escape => {
                // 与 Chrome 一致：Esc 丢弃未提交的编辑并失焦
                self.bar.blur();
                self.bar.set_url(self.history.current().to_owned());
            }
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

/// winit 逻辑键 → UI 抽象键（仅本层关心的键）。
fn ui_key(key: &Key) -> Option<ui::Key> {
    match key {
        Key::Named(NamedKey::ArrowLeft) => Some(ui::Key::Left),
        Key::Named(NamedKey::ArrowRight) => Some(ui::Key::Right),
        Key::Named(NamedKey::Home) => Some(ui::Key::Home),
        Key::Named(NamedKey::End) => Some(ui::Key::End),
        Key::Named(NamedKey::F5) => Some(ui::Key::F5),
        Key::Named(NamedKey::Escape) => Some(ui::Key::Escape),
        Key::Character(text) => text.chars().next().map(ui::Key::Character),
        _ => None,
    }
}

/// UI 抽象键 → 光标移动方向（非方向键返回 `None`）。
fn caret_direction(key: ui::Key) -> Option<CaretDirection> {
    match key {
        ui::Key::Left => Some(CaretDirection::Left),
        ui::Key::Right => Some(CaretDirection::Right),
        ui::Key::Home => Some(CaretDirection::Home),
        ui::Key::End => Some(CaretDirection::End),
        _ => None,
    }
}

/// 悬停语义 → 系统光标形状。
fn cursor_icon(kind: HoverKind) -> CursorIcon {
    match kind {
        HoverKind::Default => CursorIcon::Default,
        HoverKind::Text => CursorIcon::Text,
        HoverKind::Pointer => CursorIcon::Pointer,
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
            staging_cache: None,
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
            // staging 缓冲按 (宽, 高, 步幅) 复用（M6）：take → 填充 → 上传
            // → 还回，避免每帧重新分配整屏对齐缓冲
            let key = (width, height, padded);
            let mut staging = match self.staging_cache.take() {
                Some((cached_key, buffer)) if cached_key == key => buffer,
                _ => Vec::new(),
            };
            pad_rows_into(&mut staging, &pixmap.data, width, height, padded);
            self.upload_aligned(&texture, &staging, padded, width, height);
            self.staging_cache = Some((key, staging));
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

/// 把紧凑行优先的像素数据按行padding 到 256 字节对齐（M6 起仅测试用，
/// 生产路径走 [`pad_rows_into`] + staging 缓冲复用）。
///
/// WebGPU 的 `write_texture` 要求 `bytes_per_row` 为 256 的倍数，而 Pixmap
/// 的行跨度是 `4 * width`。本函数为每行尾部补零，使行跨度变为
/// `padded`。padding 字节的内容无关紧要（着色器只读实际宽度内的像素），
/// 但必须是**每行独立补齐**而非只在末尾补一次。
#[cfg(test)]
fn pad_rows(data: &[u8], width: u32, height: u32, padded: u32) -> Vec<u8> {
    let mut out = Vec::new();
    pad_rows_into(&mut out, data, width, height, padded);
    out
}

/// [`pad_rows`] 的写入变体（M6）：复用调用方缓冲的容量——staging 缓冲
/// 按 (宽, 高, 步幅) 缓存后每帧只重填内容，不再整屏重新分配。
fn pad_rows_into(out: &mut Vec<u8>, data: &[u8], width: u32, height: u32, padded: u32) {
    let unpadded = 4 * width;
    out.clear();
    out.reserve(padded as usize * height as usize);
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
}

/// HTML 实体转义（`&` `<` `>` `"` `'`）。
///
/// 错误页把不可信文本（URL、错误类别）拼进 HTML：不转义的话，形如
/// `<img src=//evil.test/1>` 的 URL 会被解析成标记，触发对第三方地址的
/// 子资源请求并泄露用户正在访问的 URL。
fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// 加载失败的错误页 HTML：URL 与错误文本均经转义。
fn error_page_html(url: &str, error: &str) -> String {
    format!(
        "<body style=\"font-family: sans-serif; padding: 24px\">\
         <h1 style=\"color: #c5221f\">无法加载此页</h1>\
         <p>{}</p>\
         <p style=\"color: #5f6368\">{}</p></body>",
        escape_html(url),
        escape_html(error),
    )
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
    use super::{FULLSCREEN_TRIANGLE, pad_rows};

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

#[cfg(test)]
mod error_page_tests {
    use super::{error_page_html, escape_html};
    use nexty_dom::NodeKind;

    /// 按树序收集文档中的元素局部名。
    fn element_names(document: &nexty_dom::Document) -> Vec<String> {
        fn walk(document: &nexty_dom::Document, node: nexty_dom::NodeId, out: &mut Vec<String>) {
            for child in document.children(node) {
                if let Some(NodeKind::Element(data)) = document.node(child) {
                    out.push(data.name.clone());
                }
                walk(document, child, out);
            }
        }
        let mut out = Vec::new();
        walk(document, document.root(), &mut out);
        out
    }

    /// 拼接文档全部文本。
    fn text_content(document: &nexty_dom::Document) -> String {
        fn walk(document: &nexty_dom::Document, node: nexty_dom::NodeId, out: &mut String) {
            for child in document.children(node) {
                if let Some(NodeKind::Text(data)) = document.node(child) {
                    out.push_str(data);
                }
                walk(document, child, out);
            }
        }
        let mut out = String::new();
        walk(document, document.root(), &mut out);
        out
    }

    #[test]
    fn escape_html_neutralizes_all_five_entities() {
        assert_eq!(escape_html("a&b<c>\"d'e"), "a&amp;b&lt;c&gt;&quot;d&#39;e");
        // 正常文本不受影响
        assert_eq!(
            escape_html("https://page.test/dir/page.html?q=1"),
            "https://page.test/dir/page.html?q=1"
        );
    }

    #[test]
    fn error_page_rejects_url_markup_injection() {
        // URL 含 <img src=//evil.test/1>：错误页 DOM 中不得出现 img 元素，
        // 不得有对第三方地址的子资源请求
        let url = "http://x.test/<img src=//evil.test/1>";
        let html = error_page_html(url, "HTTP 404");
        let document = nexty_html::parse_document(&html, nexty_html::ParseOptions::default());
        let names = element_names(&document);
        assert!(
            !names.iter().any(|name| name == "img"),
            "无 img 元素：{names:?}"
        );
        assert!(
            nexty_html::collect_image_resources(&document).is_empty(),
            "无第三方子资源请求"
        );
        assert!(
            nexty_html::collect_style_resources(&document).is_empty(),
            "无第三方样式表请求"
        );
        // 显示文本为字面转义后的 URL
        assert!(
            text_content(&document).contains("<img src=//evil.test/1>"),
            "URL 按字面显示"
        );
    }

    #[test]
    fn error_page_keeps_normal_url_rendering() {
        let html = error_page_html("https://page.test/dir/page.html?q=1&x=2", "请求超时");
        let document = nexty_html::parse_document(&html, nexty_html::ParseOptions::default());
        let text = text_content(&document);
        assert!(text.contains("https://page.test/dir/page.html?q=1&x=2"));
        assert!(text.contains("请求超时"));
    }
}
