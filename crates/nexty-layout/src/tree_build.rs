//! 盒树构建：arena DOM + computed style → taffy 盒树。
//!
//! 行为ground truth：CSS Display §3（盒生成规则）、CSS Box Model。
//!
//! **盒生成规则**（与 CSS Display §3 逐条对应）：
//!
//! - `display: none`：不产盒（§3.1「不生成任何盒」），整棵子树跳过；
//! - `display: contents`：元素本身不产盒（§3.2「自身不生成盒」），其子节点
//!   **提升**为父盒的直接参与盒（穿透一层）；
//! - 块级替换元素（`<img>`）：以 `image_sizes` 的自然尺寸（或 CSS Images §5.1
//!   的默认对象尺寸 300×150）为 definite size 建叶节点，不建子树；
//! - 行内内容（文本与 `display: inline*` 的元素）：**本模块不建 taffy 节点**。
//!   taffy 不做文本布局，行内内容须先由 `inline` 模块断行，再按 T4 的约定包装成
//!   匿名块节点并以 measure function 提供内在尺寸。
//! - 其余块级盒：建容器节点，子节点递归挂上。
//!
//! **映射表**：`NodeId ↔ taffy Node` 双向映射供 T5 回填 `Fragment` 时定位几何。
//! DOM `NodeId` 与 taffy `Node` 是不同类型，taffy 的 `NodeId` 只在本 crate 内部
//! 流转，绝不外泄（AGENTS.md 硬规则）。
//!
//! 块盒几何由 taffy 在 T3 接管；本模块只负责「有哪些盒、谁是谁的子盒」。
//!
//! **过渡状态**：本模块的产物`BoxTree` 在 T3 接线（`block.rs` 调用
//! `build_box_tree` 并 `compute_layout`）之前没有生产调用方，故按模块整体抑制
//! dead_code；T3 落地后移除本属性。

#![cfg_attr(not(test), allow(dead_code))]

use std::collections::HashMap;

use nexty_css::{ComputedStyle, DisplayValue};
use nexty_dom::{NodeId, NodeKind};
use taffy::geometry::Size as TaffySize;
use taffy::style_helpers::length;
use taffy::tree::{NodeId as TaffyNode, TaffyTree};

use crate::block::Context;
use crate::style_map::map_style;

/// 构建产物：taffy 盒树 + 双向映射表。
///
/// taffy 的类型不出现在本 crate 的 pub 导出中，故本类型为`pub(crate)`。
pub(crate) struct BoxTree {
    /// taffy 盒树。`compute_layout` 与 `Layout` 读取在 T3/T5 进行。
    pub tree: TaffyTree<()>,
    /// DOM 节点 → taffy 节点。仅含**实际产盒**的元素（`none`/`contents` 不在其中）。
    pub dom_to_taffy: HashMap<NodeId, TaffyNode>,
    /// taffy 节点 → DOM 节点，`dom_to_taffy` 的逆映射。
    pub taffy_to_dom: HashMap<TaffyNode, NodeId>,
}

impl BoxTree {
    /// 构造空盒树。
    #[must_use]
    pub fn new() -> Self {
        Self {
            tree: TaffyTree::new(),
            dom_to_taffy: HashMap::new(),
            taffy_to_dom: HashMap::new(),
        }
    }

    /// 登记一个已建节点的双向映射。
    fn link(&mut self, dom: NodeId, taffy: TaffyNode) {
        self.dom_to_taffy.insert(dom, taffy);
        self.taffy_to_dom.insert(taffy, dom);
    }

    /// 按 DOM 节点取taffy 节点；未产盒（`none` / `contents` / 行内内容）返回 `None`。
    #[must_use]
    pub fn taffy_node(&self, dom: NodeId) -> Option<TaffyNode> {
        self.dom_to_taffy.get(&dom).copied()
    }

    /// 按 taffy 节点取 DOM 节点。
    #[must_use]
    pub fn dom_node(&self, taffy: TaffyNode) -> Option<NodeId> {
        self.taffy_to_dom.get(&taffy).copied()
    }

    /// taffy 节点数（供测试与断言使用）。
    #[must_use]
    pub fn len(&self) -> usize {
        self.dom_to_taffy.len()
    }
}

impl Default for BoxTree {
    fn default() -> Self {
        Self::new()
    }
}

/// 自研遍历的中间结果：某个 DOM 节点在盒树中的产出。
enum Built {
    /// 该元素产盒，节点 id 已登记。
    Node(TaffyNode),
    /// `display: contents`：自身不产盒，把已建子节点提升给调用方。
    /// T4 起承载匿名块内容；当前恒为空。
    Lifted(Vec<TaffyNode>),
}

/// 构建 `node` 子树，返回它对父盒的贡献。
///
/// 顶层返回 `Node`（元素自身产盒）或 `Lifted`（`contents`，子节点提升）。
fn build_subtree(ctx: &Context<'_>, boxes: &mut BoxTree, node: NodeId) -> Built {
    let style = ctx
        .styles
        .get(&node)
        .cloned()
        .unwrap_or_else(ComputedStyle::initial);

    match style.display {
        // §3.1：display: none 不生成任何盒，整棵子树不参与布局。
        DisplayValue::None => Built::Lifted(Vec::new()),
        // §3.2：display: contents 自身不生成盒，子节点提升为父盒的参与盒。
        DisplayValue::Contents => {
            let mut promoted = Vec::new();
            for child in ctx.document.children(node) {
                collect_participant(ctx, boxes, child, &mut promoted);
            }
            Built::Lifted(promoted)
        }
        // 行内内容不在本模块建节点：taffy 不做文本布局，留给 T4 断行后包匿名块。
        DisplayValue::Inline
        | DisplayValue::InlineBlock
        | DisplayValue::InlineFlex
        | DisplayValue::InlineGrid
        | DisplayValue::InlineTable => Built::Lifted(Vec::new()),
        _ => {
            let children = build_participants(ctx, boxes, node);
            let mut style = map_style(&style);
            // 块级替换元素：以自然尺寸为 definite size 建叶节点，taffy 不再解算内容。
            if let Some(size) = replaced_leaf_size(ctx, node) {
                style.size = TaffySize {
                    width: length(size.0),
                    height: length(size.1),
                };
            }
            let Some(taffy_node) = build_node(&mut boxes.tree, style, &children) else {
                // 建节点失败（taffy 内部 SlotMap 容量错误）：不登记映射，该子树
                // 降级为「无盒」。此处刻意不用 panic —— 渲染隔离要求布局层不炸进程。
                return Built::Lifted(Vec::new());
            };
            boxes.link(node, taffy_node);
            Built::Node(taffy_node)
        }
    }
}

/// 把 `node` 的参与盒收集进 `out`（块级子盒 + `contents` 提升上来的盒）。
///
/// 行内内容按 CSS Display §3.3 归入匿名块，留给 T4；此处不贡献参与盒。
fn build_participants(ctx: &Context<'_>, boxes: &mut BoxTree, node: NodeId) -> Vec<TaffyNode> {
    let mut participants = Vec::new();
    for child in ctx.document.children(node) {
        collect_participant(ctx, boxes, child, &mut participants);
    }
    participants
}

/// 处理单个子节点：`contents` 提升（穿透），其余交给 [`build_subtree`]。
fn collect_participant(
    ctx: &Context<'_>,
    boxes: &mut BoxTree,
    child: NodeId,
    out: &mut Vec<TaffyNode>,
) {
    // 文本与注释 / 处理指令不产盒（CSS Display §3.1）。
    if !matches!(ctx.document.node(child), Some(NodeKind::Element(_))) {
        return;
    }
    match build_subtree(ctx, boxes, child) {
        Built::Node(node) => out.push(node),
        // contents：子节点提升为当前盒的直接参与盒（穿透一层）。
        Built::Lifted(mut promoted) => out.append(&mut promoted),
    }
}

/// 创建 taffy 节点：无子节点建叶节点，有子节点建容器节点。
///
/// `new_with_children` 要求子节点已存在且尚未挂父，故先递归建子再一次性挂父。
///
/// `new_leaf` / `new_with_children` 内部走 SlotMap（容量不足自动扩容），实际
/// 不返回 `Err`；仍如实传播错误而非伪造节点 id——伪造 id 会污染 `BoxTree` 的双向
/// 映射，让 T5 回填到不存在的节点上。
fn build_node(
    tree: &mut TaffyTree<()>,
    style: taffy::Style,
    children: &[TaffyNode],
) -> Option<TaffyNode> {
    let result = if children.is_empty() {
        tree.new_leaf(style)
    } else {
        tree.new_with_children(style, children)
    };
    result.ok()
}

/// `node` 若为块级替换元素（HTML 命名空间的 `<img>`），返回其 definite 尺寸。
///
/// 尺寸解析与 `inline::image_box_size` 同源（`width`/`height` 属性 → 自然尺寸 →
/// 默认对象尺寸 300×150），保证 T2 建树与既有 `<img>` 行为一致。
fn replaced_leaf_size(ctx: &Context<'_>, node: NodeId) -> Option<(f32, f32)> {
    let is_img = matches!(
        ctx.document.node(node),
        Some(NodeKind::Element(data))
            if data.namespace == nexty_dom::Namespace::Html && data.name == "img"
    );
    is_img.then(|| crate::inline::image_box_size(ctx.document, node, ctx.image_sizes))
}

/// 从根元素开始构建整棵盒树。
///
/// 返回 `None` 表示根元素未产盒（`display: none` / `contents` 提升为空）。
pub(crate) fn build_box_tree(ctx: &Context<'_>, root: NodeId) -> Option<BoxTree> {
    let mut boxes = BoxTree::new();
    match build_subtree(ctx, &mut boxes, root) {
        Built::Node(_) => Some(boxes),
        // 根元素 display: contents 时其子节点即根盒，返回已建的树。
        Built::Lifted(_) => Some(boxes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexty_css::{Edges, MarginValue, PaddingValue, SizeValue};
    use nexty_dom::{Attribute, Document, ElementData, Namespace};
    use taffy::style::{ExpandedDimension, ExpandedLengthPercentage, ExpandedLengthPercentageAuto};

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

        let boxes = build_box_tree(&fixture.context(), html).expect("root builds");

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

        let boxes = build_box_tree(&fixture.context(), html).expect("root builds");

        assert_eq!(boxes.len(), 1, "只有 html 产盒，none 的子树整棵被跳过");
        assert!(boxes.taffy_node(outer).is_none(), "none 元素无节点");
        assert!(boxes.taffy_node(inner).is_none(), "none 的直接后代无节点");
        assert!(
            boxes.taffy_node(grandchild).is_none(),
            "none 的更深后代也无节点"
        );
    }

    #[test]
    fn display_contents_promotes_children_to_parent() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        let wrapper = fixture.block_child("div");
        let lifted = fixture.display_child_under(wrapper, "section", DisplayValue::Contents);
        let deep = fixture.display_child_under(lifted, "article", DisplayValue::Block);

        let boxes = build_box_tree(&fixture.context(), html).expect("root builds");

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

        let boxes = build_box_tree(&fixture.context(), html).expect("root builds");

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

    #[test]
    fn inline_content_builds_no_taffy_node() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        let span = fixture.child_with_display("span", DisplayValue::Inline);
        fixture.text_child(span, "hello");

        let boxes = build_box_tree(&fixture.context(), html).expect("root builds");

        assert_eq!(boxes.len(), 1, "只有 html 产盒；行内内容留待 T4");
        assert!(boxes.taffy_node(span).is_none(), "inline 元素无节点");
    }

    #[test]
    fn inline_atomic_variants_also_build_no_node() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        fixture.child_with_display("span", DisplayValue::InlineBlock);
        fixture.child_with_display("span", DisplayValue::InlineFlex);
        fixture.child_with_display("table", DisplayValue::InlineTable);

        let boxes = build_box_tree(&fixture.context(), html).expect("root builds");

        assert_eq!(boxes.len(), 1, "行内原子盒同样留给 T4");
    }

    #[test]
    fn comments_and_text_produce_no_box() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        let node = fixture
            .document
            .create_node(NodeKind::Comment("note".into()));
        fixture
            .document
            .append_child(node, html)
            .expect("append comment");
        fixture.text_child(html, "loose text");

        let boxes = build_box_tree(&fixture.context(), html).expect("root builds");

        assert_eq!(boxes.len(), 1, "注释与游离文本不产盒");
    }

    #[test]
    fn img_builds_leaf_with_default_object_size() {
        let mut fixture = Fixture::new();
        let html = fixture.html();
        // 无 width 属性、无自然尺寸登记 → CSS Images §5.1 默认对象尺寸 300×150
        let img = fixture.img_child(html, None, (300.0, 150.0));
        fixture.image_sizes.remove(&img);

        let boxes = build_box_tree(&fixture.context(), html).expect("root builds");

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

        let boxes = build_box_tree(&fixture.context(), html).expect("root builds");

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

        let boxes = build_box_tree(&fixture.context(), html).expect("root builds");

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

        let boxes = build_box_tree(&fixture.context(), html).expect("root builds");

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

        let boxes = build_box_tree(&fixture.context(), html).expect("root builds");

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

        let boxes = build_box_tree(&fixture.context(), html).expect("root builds");
        let div_taffy = boxes.taffy_node(div).expect("div");
        let mapped = boxes.tree.style(div_taffy).expect("style");

        assert!(matches!(
            mapped.size.width.expand(),
            ExpandedDimension::Length(width) if (width - 200.0).abs() < 1e-3
        ));
        assert!(matches!(
            mapped.margin.top.expand(),
            ExpandedLengthPercentageAuto::Length(top) if (top - 10.0).abs() < 1e-3
        ));
        assert!(matches!(
            mapped.padding.left.expand(),
            ExpandedLengthPercentage::Length(left) if (left - 4.0).abs() < 1e-3
        ));
    }
}
