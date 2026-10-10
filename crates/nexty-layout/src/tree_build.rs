//! 盒树构建：arena DOM + computed style → taffy 盒树。
//!
//! 行为 ground truth：CSS Display §3（盒生成规则）、CSS Box Model、
//! CSS Flexbox §4（flex item 的 blockify）。
//!
//! **盒生成规则**（与 CSS Display §3 逐条对应）：
//!
//! - `display: none`：不产盒（§3.1「不生成任何盒」），整棵子树跳过；
//! - `display: contents`：元素本身不产盒（§3.2「自身不生成盒」），其子节点
//!   **提升**为父盒的直接参与盒（穿透一层，行内收集状态跨边界保持）；
//! - 块级替换元素（`<img>`）：以 `image_sizes` 的自然尺寸（或 CSS Images §5.1
//!   的默认对象尺寸 300×150）为 definite size 建叶节点，不建子树；
//! - 行内内容（文本与 `display: inline*` 的元素）：**不建元素节点**。taffy 不做
//!   文本布局，行内内容由 `inline` 模块按行内流收集（空白折叠跨子节点持续），
//!   每段连续行内流包装成一个**匿名块叶节点**（`anonymous` run leaf），measure
//!   function 按父算法给出的可用宽跑自研断行提供内在尺寸（见
//!   [`InlineRunNode::measure`]）；
//! - flex 容器（feature `flex`）：**每个子元素 blockify 成独立 item**（CSS
//!   Flexbox §4），连续文本段成为匿名 flex item（仍是 run 叶节点）；
//! - 其余块级盒：建容器节点，子节点递归挂上。
//!
//! **映射表**：`NodeId ↔ taffy Node` 双向映射 + run 叶节点表，供 `block` 回填
//! `Fragment` 时定位几何与行盒。DOM `NodeId` 与 taffy `Node` 是不同类型，taffy
//! 的 `NodeId` 只在本 crate 内部流转，绝不外泄（AGENTS.md 硬规则）。
//!
//! **已知限制**（T5 处理）：grid 容器暂按块容器走法收集子盒（行内子节点进 run
//! 叶而非 grid item），待 grid item 生成接线后改为 blockify。
//!
//! feature `block` 关闭时本模块无生产调用方（盒树构建的下游是 taffy 管线），
//! 按模块整体抑制 dead_code（裁剪配置仅供编译验证）。

#![cfg_attr(not(feature = "block"), allow(dead_code))]

use std::cell::RefCell;
use std::collections::HashMap;

use nexty_css::{ComputedStyle, DisplayValue};
use nexty_dom::{NodeId, NodeKind};
use taffy::compute::compute_leaf_layout;
use taffy::geometry::Size as TaffySize;
use taffy::style::{AvailableSpace, Display, Style};
use taffy::tree::{LayoutInput, LayoutOutput, NodeId as TaffyNode, RunMode, TaffyTree};

use crate::Context;
use crate::fragment::LineFragment;
use crate::inline::{InlineItem, InlineRun};
use crate::style_map::map_style;

/// 构建产物：taffy 盒树 + 映射表。
///
/// taffy 的类型不出现在本 crate 的 pub 导出中，故本类型为 `pub(crate)`。
pub(crate) struct BoxTree<'a> {
    /// taffy 盒树（`NodeContext = ()`；run 叶数据在 [`BoxTree::runs`]）。
    pub tree: TaffyTree,
    /// DOM 节点 → taffy 节点。仅含**实际产盒**的元素（`none`/`contents` 不在其中）。
    pub dom_to_taffy: HashMap<NodeId, TaffyNode>,
    /// taffy 节点 → DOM 节点，`dom_to_taffy` 的逆映射。
    pub taffy_to_dom: HashMap<TaffyNode, NodeId>,
    /// 行内 run 匿名叶节点表（taffy 节点 → run 数据）。
    pub runs: HashMap<TaffyNode, InlineRunNode<'a>>,
    /// 根 taffy 节点；`None` 表示根不产盒（`display: none` 或 contents 提升为空）。
    pub root: Option<TaffyNode>,
    /// 根节点是否为根元素自身的盒（`false` = contents 根的合成根，根元素无自身盒，
    /// 其 margin 不参与定位）。
    pub root_is_element_box: bool,
}

impl BoxTree<'_> {
    fn new() -> Self {
        Self {
            tree: TaffyTree::new(),
            dom_to_taffy: HashMap::new(),
            taffy_to_dom: HashMap::new(),
            runs: HashMap::new(),
            root: None,
            root_is_element_box: true,
        }
    }

    /// 登记一个已建节点的双向映射。
    fn link(&mut self, dom: NodeId, taffy: TaffyNode) {
        self.dom_to_taffy.insert(dom, taffy);
        self.taffy_to_dom.insert(taffy, dom);
    }

    // ---- 测试专用断言辅助（生产路径直接读 pub 字段） ----

    /// 按 DOM 节点取 taffy 节点；未产盒（`none` / `contents` / 行内内容）返回 `None`。
    #[must_use]
    #[cfg(test)]
    pub fn taffy_node(&self, dom: NodeId) -> Option<TaffyNode> {
        self.dom_to_taffy.get(&dom).copied()
    }

    /// 按 taffy 节点取 DOM 节点。
    #[must_use]
    #[cfg(test)]
    pub fn dom_node(&self, taffy: TaffyNode) -> Option<NodeId> {
        self.taffy_to_dom.get(&taffy).copied()
    }

    /// taffy 节点数（供测试与断言使用；run 叶不计入）。
    #[must_use]
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.dom_to_taffy.len()
    }
}

/// 一段连续行内流的 run 叶节点数据。
///
/// 行内项在树构建期收集完毕（空白折叠跨子节点与 `contents` 边界持续）；原子盒
/// 项（inline-block 等）的片段在 measure 期按 taffy 给出的可用宽**惰性布局**
/// （见 [`InlineRunNode::lines_at`]），断行结果按可用宽缓存。
pub(crate) struct InlineRunNode<'a> {
    ctx: &'a Context<'a>,
    items: RefCell<Vec<InlineItem>>,
    /// strut 来源：匿名块的父元素 computed style（CSS 2.1 §10.8.1）。
    strut_style: ComputedStyle,
    /// (可用宽, 行盒) 缓存：measure 在 max-content 与定宽等多个约束下各调一次。
    lines_cache: RefCell<Vec<(f32, Vec<LineFragment>)>>,
}

impl InlineRunNode<'_> {
    /// 以 `available_width` 为断行宽取行盒；未缓存则现算并缓存。
    ///
    /// 原子盒项先按同一可用宽布局（其片段宽高参与断行与基线对齐）。
    pub(crate) fn lines_at(&self, available_width: f32) -> Vec<LineFragment> {
        if let Some((_, lines)) = self
            .lines_cache
            .borrow()
            .iter()
            .find(|(width, _)| *width == available_width)
        {
            return lines.clone();
        }
        for item in self.items.borrow_mut().iter_mut() {
            if let InlineItem::Block(atomic) = item {
                // feature `block` 关闭时无原子盒递归布局管线：保持占位片段
                // （0×0），该裁剪配置仅供盒树构建层单独编译验证。
                #[cfg(feature = "block")]
                {
                    atomic.fragment = crate::block::layout_atomic_subtree(
                        self.ctx,
                        atomic.fragment.node,
                        &atomic.style,
                        AvailableSpace::Definite(available_width),
                    );
                }
                #[cfg(not(feature = "block"))]
                {
                    let _ = (&atomic, available_width);
                }
            }
        }
        let lines = crate::inline::build_lines(
            self.ctx.shaper,
            &self.items.borrow(),
            available_width,
            &self.strut_style,
        );
        self.lines_cache
            .borrow_mut()
            .push((available_width, lines.clone()));
        lines
    }

    /// measure function：taffy 以可用空间询问内在尺寸。
    ///
    /// 断行宽取已知宽或定宽可用空间；`MaxContent` 按不折行（单行）测量，
    /// `MinContent` 按 0 宽断行（每项一行，取最长项）近似内在尺寸。
    /// 行盒由自研断行决定，taffy 只消费尺寸结果。
    pub(crate) fn measure(&self, input: LayoutInput) -> LayoutOutput {
        compute_leaf_layout(
            input,
            &run_leaf_style(),
            |_, _| 0.0,
            |known, available| {
                let break_width = known
                    .width
                    .or_else(|| available.width.into_option())
                    .unwrap_or(match available.width {
                        AvailableSpace::MinContent => 0.0,
                        _ => f32::INFINITY,
                    });
                let lines = self.lines_at(break_width);
                let content_width = match available.width {
                    AvailableSpace::Definite(width) => width,
                    // 不折行：行内项总宽（缓存行恰为单行）
                    AvailableSpace::MaxContent => lines.last().map(line_width).unwrap_or(0.0),
                    // 最长不可拆项
                    AvailableSpace::MinContent => lines.iter().map(line_width).fold(0.0, f32::max),
                };
                TaffySize {
                    width: known.width.unwrap_or(content_width),
                    height: lines.iter().map(|line| line.height).sum(),
                }
            },
        )
    }
}

/// 一行行内内容的宽度（取文本 / 图片 / 原子盒右缘最大值）。
fn line_width(line: &LineFragment) -> f32 {
    let mut width = 0.0_f32;
    for run in &line.runs {
        if let Some(glyph) = run.glyphs.last() {
            width = width.max(glyph.x + glyph.advance);
        }
    }
    for image in &line.images {
        width = width.max(image.x + image.width);
    }
    for atomic in &line.boxes {
        width = width.max(atomic.border_box.x + atomic.border_box.width);
    }
    width
}

/// 匿名 run 叶节点的 taffy 样式：普通块级叶，无 margin/padding/border。
pub(crate) fn run_leaf_style() -> Style {
    Style {
        display: Display::Block,
        ..Style::DEFAULT
    }
}

/// measure 分发：run 叶走 [`InlineRunNode::measure`]，其余叶（如 definite size
/// 的 `<img>`）按零内容尺寸处理（尺寸由样式决定，无需测量）。
pub(crate) fn measure_dispatch(
    input: LayoutInput,
    node: TaffyNode,
    style: &Style,
    runs: &HashMap<TaffyNode, InlineRunNode<'_>>,
) -> LayoutOutput {
    if input.run_mode == RunMode::PerformHiddenLayout {
        return LayoutOutput::HIDDEN;
    }
    match runs.get(&node) {
        Some(run) => run.measure(input),
        None => compute_leaf_layout(input, style, |_, _| 0.0, |_, _| TaffySize::ZERO),
    }
}

/// 容器子内容的收集方式。
///
/// - `BlockContainer`：行内内容进 run 叶（匿名块，CSS Display §3.3）；
/// - `FlexItems`：每个子元素 blockify 成独立 item，连续文本段成为匿名 item
///   （CSS Flexbox §4）。
#[derive(Clone, Copy, PartialEq)]
enum WalkKind {
    BlockContainer,
    /// flex 走法由 feature `flex` 门控（关闭时 flex 容器按块容器走法降级）。
    #[cfg(feature = "flex")]
    FlexItems,
}

/// 容器的子内容收集走法。flex 容器走 item blockify；grid 容器暂按块容器走法
/// （已知限制，见模块文档）。
#[cfg_attr(not(feature = "flex"), allow(unused_variables))]
fn container_kind(style: &ComputedStyle) -> WalkKind {
    #[cfg(feature = "flex")]
    if matches!(style.display, DisplayValue::Flex | DisplayValue::InlineFlex) {
        return WalkKind::FlexItems;
    }
    WalkKind::BlockContainer
}

/// 行内级 display（块容器走法下进 run 叶；flex 走法下被 blockify）。
fn is_inline_level(display: DisplayValue) -> bool {
    matches!(
        display,
        DisplayValue::Inline
            | DisplayValue::InlineBlock
            | DisplayValue::InlineFlex
            | DisplayValue::InlineGrid
            | DisplayValue::InlineTable
    )
}

/// 从根元素开始构建整棵盒树。
///
/// 根元素 `display: none` 或 `contents` 提升为空时不产根节点（[`BoxTree::root`]
/// 为 `None`）；contents 根的参与盒挂在合成根节点下（根元素自身无盒）。
pub(crate) fn build_box_tree<'a>(ctx: &'a Context<'a>, root: NodeId) -> BoxTree<'a> {
    let mut boxes = BoxTree::new();
    let style = root_style(ctx, root);
    match style.display {
        // §3.1：display: none 不生成任何盒。
        DisplayValue::None => {}
        // §3.2：contents 根自身无盒，参与盒提升到合成根下。
        DisplayValue::Contents => {
            let mut run = None;
            let mut participants = Vec::new();
            collect_children_boxes(
                ctx,
                &mut boxes,
                root,
                &style,
                WalkKind::BlockContainer,
                &mut run,
                &mut participants,
            );
            flush_run(&mut run, ctx, &mut boxes, &style, &mut participants);
            boxes.root_is_element_box = false;
            if let Ok(synthetic) = boxes
                .tree
                .new_with_children(run_leaf_style(), &participants)
            {
                boxes.root = Some(synthetic);
            }
        }
        _ => {
            if let Some(taffy) = build_box(ctx, &mut boxes, root) {
                boxes.root = Some(taffy);
            } else {
                boxes.root_is_element_box = false;
            }
        }
    }
    boxes
}

fn root_style(ctx: &Context<'_>, root: NodeId) -> ComputedStyle {
    ctx.styles
        .get(&root)
        .cloned()
        .unwrap_or_else(ComputedStyle::initial)
}

/// 为确定产盒的元素建 taffy 节点（含子内容收集）。
///
/// 调用方保证 `node` 产盒（非 `none`/`contents`；行内级仅在 blockify 语境到达）。
/// 返回 `None` 表示 taffy 建节点失败（内部 SlotMap 容量错误）：不登记映射，该
/// 子树降级为「无盒」。刻意不用 panic——渲染隔离要求布局层不炸进程。
fn build_box<'a>(ctx: &'a Context<'a>, boxes: &mut BoxTree<'a>, node: NodeId) -> Option<TaffyNode> {
    let style = root_style(ctx, node);
    let kind = container_kind(&style);

    // 块级替换元素：以自然尺寸为 definite size 建叶节点，taffy 不再解算内容，
    // 子树（alt 文本等）不参与布局。
    let replaced = replaced_leaf_size(ctx, node);
    let mut participants = Vec::new();
    if replaced.is_none() {
        let mut run = None;
        collect_children_boxes(ctx, boxes, node, &style, kind, &mut run, &mut participants);
        flush_run(&mut run, ctx, boxes, &style, &mut participants);
    }

    let mut taffy_style = map_style(&style);
    if let Some((width, height)) = replaced {
        taffy_style.size = TaffySize {
            width: taffy::style_helpers::length(width),
            height: taffy::style_helpers::length(height),
        };
    }
    let taffy_node = if participants.is_empty() {
        boxes.tree.new_leaf(taffy_style)
    } else {
        boxes.tree.new_with_children(taffy_style, &participants)
    }
    .ok()?;
    boxes.link(node, taffy_node);
    Some(taffy_node)
}

/// 收集容器 `node` 的参与盒：块级子盒 + run 叶节点（按 DOM 序）。
///
/// `style` 是容器的 computed style（直接文本子节点的样式来源）；行内收集状态
/// 跨 `contents` 边界持续，避免跨界空白丢失。
fn collect_children_boxes<'a>(
    ctx: &'a Context<'a>,
    boxes: &mut BoxTree<'a>,
    node: NodeId,
    style: &ComputedStyle,
    kind: WalkKind,
    run: &mut Option<InlineRun<'a>>,
    participants: &mut Vec<TaffyNode>,
) {
    for child in ctx.document.children(node) {
        match ctx.document.node(child) {
            Some(NodeKind::Text(text)) => {
                // 直接文本：样式取父元素
                run.get_or_insert_with(|| InlineRun::new(ctx))
                    .push_text(text, node, style);
            }
            Some(NodeKind::Element(_)) => {
                let Some(child_style) = ctx.styles.get(&child) else {
                    continue;
                };
                match child_style.display {
                    DisplayValue::None => {}
                    // contents：盒子移除，子内容提升到当前层（穿透；收集状态保持）
                    DisplayValue::Contents => {
                        collect_children_boxes(
                            ctx,
                            boxes,
                            child,
                            child_style,
                            kind,
                            run,
                            participants,
                        );
                    }
                    // 块容器走法：行内级子元素进 run 叶
                    _ if kind == WalkKind::BlockContainer
                        && is_inline_level(child_style.display) =>
                    {
                        run.get_or_insert_with(|| InlineRun::new(ctx))
                            .push_element(child, child_style);
                    }
                    // 其余产盒子元素（块级；或 flex 走法下的全部子元素 blockify）
                    _ => {
                        flush_run(run, ctx, boxes, style, participants);
                        if let Some(taffy) = build_box(ctx, boxes, child) {
                            participants.push(taffy);
                        }
                    }
                }
            }
            // 注释与处理指令不产生盒
            _ => {}
        }
    }
}

/// 把当前行内流刷成一个匿名 run 叶节点（CSS Display §3.3 / CSS Flexbox §4）。
///
/// 空白序列不产生空叶节点；建节点失败时该段行内内容降级丢弃（与 build_box 的
/// 降级策略一致）。
fn flush_run<'a>(
    run: &mut Option<InlineRun<'a>>,
    ctx: &'a Context<'a>,
    boxes: &mut BoxTree<'a>,
    strut_style: &ComputedStyle,
    participants: &mut Vec<TaffyNode>,
) {
    let Some(finished) = run.take() else {
        return;
    };
    let items = finished.finish();
    if items.is_empty() {
        return;
    }
    if let Ok(taffy) = boxes.tree.new_leaf(run_leaf_style()) {
        boxes.runs.insert(
            taffy,
            InlineRunNode {
                ctx,
                items: RefCell::new(items),
                strut_style: strut_style.clone(),
                lines_cache: RefCell::new(Vec::new()),
            },
        );
        participants.push(taffy);
    }
}

/// `node` 若为块级替换元素（HTML 命名空间的 `<img>`），返回其 definite 尺寸。
///
/// 尺寸解析与 `inline::image_box_size` 同源（`width`/`height` 属性 → 自然尺寸 →
/// 默认对象尺寸 300×150），保证建树与行内 `<img>` 行为一致。
fn replaced_leaf_size(ctx: &Context<'_>, node: NodeId) -> Option<(f32, f32)> {
    let is_img = matches!(
        ctx.document.node(node),
        Some(NodeKind::Element(data))
            if data.namespace == nexty_dom::Namespace::Html && data.name == "img"
    );
    is_img.then(|| crate::inline::image_box_size(ctx.document, node, ctx.image_sizes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexty_css::{Edges, MarginValue, PaddingValue, SizeValue};
    use nexty_dom::{Attribute, Document, ElementData, Namespace};
    use taffy::style::ExpandedDimension;

    /// 测试夹具：自持 arena DOM / 样式表 / shaper / 图片尺寸，按需借出 `Context`。
    ///
    /// shaper 用真实 `ParleyTextShaper`（`TextShaper` 的唯一实现）；它内部是
    /// `RefCell` 故非 `Sync`，由夹具独占而非全局共享。本模块不涉字形测量，
    /// shaper 仅为满足 `Context` 的签名。
    struct Fixture {
        document: Document,
        shaper: nexty_text::ParleyTextShaper,
        styles: HashMap<NodeId, ComputedStyle>,
        image_sizes: HashMap<NodeId, (f32, f32)>,
    }

    impl Fixture {
        /// 新建夹具：根 `<html>`，并登记它为块级。
        fn new() -> Self {
            let mut document = Document::new();
            let root = document.root();
            let html = document.create_node(NodeKind::Element(ElementData {
                name: "html".into(),
                namespace: Namespace::Html,
                attributes: Vec::new(),
            }));
            document.append_child(html, root).expect("append html");

            let mut styles = HashMap::new();
            styles.insert(html, display_style(DisplayValue::Block));

            Self {
                document,
                shaper: nexty_text::ParleyTextShaper::new(),
                styles,
                image_sizes: HashMap::new(),
            }
        }

        /// 根元素 `<html>` 的 id。
        fn html(&self) -> NodeId {
            self.document
                .first_child(self.document.root())
                .expect("html 存在")
        }

        /// 追加一个块级子元素并登记样式，返回其 id。
        fn block_child(&mut self, name: &str) -> NodeId {
            let node = self.element(name);
            self.styles.insert(node, display_style(DisplayValue::Block));
            node
        }

        /// 追加一个指定 display 的子元素并登记样式，返回其 id。
        fn child_with_display(&mut self, name: &str, display: DisplayValue) -> NodeId {
            let node = self.element(name);
            self.styles.insert(node, display_style(display));
            node
        }

        /// 追加一个 HTML 命名空间的元素到 `<html>` 下（不登记样式）。
        fn element(&mut self, name: &str) -> NodeId {
            let parent = self.html();
            self.element_under(parent, name)
        }

        /// 追加一个 HTML 命名空间的元素到指定父下（不登记样式）。
        fn element_under(&mut self, parent: NodeId, name: &str) -> NodeId {
            let node = self.document.create_node(NodeKind::Element(ElementData {
                name: name.into(),
                namespace: Namespace::Html,
                attributes: Vec::new(),
            }));
            self.document
                .append_child(node, parent)
                .expect("append element");
            node
        }

        /// 追加一个指定 display 的元素到指定父下并登记样式。
        fn display_child_under(
            &mut self,
            parent: NodeId,
            name: &str,
            display: DisplayValue,
        ) -> NodeId {
            let node = self.element_under(parent, name);
            self.styles.insert(node, display_style(display));
            node
        }

        /// 在指定父下追加 `<img>`，登记为块级，并登记自然尺寸。
        fn img_child(
            &mut self,
            parent: NodeId,
            width_attr: Option<&str>,
            natural: (f32, f32),
        ) -> NodeId {
            let attributes = width_attr
                .map(|value| {
                    vec![Attribute {
                        name: "width".into(),
                        namespace: Namespace::None,
                        value: value.into(),
                    }]
                })
                .unwrap_or_default();
            let node = self.document.create_node(NodeKind::Element(ElementData {
                name: "img".into(),
                namespace: Namespace::Html,
                attributes,
            }));
            self.document
                .append_child(node, parent)
                .expect("append img");
            self.styles.insert(node, display_style(DisplayValue::Block));
            self.image_sizes.insert(node, natural);
            node
        }

        /// 追加一个文本子节点。
        fn text_child(&mut self, parent: NodeId, data: &str) -> NodeId {
            let node = self.document.create_node(NodeKind::Text(data.into()));
            self.document
                .append_child(node, parent)
                .expect("append text");
            node
        }

        fn context(&self) -> Context<'_> {
            Context {
                document: &self.document,
                shaper: &self.shaper,
                styles: &self.styles,
                image_sizes: &self.image_sizes,
            }
        }
    }

    fn display_style(display: DisplayValue) -> ComputedStyle {
        let mut style = ComputedStyle::initial();
        style.display = display;
        style
    }

    #[test]
    fn single_block_element_builds_one_node() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        let div = fixture.block_child("div");

        let ctx = fixture.context();
        let boxes = build_box_tree(&ctx, html);

        assert_eq!(boxes.len(), 2, "html 与 div 各产一个盒");
        let div_taffy = boxes.taffy_node(div).expect("div 有 taffy 节点");
        assert_eq!(boxes.dom_node(div_taffy), Some(div), "逆映射正确");
    }

    #[test]
    fn display_none_skips_subtree_entirely() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        // outer 是 none，inner 挂在外层（真正是outer 的后代）
        let outer = fixture.child_with_display("div", DisplayValue::None);
        let inner = fixture.display_child_under(outer, "span", DisplayValue::Block);
        let grandchild = fixture.display_child_under(inner, "em", DisplayValue::Block);

        let ctx = fixture.context();
        let boxes = build_box_tree(&ctx, html);

        assert_eq!(boxes.len(), 1, "只有 html 产盒，none 的子树整棵被跳过");
        assert!(boxes.taffy_node(outer).is_none(), "none 元素无节点");
        assert!(boxes.taffy_node(inner).is_none(), "none 的直接后代无节点");
        assert!(
            boxes.taffy_node(grandchild).is_none(),
            "none 的更深后代也无节点"
        );
    }

    #[test]
    fn display_none_root_builds_no_root_node() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        fixture.child_with_display("div", DisplayValue::Block);
        fixture
            .styles
            .insert(html, display_style(DisplayValue::None));

        let ctx = fixture.context();
        let boxes = build_box_tree(&ctx, html);

        assert!(boxes.root.is_none(), "none 根不产盒");
        assert!(boxes.root_is_element_box);
    }

    #[test]
    fn display_contents_promotes_children_to_parent() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        let wrapper = fixture.block_child("div");
        let lifted = fixture.display_child_under(wrapper, "section", DisplayValue::Contents);
        let deep = fixture.display_child_under(lifted, "article", DisplayValue::Block);

        let ctx = fixture.context();
        let boxes = build_box_tree(&ctx, html);

        // contents 自身不产盒，deep 被提升为 wrapper 的直接参与盒
        assert_eq!(
            boxes.len(),
            3,
            "html + wrapper + deep 产盒，contents 不产盒"
        );
        assert!(boxes.taffy_node(lifted).is_none(), "contents 元素无节点");
        assert!(boxes.taffy_node(deep).is_some(), "deep 提升后仍建盒");

        // deep 的 taffy 父节点是 wrapper（穿透一层），不是 lifted
        let wrapper_taffy = boxes.taffy_node(wrapper).expect("wrapper");
        let deep_taffy = boxes.taffy_node(deep).expect("deep");
        assert_eq!(
            boxes.tree.children(wrapper_taffy).expect("children"),
            vec![deep_taffy],
            "deep 挂在 wrapper 下而非 lifted 下"
        );
    }

    #[test]
    fn nested_contents_promotes_through_multiple_levels() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        let outer = fixture.display_child_under(html, "div", DisplayValue::Contents);
        let middle = fixture.display_child_under(outer, "div", DisplayValue::Contents);
        let deep = fixture.display_child_under(middle, "article", DisplayValue::Block);

        let ctx = fixture.context();
        let boxes = build_box_tree(&ctx, html);

        // 两层 contents 都穿透，deep 直接成为 html 的参与盒
        assert_eq!(boxes.len(), 2, "只有 html 与 deep 产盒");
        assert!(boxes.taffy_node(outer).is_none());
        assert!(boxes.taffy_node(middle).is_none());

        let html_taffy = boxes.taffy_node(html).expect("html");
        assert_eq!(
            boxes.tree.children(html_taffy).expect("children"),
            vec![boxes.taffy_node(deep).expect("deep")],
            "deep 穿透两层后挂在 html 下"
        );
    }

    /// 行内内容包装成 run 叶节点（不占元素映射表），flex 容器外行内元素进 run。
    #[test]
    fn inline_content_becomes_run_leaf() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        let span = fixture.child_with_display("span", DisplayValue::Inline);
        fixture.text_child(span, "hello");

        let ctx = fixture.context();
        let boxes = build_box_tree(&ctx, html);

        assert_eq!(boxes.len(), 1, "只有 html 产元素盒；行内内容是 run 叶");
        assert!(boxes.taffy_node(span).is_none(), "inline 元素无节点");
        let html_taffy = boxes.taffy_node(html).expect("html");
        let children = boxes.tree.children(html_taffy).expect("children");
        assert_eq!(children.len(), 1, "一段连续行内流 → 一个 run 叶");
        assert!(boxes.runs.contains_key(&children[0]), "子节点是 run 叶");
    }

    #[test]
    fn whitespace_only_runs_produce_no_leaf() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        fixture.text_child(html, "   ");

        let ctx = fixture.context();
        let boxes = build_box_tree(&ctx, html);

        assert_eq!(boxes.len(), 1);
        let html_taffy = boxes.taffy_node(html).expect("html");
        assert!(
            boxes
                .tree
                .children(html_taffy)
                .expect("children")
                .is_empty(),
            "纯空白不产 run 叶"
        );
    }

    /// 块级子盒切断行内流：两段行内流各自成 run 叶，与块级子盒按序排列。
    #[test]
    fn block_children_interrupt_runs() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        fixture.text_child(html, "before ");
        let middle = fixture.block_child("div");
        fixture.text_child(html, " after");

        let ctx = fixture.context();
        let boxes = build_box_tree(&ctx, html);

        let html_taffy = boxes.taffy_node(html).expect("html");
        let children = boxes.tree.children(html_taffy).expect("children");
        assert_eq!(children.len(), 3, "run 叶 + div + run 叶");
        assert!(boxes.runs.contains_key(&children[0]));
        assert_eq!(boxes.taffy_node(middle), Some(children[1]));
        assert!(boxes.runs.contains_key(&children[2]));
    }

    /// flex 容器把行内级子元素 blockify 成独立 item（CSS Flexbox §4）。
    #[cfg(feature = "flex")]
    #[test]
    fn flex_container_blockifies_inline_children() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        let container = fixture.display_child_under(html, "div", DisplayValue::Flex);
        let span = fixture.display_child_under(container, "span", DisplayValue::Inline);
        let block = fixture.display_child_under(container, "p", DisplayValue::Block);

        let ctx = fixture.context();
        let boxes = build_box_tree(&ctx, html);

        let container_taffy = boxes.taffy_node(container).expect("container");
        let children = boxes.tree.children(container_taffy).expect("children");
        assert_eq!(children.len(), 2, "span 与 p 都 blockify 成 flex item");
        assert_eq!(boxes.taffy_node(span), Some(children[0]));
        assert_eq!(boxes.taffy_node(block), Some(children[1]));
    }

    /// flex 容器的连续文本段成为匿名 flex item（run 叶）。
    #[cfg(feature = "flex")]
    #[test]
    fn flex_container_text_runs_become_anonymous_items() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        let container = fixture.display_child_under(html, "div", DisplayValue::Flex);
        fixture.text_child(container, "item text ");

        let ctx = fixture.context();
        let boxes = build_box_tree(&ctx, html);

        let container_taffy = boxes.taffy_node(container).expect("container");
        let children = boxes.tree.children(container_taffy).expect("children");
        assert_eq!(children.len(), 1, "连续文本段 → 一个匿名 item");
        assert!(boxes.runs.contains_key(&children[0]));
    }

    #[test]
    fn comments_produce_no_box() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        let node = fixture
            .document
            .create_node(NodeKind::Comment("note".into()));
        fixture
            .document
            .append_child(node, html)
            .expect("append comment");

        let ctx = fixture.context();
        let boxes = build_box_tree(&ctx, html);

        assert_eq!(boxes.len(), 1, "注释不产盒");
    }

    #[test]
    fn img_builds_leaf_with_default_object_size() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        // 无 width 属性、无自然尺寸登记 → CSS Images §5.1 默认对象尺寸 300×150
        let img = fixture.img_child(html, None, (300.0, 150.0));
        fixture.image_sizes.remove(&img);

        let ctx = fixture.context();
        let boxes = build_box_tree(&ctx, html);

        assert_eq!(boxes.len(), 2, "img 建叶节点");
        let img_taffy = boxes.taffy_node(img).expect("img 节点");
        assert!(
            boxes
                .tree
                .children(img_taffy)
                .expect("img children")
                .is_empty(),
            "img 是叶节点"
        );

        let style = boxes.tree.style(img_taffy).expect("img style");
        assert!(matches!(
            style.size.width.expand(),
            ExpandedDimension::Length(width) if (width - 300.0).abs() < 1e-3
        ));
        assert!(matches!(
            style.size.height.expand(),
            ExpandedDimension::Length(height) if (height - 150.0).abs() < 1e-3
        ));
    }

    #[test]
    fn img_uses_image_sizes_over_default() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        let img = fixture.img_child(html, None, (64.0, 32.0));

        let ctx = fixture.context();
        let boxes = build_box_tree(&ctx, html);

        let img_taffy = boxes.taffy_node(img).expect("img 节点");
        let style = boxes.tree.style(img_taffy).expect("img style");
        assert!(matches!(
            style.size.width.expand(),
            ExpandedDimension::Length(width) if (width - 64.0).abs() < 1e-3
        ));
        assert!(matches!(
            style.size.height.expand(),
            ExpandedDimension::Length(height) if (height - 32.0).abs() < 1e-3
        ));
    }

    #[test]
    fn img_width_attribute_beats_natural_size() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        let img = fixture.img_child(html, Some("50"), (64.0, 32.0));

        let ctx = fixture.context();
        let boxes = build_box_tree(&ctx, html);

        let img_taffy = boxes.taffy_node(img).expect("img 节点");
        let style = boxes.tree.style(img_taffy).expect("img style");
        assert!(
            matches!(
                style.size.width.expand(),
                ExpandedDimension::Length(width) if (width - 50.0).abs() < 1e-3
            ),
            "width 属性优先于自然尺寸"
        );
    }

    #[test]
    fn mapping_is_bidirectional_and_complete() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        fixture.block_child("div");
        fixture.child_with_display("section", DisplayValue::Flex);

        let ctx = fixture.context();
        let boxes = build_box_tree(&ctx, html);

        assert_eq!(boxes.len(), 3);
        assert_eq!(
            boxes.dom_to_taffy.len(),
            boxes.taffy_to_dom.len(),
            "双向映射等长"
        );
        for (dom, taffy) in &boxes.dom_to_taffy {
            assert_eq!(boxes.dom_node(*taffy), Some(*dom), "双向映射自洽");
        }
    }

    #[test]
    fn container_parent_child_relationship() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        let parent = fixture.block_child("div");
        let child = fixture.display_child_under(parent, "p", DisplayValue::Block);

        let ctx = fixture.context();
        let boxes = build_box_tree(&ctx, html);

        let parent_taffy = boxes.taffy_node(parent).expect("parent");
        let child_taffy = boxes.taffy_node(child).expect("child");
        assert_eq!(
            boxes.tree.children(parent_taffy).expect("children"),
            vec![child_taffy],
            "child 挂在 parent 下"
        );
    }

    #[test]
    fn explicit_sizes_and_margins_reach_taffy_style() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        let div = fixture.block_child("div");

        let mut style = display_style(DisplayValue::Block);
        style.width = SizeValue::Length(200.0);
        style.height = SizeValue::Length(100.0);
        style.margin = Edges {
            top: MarginValue::Length(10.0),
            right: MarginValue::Length(0.0),
            bottom: MarginValue::Length(0.0),
            left: MarginValue::Length(0.0),
        };
        style.padding = Edges {
            top: PaddingValue::Length(4.0),
            right: PaddingValue::Length(4.0),
            bottom: PaddingValue::Length(4.0),
            left: PaddingValue::Length(4.0),
        };
        fixture.styles.insert(div, style);

        let ctx = fixture.context();
        let boxes = build_box_tree(&ctx, html);
        let div_taffy = boxes.taffy_node(div).expect("div");
        let mapped = boxes.tree.style(div_taffy).expect("style");

        assert!(matches!(
            mapped.size.width.expand(),
            ExpandedDimension::Length(width) if (width - 200.0).abs() < 1e-3
        ));
        assert!(matches!(
            mapped.margin.top.expand(),
            taffy::style::ExpandedLengthPercentageAuto::Length(top) if (top - 10.0).abs() < 1e-3
        ));
        assert!(matches!(
            mapped.padding.left.expand(),
            taffy::style::ExpandedLengthPercentage::Length(left) if (left - 4.0).abs() < 1e-3
        ));
    }
}
