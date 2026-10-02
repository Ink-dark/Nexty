//! Nexty 自研盒级布局。
//!
//! 不依赖外部布局引擎：`taffy` 虽成熟，但布局是 WPT 对齐的关键层，需完全可控。
//! 决策见 `docs/decisions/2026-10-01-crate-selection.md`。
//!
//! 行为 ground truth：CSS 2.1 可视格式化模型（普通流：块级 + 行内）、
//! CSS Display、CSS Box Model。当前实现范围与偏差清单见 `block` 模块文档。
//!
//! 管线入口 [`layout_document`]：输入 arena 文档、各元素的 computed style
//! （nexty-css 的 `compute_document_styles` 产物）与视口宽度，输出片段树
//! [`Fragment`]（含边框盒几何、盒模型、行内文本行），供 paint 层消费。
//!
//! 上游类型一律不得出现在本 crate 的 pub 导出中（AGENTS.md 硬规则）。

#![forbid(unsafe_code)]

use std::collections::HashMap;

use nexty_css::ComputedStyle;
use nexty_dom::{Document, NodeId, NodeKind};
use nexty_text::TextShaper;

mod block;
mod fragment;
mod inline;

pub use fragment::{Edges, Fragment, ImageRun, LineFragment, Rect, TextRun};

/// 布局整份文档，返回根元素（`<html>`）的片段树。
///
/// `styles` 是文档树序计算的 computed style（见 nexty-css 的
/// `compute_document_styles`）；缺失样式的元素按 initial 值处理。
/// `image_sizes` 提供已解码图片的自然尺寸（`<img>` 节点 → 像素宽高）；
/// 未收录的图片按 CSS Images §5.1 默认对象尺寸 300×150 处理。
/// `viewport_width` 是根包含块的内容宽（CSS px）。
///
/// 返回 `None` 表示文档没有元素节点。
#[must_use]
pub fn layout_document(
    document: &Document,
    shaper: &dyn TextShaper,
    styles: &HashMap<NodeId, ComputedStyle>,
    image_sizes: &HashMap<NodeId, (f32, f32)>,
    viewport_width: f32,
) -> Option<Fragment> {
    let root = document
        .children(document.root())
        .find(|child| matches!(document.node(*child), Some(NodeKind::Element(_))))?;
    let ctx = block::Context {
        document,
        shaper,
        styles,
        image_sizes,
    };
    Some(block::layout_root(&ctx, root, viewport_width))
}
