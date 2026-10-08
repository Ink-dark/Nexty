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

use nexty_css::{ComputedStyle, Rgba, Stylesheet, compute_document_styles, html_ua_stylesheet};
use nexty_dom::{Document, NodeId};
use nexty_html::ParseOptions;
use nexty_layout::Fragment;
use nexty_paint::{Color, Command, Image, Rect as PaintRect, Scene, Size, TextGlyph};
use nexty_text::TextShaper;

/// 一个已解析、已级联的页面。
pub struct Page {
    /// arena 文档。
    pub document: Document,
    /// 各元素的 computed style。
    pub styles: HashMap<NodeId, ComputedStyle>,
    /// 已解码图片（`<img>` 节点 → RGBA 图像），构建 Scene 时进入图片池。
    pub images: HashMap<NodeId, Image>,
}

/// 用已解析文档与作者样式表构建页面（UA 样式表固定为 nexty-css 的 HTML 默认）。
///
/// `stylesheets` 按级联优先级排列（CSS Cascade：源顺序靠后者胜出）——
/// inline `<style>` 与外链样式表按文档出现顺序交错。
#[must_use]
pub fn build_page(
    document: Document,
    stylesheets: Vec<Stylesheet>,
    images: HashMap<NodeId, Image>,
) -> Page {
    let sheet_refs: Vec<&Stylesheet> = stylesheets.iter().collect();
    let styles = compute_document_styles(&document, &sheet_refs, Some(html_ua_stylesheet()));
    Page {
        document,
        styles,
        images,
    }
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
    let mut scene = Scene::default();
    let mut pool: HashMap<NodeId, usize> = HashMap::new();
    emit(page, root, origin_x, origin_y, &mut scene, &mut pool);
    scene
}

fn emit(
    page: &Page,
    fragment: &Fragment,
    origin_x: f32,
    origin_y: f32,
    scene: &mut Scene,
    pool: &mut HashMap<NodeId, usize>,
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
                    })
                    .collect(),
                baseline,
                families: run.style.families.clone(),
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
            emit(page, atomic, content_x, line_y, scene, pool);
        }
        line_offset += line.height;
    }

    for child in &fragment.children {
        emit(page, child, content_x, content_y, scene, pool);
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
            scene.images.push(source.clone());
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
