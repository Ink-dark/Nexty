//! 块级布局：盒级几何由 taffy 接管（普通流块级 / flex / grid），自研行内断行
//! 通过匿名块 measure 节点接入。
//!
//! 行为 ground truth：
//! - [CSS 2.1 §8.3.1](https://www.w3.org/TR/CSS21/box.html#collapsing-margins)
//!   （margin 折叠，含穿透与正负混合）
//! - [§9.2.1](https://www.w3.org/TR/CSS21/visuren.html#block-level)（块级盒与匿名块）
//! - [§10.3.3](https://www.w3.org/TR/CSS21/visudet.html#blockwidth)（块级非替换
//!   宽度解析，含 margin auto 居中与过约束）
//! - [§10.5/§10.6/§10.7](https://www.w3.org/TR/CSS21/visudet.html#the-height-property)
//!   （高度、min/max 收束）
//! - [CSS Flexbox](https://www.w3.org/TR/css-flexbox-1/)（flex 容器完整算法，
//!   含 wrap / shrink / 对齐值）
//!
//! 几何解算管线（三步，见 `lib.rs` 模块文档）：`tree_build` 建盒树 →
//! `taffy::TaffyTree::compute_layout` 解算 → 本模块读回 `taffy::Layout` 组装
//! [`Fragment`]。自研保留的部分：行内断行（`inline`，经 run 叶 measure 节点
//! 喂给 taffy）、原子行内盒的递归布局（[`layout_atomic_subtree`]，以原子盒为
//! 根建独立 taffy 子树后由断行定位）、根元素 margin 的手动偏移（taffy 根节点
//! 的 location 恒为 0，自身 margin 只缩可用空间）。
//!
//! # taffy 与自研的比对结论（T3，逐项人工核验）
//!
//! 以下差异点已比对并**固化 taffy 行为**（其 WPT 对齐优于旧自研实现）：
//!
//! 1. **margin 折叠**：taffy 的 block 算法完整实现 §8.3.1——相邻兄弟、父子
//!    （首个/末个）、空盒穿透、正负混合取「最大正 + 最小负」。与旧自研语义一致，
//!    仅空盒自身的边框边定位（规范未定义）可能不同：taffy 把穿透盒放在活动
//!    折叠集解析处，旧自研放在 `c(上边距链)` 之后。空盒片段的 y 以 taffy 为准。
//! 2. **flex-shrink 生效**：旧自研不实现收缩（超基和的项溢出）；taffy 按
//!    CSS Flexbox §9.7 收缩（`flex-shrink` 初始值 1）。溢出断言类测试已按
//!    规范行为更新。
//! 3. **百分比高度**：包含块高度不定（auto 链）时按 auto——taffy 与旧自研一致。
//!    包含块高度明确（指定 / min/max 收束后）时按百分比解析，旧自研仅在
//!    直接父级指定高度时解析，taffy 覆盖面更完整。
//! 4. **min/max 收束**：taffy 实现 §10.4/§10.7（含 min 优先、min-height 决定
//!    使用高度时末子盒下边距不再与父折叠的规范细节），与旧自研结论一致。
//! 5. **过约束宽度**（§10.3.3 ltr）：忽略 margin-right——taffy 通过左 margin
//!    定位实现同结论，盒子停在 margin-left 处。
//! 6. **基线传播**：run 叶不向 taffy 上报 baselines（`Baselines::NONE`），
//!    flex `align-items: baseline` 退化为顶部对齐。旧自研同样未实现基线对齐，
//!    列为已知限制（T4/后续补）。
//! 7. **`display: contents` 的根**：根元素无自身盒，其 margin 不再参与定位
//!    （旧自研会给根加 margin 偏移，规范上无盒元素无 margin 可言）。
//! 8. **绝对定位的包含块**（T6）：taffy 不做「最近定位祖先」搜索，绝对定位盒
//!    相对其**树内父盒**定位（CSS Position §6 应上溯最近的定位祖先）。父盒自身
//!    定位（relative/absolute）时与 CSS 一致——这是常见用法；父盒 static 时
//!    偏差，列为已知限制。`position: fixed` 同样映射为 taffy `Absolute`
//!    （视口锚定未建模）。

use nexty_css::{ComputedStyle, MarginValue};
use nexty_dom::NodeId;
use taffy::geometry::Size as TaffySize;
use taffy::style::AvailableSpace;
use taffy::tree::{Layout, NodeId as TaffyNode};

use crate::Context;
use crate::fragment::{Edges, Fragment, LineFragment, Rect};
use crate::tree_build::{BoxTree, build_box_tree, measure_dispatch};

/// 布局整棵文档，返回根元素片段。
///
/// 根元素的 margin 不与任何东西折叠（CSS 2.1 §8.3.1 根例外），直接决定其相对
/// 视口的偏移；`display: contents` 的根自身无盒，不加偏移（见模块文档差异 7）。
pub(crate) fn layout_root(ctx: &Context<'_>, root: NodeId, viewport_width: f32) -> Fragment {
    let style = ctx
        .styles
        .get(&root)
        .cloned()
        .unwrap_or_else(ComputedStyle::initial);
    let mut boxes = build_box_tree(ctx, root);
    let Some(root_taffy) = boxes.root else {
        // 根不产盒（display: none / contents 提升为空）：返回空片段，
        // 维持「有元素必有片段」契约
        return empty_fragment(root, &style);
    };

    compute_tree(
        &mut boxes,
        root_taffy,
        TaffySize {
            width: AvailableSpace::Definite(viewport_width),
            height: AvailableSpace::MaxContent,
        },
    );

    let (margin_left, margin_top) = if boxes.root_is_element_box {
        (
            used_edge(style.margin.left, viewport_width),
            used_edge(style.margin.top, viewport_width),
        )
    } else {
        (0.0, 0.0)
    };
    let mut fragment = assemble_element(&boxes, ctx, root_taffy, root, (0.0, 0.0));
    fragment.border_box.x = margin_left;
    fragment.border_box.y = margin_top;
    fragment
}

/// 以 `node` 为根布局一个**行内原子盒**（inline-block / inline-flex 等），
/// 返回其片段（供 run 叶断行与基线对齐使用）。
///
/// 原子盒是独立格式化上下文：以它为根建独立 taffy 子树，宽度在 `available_width`
/// 下解算（指定宽直接生效；auto 按 shrink-to-fit，taffy 经 run 叶的 min/max-content
/// 测量通道拿到内在尺寸）。`border_box.x` 取自身 margin-left（`y` 占位 0，
/// 最终位置由断行阶段按基线对齐折算）。
pub(crate) fn layout_atomic_subtree(
    ctx: &Context<'_>,
    node: NodeId,
    style: &ComputedStyle,
    available_width: AvailableSpace,
) -> Fragment {
    let mut boxes = build_box_tree(ctx, node);
    let Some(taffy_root) = boxes.root else {
        return empty_fragment(node, style);
    };
    compute_tree(
        &mut boxes,
        taffy_root,
        TaffySize {
            width: available_width,
            height: AvailableSpace::MaxContent,
        },
    );
    let layout = *boxes.tree.layout(taffy_root).expect("atomic layout");
    let mut fragment = assemble_element(&boxes, ctx, taffy_root, node, (0.0, 0.0));
    fragment.border_box.width = layout.size.width;
    fragment.border_box.height = layout.size.height;
    fragment.border_box.x = layout.margin.left;
    fragment.border_box.y = 0.0;
    fragment
}

/// 对一棵盒树执行 taffy 布局（禁用取整，保持与旧自研一致的小数几何）。
fn compute_tree(boxes: &mut BoxTree<'_>, root: TaffyNode, available: TaffySize<AvailableSpace>) {
    boxes.tree.disable_rounding();
    let runs = &boxes.runs;
    let _ = boxes
        .tree
        .compute_layout_with_measure(root, available, |input, node, _, style| {
            measure_dispatch(input, node, style, runs)
        });
}

/// 组装一个产盒元素（或合成根）的 [`Fragment`]。
///
/// `parent_inset` 是父盒内容盒相对其边框盒的偏移（border+padding），用于把
/// taffy 的「相对父边框盒」location 换算到 Fragment 契约的「相对父内容盒」。
///
/// 子节点分派：run 叶 → 全为 run 且容器非 flex/grid 时行盒并入 `lines`（行内
/// 内容直挂块容器的契约），否则生成 `anonymous` 匿名块片段（混排/弹性容器）；
/// 元素盒 → 递归。
fn assemble_element(
    boxes: &BoxTree<'_>,
    ctx: &Context<'_>,
    taffy_node: TaffyNode,
    dom_node: NodeId,
    parent_inset: (f32, f32),
) -> Fragment {
    let layout = *boxes.tree.layout(taffy_node).expect("layout");
    let border_box = Rect {
        x: layout.location.x - parent_inset.0,
        y: layout.location.y - parent_inset.1,
        width: layout.size.width,
        height: layout.size.height,
    };
    let style = ctx
        .styles
        .get(&dom_node)
        .cloned()
        .unwrap_or_else(ComputedStyle::initial);
    // 行盒并入容器仅限块容器走法：flex/grid 容器的 run 叶是独立的
    // 匿名 flex/grid item，行盒语义不同（不应视为容器自身的文本行）
    let merges_lines = !matches!(
        style.display,
        nexty_css::DisplayValue::Flex
            | nexty_css::DisplayValue::InlineFlex
            | nexty_css::DisplayValue::Grid
            | nexty_css::DisplayValue::InlineGrid
    );
    let inset = content_inset(&layout);

    let mut children = Vec::new();
    let mut lines = Vec::new();
    let taffy_children = boxes.tree.children(taffy_node).unwrap_or_default();
    // 行盒并入容器仅当：容器走块容器语义（非 flex/grid，见上方 matches!）
    // 且**全部**子节点都是 run 叶（混排时行盒由匿名块片段承接）。
    let merge_lines = merges_lines
        && !taffy_children.is_empty()
        && taffy_children
            .iter()
            .all(|child| boxes.runs.contains_key(child));
    for child in taffy_children {
        if let Some(run) = boxes.runs.get(&child) {
            let child_layout = *boxes.tree.layout(child).expect("run layout");
            // run 叶无 margin/padding/border：内容盒 = 边框盒
            let position = (
                child_layout.location.x - inset.0,
                child_layout.location.y - inset.1,
            );
            let run_lines = run.lines_at(child_layout.size.width);
            if merge_lines {
                for line in run_lines {
                    lines.push(offset_line(line, position.0, position.1));
                }
            } else {
                children.push(Fragment {
                    node: dom_node,
                    anonymous: true,
                    border_box: Rect {
                        x: position.0,
                        y: position.1,
                        width: child_layout.size.width,
                        height: child_layout.size.height,
                    },
                    border: Edges::default(),
                    padding: Edges::default(),
                    style: style.clone(),
                    children: Vec::new(),
                    lines: run_lines,
                });
            }
        } else if let Some(dom) = boxes.taffy_to_dom.get(&child).copied() {
            children.push(assemble_element(boxes, ctx, child, dom, inset));
        }
    }

    Fragment {
        node: dom_node,
        anonymous: false,
        border_box,
        border: to_edges(layout.border),
        padding: to_edges(layout.padding),
        style,
        children,
        lines,
    }
}

/// 把行盒从 run 叶内容盒坐标系平移到容器内容盒坐标系。
fn offset_line(mut line: LineFragment, dx: f32, dy: f32) -> LineFragment {
    line.baseline += dy;
    for run in &mut line.runs {
        for glyph in &mut run.glyphs {
            glyph.x += dx;
        }
    }
    for image in &mut line.images {
        image.x += dx;
        image.y += dy;
    }
    for atomic in &mut line.boxes {
        atomic.border_box.x += dx;
        atomic.border_box.y += dy;
    }
    line
}

/// 内容盒 inset：(border.left + padding.left, border.top + padding.top)。
fn content_inset(layout: &Layout) -> (f32, f32) {
    (
        layout.border.left + layout.padding.left,
        layout.border.top + layout.padding.top,
    )
}

/// taffy 四边 f32 → 片段树的 Edges。
fn to_edges(edges: taffy::geometry::Rect<f32>) -> Edges {
    Edges {
        top: edges.top,
        right: edges.right,
        bottom: edges.bottom,
        left: edges.left,
    }
}

/// margin 单边的 used value（百分比相对包含块宽度；auto 视作 0）。
///
/// 仅用于根元素偏移（树内节点的 margin 由 taffy 解算）。
fn used_edge(value: MarginValue, containing_width: f32) -> f32 {
    match value {
        MarginValue::Length(px) => px,
        MarginValue::Percent(fraction) => fraction * containing_width,
        MarginValue::Auto => 0.0,
    }
}

/// 根/原子盒不产盒时的空片段兜底。
fn empty_fragment(node: NodeId, style: &ComputedStyle) -> Fragment {
    Fragment {
        node,
        anonymous: false,
        border_box: Rect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        },
        border: Edges::default(),
        padding: Edges::default(),
        style: style.clone(),
        children: Vec::new(),
        lines: Vec::new(),
    }
}
