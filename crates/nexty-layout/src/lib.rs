//! Nexty 盒级布局。
//!
//! 盒级几何（block / flex / grid / absolute）由 [`taffy`]（MIT，被 Servo/Blitz 采用）
//! 接管；**inline / 文本布局与 table 仍自研**。选型和能力边界见
//! `docs/decisions/2026-10-01-crate-selection.md` 的修正记录。
//!
//! 行为 ground truth：CSS 2.1 可视格式化模型（普通流：块级 + 行内）、CSS Display、
//! CSS Box Model、CSS Flexbox；块级与弹性算法对齐 taffy 的 WPT 实现。当前实现范围
//! 与偏差清单见 `block` / `tree_build` / `style_map` 模块文档。
//!
//! 管线分三步（`layout_document` 入口）：
//! 1. `tree_build`：arena DOM + computed style → taffy 盒树。行内内容不建元素节点，
//!    由 `inline` 模块按行内流收集后包装成**匿名块叶节点**，measure function 按
//!    taffy 给出的可用宽跑自研断行（`inline::build_lines`）提供内在尺寸；
//! 2. `taffy::TaffyTree::compute_layout`：解算全部盒级几何（块级流含 margin 折叠、
//!    宽度解析 §10.3.3、min/max 收束；flex 容器含 wrap/shrink/对齐）；
//! 3. `block`：读回各节点 `taffy::Layout`（location/size/border/padding），换算到
//!    「相对包含块内容盒」的坐标约定，组装 [`Fragment`] 树供 paint 层消费。
//!
//! 输入输出契约冻结：`Document + ComputedStyle + image_sizes + viewport_width` →
//! [`Fragment`] 树（定义见 [`fragment`]），paint 层零改动。
//!
//! **上游类型隔离**：`taffy` 的 `Style` / `TaffyTree` / `Layout` 等类型只在本 crate
//! 内部流动，**绝不**出现在 pub 导出中（AGENTS.md 硬规则）。`style_map` 是唯一的
//! 类型边界：输入 `ComputedStyle`、输出 `taffy::Style`，二者都不外泄。
//!
//! **Feature**：按布局能力一一对应——`block`（块级流，隐含 `inline` 与
//! `taffy-map`：taffy 管线是块级流的唯一实现）、`inline`（行内断行）、
//! `flex`（flex 容器 blockify 与 taffy Display::Flex 映射）、`replaced`（`<img>`
//! 替换盒）、`taffy-map`（`ComputedStyle → taffy::Style` 映射层与盒树构建）。
//! `grid` / `table` / `absolute` 为预留槽位，当前未实现故不在 default。`taffy`
//! 依赖常驻（feature 只切能力开关，不切依赖）。

#![forbid(unsafe_code)]

use std::collections::HashMap;

use nexty_css::ComputedStyle;
#[cfg(feature = "block")]
use nexty_dom::NodeKind;
use nexty_dom::{Document, NodeId};
use nexty_text::TextShaper;

#[cfg(feature = "block")]
mod block;
mod fragment;
#[cfg(any(feature = "inline", feature = "replaced"))]
mod inline;
#[cfg(feature = "taffy-map")]
mod style_map;
#[cfg(feature = "taffy-map")]
mod tree_build;

pub use fragment::{Edges, Fragment, ImageRun, LineFragment, Rect, TextRun};

/// 布局上下文：整棵树共享的输入引用。
///
/// 由 `layout_document` 构造，`block` / `inline` / `tree_build` 共用；feature
/// `block` 关闭时无构造方（仅为下层模块保类型），整体抑制 dead_code。
#[cfg_attr(not(feature = "block"), allow(dead_code))]
pub(crate) struct Context<'a> {
    pub document: &'a Document,
    pub shaper: &'a dyn TextShaper,
    pub styles: &'a HashMap<NodeId, ComputedStyle>,
    /// 已解码图片的自然尺寸（node → 像素宽高，CSS px）。
    pub image_sizes: &'a HashMap<NodeId, (f32, f32)>,
}

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
#[cfg(feature = "block")]
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
    let ctx = Context {
        document,
        shaper,
        styles,
        image_sizes,
    };
    Some(block::layout_root(&ctx, root, viewport_width))
}
