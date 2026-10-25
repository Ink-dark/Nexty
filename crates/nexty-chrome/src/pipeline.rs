//! 导航管线：HTML 字符串 → 级联 → 布局 → 片段树 → paint 显示列表。
//!
//! 行为分工：解析归 nexty-html，级联归 nexty-css（UA + author 双 origin），
//! 布局归 nexty-layout，本模块负责把片段树转译为 [`nexty_paint::Scene`]：
//! 背景与 CSS 边框（四条填充带，宽 0 的边跳过）、行内文本 run
//! （坐标换算到绝对坐标系）、行内/块级图片（`DrawImage`，像素从页面图片池
//! 按节点索引）。匿名片段不产生背景与边框。
//!
//! 已知偏差：`<html>`/`<body>` 背景不向画布传播（画布保持透明，由呈现层
//! 决定底色）；片段树不做滚动裁剪。

use std::collections::HashMap;
use std::sync::Arc;

use nexty_css::{ComputedStyle, MediaViewport, Rgba, Stylesheet, html_ua_stylesheet};
use nexty_dom::{Document, NodeId};
use nexty_html::ParseOptions;
use nexty_layout::{Fragment, Rect as LayoutRect};
use nexty_paint::{Color, Command, Image, Rect as PaintRect, Scene, Size, TextGlyph};
use nexty_text::TextShaper;

/// 一个已解析、已级联的页面。
pub struct Page {
    /// arena 文档。
    pub document: Document,
    /// 各元素的 computed style。
    pub styles: HashMap<NodeId, ComputedStyle>,
    /// 作者样式表（视口变化后重级联用；UA 样式表固定不存）。
    stylesheets: Vec<Stylesheet>,
    /// 内联样式解析缓存（M5 ②）：resize 后 recascade 按 style 属性原文
    /// 校验复用，不重解析字符串。
    inline_style_cache: nexty_css::InlineStyleCache,
    /// 已解码图片（`<img>` 节点 → RGBA 图像），构建 Scene 时进入图片池。
    /// 按 `Arc` 共享：构建/发送/光栅全程零像素拷贝（M1）。
    pub images: HashMap<NodeId, Arc<Image>>,
}

/// 用已解析文档与作者样式表构建页面（UA 样式表固定为 nexty-css 的 HTML 默认）。
///
/// `stylesheets` 按级联优先级排列（CSS Cascade：源顺序靠后者胜出）——
/// inline `<style>` 与外链样式表按文档出现顺序交错。
///
/// 级联在此先做一次（无视口，视口单位保持未折算）；布局前由调用方以
/// 实际视口走 [`recascade`]（后台线程拿不到窗口尺寸）。
#[must_use]
pub fn build_page(
    document: Document,
    stylesheets: Vec<Stylesheet>,
    images: HashMap<NodeId, Arc<Image>>,
) -> Page {
    let sheet_refs: Vec<&Stylesheet> = stylesheets.iter().collect();
    let mut inline_style_cache = nexty_css::InlineStyleCache::new();
    let styles = nexty_css::compute_document_styles_in_with_cache(
        &document,
        &sheet_refs,
        Some(html_ua_stylesheet()),
        None,
        &mut inline_style_cache,
    );
    Page {
        document,
        styles,
        stylesheets,
        inline_style_cache,
        images,
    }
}

/// 按给定视口重新级联：视口单位（vw/vh/vmin/vmax）在 computed 阶段
/// 折算为绝对 px。resize 后布局前必须调用，否则视口尺寸滞后。
pub fn recascade(page: &mut Page, viewport: MediaViewport) {
    let sheet_refs: Vec<&Stylesheet> = page.stylesheets.iter().collect();
    page.styles = nexty_css::compute_document_styles_in_with_cache(
        &page.document,
        &sheet_refs,
        Some(html_ua_stylesheet()),
        Some(viewport),
        &mut page.inline_style_cache,
    );
}

/// 解析并级联一份页面（单张作者样式表，无图片——内部页/空白页用）。
#[must_use]
pub fn load_page(html: &str, author_css: &str) -> Page {
    let document = nexty_html::parse_document(html, ParseOptions::default());
    let sheet = Stylesheet::parse(author_css);
    build_page(document, vec![sheet], HashMap::new())
}

/// 对页面做布局，返回根元素片段。
#[must_use]
pub fn layout_page(page: &Page, shaper: &dyn TextShaper, viewport_width: f32) -> Option<Fragment> {
    // 布局层只要自然尺寸（CSS px = 像素 1:1）
    let image_sizes: HashMap<NodeId, (f32, f32)> = page
        .images
        .iter()
        .map(|(node, image)| (*node, (image.width as f32, image.height as f32)))
        .collect();
    nexty_layout::layout_document(
        &page.document,
        shaper,
        &page.styles,
        &image_sizes,
        viewport_width,
    )
}

/// 选区高亮色（半透明蓝，src-over 叠在页面背景之上）。
const SELECTION_COLOR: Color = Color {
    r: 0,
    g: 100,
    b: 255,
    a: 64,
};

/// 把片段树转译为显示列表，原点在视口左上角。
#[must_use]
pub fn build_scene(page: &Page, root: &Fragment) -> Scene {
    build_scene_at(page, root, 0.0, 0.0)
}

/// 把片段树转译为显示列表，并整体平移到 `(origin_x, origin_y)`。
///
/// 用于给页面内容留出顶部 UI（如地址栏）占用的区域。图片按绘制顺序进入
/// `Scene.images` 池，`DrawImage` 按下标引用（同一节点多图共享一份像素）。
#[must_use]
pub fn build_scene_at(page: &Page, root: &Fragment, origin_x: f32, origin_y: f32) -> Scene {
    build_scene_at_with_highlights(page, root, origin_x, origin_y, &[])
}

/// 同 [`build_scene_at`]，并在每个行盒的文字之前插入选区高亮矩形。
///
/// `highlights` 是 `TextMap::rects` 的输出（相对根片段边框盒）。高亮按行
/// 发射：只在该行自身字形与矩形相交时绘制，故位于页面背景/边框之上、
/// 文字与图片之下；嵌套行内原子盒的矩形不会在父行重复绘制。
#[must_use]
pub fn build_scene_at_with_highlights(
    page: &Page,
    root: &Fragment,
    origin_x: f32,
    origin_y: f32,
    highlights: &[LayoutRect],
) -> Scene {
    let mut scene = Scene::default();
    let mut pool: HashMap<NodeId, usize> = HashMap::new();
    // 高亮矩形相对根片段；先平移到场景坐标，递归 emit 里不再叠加 origin
    //（emit 的 origin 是父链累积位置，不是场景原点）。
    let highlights: Vec<LayoutRect> = highlights
        .iter()
        .map(|rect| LayoutRect {
            x: rect.x + origin_x,
            y: rect.y + origin_y,
            ..*rect
        })
        .collect();
    emit(
        page,
        root,
        origin_x,
        origin_y,
        &mut scene,
        &mut pool,
        &highlights,
    );
    scene
}

fn emit(
    page: &Page,
    fragment: &Fragment,
    origin_x: f32,
    origin_y: f32,
    scene: &mut Scene,
    pool: &mut HashMap<NodeId, usize>,
    highlights: &[LayoutRect],
) {
    let x = origin_x + fragment.border_box.x;
    let y = origin_y + fragment.border_box.y;
    let width = fragment.border_box.width;
    let height = fragment.border_box.height;
    let style = &fragment.style;

    if !fragment.anonymous {
        // 背景（透明跳过）
        if style.background_color.a > 0 {
            scene.commands.push(Command::FillRect {
                rect: PaintRect {
                    x,
                    y,
                    width,
                    height,
                },
                color: to_color(style.background_color),
            });
        }
        // CSS 边框：画在边框盒内侧，用四条填充带精确表达（宽 0 跳过）
        let border = fragment.border;
        let colors = &style.border_color;
        if border.top > 0.0 {
            push_strip(scene, x, y, width, border.top, colors.top);
        }
        if border.bottom > 0.0 {
            push_strip(
                scene,
                x,
                y + height - border.bottom,
                width,
                border.bottom,
                colors.bottom,
            );
        }
        if border.left > 0.0 {
            push_strip(
                scene,
                x,
                y + border.top,
                border.left,
                height - border.top - border.bottom,
                colors.left,
            );
        }
        if border.right > 0.0 {
            push_strip(
                scene,
                x + width - border.right,
                y + border.top,
                border.right,
                height - border.top - border.bottom,
                colors.right,
            );
        }
    }

    // 内容盒原点：子片段与文本行的参照
    let content_x = x + fragment.border.left + fragment.padding.left;
    let content_y = y + fragment.border.top + fragment.padding.top;

    // 块级替换元素（`<img>` display:block）：内容盒即图片矩形
    if is_image_node(&page.document, fragment.node) {
        draw_image_at(
            page,
            fragment.node,
            PaintRect {
                x: content_x,
                y: content_y,
                width: width
                    - fragment.border.left
                    - fragment.border.right
                    - fragment.padding.left
                    - fragment.padding.right,
                height: height
                    - fragment.border.top
                    - fragment.border.bottom
                    - fragment.padding.top
                    - fragment.padding.bottom,
            },
            scene,
            pool,
        );
    }

    // 行盒纵向堆叠：LineFragment 内坐标是行内局部系（baseline、图片/原子盒
    // 的 y 相对行顶），绘制前累加前面各行高
    let mut line_offset = 0.0_f32;
    for line in &fragment.lines {
        let line_y = content_y + line_offset;
        let baseline = line_y + line.baseline;
        // 选区高亮：垫在本行文字之下；只画与本行自身字形相交的矩形，
        // 避免嵌套行内原子盒的矩形在父行重复叠加。
        for highlight in highlights {
            let highlight_x = highlight.x;
            let highlight_y = highlight.y;
            if (highlight_y - line_y).abs() > 0.5 {
                continue;
            }
            let overlaps = line.runs.iter().flat_map(|run| &run.glyphs).any(|glyph| {
                let glyph_x = content_x + glyph.x;
                highlight_x < glyph_x + glyph.advance && highlight_x + highlight.width > glyph_x
            });
            if overlaps {
                scene.commands.push(Command::FillRect {
                    rect: PaintRect {
                        x: highlight_x,
                        y: highlight_y,
                        width: highlight.width,
                        height: highlight.height,
                    },
                    color: SELECTION_COLOR,
                });
            }
        }
        for run in &line.runs {
            if run.glyphs.is_empty() {
                continue;
            }
            scene.commands.push(Command::DrawText {
                glyphs: run
                    .glyphs
                    .iter()
                    .map(|glyph| TextGlyph {
                        id: glyph.id,
                        x: content_x + glyph.x,
                        y: glyph.y,
                        font: glyph.font,
                    })
                    .collect(),
                baseline,
                fonts: run.fonts.clone(),
                size: run.style.size,
                color: to_color(run.color),
            });
        }
        // 行内图片：底边对齐基线（布局层已折算 y）
        for image in &line.images {
            draw_image_at(
                page,
                image.node,
                PaintRect {
                    x: content_x + image.x,
                    y: line_y + image.y,
                    width: image.width,
                    height: image.height,
                },
                scene,
                pool,
            );
        }
        // 行内原子盒（inline-block 等）：完整子片段树，x/y 已相对行顶定位，
        // 按子片段递归（原点 = 本行顶）
        for atomic in &line.boxes {
            emit(page, atomic, content_x, line_y, scene, pool, highlights);
        }
        line_offset += line.height;
    }

    for child in &fragment.children {
        emit(page, child, content_x, content_y, scene, pool, highlights);
    }
}

/// `node` 是否为 HTML 命名空间的 `<img>`（块级替换路径的判定）。
fn is_image_node(document: &Document, node: NodeId) -> bool {
    use nexty_dom::{Namespace, NodeKind};
    matches!(
        document.node(node),
        Some(NodeKind::Element(data))
            if data.namespace == Namespace::Html && data.name == "img"
    )
}

/// 命中的内容类型（决定悬停光标等交互反馈）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitKind {
    /// 命中的是盒本身（未深入行内内容或块级子盒）。
    Fragment,
    /// 命中了行内文本 run。
    TextRun,
    /// 命中了图片（行内替换元素）。
    Image,
}

/// 命中结果：最深片段节点 + 命中类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    /// 命中的节点。
    pub node: NodeId,
    /// 命中类型。
    pub kind: HitKind,
}

/// 悬停光标语义：链接 → 手型，文本 → I 形，其余 → 默认箭头。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoverKind {
    /// 默认箭头。
    Default,
    /// I 形（文本）。
    Text,
    /// 手型（链接）。
    Pointer,
}

/// 计算页面点 `(x, y)` 的悬停光标语义（坐标与 [`hit_test_ex`] 同系）。
///
/// 链接触发优先于文本：命中节点的祖先链上存在带 `href` 的 `<a>` 即为链接。
#[must_use]
pub fn hover_kind(page: &Page, root: &Fragment, x: f32, y: f32) -> HoverKind {
    let Some(hit) = hit_test_ex(root, x, y) else {
        return HoverKind::Default;
    };
    if link_target(page, hit.node).is_some() {
        return HoverKind::Pointer;
    }
    match hit.kind {
        HitKind::TextRun => HoverKind::Text,
        HitKind::Fragment | HitKind::Image => HoverKind::Default,
    }
}

/// 命中测试：返回视口点 `(x, y)` 命中的最深片段节点与命中类型。
///
/// 坐标以 `root` 的边框盒原点为参照（与 [`build_scene_at`] 的原点参数
/// 一致：窗口坐标减去原点即得）。命中顺序镜像绘制顺序：行内内容
/// （文本 run / 图片 / 行内原子盒）优先于块级子盒；点在盒内但未命中
/// 更深层时返回本盒节点。
#[must_use]
pub fn hit_test_ex(root: &Fragment, x: f32, y: f32) -> Option<Hit> {
    hit_fragment(root, 0.0, 0.0, x, y)
}

/// 命中测试：只取命中的节点（[`hit_test_ex`] 的薄封装）。
#[must_use]
pub fn hit_test(root: &Fragment, x: f32, y: f32) -> Option<NodeId> {
    hit_test_ex(root, x, y).map(|hit| hit.node)
}

/// `hit_test_ex` 的递归体：`(ox, oy)` 为父链累积的本片段边框盒原点。
fn hit_fragment(fragment: &Fragment, ox: f32, oy: f32, x: f32, y: f32) -> Option<Hit> {
    let bx = ox + fragment.border_box.x;
    let by = oy + fragment.border_box.y;
    if x < bx || x > bx + fragment.border_box.width || y < by || y > by + fragment.border_box.height
    {
        return None;
    }
    let content_x = bx + fragment.border.left + fragment.padding.left;
    let content_y = by + fragment.border.top + fragment.padding.top;

    // 行内内容按行堆叠（与 emit 的行偏移累加一致）
    let mut line_offset = 0.0_f32;
    for line in &fragment.lines {
        let line_y = content_y + line_offset;
        if y >= line_y && y <= line_y + line.height {
            // 文本 run：字形 x 范围（glyph.x 已相对本片段内容盒）
            for run in &line.runs {
                let Some(first) = run.glyphs.first() else {
                    continue;
                };
                let Some(last) = run.glyphs.last() else {
                    continue;
                };
                let run_start = content_x + first.x;
                let run_end = content_x + last.x + last.advance;
                if x >= run_start && x <= run_end {
                    return Some(Hit {
                        node: run.node,
                        kind: HitKind::TextRun,
                    });
                }
            }
            for image in &line.images {
                let top = line_y + image.y;
                if x >= content_x + image.x
                    && x <= content_x + image.x + image.width
                    && y >= top
                    && y <= top + image.height
                {
                    return Some(Hit {
                        node: image.node,
                        kind: HitKind::Image,
                    });
                }
            }
            for atomic in &line.boxes {
                if let Some(hit) = hit_fragment(atomic, content_x, line_y, x, y) {
                    return Some(hit);
                }
            }
        }
        line_offset += line.height;
    }

    for child in &fragment.children {
        if let Some(hit) = hit_fragment(child, content_x, content_y, x, y) {
            return Some(hit);
        }
    }
    Some(Hit {
        node: fragment.node,
        kind: HitKind::Fragment,
    })
}

/// `node` 向上找最近的 `<a href>` 祖先（含自身），返回 href 原文。
///
/// 返回的是文档里的原始属性值；相对地址由调用方以文档 URL 为 base
/// 归一（`nexty_network::resolve`）。无 `<a>` 祖先或 href 为空返回 `None`。
pub fn link_target(page: &Page, node: NodeId) -> Option<String> {
    use nexty_dom::{Namespace, NodeKind};
    let mut current = Some(node);
    while let Some(id) = current {
        if let Some(NodeKind::Element(data)) = page.document.node(id)
            && data.namespace == Namespace::Html
            && data.name == "a"
        {
            let href = data
                .attributes
                .iter()
                .find(|attr| attr.name == "href")
                .map(|attr| attr.value.trim().to_owned())
                .unwrap_or_default();
            if href.is_empty() {
                return None;
            }
            return Some(href);
        }
        current = page.document.parent(id);
    }
    None
}

/// 提取文档 `<title>` 的文本（HTML 命名空间的 title 元素，首个非空者）。
///
/// 供窗口标题显示；缺失或空白返回 `None`。SVG 的 `<title>` 不在此列。
pub fn document_title(document: &Document) -> Option<String> {
    use nexty_dom::{Namespace, NodeKind};
    fn walk(document: &Document, node: NodeId) -> Option<String> {
        // Document/文本等非元素节点没有 title，但子树仍需下钻
        if let Some(NodeKind::Element(data)) = document.node(node)
            && data.namespace == Namespace::Html
            && data.name == "title"
        {
            let mut text = String::new();
            for child in document.children(node) {
                if let Some(NodeKind::Text(content)) = document.node(child) {
                    text.push_str(content);
                }
            }
            let trimmed = text.trim();
            return (!trimmed.is_empty()).then(|| trimmed.to_owned());
        }
        document
            .children(node)
            .find_map(|child| walk(document, child))
    }
    walk(document, document.root())
}

/// 在指定矩形下发一张图片：像素进池（按节点去重），指令按下标引用。
fn draw_image_at(
    page: &Page,
    node: NodeId,
    rect: PaintRect,
    scene: &mut Scene,
    pool: &mut HashMap<NodeId, usize>,
) {
    // 未解码成功的图片没有像素（抓取失败降级）：矩形跳过，不影响其余内容
    let Some(source) = page.images.get(&node) else {
        return;
    };
    let index = match pool.get(&node) {
        Some(index) => *index,
        None => {
            let index = scene.images.len();
            // Arc 引用计数共享像素：同一节点多图与跨帧重建均零拷贝（M1）
            scene.images.push(Arc::clone(source));
            pool.insert(node, index);
            index
        }
    };
    scene
        .commands
        .push(Command::DrawImage { rect, image: index });
}

fn push_strip(scene: &mut Scene, x: f32, y: f32, width: f32, height: f32, color: Rgba) {
    if color.a > 0 {
        scene.commands.push(Command::FillRect {
            rect: PaintRect {
                x,
                y,
                width,
                height,
            },
            color: to_color(color),
        });
    }
}

fn to_color(rgba: Rgba) -> Color {
    Color {
        r: rgba.r,
        g: rgba.g,
        b: rgba.b,
        a: rgba.a,
    }
}

/// 视口尺寸别名（paint 的 `Size`）。
pub type Viewport = Size;

/// resize 合帧器（M6）：连发的 `Resized` 事件合并到下一次重绘。
///
/// winit 拖动窗口边缘时 `Resized` 连发，逐事件全管线重排会把管线跑成
/// 每事件一次；合帧后事件只置脏标志，重排在下一次 `RedrawRequested`
/// 里至多执行一次（app 与基准共用本状态机，保证「每帧一次」语义同源）。
#[derive(Debug, Default)]
pub struct ResizeCoalescer {
    dirty: bool,
}

impl ResizeCoalescer {
    /// 一个 `Resized` 事件到达（只置脏，不做任何重排工作）。
    pub fn on_resized(&mut self) {
        self.dirty = true;
    }

    /// 取走脏标志：返回真时调用方执行一次重排（每帧至多一次）。
    pub fn take(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
}

/// 页面坐标系显示列表缓存（M6）。
///
/// 场景按（页面代次、视口宽、高亮集合）缓存为 **页面坐标系**版本
/// （origin 全零）：滚动、滚动条拖拽只是光栅前的一次
/// [`Scene::translated`] origin 平移，不再重建显示列表。导航与 resize
/// 推进页面代次（布局结果变化）、选区变化改变高亮集合——两者都使缓存
/// 失效重建。
#[derive(Default)]
pub struct SceneCache {
    cached: Option<CachedScene>,
    /// 累计 emit（全片段树重建）次数；基准与测试用它断言滚动路径零 emit。
    emits: usize,
    /// 上一次 [`SceneCache::scene_for`] 是否命中。
    last_hit: bool,
}

struct CachedScene {
    generation: u64,
    viewport_width: f32,
    highlights: Vec<LayoutRect>,
    scene: Scene,
}

impl SceneCache {
    /// 空缓存。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 取页面坐标系场景：键（代次、视口宽、高亮）命中直接复用，未命中
    /// 走一次全片段树 emit 并入库。
    pub fn scene_for(
        &mut self,
        generation: u64,
        page: &Page,
        root: &Fragment,
        viewport_width: f32,
        highlights: &[LayoutRect],
    ) -> &Scene {
        let hit = self.cached.as_ref().is_some_and(|cached| {
            cached.generation == generation
                && cached.viewport_width == viewport_width
                && cached.highlights == highlights
        });
        self.last_hit = hit;
        if !hit {
            self.emits += 1;
            let scene = build_scene_at_with_highlights(page, root, 0.0, 0.0, highlights);
            self.cached = Some(CachedScene {
                generation,
                viewport_width,
                highlights: highlights.to_vec(),
                scene,
            });
        }
        &self.cached.as_ref().expect("cache populated on miss").scene
    }

    /// 上一次 [`SceneCache::scene_for`] 是否命中缓存（未走 emit）。
    #[must_use]
    pub fn last_was_hit(&self) -> bool {
        self.last_hit
    }

    /// 累计 emit 次数。
    #[must_use]
    pub fn emits(&self) -> usize {
        self.emits
    }

    /// emit 计数清零（基准分场景统计）。
    pub fn reset_emits(&mut self) {
        self.emits = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexty_css::SizeValue;
    use nexty_dom::NodeKind;

    /// recascade 按视口折算视口单位：50vw @800×600 → 400px、10vh → 60px。
    #[test]
    fn recascade_resolves_viewport_units() {
        let document = nexty_html::parse_document("<div></div>", ParseOptions::default());
        let sheet = Stylesheet::parse("div { width: 50vw; height: 10vh }");
        let mut page = build_page(document, vec![sheet], HashMap::new());

        // 构建期无视口：视口单位保持未折算
        let div = *page
            .styles
            .keys()
            .find(|&&id| {
                matches!(
                    page.document.node(id),
                    Some(NodeKind::Element(data)) if data.name == "div"
                )
            })
            .expect("div element");
        assert!(matches!(page.styles[&div].width, SizeValue::Viewport(..)));

        recascade(
            &mut page,
            MediaViewport {
                width: 800.0,
                height: 600.0,
            },
        );
        assert_eq!(page.styles[&div].width, SizeValue::Length(400.0), "50vw");
        assert_eq!(page.styles[&div].height, SizeValue::Length(60.0), "10vh");
    }

    /// 布局一段文本页供缓存测试用。
    fn text_page() -> (Page, Fragment) {
        let page = load_page(
            "<body><p>hello 缓存 scene</p><p>second line of text</p></body>",
            "",
        );
        let shaper = nexty_text::ParleyTextShaper::new();
        let root = layout_page(&page, &shaper, 400.0).expect("root");
        (page, root)
    }

    /// 同键重复请求复用缓存（零 emit），键变化（代次/视口宽/高亮）失效重建。
    #[test]
    fn scene_cache_reuses_on_same_key_and_invalidates_on_key_change() {
        let (page, root) = text_page();
        let mut cache = SceneCache::new();

        let _ = cache.scene_for(7, &page, &root, 400.0, &[]);
        assert!(!cache.last_was_hit(), "首次构建必 miss");
        assert_eq!(cache.emits(), 1);

        let _ = cache.scene_for(7, &page, &root, 400.0, &[]);
        assert!(cache.last_was_hit(), "同键命中");
        assert_eq!(cache.emits(), 1, "命中不 emit");

        // 页面代次推进（导航 / resize 重排）→ 失效
        let _ = cache.scene_for(8, &page, &root, 400.0, &[]);
        assert!(!cache.last_was_hit());
        assert_eq!(cache.emits(), 2);

        // 视口宽变化 → 失效
        let _ = cache.scene_for(8, &page, &root, 500.0, &[]);
        assert!(!cache.last_was_hit());

        // 高亮集合变化（选区）→ 失效
        let highlights = [LayoutRect {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 20.0,
        }];
        let _ = cache.scene_for(8, &page, &root, 500.0, &highlights);
        assert!(!cache.last_was_hit());
        let _ = cache.scene_for(8, &page, &root, 500.0, &highlights);
        assert!(cache.last_was_hit(), "相同高亮集合命中");
        assert_eq!(cache.emits(), 4);
    }

    /// 缓存场景是页面坐标系（origin 全零）：平移到视口后几何与直接以
    /// 该 origin 构建的场景逐字节一致（M6 的正确性判据）。
    #[test]
    fn cached_page_scene_translated_matches_direct_build() {
        let (page, root) = text_page();
        let mut cache = SceneCache::new();
        let page_scene = cache.scene_for(1, &page, &root, 400.0, &[]);
        let origin_y = 48.0 - 120.0; // BAR_HEIGHT - scroll 的样例
        let translated = page_scene.translated(0.0, origin_y);
        let direct = build_scene_at_with_highlights(&page, &root, 0.0, origin_y, &[]);
        assert_eq!(format!("{translated:?}"), format!("{direct:?}"));
    }

    /// 合帧器：连发事件折叠为一次 take。
    #[test]
    fn resize_coalescer_collapses_bursts() {
        let mut coalescer = ResizeCoalescer::default();
        assert!(!coalescer.take(), "初始无脏标志");
        coalescer.on_resized();
        coalescer.on_resized();
        coalescer.on_resized();
        assert!(coalescer.take(), "三次事件折叠为一次");
        assert!(!coalescer.take(), "take 后标志复位");
        coalescer.on_resized();
        assert!(coalescer.take());
    }
}
