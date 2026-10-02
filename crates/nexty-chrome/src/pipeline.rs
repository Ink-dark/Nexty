//! 导航管线：HTML 字符串 → 级联 → 布局 → 片段树 → paint 显示列表。
//!
//! 行为分工：解析归 nexty-html，级联归 nexty-css（UA + author 双 origin），
//! 布局归 nexty-layout，本模块负责把片段树转译为 [`nexty_paint::Scene`]：
//! 背景与 CSS 边框（四条填充带，宽 0 的边跳过）、行内文本 run
//! （坐标换算到绝对坐标系）；匿名片段不产生背景与边框。
//!
//! 已知偏差：`<html>`/`<body>` 背景不向画布传播（画布保持透明，由呈现层
//! 决定底色）；片段树不做滚动裁剪。

use std::collections::HashMap;

use nexty_css::{ComputedStyle, Rgba, Stylesheet, compute_document_styles, html_ua_stylesheet};
use nexty_dom::{Document, NodeId};
use nexty_html::ParseOptions;
use nexty_layout::Fragment;
use nexty_paint::{Color, Command, Rect as PaintRect, Scene, Size, TextGlyph};
use nexty_text::TextShaper;

/// 一个已解析、已级联的页面。
pub struct Page {
    /// arena 文档。
    pub document: Document,
    /// 各元素的 computed style。
    pub styles: HashMap<NodeId, ComputedStyle>,
}

/// 解析并级联一份页面（UA 样式表固定为 nexty-css 的 HTML 默认）。
#[must_use]
pub fn load_page(html: &str, author_css: &str) -> Page {
    let document = nexty_html::parse_document(html, ParseOptions::default());
    let sheet = Stylesheet::parse(author_css);
    let styles = compute_document_styles(&document, &[&sheet], Some(html_ua_stylesheet()));
    Page { document, styles }
}

/// 对页面做布局，返回根元素片段。
#[must_use]
pub fn layout_page(page: &Page, shaper: &dyn TextShaper, viewport_width: f32) -> Option<Fragment> {
    nexty_layout::layout_document(&page.document, shaper, &page.styles, viewport_width)
}

/// 把片段树转译为显示列表，原点在视口左上角。
#[must_use]
pub fn build_scene(root: &Fragment) -> Scene {
    build_scene_at(root, 0.0, 0.0)
}

/// 把片段树转译为显示列表，并整体平移到 `(origin_x, origin_y)`。
///
/// 用于给页面内容留出顶部 UI（如地址栏）占用的区域。
#[must_use]
pub fn build_scene_at(root: &Fragment, origin_x: f32, origin_y: f32) -> Scene {
    let mut scene = Scene::default();
    emit(root, origin_x, origin_y, &mut scene);
    scene
}

fn emit(fragment: &Fragment, origin_x: f32, origin_y: f32, scene: &mut Scene) {
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

    for line in &fragment.lines {
        let baseline = content_y + line.baseline;
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
    }

    for child in &fragment.children {
        emit(child, content_x, content_y, scene);
    }
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
