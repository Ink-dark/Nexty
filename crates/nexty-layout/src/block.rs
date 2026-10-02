//! 块级布局：宽度/高度解析、盒模型与 margin 折叠。
//!
//! 行为 ground truth：
//! - [CSS 2.1 §8.3.1](https://www.w3.org/TR/CSS21/box.html#collapsing-margins)（margin 折叠）
//! - [§9.2.1.1](https://www.w3.org/TR/CSS21/visuren.html#anonymous-block-level)（匿名块盒）
//! - [§10.3.3](https://www.w3.org/TR/CSS21/visudet.html#blockwidth)（块级非替换宽度解析）
//! - [§10.5/§10.6.3](https://www.w3.org/TR/CSS21/visudet.html#the-height-property)（高度）
//!
//! 已知偏差：`display: inline-block` 按 inline 参与行内流；`float`、定位、
//! min/max 尺寸、box-sizing 未实现；百分比高度仅在包含块高度明确时解析，
//! 否则按 auto；末尾自折叠子盒对父内容高的贡献按折叠链近似处理。

use std::collections::HashMap;

use nexty_css::{ComputedStyle, DisplayValue, MarginValue, PaddingValue, SizeValue};
use nexty_dom::{Document, Namespace, NodeId, NodeKind};
use nexty_text::TextShaper;

use crate::fragment::{Edges, Fragment, Rect};
use crate::inline::{InlineItem, InlineRun, build_lines, image_box_size};

/// 布局上下文。
pub(crate) struct Context<'a> {
    pub document: &'a Document,
    pub shaper: &'a dyn TextShaper,
    pub styles: &'a HashMap<NodeId, ComputedStyle>,
    /// 已解码图片的自然尺寸（node → 像素宽高，CSS px）。
    pub image_sizes: &'a HashMap<NodeId, (f32, f32)>,
}

/// 一个块的布局结果：片段 + 参与 margin 折叠的边界值。
pub(crate) struct BlockBox {
    pub fragment: Fragment,
    /// 可折叠上外边距（含后代穿透链）。
    pub margin_top: f32,
    /// 可折叠下外边距（含后代穿透链）。
    pub margin_bottom: f32,
    /// 上下外边距是否相邻（自折叠盒，可被穿透）。
    pub self_collapsing: bool,
}

/// 两个相邻 margin 的折叠值（CSS 2.1 §8.3.1）：
/// 全正取最大，全负取最小（绝对值最大），正负混合为最大正值 + 最小负值。
pub(crate) fn collapsed(a: f32, b: f32) -> f32 {
    if a >= 0.0 && b >= 0.0 {
        a.max(b)
    } else if a < 0.0 && b < 0.0 {
        a.min(b)
    } else {
        a + b
    }
}

/// margin/padding 单边的 used value（百分比相对包含块宽度；auto 视作 0）。
fn used_edge(value: MarginValue, containing_width: f32) -> f32 {
    match value {
        MarginValue::Length(px) => px,
        MarginValue::Percent(fraction) => fraction * containing_width,
        MarginValue::Auto => 0.0,
    }
}

fn used_padding(value: PaddingValue, containing_width: f32) -> f32 {
    match value {
        PaddingValue::Length(px) => px,
        PaddingValue::Percent(fraction) => fraction * containing_width,
    }
}

/// 布局整棵文档，返回根元素片段；文档没有元素时返回 `None`。
///
/// 根元素的 margin 不与任何东西折叠（CSS 2.1 §8.3.1），直接决定其相对
/// 视口的偏移。
pub(crate) fn layout_root(ctx: &Context<'_>, root: NodeId, viewport_width: f32) -> Fragment {
    let style = ctx
        .styles
        .get(&root)
        .cloned()
        .unwrap_or_else(ComputedStyle::initial);
    let margins = resolve_edges(style.margin, viewport_width);
    let mut fragment = layout_block_box(
        ctx,
        root,
        &style,
        viewport_width,
        None,
        /* is_root */ true,
    )
    .fragment;
    fragment.border_box.x += margins.left;
    fragment.border_box.y += margins.top;
    fragment
}

/// 四边 used value（margin 用；auto → 0）。
fn resolve_edges(margins: nexty_css::Edges<MarginValue>, containing_width: f32) -> Edges {
    Edges {
        top: used_edge(margins.top, containing_width),
        right: used_edge(margins.right, containing_width),
        bottom: used_edge(margins.bottom, containing_width),
        left: used_edge(margins.left, containing_width),
    }
}

/// css 层的四边 f32 值 → 片段树的 Edges。
fn to_layout_edges(edges: nexty_css::Edges<f32>) -> Edges {
    Edges {
        top: edges.top,
        right: edges.right,
        bottom: edges.bottom,
        left: edges.left,
    }
}

/// `node` 是否为 HTML 命名空间的 `<img>`（替换元素）。
fn is_image_element(document: &Document, node: NodeId) -> bool {
    matches!(
        document.node(node),
        Some(NodeKind::Element(data))
            if data.namespace == Namespace::Html && data.name == "img"
    )
}

fn resolve_padding(paddings: nexty_css::Edges<PaddingValue>, containing_width: f32) -> Edges {
    Edges {
        top: used_padding(paddings.top, containing_width),
        right: used_padding(paddings.right, containing_width),
        bottom: used_padding(paddings.bottom, containing_width),
        left: used_padding(paddings.left, containing_width),
    }
}

/// 块容器的子内容分组：连续的行内级内容归为一组（匿名块候选）。
pub(crate) enum ChildGroup {
    /// 块级元素子盒。
    Block(NodeId, ComputedStyle),
    /// 一段连续的行内级内容（词/原子图片序列）。
    Inline(Vec<InlineItem>),
}

fn group_children(ctx: &Context<'_>, node: NodeId) -> Vec<ChildGroup> {
    let mut groups: Vec<ChildGroup> = Vec::new();
    collect_groups(ctx, node, &mut groups);
    groups
}

fn collect_groups(ctx: &Context<'_>, node: NodeId, groups: &mut Vec<ChildGroup>) {
    let style = ctx
        .styles
        .get(&node)
        .cloned()
        .unwrap_or_else(ComputedStyle::initial);
    // 行内内容按树序一次性收集（InlineRun 跨子节点保持空白折叠状态），
    // 遇块级子盒时把当前行内流刷成一组——匿名块只承接真正的混排边界
    let mut run: Option<InlineRun<'_>> = None;
    for child in ctx.document.children(node) {
        match ctx.document.node(child) {
            Some(NodeKind::Text(text)) => {
                // 文本属于行内内容，样式取父元素
                run.get_or_insert_with(|| {
                    InlineRun::new(ctx.document, ctx.styles, ctx.image_sizes)
                })
                .push_text(text, node, &style);
            }
            Some(NodeKind::Element(_)) => {
                let Some(child_style) = ctx.styles.get(&child) else {
                    continue;
                };
                match child_style.display {
                    DisplayValue::None => {}
                    // display: contents：盒子移除，行内/块级内容都提升到当前层
                    DisplayValue::Contents => {
                        if let Some(finished) = run.take() {
                            push_items(groups, finished.finish());
                        }
                        collect_groups(ctx, child, groups);
                    }
                    DisplayValue::Inline
                    | DisplayValue::InlineBlock
                    | DisplayValue::InlineFlex
                    | DisplayValue::InlineGrid
                    | DisplayValue::InlineTable => {
                        run.get_or_insert_with(|| {
                            InlineRun::new(ctx.document, ctx.styles, ctx.image_sizes)
                        })
                        .push_element(child, child_style);
                    }
                    _ => {
                        // 块级子盒切断行内流
                        if let Some(finished) = run.take() {
                            push_items(groups, finished.finish());
                        }
                        groups.push(ChildGroup::Block(child, child_style.clone()));
                    }
                }
            }
            // 注释与处理指令不产生盒
            _ => {}
        }
    }
    if let Some(finished) = run.take() {
        push_items(groups, finished.finish());
    }
}

/// 把行内项追加到行内分组；空白序列不产生空匿名块。
fn push_items(groups: &mut Vec<ChildGroup>, items: Vec<InlineItem>) {
    if items.is_empty() {
        return;
    }
    match groups.last_mut() {
        Some(ChildGroup::Inline(existing)) => existing.extend(items),
        _ => groups.push(ChildGroup::Inline(items)),
    }
}

/// 布局一个块级盒。
///
/// 返回片段的 `border_box.x` 已相对包含块内容盒左缘定位；`border_box.y`
/// 为占位 0，垂直位置由调用方按 margin 折叠放置。子片段的 y 相对**本盒
/// 内容盒**顶部。
///
/// `is_root` 的盒不与子盒折叠上下边距（CSS 2.1 §8.3.1 根例外）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn layout_block_box(
    ctx: &Context<'_>,
    node: NodeId,
    style: &ComputedStyle,
    containing_width: f32,
    containing_height: Option<f32>,
    is_root: bool,
) -> BlockBox {
    // ---- 块级替换元素（块级 `<img>`）：§10.3.2 的最小近似 ----
    // 内容尺寸来自图片盒尺寸（属性/自然尺寸/默认对象尺寸），CSS width/height
    // 暂不参与；margin auto 视作 0（不做居中，偏差）。
    if is_image_element(ctx.document, node) {
        let border = to_layout_edges(style.border_width);
        let padding = resolve_padding(style.padding, containing_width);
        let margins = resolve_edges(style.margin, containing_width);
        let (image_width, image_height) = image_box_size(ctx.document, node, ctx.image_sizes);
        let fragment = Fragment {
            node,
            anonymous: false,
            border_box: Rect {
                x: margins.left,
                y: 0.0,
                width: image_width + border.left + padding.left + border.right + padding.right,
                height: image_height + border.top + padding.top + border.bottom + padding.bottom,
            },
            border,
            padding,
            style: style.clone(),
            children: Vec::new(),
            lines: Vec::new(),
        };
        return BlockBox {
            fragment,
            margin_top: margins.top,
            margin_bottom: margins.bottom,
            self_collapsing: false,
        };
    }

    // ---- 盒模型 used value ----
    let border = to_layout_edges(style.border_width);
    let padding = resolve_padding(style.padding, containing_width);
    let horizontal_fixed = border.left + padding.left + border.right + padding.right;

    // ---- 宽度解析（§10.3.3，ltr） ----
    let (margin_left, _margin_right, content_width) = resolve_width(
        style.width,
        containing_width,
        &style.margin,
        horizontal_fixed,
    );

    // ---- 子内容分组（§9.2.1.1） ----
    let groups = group_children(ctx, node);
    let has_block_children = groups
        .iter()
        .any(|group| matches!(group, ChildGroup::Block(..)));

    // ---- 高度（§10.5） ----
    let specified_height = resolve_height(style.height, containing_height);

    // ---- margin 折叠前提 ----
    let top_separated = border.top > 0.0 || padding.top > 0.0;
    let bottom_separated = border.bottom > 0.0 || padding.bottom > 0.0;
    // 父的上边距与首个块级子盒的上边距相邻的条件：无上 border/padding；
    // 根元素不与子盒折叠（§8.3.1 根例外）
    let top_collapses = has_block_children && !top_separated && !is_root;
    let bottom_may_collapse = has_block_children && !bottom_separated && !is_root;

    // ---- 子内容布局 ----
    let mut children: Vec<Fragment> = Vec::new();
    let mut y = 0.0; // 最后一个已放置（非自折叠）内容的底边框边
    let mut pending: Option<f32> = None; // 与下一块的上边距相邻的折叠边距
    let mut first_margin_top = 0.0;
    let mut last_margin_bottom = 0.0;
    let mut all_children_self_collapsing = true;

    for group in &groups {
        match group {
            ChildGroup::Block(child_node, child_style) => {
                let result = layout_block_box(
                    ctx,
                    *child_node,
                    child_style,
                    content_width,
                    specified_height,
                    /* is_root */ false,
                );
                let border_y = match pending {
                    None => {
                        first_margin_top = result.margin_top;
                        if top_collapses {
                            // 上边距折叠进父链：子盒边框边与父内容顶重合
                            0.0
                        } else {
                            result.margin_top
                        }
                    }
                    Some(previous_bottom) => y + collapsed(previous_bottom, result.margin_top),
                };
                let mut fragment = result.fragment;
                fragment.border_box.y = border_y;
                all_children_self_collapsing &= result.self_collapsing;
                if result.self_collapsing {
                    // 穿透盒：不推进流；其上下边距并入 pending
                    pending = Some(collapsed(
                        collapsed(pending.unwrap_or(0.0), result.margin_top),
                        result.margin_bottom,
                    ));
                } else {
                    y = border_y + fragment.border_box.height;
                    pending = Some(result.margin_bottom);
                }
                last_margin_bottom = result.margin_bottom;
                children.push(fragment);
            }
            ChildGroup::Inline(items) => {
                // 无块级子盒时行内内容由本盒的 own_lines 承接（见下方），
                // 此处只处理需要匿名块承接的混排情形
                if !has_block_children {
                    continue;
                }
                // 匿名块盒（§9.2.1.1）：边框/外边距为 0，样式取父元素；
                // 绘制端按 anonymous 标志透明处理
                let anonymous_lines = build_lines(ctx.shaper, items, content_width, style);
                let height: f32 = anonymous_lines.iter().map(|line| line.height).sum();
                let border_y = match pending {
                    None => 0.0,
                    Some(previous_bottom) => y + previous_bottom,
                };
                let fragment = Fragment {
                    node,
                    anonymous: true,
                    border_box: Rect {
                        x: 0.0,
                        y: border_y,
                        width: content_width,
                        height,
                    },
                    border: Edges::default(),
                    padding: Edges::default(),
                    style: style.clone(),
                    children: Vec::new(),
                    lines: anonymous_lines,
                };
                all_children_self_collapsing = false;
                y = border_y + height;
                pending = Some(0.0);
                last_margin_bottom = 0.0;
                children.push(fragment);
            }
        }
    }

    // 本盒自身的行盒：仅当没有任何块级子盒时，行内内容直接挂在上面
    // （有块级子盒时行内内容已被匿名块盒承接）
    let own_lines = if has_block_children {
        Vec::new()
    } else {
        let items = groups
            .into_iter()
            .flat_map(|group| match group {
                ChildGroup::Block(..) => Vec::new(),
                ChildGroup::Inline(items) => items,
            })
            .collect::<Vec<_>>();
        build_lines(ctx.shaper, &items, content_width, style)
    };
    let has_line_boxes = !own_lines.is_empty();

    // ---- margin 折叠链（§8.3.1） ----
    let used_margin_top = used_edge(style.margin.top, containing_width);
    let used_margin_bottom = used_edge(style.margin.bottom, containing_width);
    let height_auto = specified_height.is_none();

    let margin_top_collapse = if has_block_children && !top_separated {
        collapsed(used_margin_top, first_margin_top)
    } else {
        used_margin_top
    };
    let margin_bottom_collapse = if bottom_may_collapse && height_auto {
        collapsed(used_margin_bottom, last_margin_bottom)
    } else {
        used_margin_bottom
    };

    // 自折叠：上下边距相邻——无上下 border/padding、高度 0 或 auto、
    // 无行盒、且全部块级子盒自折叠（或没有块级子盒）
    let self_collapsing = !top_separated
        && !bottom_separated
        && specified_height.is_none_or(|height| height == 0.0)
        && !has_line_boxes
        && (!has_block_children || all_children_self_collapsing);

    // ---- 内容高度（§10.6.3） ----
    let content_height = match specified_height {
        Some(height) => height,
        None => {
            if has_block_children {
                // 末块底边框边；若末块下边距不与父的下边距折叠
                //（父有下 border/padding，或父为根），该边距计入内容高
                y + if bottom_may_collapse {
                    0.0
                } else {
                    pending.unwrap_or(0.0)
                }
            } else {
                own_lines.iter().map(|line| line.height).sum()
            }
        }
    };

    let fragment = Fragment {
        node,
        anonymous: false,
        border_box: Rect {
            x: margin_left,
            y: 0.0,
            width: content_width + horizontal_fixed,
            height: content_height + border.top + padding.top + border.bottom + padding.bottom,
        },
        border,
        padding,
        style: style.clone(),
        children,
        lines: own_lines,
    };

    BlockBox {
        fragment,
        margin_top: margin_top_collapse,
        margin_bottom: margin_bottom_collapse,
        self_collapsing,
    }
}

/// 块级非替换元素的宽度解析（CSS 2.1 §10.3.3，ltr）。
///
/// 返回 (used margin-left, used margin-right, content width)。
fn resolve_width(
    width: SizeValue,
    containing_width: f32,
    margins: &nexty_css::Edges<MarginValue>,
    horizontal_fixed: f32,
) -> (f32, f32, f32) {
    let margin_left = used_edge(margins.left, containing_width);
    let margin_right = used_edge(margins.right, containing_width);
    match width {
        // width: auto：占满扣除 margin/border/padding 后的剩余
        SizeValue::Auto => {
            let content = containing_width - horizontal_fixed - margin_left - margin_right;
            (margin_left, margin_right, content)
        }
        SizeValue::Length(px) => resolve_specified_width(
            px,
            containing_width,
            margins,
            margin_left,
            margin_right,
            horizontal_fixed,
        ),
        SizeValue::Percent(fraction) => resolve_specified_width(
            fraction * containing_width,
            containing_width,
            margins,
            margin_left,
            margin_right,
            horizontal_fixed,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_specified_width(
    width: f32,
    containing_width: f32,
    margins: &nexty_css::Edges<MarginValue>,
    margin_left: f32,
    margin_right: f32,
    horizontal_fixed: f32,
) -> (f32, f32, f32) {
    let remaining = containing_width - horizontal_fixed - width;
    match (margins.left, margins.right) {
        // 两侧 auto：等分居中
        (MarginValue::Auto, MarginValue::Auto) => (remaining / 2.0, remaining / 2.0, width),
        // 左 auto：右按指定，左吃剩余（可为负）
        (MarginValue::Auto, _) => (remaining - margin_right, margin_right, width),
        // 右 auto：左按指定，右吃剩余
        (_, MarginValue::Auto) => (margin_left, remaining - margin_left, width),
        // 过约束（ltr）：忽略 margin-right
        (_, _) => (
            margin_left,
            containing_width - horizontal_fixed - width - margin_left,
            width,
        ),
    }
}

/// 高度解析：长度直接生效；百分比仅在包含块高度明确时解析，否则按 auto
/// （CSS 2.1 §10.5）。
fn resolve_height(height: SizeValue, containing_height: Option<f32>) -> Option<f32> {
    match height {
        SizeValue::Auto => None,
        SizeValue::Length(px) => Some(px),
        SizeValue::Percent(fraction) => containing_height.map(|containing| fraction * containing),
    }
}
