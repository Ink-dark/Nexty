//! Nexty 盒级布局。
//!
//! 盒级几何（block / flex / grid / absolute）由 [`taffy`]（MIT，被 Servo/Blitz 采用）
//! 接管；**inline / 文本布局与 table 仍自研**。选型和能力边界见
//! `docs/decisions/2026-10-01-crate-selection.md` 的修正记录。
//!
//! 行为 ground truth：CSS 2.1 可视格式化模型（普通流：块级 + 行内）、
//! CSS Display、CSS Box Model，盒级算法对齐 taffy 的 WPT 实现。当前实现范围与
//! 偏差清单见 `block` / `style_map` 模块文档。
//!
//! 管线入口 [`layout_document`]：输入 arena 文档、各元素的 computed style
//! （nexty-css 的 `compute_document_styles` 产物）与视口宽度，输出片段树
//! [`Fragment`]（含边框盒几何、盒模型、行内文本行），供 paint 层消费。
//!
//! **上游类型隔离**：`taffy` 的 `Style` / `TaffyTree` / `Layout` 等类型只在本 crate
//! 内部流动，**绝不**出现在 pub 导出中（AGENTS.md 硬规则）。`style_map` 是唯一的
//! 类型边界：输入 `ComputedStyle`、输出 `taffy::Style`，二者都不外泄。
//!
//! **Feature**：按布局能力一一对应，默认全开已实现能力——`block`（块级流）、
//! `inline`（行内流）、`flex`（单行弹性布局）、`replaced`（`<img>` 替换盒）。
//! `grid` / `table` / `absolute` 为预留槽位，当前未实现故不在 default，
//! 待 T2–T5 由 taffy 接管后补全并加入 default。`taffy` 依赖常驻（feature 只切
//! 能力开关，不切依赖），映射层 `style_map` 受 `taffy-map` 控制。

#![forbid(unsafe_code)]

use std::collections::HashMap;

use nexty_css::ComputedStyle;
use nexty_dom::{Document, NodeId, NodeKind};
use nexty_text::TextShaper;

mod block;
mod fragment;
#[cfg(any(feature = "inline", feature = "replaced"))]
mod inline;
#[cfg(any(feature = "inline", feature = "replaced"))]
mod intrinsic;
#[cfg(feature = "taffy-map")]
mod style_map;
#[cfg(feature = "taffy-map")]
mod tree_build;

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
