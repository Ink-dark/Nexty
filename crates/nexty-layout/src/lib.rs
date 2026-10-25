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
//! `flex`（flex 容器 blockify 与 taffy Display::Flex 映射，含 wrap/shrink/对齐）、
//! `replaced`（`<img>` 替换盒）、`taffy-map`（`ComputedStyle → taffy::Style` 映射层
//! 与盒树构建）、`grid`（grid-* 样式映射与 Display::Grid）、`absolute`（position
//! 与 inset 映射）、`selection`（文本命中、选区矩形与复制文本重建）。`table`
//! 为预留槽位，当前未实现故不在 default。`taffy`
//! 依赖常驻（feature 只切能力开关，不切依赖）。

#![forbid(unsafe_code)]

use std::cell::RefCell;
use std::collections::HashMap;

use nexty_css::ComputedStyle;
#[cfg(feature = "block")]
use nexty_dom::NodeKind;
use nexty_dom::{Document, NodeId};
use nexty_text::{FontMetrics, TextShaper, TextStyle};

#[cfg(feature = "block")]
mod block;
mod fragment;
#[cfg(any(feature = "inline", feature = "replaced"))]
mod inline;
#[cfg(feature = "selection")]
mod selection;
#[cfg(feature = "taffy-map")]
mod style_map;
#[cfg(feature = "taffy-map")]
mod tree_build;

pub use fragment::{Edges, Fragment, ImageRun, LineFragment, LineStart, Rect, SourceChar, TextRun};

#[cfg(feature = "selection")]
pub use selection::{TextMap, TextPosition};

#[cfg(feature = "inline")]
pub use inline::{
    BreakOptions, OverflowWrap, Span, WordBreak, break_opportunities, cluster_map,
    min_content_width, split_spans,
};

/// `MetricsCache` 的键：`TextStyle` 含 f32 字号（无 `Eq`/`Hash`），按位
/// 转换作键。同值 f32 位型相同；字号在 computed 阶段已数值卫生，位型
/// 稳定。
/// 以下三者仅被 block 管线（⇒ inline）消费，裁剪配置整体抑制 dead_code。
#[cfg_attr(not(feature = "block"), allow(dead_code))]
#[derive(PartialEq, Eq, Hash)]
struct MetricsKey {
    families: Vec<String>,
    size_bits: u32,
}

#[cfg_attr(not(feature = "block"), allow(dead_code))]
impl MetricsKey {
    fn of(style: &TextStyle) -> Self {
        Self {
            families: style.families.clone(),
            size_bits: style.size.to_bits(),
        }
    }
}

/// 布局级字体度量缓存：`TextStyle → FontMetrics`（H4）。
///
/// `metrics` 的探针文本固定（NBSP），同一 `TextStyle` 的结果必然相同，
/// 故整棵树共享一份表：每个不同样式整次布局只整形一次探针。没有它，
/// `build_lines` 的 strut 与每行盒组装按 run 节点 × measure 档位重复
/// 调用（同一页 2,000 个同样式段落 = 2,000+ 次探针整形）。
#[cfg_attr(not(feature = "block"), allow(dead_code))]
#[derive(Default)]
pub(crate) struct MetricsCache {
    map: RefCell<HashMap<MetricsKey, FontMetrics>>,
}

#[cfg_attr(not(feature = "block"), allow(dead_code))]
impl MetricsCache {
    /// 取字体度量：命中缓存直接返回，未命中整形一次探针并入库。
    ///
    /// 整形失败按零度量入库（与原先每次失败时的降级一致：探针文本固定，
    /// 同样式失败结果也必然相同）。
    pub(crate) fn metrics(&self, shaper: &dyn TextShaper, style: &TextStyle) -> FontMetrics {
        let key = MetricsKey::of(style);
        let mut map = self.map.borrow_mut();
        if let Some(metrics) = map.get(&key) {
            return *metrics;
        }
        let metrics = shaper.metrics(style).unwrap_or(FontMetrics {
            ascent: 0.0,
            descent: 0.0,
            line_height: 0.0,
        });
        map.insert(key, metrics);
        metrics
    }
}

/// 整形上下文：shaper 与布局级度量缓存的成对引用（H4）。
///
/// 度量缓存 miss 时需要 shaper 补测一次探针，二者总是成对出现；合并后
/// `build_lines` 少一个参数，行盒组装侧经 [`ShapingContext::metrics`]
/// 查表。
#[cfg_attr(not(feature = "block"), allow(dead_code))]
pub(crate) struct ShapingContext<'a> {
    pub shaper: &'a dyn TextShaper,
    pub metrics: &'a MetricsCache,
}

#[cfg_attr(not(feature = "block"), allow(dead_code))]
impl ShapingContext<'_> {
    /// 一个 TextStyle 下的字体度量（布局级缓存，同一样式只整形一次探针）。
    pub(crate) fn metrics(&self, style: &TextStyle) -> FontMetrics {
        self.metrics.metrics(self.shaper, style)
    }
}

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
    /// 布局级字体度量缓存（H4：metrics 调用数 ≤ 不同 TextStyle 数）。
    pub metrics: MetricsCache,
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
        metrics: MetricsCache::default(),
    };
    Some(block::layout_root(&ctx, root, viewport_width))
}
