//! 子资源与主文档抓取编排：清单 → URL 解析 → 并发抓取 → 级联输入。
//!
//! 职责边界：解析归 nexty-html（清单收集）、传输归 nexty-network（字节）、
//! 解码归 nexty-paint（RGBA8）与 nexty-network（文本编码）、级联归 nexty-css
//! （多表合并）。本模块只做编排：把清单里的相对 URL 解析为绝对地址，并发
//! 抓取后按清单顺序交回；主文档的「抓取 → 解码 → 规划解析 → 子资源编排」
//! 序列收口在 [`fetch_document`]（K3 T5），地址栏输入的 scheme 规范化在
//! [`normalize_address_bar_input`]（决策见
//! `docs/decisions/2026-10-31-addressbar-url.md`）。
//!
//! 降级语义（T5 退出条件）：单个子资源失败（URL 非法 / 传输错误 / 非 2xx /
//! 解码失败）只跳过该资源，不影响其余渲染——外链 CSS 失败 = 该表不参与级联；
//! 图片失败 = 该节点不产生像素（布局回退默认对象尺寸 300×150）。
//!
//! 字体子资源（K4 T3）：`@font-face` 的 `url()` 源按文档 base 解析后由
//! [`FontOrchestrator`] 编排——收集（主线程，页面装配后）→ [`fetch_fonts`]
//! （后台线程，预算 [`MAX_FONT_BODY_BYTES`]）→ 注册进会话级
//! `nexty_text::FontRegistry`（主线程，字节到达时）。字体不在首轮 settle /
//! load 门控内（CSS Fonts font-display 的 swap 简化，偏差同 nexty-css）：
//! 首帧用系统字体照常渲染，字节就绪注册后由调用方触发恰好一轮重排 + 重绘
//! （注册表已接线的整形器自动生效，无需重建）。
//!
//! 字体编排偏差：woff/woff2 提示的源跳过抓取（harfrust 不消费该容器格式）；
//! 无提示的源乐观抓取，非字体数据由注册期拒收兜底（双重防线）；`local()`
//! 源不经抓取（已安装字体的命中交给系统回退，不按 local 名单显式解析）；
//! 会话去重按 URL 键控（与既有子资源同一键控语义）——同 URL 注册成功后，
//! 后续导航对同 URL 的不同描述符不再抓取（罕见边角，接受降级）；负结果
//! （404 / 非字体数据）不入会话集，再次导航时重试。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use nexty_css::{FontFaceSource, FontFaceStyle, Stylesheet};
use nexty_dom::{Document, NodeId};
use nexty_html::StyleResource;
use nexty_network::{Method, NetworkError, NetworkFetcher, Request, decode_document, resolve};
use nexty_paint::{Image, decode};
use nexty_text::{FontFaceDescriptor, FontRegistry, FontStyle};

/// 并发抓取的线程上限：清单可能很大（真实页面几十张图），逐资源开线程
/// 会撑爆调度器；分块处理后每块内并发、块间串行。
const MAX_CONCURRENT_FETCHES: usize = 8;

/// 响应体字节预算（不可信输入防线：巨响应/解压炸弹的分配失败是 abort，
/// 渲染隔离的 panic 兜底兜不住）。预算放编排层，`NetworkFetcher` trait 与
/// network crate 不动；超预算按既有降级语义处理（等同抓取失败，跳过资源）。
///
/// 文档与样式表是文本，10 MB 已远超真实页面（Chromium 同量级）；图片按
/// 压缩字节 50 MB 上限——解压后的像素尺寸另由 nexty-paint 的解码预检
/// （边长/面积上限）控制，两层预算互补。
pub const MAX_DOCUMENT_BODY_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_STYLESHEET_BODY_BYTES: usize = 10 * 1024 * 1024;
pub const MAX_IMAGE_BODY_BYTES: usize = 50 * 1024 * 1024;

/// 字体响应体字节预算（K4 T3）：单个字体文件（TTF/OTF 静态字体）远小于
/// 图片压缩体积，20 MB 已远超真实 Web 字体；超预算按既有降级语义跳过该源
/// （分配失败是 abort，panic 兜底兜不住）。
pub const MAX_FONT_BODY_BYTES: usize = 20 * 1024 * 1024;

/// 抓取完成的子资源。
pub struct Subresources {
    /// 作者样式表，按级联优先级排列（inline 与外链按文档出现顺序交错）。
    pub stylesheets: Vec<Stylesheet>,
    /// 已解码图片（`<img>` 节点 → RGBA 图像）。同一 URL 的多个节点共享
    /// 一份像素（`Arc` 引用计数，M1 零拷贝）。
    pub images: HashMap<NodeId, Arc<Image>>,
}

/// 后台线程抓取产物（K1 T3：全 `Send` 数据面）。
///
/// K1 起 `Document` 携带监听器注册表（`Box<dyn EventListener>`，!Send），
/// 只能在主线程构建——后台抓取线程不再产出 `Page`，只产出本结构（HTML +
/// 样式表字节 + 已解码图片，按绝对 URL 键控），随事件代理送回主线程后由
/// [`assemble_page_data`] 装配。
pub struct FetchedPageData {
    /// 主文档 HTML（已 UTF-8 校验）。
    pub html: String,
    /// 外链样式表字节：绝对 URL → 字节。失败/超时/超预算/URL 解析失败的
    /// URL 不在表内（降级语义：该表不参与级联）。
    pub css: HashMap<String, Vec<u8>>,
    /// 已解码图片：绝对 URL → 像素。同上缺席 = 该图所有节点无像素；同
    /// URL 一份像素，装配期按 `Arc` 共享（M1 零拷贝）。
    pub images: HashMap<String, Arc<Image>>,
}

/// 抓取文档的子资源（CSS + 图片）。
///
/// `base_url` 是文档 URL（相对 URL 的解析基准）；`about:blank` 等无网络
/// 内部页直接传空文档清单即可（本函数对空清单返回空结果）。不需 settle
/// 信号时用本便捷封装，等价于 settle 回调为空。
#[must_use]
pub fn fetch_subresources(
    fetcher: &dyn NetworkFetcher,
    base_url: &str,
    document: &Document,
) -> Subresources {
    fetch_subresources_with_settle(fetcher, base_url, document, &mut || {})
}

/// 抓取文档的子资源，并在**首轮编排 settle** 时触发一次 `on_settle`（K1 T3）。
///
/// settle = 清单里的全部抓取/解码任务都已出结果——个别资源的失败/超时/
/// 超预算按降级语义跳过、不阻塞编排，因此无论结果如何回调恰触发一次。
/// 回调供后台抓取线程投递「子资源就绪」信号（load 生命周期事件的派发
/// 门控）：只做信号记录，不得在回调内触碰 `Document`（跨线程约束——
/// Document 随页面载荷回主线程后才可派发）。
#[must_use]
pub fn fetch_subresources_with_settle(
    fetcher: &dyn NetworkFetcher,
    base_url: &str,
    document: &Document,
    on_settle: &mut dyn FnMut(),
) -> Subresources {
    let (css, images) = fetch_resolved(fetcher, base_url, document, on_settle);
    assemble_from_maps(document, base_url, &css, &images)
}

/// 抓取后台数据面（K1 T3）：规划解析清单 → 并发抓取 → 解码，全部产出为
/// `Send` 数据（无 `Document`/`NodeId`）。
///
/// `document` 是调用方在后台线程的规划解析（同一 HTML 稍后由主线程重建
/// 正式文档）；`html` 原样随产物回传。settle 回调语义同
/// [`fetch_subresources_with_settle`]。
#[must_use]
pub fn fetch_page_data(
    fetcher: &dyn NetworkFetcher,
    base_url: &str,
    document: &Document,
    html: String,
    on_settle: &mut dyn FnMut(),
) -> FetchedPageData {
    let (css, images) = fetch_resolved(fetcher, base_url, document, on_settle);
    FetchedPageData { html, css, images }
}

/// 主文档抓取编排的产物（K3 T5）：[`fetch_document`] 的成功输出，全部为
/// `Send` 数据面，供后台线程随事件载荷送回主线程装配。
pub struct FetchedDocument {
    /// 页面数据面（HTML + 子资源字节/像素）。
    pub data: FetchedPageData,
    /// 文档 base：响应的最终 URL（重定向落点；无重定向即请求 URL——
    /// WHATWG fetch 的 final URL 语义）。子资源相对地址的解析与装配查表
    /// 都按它进行：重定向后相对子资源随落点而非原始请求 URL。
    pub base_url: String,
}

/// 抓取并编排主文档（导航管线的后台数据面入口，K3 T5）。
///
/// 编排序列与降级语义（错误串一律收敛，供错误页按页面语义渲染）：
/// - 抓取失败（URL 非法 / 传输 / 超时）→ `Err`，错误串收敛为「错误类别」——
///   不回显上游原始错误（reqwest 的错误文本内含目标 URL，防注入）；
/// - 非 2xx → `Err("HTTP <状态码>")`；
/// - 响应体超 [`MAX_DOCUMENT_BODY_BYTES`] → `Err`（巨响应的分配失败是
///   abort，panic 兜底兜不住）；
/// - 文本解码按 [`decode_document`] 的四层优先级（HTTP charset > BOM >
///   meta 预扫 > UTF-8），替换语义兜底不失败——非 UTF-8 页面不再进错误页；
/// - 解析与子资源编排见 [`fetch_page_data`]（settle 恰触发一次，含失败 /
///   超时 / 预算降级路径）。
///
/// `fetcher` 通常是被 `nexty_network::CachedFetcher` 装饰的共享实例（K3
/// T4/T5）：主文档与子资源同栈，同 URL 二次导航直接命中内存缓存。
///
/// # Errors
///
/// 抓取失败、非 2xx、响应体超预算时返回用户可读的错误类别串。
pub fn fetch_document(fetcher: &dyn NetworkFetcher, url: &str) -> Result<FetchedDocument, String> {
    let request = Request {
        url: url.to_owned(),
        method: Method::Get,
        // 不少真实站点对无 UA 请求直接 403/429
        headers: vec![(
            "User-Agent".to_owned(),
            format!("Nexty/{}", env!("CARGO_PKG_VERSION")),
        )],
    };
    let response = fetcher
        .fetch(&request)
        .map_err(|error| load_error_text(&error).to_owned())?;
    if !(200..300).contains(&response.status) {
        return Err(format!("HTTP {}", response.status));
    }
    if response.body.len() > MAX_DOCUMENT_BODY_BYTES {
        return Err("响应体超出预算".to_owned());
    }
    let content_type = content_type_header(&response.headers);
    let decoded = decode_document(content_type, &response.body);
    let document = nexty_html::parse_document(&decoded.text, nexty_html::ParseOptions::default());
    // 子资源 base = 最终 URL（重定向落点）；mock 后端无重定向时即请求 URL
    let data = fetch_page_data(
        fetcher,
        &response.final_url,
        &document,
        decoded.text,
        &mut || {},
    );
    Ok(FetchedDocument {
        data,
        base_url: response.final_url,
    })
}

/// 传输失败 → 错误类别文本。
///
/// 不回显上游原始错误串：reqwest 的错误文本内含目标 URL，而错误页是按
/// 页面语义渲染的 HTML——错误信息只保留「类别」这一用户需要知道的内容。
fn load_error_text(error: &NetworkError) -> &'static str {
    match error {
        NetworkError::InvalidUrl => "无效的地址",
        NetworkError::Timeout => "请求超时",
        NetworkError::Transport => "网络错误",
    }
}

/// 取响应头里的 Content-Type 值（头名不区分大小写；reqwest 的头表不保序，
/// 条目完整但顺序与源报文无关）。
fn content_type_header(headers: &[(String, String)]) -> Option<&str> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.as_str())
}

/// 地址栏输入的 URL 规范化（K3 T5 决策，完整记录见
/// `docs/decisions/2026-10-31-addressbar-url.md`）。
///
/// 输入缺 scheme 时补 `http://`（HTTP-first，与主流浏览器地址栏默认一致；
/// 不猜测 `www.` 前缀、不在 http/https 间猜测升级）。scheme 判定按 WHATWG
/// URL 的 scheme 语法前缀匹配（`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )
/// ":"`）——带合法 scheme 的输入（`https://…`、`about:blank`、`mailto:…`）
/// 原样返回，各自走到其语义终点。合法性不在这里前置校验：非法输入交给
/// 抓取路径，由协议白名单判 `InvalidUrl` 进既有错误页（单点错误路径）。
#[must_use]
pub fn normalize_address_bar_input(input: &str) -> String {
    if has_url_scheme(input) {
        input.to_owned()
    } else {
        format!("http://{input}")
    }
}

/// 输入是否以 WHATWG URL 的 scheme 前缀开头
/// （`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ) ":"`）。
fn has_url_scheme(input: &str) -> bool {
    let mut chars = input.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    for char in chars {
        if char == ':' {
            return true;
        }
        if !(char.is_ascii_alphanumeric() || char == '+' || char == '-' || char == '.') {
            return false;
        }
    }
    false
}

/// 把后台数据面装配为子资源（主线程调用）：按**本线程文档**的清单顺序
/// 组装——inline 用原文、外链按绝对 URL 查表，失败/缺席的槽位跳过（降级
/// 语义与 [`fetch_subresources`] 一致）；图片按绝对 URL 查表映射到节点，
/// 同 URL 共享一份像素（M1）。
#[must_use]
pub fn assemble_page_data(
    document: &Document,
    base_url: &str,
    fetched: &FetchedPageData,
) -> Subresources {
    assemble_from_maps(document, base_url, &fetched.css, &fetched.images)
}

/// 共享抓取核心：收集清单 → URL 解析/去重 → 并发抓取（按类别带预算）→
/// 解码，成功者按绝对 URL 打包。settle（含失败/超时/预算降级路径）在
/// 返回前恰触发一次回调。
fn fetch_resolved(
    fetcher: &dyn NetworkFetcher,
    base_url: &str,
    document: &Document,
    on_settle: &mut dyn FnMut(),
) -> (HashMap<String, Vec<u8>>, HashMap<String, Arc<Image>>) {
    let style_resources = nexty_html::collect_style_resources(document);
    let image_resources = nexty_html::collect_image_resources(document);

    // ---- 阶段一：解析 URL，编出抓取任务（同 URL 只抓一次）----
    let mut css_seen: HashSet<String> = HashSet::new();
    let mut css_tasks: Vec<String> = Vec::new();
    let mut image_seen: HashSet<String> = HashSet::new();
    let mut image_tasks: Vec<String> = Vec::new();

    for resource in &style_resources {
        if let StyleResource::External { href } = resource
            && let Ok(url) = resolve(base_url, href)
            && css_seen.insert(url.clone())
        {
            css_tasks.push(url);
        }
    }

    for resource in &image_resources {
        if let Ok(url) = resolve(base_url, &resource.src)
            && image_seen.insert(url.clone())
        {
            image_tasks.push(url);
        }
    }

    // ---- 阶段二：并发抓取（分块限流，按类别带响应体预算）----
    let css_bytes = fetch_many(fetcher, &css_tasks, MAX_STYLESHEET_BODY_BYTES);
    let image_bytes = fetch_many(fetcher, &image_tasks, MAX_IMAGE_BODY_BYTES);

    // ---- 阶段三：成功者按绝对 URL 打包（图片解码在本线程完成）----
    let mut css = HashMap::new();
    for (task, url) in css_tasks.iter().enumerate() {
        if let Some(bytes) = &css_bytes[task] {
            css.insert(url.clone(), bytes.clone());
        }
    }
    let mut images: HashMap<String, Arc<Image>> = HashMap::new();
    for (task, url) in image_tasks.iter().enumerate() {
        let Some(bytes) = &image_bytes[task] else {
            continue; // 抓取失败：该图所有节点都无像素（降级）
        };
        if let Ok(image) = decode(bytes) {
            images.insert(url.clone(), Arc::new(image));
        } // 解码失败：同样降级
    }

    // ---- 首轮编排 settle：全部抓取/解码任务已出结果（含降级路径），
    // 恰触发一次回调 ----
    on_settle();

    (css, images)
}

/// 按**本线程文档**的清单顺序装配（inline 用原文、外链按绝对 URL 查表、
/// 图片按 URL 映射到节点）：顺序即级联优先级，失败/缺席槽位跳过。
fn assemble_from_maps(
    document: &Document,
    base_url: &str,
    css: &HashMap<String, Vec<u8>>,
    images: &HashMap<String, Arc<Image>>,
) -> Subresources {
    let mut stylesheets = Vec::new();
    for resource in nexty_html::collect_style_resources(document) {
        match resource {
            StyleResource::Inline { css } => stylesheets.push(Stylesheet::parse(&css)),
            StyleResource::External { href } => {
                if let Ok(url) = resolve(base_url, &href)
                    && let Some(bytes) = css.get(&url)
                {
                    let css = String::from_utf8_lossy(bytes);
                    stylesheets.push(Stylesheet::parse(&css));
                }
            }
        }
    }

    let mut images_by_node: HashMap<NodeId, Arc<Image>> = HashMap::new();
    for resource in nexty_html::collect_image_resources(document) {
        if let Ok(url) = resolve(base_url, &resource.src)
            && let Some(image) = images.get(&url)
        {
            // 一份像素按 Arc 共享给该 URL 的全部节点（M1 零拷贝）
            images_by_node.insert(resource.node, Arc::clone(image));
        }
    }

    Subresources {
        stylesheets,
        images: images_by_node,
    }
}

/// @font-face 待抓取源（K4 T3）：绝对 URL + 注册用描述符。
///
/// 同一 URL 可携带不同描述符出现多次（同一文件按多条 `@font-face` 声明为
/// 不同 weight/style face）；抓取按 URL 去重，注册按描述符逐条进行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontRequest {
    /// 字体文件的绝对地址（已按文档 base 解析，`FetchedDocument.base_url`
    /// 语义——重定向落点）。
    pub url: String,
    /// 注册描述符：css `FontFaceRule` → text `FontFaceDescriptor` 映射
    /// （weight 缺省 400、style 缺省 normal——CSS Fonts 描述符 initial）。
    pub descriptor: FontFaceDescriptor,
}

/// 抓取成功的字体字节（后台线程 → 主线程注册，全 `Send` 数据面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedFont {
    /// 来源绝对 URL（会话去重键）。
    pub url: String,
    /// 注册描述符（与请求一致）。
    pub descriptor: FontFaceDescriptor,
    /// 字体文件字节。
    pub bytes: Vec<u8>,
}

/// 一批字体字节的注册结果（K4 T3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FontRegistration {
    /// 新注册成功的 face 数：> 0 时调用方触发恰好一轮重排 + 重绘。
    pub registered: usize,
    /// 注册失败（非字体数据 / 空族名）跳过的源数：页面回退系统字体照常
    /// 渲染（降级语义）。
    pub rejected: usize,
}

/// 字体子资源编排会话（K4 T3）：会话级注册表 + 已注册 URL 集。
///
/// 会话级 = 浏览器生命周期内持续有效：跨导航共享已注册字体（同 URL 重复
/// 导航不再抓取）。三段编排与主文档抓取同构，全部为纯逻辑、可无头测试：
/// ① [`FontOrchestrator::collect`]（主线程，页面装配后）收集待抓取源；
/// ② [`fetch_fonts`]（后台线程）并发抓取；
/// ③ [`FontOrchestrator::register`]（主线程，字节到达）注册进注册表。
///
/// 注册表经 [`FontOrchestrator::registry`] 交给 `ParleyTextShaper::with_registry`
/// 接线（构造点一次完成）；此后每次 `register` 即生效，无需重建整形器
/// （nexty-text T2 语义）。本类型不持 `Document`，跨线程边界只传
/// [`FontRequest`] / [`FetchedFont`] 数据面。
#[derive(Default)]
pub struct FontOrchestrator {
    /// 会话级 @font-face 注册表（`Clone` = 廉价共享句柄）。
    registry: FontRegistry,
    /// 已成功注册的字体 URL 集（会话去重；只记成功——负结果允许重试）。
    fetched: HashSet<String>,
}

impl FontOrchestrator {
    /// 空会话（空注册表）。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 会话级注册表句柄（供 `ParleyTextShaper::with_registry` 接线）。
    #[must_use]
    pub fn registry(&self) -> &FontRegistry {
        &self.registry
    }

    /// 收集阶段（主线程，页面装配后）：从解析后的作者样式表收集 `@font-face`
    /// 的 `url()` 源（[CSS Fonts 4 §4](https://drafts.csswg.org/css-fonts-4/#font-face-rule)
    /// 简化档）。
    ///
    /// 过滤规则：`local()` 源不抓取；format 提示含 woff/woff2 的源跳过
    /// （harfrust 不消费，偏差）；URL 非法跳过；会话内已注册 URL 跳过；
    /// 同页内同（URL, 描述符）去重。描述符映射按 css 层映射表：weight
    /// `unwrap_or(400)`、style `None→Normal` / `Italic→Italic`。
    #[must_use]
    pub fn collect(&self, stylesheets: &[Stylesheet], base_url: &str) -> Vec<FontRequest> {
        let mut requests: Vec<FontRequest> = Vec::new();
        // 页内去重键（URL, family, weight, style）：同 URL 同描述符只出一项
        let mut seen: HashSet<(String, String, u16, FontStyle)> = HashSet::new();
        for sheet in stylesheets {
            for face in sheet.font_faces() {
                let descriptor = FontFaceDescriptor {
                    family: face.family.clone(),
                    weight: face.weight.unwrap_or(400),
                    style: match face.style {
                        Some(FontFaceStyle::Italic) => FontStyle::Italic,
                        Some(FontFaceStyle::Normal) | None => FontStyle::Normal,
                    },
                };
                for source in &face.sources {
                    let FontFaceSource::Url { url, format_hints } = source else {
                        continue; // local()：已安装字体，不经网络
                    };
                    if format_hints.iter().any(|hint| is_skipped_font_format(hint)) {
                        continue; // woff/woff2：harfrust 不消费（偏差），跳过
                    }
                    let Ok(absolute) = resolve(base_url, url) else {
                        continue; // URL 非法：降级跳过（回退系统字体）
                    };
                    if self.fetched.contains(&absolute) {
                        continue; // 会话去重：同 URL 已注册，不重复抓取
                    }
                    let key = (
                        absolute.clone(),
                        descriptor.family.clone(),
                        descriptor.weight,
                        descriptor.style,
                    );
                    if seen.insert(key) {
                        requests.push(FontRequest {
                            url: absolute,
                            descriptor: descriptor.clone(),
                        });
                    }
                }
            }
        }
        requests
    }

    /// 注册阶段（主线程，字节到达）：逐条注册进注册表。
    ///
    /// `InvalidFontData` / `EmptyFamily` 跳过并记入 [`FontRegistration::rejected`]，
    /// 页面回退系统字体照常渲染（降级语义）；注册成功的 URL 记入会话集
    /// （跨导航去重）。调用方按 [`FontRegistration::registered`] > 0 触发
    /// **恰好一轮**重排 + 重绘——重复/迟到载荷注册数为 0，不产生额外重排。
    pub fn register(&mut self, fonts: Vec<FetchedFont>) -> FontRegistration {
        let mut result = FontRegistration::default();
        for font in fonts {
            // 会话去重：同 URL 已注册的载荷（迟到 / 竞态重复抓取）跳过——
            // 这是「registered > 0 恰好出现一次 → 重排恰一轮」的保证。
            if self.fetched.contains(&font.url) {
                continue;
            }
            match self.registry.register(font.bytes, font.descriptor) {
                Ok(()) => {
                    // 只记成功：失败的源（404 / 非字体数据）再次导航时重试
                    self.fetched.insert(font.url);
                    result.registered += 1;
                }
                Err(_) => result.rejected += 1,
            }
        }
        result
    }
}

/// 抓取阶段（后台线程）：并发抓取一批字体源，成功者逐请求打包。
///
/// 同 URL 只抓一次（复用既有子资源 URL 键控语义，多个描述符共享一份字节）；
/// 预算 [`MAX_FONT_BODY_BYTES`]，超预算 / 非 2xx / 传输错误按降级语义缺席
/// （该源回退系统字体）。空请求列表返回空结果，不起任何抓取。
#[must_use]
pub fn fetch_fonts(fetcher: &dyn NetworkFetcher, requests: &[FontRequest]) -> Vec<FetchedFont> {
    // URL 去重（保序）：同 URL 只抓一次
    let mut urls: Vec<String> = Vec::new();
    for request in requests {
        if !urls.contains(&request.url) {
            urls.push(request.url.clone());
        }
    }
    let bodies = fetch_many(fetcher, &urls, MAX_FONT_BODY_BYTES);
    requests
        .iter()
        .filter_map(|request| {
            let index = urls.iter().position(|url| *url == request.url)?;
            let bytes = bodies[index].clone()?;
            Some(FetchedFont {
                url: request.url.clone(),
                descriptor: request.descriptor.clone(),
                bytes,
            })
        })
        .collect()
}

/// harfrust 不消费的容器格式提示（woff / woff2，K4 记偏差）：带这些提示的
/// 源跳过抓取。比较不区分 ASCII 大小写（format 串规范未约束大小写）。
fn is_skipped_font_format(hint: &str) -> bool {
    hint.eq_ignore_ascii_case("woff") || hint.eq_ignore_ascii_case("woff2")
}

/// 并发抓取一组 URL，返回与 `urls` 等长的结果槽（失败或超预算为 `None`）。
fn fetch_many(
    fetcher: &dyn NetworkFetcher,
    urls: &[String],
    max_body_bytes: usize,
) -> Vec<Option<Vec<u8>>> {
    let mut results: Vec<Option<Vec<u8>>> = Vec::with_capacity(urls.len());
    for chunk in urls.chunks(MAX_CONCURRENT_FETCHES) {
        let chunk_results: Vec<Option<Vec<u8>>> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|url| {
                    let request = Request {
                        url: url.clone(),
                        method: Method::Get,
                        headers: Vec::new(),
                    };
                    scope.spawn(move || fetch_ok(fetcher, &request, max_body_bytes))
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().ok().flatten())
                .collect()
        });
        results.extend(chunk_results);
    }
    results
}

/// 抓取并把非 2xx / 错误 / 超预算归为 `None`（降级语义）。
fn fetch_ok(
    fetcher: &dyn NetworkFetcher,
    request: &Request,
    max_body_bytes: usize,
) -> Option<Vec<u8>> {
    match fetcher.fetch(request) {
        Ok(response) if (200..300).contains(&response.status) => {
            if response.body.len() > max_body_bytes {
                None
            } else {
                Some(response.body)
            }
        }
        Ok(_) => None,
        Err(NetworkError::InvalidUrl | NetworkError::Transport | NetworkError::Timeout) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexty_html::ParseOptions;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// 内存 mock fetcher：URL → 响应（body 或状态码）。未登记的 URL 返回 404。
    struct MockFetcher {
        routes: Mutex<BTreeMap<String, Result<Vec<u8>, u16>>>,
        /// 抓取计数（验证去重）。
        hits: Mutex<Vec<String>>,
    }

    impl MockFetcher {
        fn new() -> Self {
            Self {
                routes: Mutex::new(BTreeMap::new()),
                hits: Mutex::new(Vec::new()),
            }
        }

        fn serve(&self, url: &str, body: &[u8]) {
            self.routes
                .lock()
                .unwrap()
                .insert(url.to_owned(), Ok(body.to_vec()));
        }

        fn fail(&self, url: &str, status: u16) {
            self.routes
                .lock()
                .unwrap()
                .insert(url.to_owned(), Err(status));
        }
    }

    impl NetworkFetcher for MockFetcher {
        fn fetch(&self, request: &Request) -> Result<nexty_network::Response, NetworkError> {
            self.hits.lock().unwrap().push(request.url.clone());
            let routes = self.routes.lock().unwrap();
            match routes.get(&request.url) {
                Some(Ok(body)) => Ok(nexty_network::Response {
                    status: 200,
                    headers: Vec::new(),
                    body: body.clone(),
                    final_url: request.url.clone(),
                }),
                Some(Err(status)) => Ok(nexty_network::Response {
                    status: *status,
                    headers: Vec::new(),
                    body: Vec::new(),
                    final_url: request.url.clone(),
                }),
                None => Ok(nexty_network::Response {
                    status: 404,
                    headers: Vec::new(),
                    body: Vec::new(),
                    final_url: request.url.clone(),
                }),
            }
        }
    }

    fn document(html: &str) -> Document {
        nexty_html::parse_document(html, ParseOptions::default())
    }

    /// 最小合法 PNG：1×1 红色像素。
    fn tiny_png() -> Vec<u8> {
        let mut buffer = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([255, 0, 0, 255]),
        ))
        .write_to(
            &mut std::io::Cursor::new(&mut buffer),
            image::ImageFormat::Png,
        )
        .expect("encode png");
        buffer
    }

    const BASE: &str = "https://page.test/index.html";

    #[test]
    fn inline_and_external_stylesheets_cascade_in_tree_order() {
        let fetcher = MockFetcher::new();
        fetcher.serve(
            "https://page.test/ext.css",
            b"p { color: blue; border: 1px solid black }",
        );
        let document = document(
            "<head><style>p { color: red }</style>\
             <link rel=stylesheet href=ext.css></head><body><p>x</p></body>",
        );

        let resources = fetch_subresources(&fetcher, BASE, &document);
        assert_eq!(resources.stylesheets.len(), 2, "inline + 外链");

        // 后者覆盖前者：border 来自外链表，color 也应是 blue
        let sheet = resources.stylesheets.last().unwrap();
        let colors: Vec<_> = sheet
            .rules()
            .iter()
            .flat_map(|rule| rule.declarations())
            .filter(|declaration| declaration.property == nexty_css::PropertyId::Color)
            .collect();
        assert!(!colors.is_empty(), "外链表的颜色声明应保留");
    }

    #[test]
    fn failing_stylesheet_is_skipped_without_blocking_others() {
        let fetcher = MockFetcher::new();
        fetcher.serve("https://page.test/ok.css", b"div { margin: 0 }");
        fetcher.fail("https://page.test/broken.css", 500);
        let document = document(
            "<head>\
             <link rel=stylesheet href=broken.css>\
             <style>p { color: red }</style>\
             <link rel=stylesheet href=ok.css>\
             </head><body><p>x</p></body>",
        );

        let resources = fetch_subresources(&fetcher, BASE, &document);
        assert_eq!(
            resources.stylesheets.len(),
            2,
            "失败的表跳过，其余两张按序保留"
        );
    }

    #[test]
    fn images_decode_and_map_to_nodes_deduped_by_url() {
        let fetcher = MockFetcher::new();
        fetcher.serve("https://page.test/logo.png", &tiny_png());
        let document = document("<body><img src=logo.png><p>x</p><img src=logo.png></body>");

        let resources = fetch_subresources(&fetcher, BASE, &document);
        assert_eq!(resources.images.len(), 2, "两个节点都有像素");
        // URL 只抓一次（去重）
        assert_eq!(
            fetcher
                .hits
                .lock()
                .unwrap()
                .iter()
                .filter(|url| **url == "https://page.test/logo.png")
                .count(),
            1
        );
    }

    #[test]
    fn failing_or_undecodable_image_degrades_to_missing_pixels() {
        let fetcher = MockFetcher::new();
        fetcher.fail("https://page.test/lost.png", 404);
        fetcher.serve("https://page.test/junk.png", b"not an image");
        let document = document("<body><img src=lost.png><img src=junk.png></body>");

        let resources = fetch_subresources(&fetcher, BASE, &document);
        assert!(resources.images.is_empty(), "失败图片不产生像素");
    }

    #[test]
    fn relative_urls_resolve_against_document_base() {
        let fetcher = MockFetcher::new();
        // /dir/page.html 的 ../css/a.css → https://page.test/css/a.css
        fetcher.serve("https://page.test/css/a.css", b"body { margin: 0 }");
        let document =
            document("<head><link rel=stylesheet href=\"../css/a.css\"></head><body></body>");
        let resources = fetch_subresources(&fetcher, "https://page.test/dir/page.html", &document);
        assert_eq!(resources.stylesheets.len(), 1);
        assert!(
            fetcher
                .hits
                .lock()
                .unwrap()
                .contains(&"https://page.test/css/a.css".to_owned()),
            "相对 URL 按 base 解析"
        );
    }

    // ---- 响应体预算：超预算等同抓取失败（降级） ----

    #[test]
    fn oversized_bodies_are_skipped_within_budget_path_unchanged() {
        let fetcher = MockFetcher::new();
        // 样式表预算 10 MB：超预算的表整体跳过
        let huge_css = vec![b' '; MAX_STYLESHEET_BODY_BYTES + 1];
        fetcher.serve("https://page.test/huge.css", &huge_css);
        // 图片预算 50 MB：超预算跳过
        let huge_image = vec![0u8; MAX_IMAGE_BODY_BYTES + 1];
        fetcher.serve("https://page.test/huge.png", &huge_image);
        // 预算内的正常路径回归
        fetcher.serve("https://page.test/ok.css", b"p { color: red }");
        fetcher.serve("https://page.test/ok.png", &tiny_png());
        let document = document(
            "<head><link rel=stylesheet href=huge.css>\
             <link rel=stylesheet href=ok.css></head>\
             <body><img src=huge.png><img src=ok.png></body>",
        );

        let resources = fetch_subresources(&fetcher, BASE, &document);
        assert_eq!(
            resources.stylesheets.len(),
            1,
            "超预算样式表跳过，预算内的保留"
        );
        assert_eq!(resources.images.len(), 1, "超预算图片跳过，预算内的保留");
    }

    // ---- 地址栏输入规范化（K3 T5）----

    #[test]
    fn address_bar_input_without_scheme_gets_http_prefix() {
        assert_eq!(
            normalize_address_bar_input("example.com"),
            "http://example.com"
        );
        assert_eq!(
            normalize_address_bar_input("example.com/path?q=1"),
            "http://example.com/path?q=1"
        );
        // 空输入同样补全：交给抓取路径进错误页（不白屏、不 panic）
        assert_eq!(normalize_address_bar_input(""), "http://");
    }

    #[test]
    fn address_bar_input_with_scheme_is_untouched() {
        assert_eq!(
            normalize_address_bar_input("https://example.com"),
            "https://example.com"
        );
        assert_eq!(
            normalize_address_bar_input("http://example.com"),
            "http://example.com"
        );
        // 内部 scheme 与非网络 scheme 原样返回，各自走到其语义终点
        assert_eq!(normalize_address_bar_input("about:blank"), "about:blank");
        assert_eq!(
            normalize_address_bar_input("mailto:a@b.test"),
            "mailto:a@b.test"
        );
    }

    #[test]
    fn address_bar_scheme_prefix_follows_whatwg_syntax() {
        // scheme 首字符必须是字母，否则视为无 scheme
        assert_eq!(normalize_address_bar_input("1http://x"), "http://1http://x");
        // scheme 字符集外（空格）截断判定 → 无 scheme
        assert_eq!(normalize_address_bar_input("ht tp://x"), "http://ht tp://x");
        // scheme 允许字母/数字/+-.
        assert_eq!(normalize_address_bar_input("a-b.c+x:rest"), "a-b.c+x:rest");
    }

    // ---- @font-face 字体子资源编排（K4 T3）----

    /// 测试字体字节：nexty-text T2 入库的 OFL 字体资产（工作区内复用同一
    /// 份，不另入库副本）。
    fn test_font_bytes() -> Vec<u8> {
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../nexty-text/tests/data/SpaceMono-Regular.ttf"
        ))
        .expect("test font asset (nexty-text tests/data)")
    }

    /// 构造一个字体抓取请求。
    fn font_request(url: &str, family: &str, weight: u16, style: FontStyle) -> FontRequest {
        FontRequest {
            url: url.to_owned(),
            descriptor: FontFaceDescriptor {
                family: family.to_owned(),
                weight,
                style,
            },
        }
    }

    /// 描述符映射（weight 缺省 400、style 缺省 normal/italic）+ 过滤
    /// （woff/woff2 提示、local()）+ 页内去重。
    #[test]
    fn font_requests_map_descriptors_and_filter_sources() {
        let sheet = Stylesheet::parse(
            "@font-face { font-family: A; src: url(a.ttf); } \
             @font-face { font-family: W; src: url(w.woff2) format(\"woff2\"); } \
             @font-face { font-family: W2; src: url(w2.woff) format(\"woff\"); } \
             @font-face { font-family: L; src: local(\"Sys X\"), url(l.ttf); font-weight: 700; font-style: italic; } \
             @font-face { font-family: OnlyLocal; src: local(\"Sys Y\"); } \
             @font-face { font-family: A; src: url(a.ttf); }",
        );
        let fonts = FontOrchestrator::new();
        let requests = fonts.collect(std::slice::from_ref(&sheet), BASE);
        assert_eq!(
            requests,
            vec![
                font_request("https://page.test/a.ttf", "A", 400, FontStyle::Normal),
                font_request("https://page.test/l.ttf", "L", 700, FontStyle::Italic),
            ],
            "woff/woff2 提示与 local() 跳过；描述符按映射表（缺省 400/normal，italic 映射）；页内同 URL 同描述符去重"
        );
    }

    /// 相对 URL 按文档 base 解析、非法 URL 跳过、会话去重（注册成功后
    /// 同 URL 再收集为空——同 URL 重复导航不重复抓取的收集侧语义）。
    #[test]
    fn font_requests_resolve_base_skip_invalid_and_dedupe_by_session() {
        let sheet = Stylesheet::parse(
            "@font-face { font-family: A; src: url(../fonts/a.ttf); } \
             @font-face { font-family: Bad; src: url(\"http://bad host/x.ttf\"); }",
        );
        let fonts = FontOrchestrator::new();
        let requests = fonts.collect(
            std::slice::from_ref(&sheet),
            "https://page.test/dir/page.html",
        );
        assert_eq!(
            requests,
            vec![font_request(
                "https://page.test/fonts/a.ttf",
                "A",
                400,
                FontStyle::Normal
            )],
            "相对 URL 按 base 解析，非法 URL 跳过"
        );

        let mut fonts = FontOrchestrator::new();
        let sheet = Stylesheet::parse("@font-face { font-family: A; src: url(a.ttf) }");
        assert_eq!(fonts.collect(std::slice::from_ref(&sheet), BASE).len(), 1);
        let registered = fonts.register(vec![FetchedFont {
            url: "https://page.test/a.ttf".to_owned(),
            descriptor: font_request("x", "A", 400, FontStyle::Normal).descriptor,
            bytes: test_font_bytes(),
        }]);
        assert_eq!(registered.registered, 1);
        assert!(
            fonts.collect(std::slice::from_ref(&sheet), BASE).is_empty(),
            "注册成功后同 URL 不再收集（跨导航会话去重）"
        );
    }

    /// 抓取：同 URL 只抓一次（多描述符共享一份字节）；404 / 超预算按
    /// 降级语义缺席。
    #[test]
    fn fetch_fonts_dedupes_urls_and_degrades_failures_and_budget() {
        let fetcher = MockFetcher::new();
        fetcher.serve("https://page.test/a.ttf", b"font-a");
        fetcher.fail("https://page.test/lost.ttf", 404);
        fetcher.serve(
            "https://page.test/huge.ttf",
            &vec![0u8; MAX_FONT_BODY_BYTES + 1],
        );
        let requests = vec![
            font_request("https://page.test/a.ttf", "A", 400, FontStyle::Normal),
            font_request("https://page.test/a.ttf", "A", 700, FontStyle::Normal),
            font_request("https://page.test/lost.ttf", "B", 400, FontStyle::Normal),
            font_request("https://page.test/huge.ttf", "C", 400, FontStyle::Normal),
        ];

        let fonts = fetch_fonts(&fetcher, &requests);
        assert_eq!(
            fetcher
                .hits
                .lock()
                .unwrap()
                .iter()
                .filter(|url| **url == "https://page.test/a.ttf")
                .count(),
            1,
            "同 URL 只抓一次"
        );
        assert_eq!(
            fonts.len(),
            2,
            "404 与超预算缺席，成功 URL 的两个描述符都在"
        );
        assert_eq!(fonts[0].bytes, b"font-a");
        assert_eq!(fonts[0].descriptor.weight, 400);
        assert_eq!(
            fonts[1].descriptor.weight, 700,
            "同 URL 不同描述符共享一次抓取"
        );
    }

    /// 注册：非字体数据拒收记数且不入会话集（可重试）；成功注册记入
    /// 会话集；重复/迟到载荷（同 URL 已注册）跳过——恰一轮重排的保证。
    #[test]
    fn register_rejects_invalid_data_marks_success_and_skips_duplicates() {
        let mut fonts = FontOrchestrator::new();
        let junk_sheet = Stylesheet::parse("@font-face { font-family: Junk; src: url(junk.ttf) }");
        let a_sheet = Stylesheet::parse("@font-face { font-family: A; src: url(a.ttf) }");

        let rejected = fonts.register(vec![FetchedFont {
            url: "https://page.test/junk.ttf".to_owned(),
            descriptor: font_request("x", "Junk", 400, FontStyle::Normal).descriptor,
            bytes: b"not a font".to_vec(),
        }]);
        assert_eq!(
            rejected,
            FontRegistration {
                registered: 0,
                rejected: 1
            },
            "非字体数据拒收记数"
        );
        assert_eq!(
            fonts.collect(std::slice::from_ref(&junk_sheet), BASE).len(),
            1,
            "负结果不入会话集，再次导航可重试"
        );

        let font_bytes = test_font_bytes();
        let payload = FetchedFont {
            url: "https://page.test/a.ttf".to_owned(),
            descriptor: font_request("x", "A", 400, FontStyle::Normal).descriptor,
            bytes: font_bytes.clone(),
        };
        let ok = fonts.register(vec![payload.clone()]);
        assert_eq!(
            ok,
            FontRegistration {
                registered: 1,
                rejected: 0
            }
        );
        assert!(
            fonts
                .collect(std::slice::from_ref(&a_sheet), BASE)
                .is_empty(),
            "成功注册后同 URL 不再收集"
        );
        // 重复/迟到载荷：同 URL 已注册 → 跳过（registered == 0 → 不重排）
        let duplicate = fonts.register(vec![payload]);
        assert_eq!(
            duplicate,
            FontRegistration {
                registered: 0,
                rejected: 0
            },
            "同 URL 重复载荷不重复注册"
        );
    }
}
