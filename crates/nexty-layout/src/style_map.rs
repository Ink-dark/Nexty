//! 样式映射层：把 nexty-css 的 [`ComputedStyle`] 单向映射为 taffy 的 [`taffy::Style`]。
//!
//! 本模块是 taffy 接入的「边界」：taffy 类型只在本 crate 内部流动，**绝不**出现在
//! `nexty-layout` 的 pub 导出中（见 `lib.rs` 顶部约束与 AGENTS.md「依赖类型不得外泄」）。
//!
//! # 映射范围
//! 覆盖盒级几何相关属性：`display` / `position` / `box-sizing` / `inset` /
//! `margin` / `padding` / `border` 宽度 / `width` `height` `min-*` `max-*` /
//! flex-*（`direction` `wrap` `grow` `shrink` `basis` `align-items` `align-self`
//! `justify-content`）/ grid-*（`template-columns` `template-rows` `gap` `auto-flow`）。
//!
//! 纯绘制属性（`color` / `background` / `border` 颜色与样式 / `font-*` / 文本度量）
//! 不在此映射，由 [`Fragment`] 直接持有 [`ComputedStyle`] 在 paint 层消费。
//!
//! # 降级与偏差（记入 T3/T6 文档化结论，本轮 intentional）
//! - `var()`：cascade 阶段已按无效声明丢弃，此处走 initial。
//! - CSS 逻辑属性（`margin-block` / `inset-inline` 等）：未建模，按物理属性 initial。
//! - `aspect-ratio`：taffy 支持 `Option<f32>` 但本轮 computed style 未产出，走 `None`。
//! - `display: inline` / `display: contents`：taffy 不做文本/行内布局，本应**不**进
//!   taffy 建节点（由 T2 `tree_build` 过滤，inline 内容在 T4 包装成匿名块）。若意外
//!   到达本函数，按 `Display::Block` blockify 兜底（仅保护几何，行内语义不在此还原）。
//! - `display: table` 及表格内部盒：taffy roadmap 未覆盖 table，本轮按 `Display::Block`
//!   近似（维持现状，非目标）。
//! - grid `min` 尺寸函数中出现 `fr`（非法 CSS）：降级为 `auto`。
//! - `overflow` 仅 `visible` / `hidden` 被建模（`auto`/`scroll`/`clip` 在 cascade 已丢弃），
//!   影响 taffy 的 flex/grid item 自动最小尺寸。

// 过渡期：本模块入口 `map_style` 在 T2/T3 接入前无调用方，其私有辅助函数会被判为
// dead_code。待 `tree_build` / `block` 消费 `map_style` 后移除本行。
#![allow(dead_code)]

use nexty_css::{
    AlignItemsValue, AlignSelfValue, BoxSizingValue, ComputedStyle, DisplayValue,
    FlexDirectionValue, FlexWrapValue, GapValue, GridAutoFlowValue, GridTrack, GridTrackList,
    InsetValue, JustifyContentValue, MarginValue, OverflowValue, PaddingValue, PositionValue,
    SizeValue,
};
use taffy::{
    AlignItems, AlignSelf, BoxSizing, CheapCloneStr, Dimension, Display, FlexDirection, FlexWrap,
    GridAutoFlow, GridTemplateComponent, JustifyContent, LengthPercentage, LengthPercentageAuto,
    MaxTrackSizingFunction, MinTrackSizingFunction, Overflow, Point, Position, Rect, Size, Style,
    style_helpers as sh,
};

/// 把 computed style 映射为 taffy 的盒级几何样式。
///
/// 对未覆盖属性一律走 taffy initial（见模块文档）。
#[must_use]
pub(crate) fn map_style(style: &ComputedStyle) -> Style {
    let mut s = Style::DEFAULT;

    s.display = map_display(style.display);
    s.box_sizing = match style.box_sizing {
        BoxSizingValue::ContentBox => BoxSizing::ContentBox,
        BoxSizingValue::BorderBox => BoxSizing::BorderBox,
    };
    // taffy 无 `Static`：Relative 在 inset 全 auto 时等价于 CSS static。
    s.position = match style.position {
        PositionValue::Static | PositionValue::Relative => Position::Relative,
        PositionValue::Absolute | PositionValue::Fixed => Position::Absolute,
    };
    s.inset = Rect {
        left: inset_side(style.inset.left),
        right: inset_side(style.inset.right),
        top: inset_side(style.inset.top),
        bottom: inset_side(style.inset.bottom),
    };
    s.size = Size {
        width: map_dimension(style.width),
        height: map_dimension(style.height),
    };
    s.min_size = Size {
        width: map_lp_auto(style.min_width),
        height: map_lp_auto(style.min_height),
    };
    s.max_size = Size {
        width: map_lp_auto(style.max_width),
        height: map_lp_auto(style.max_height),
    };
    s.margin = Rect {
        left: margin_side(style.margin.left),
        right: margin_side(style.margin.right),
        top: margin_side(style.margin.top),
        bottom: margin_side(style.margin.bottom),
    };
    s.padding = Rect {
        left: padding_side(style.padding.left),
        right: padding_side(style.padding.right),
        top: padding_side(style.padding.top),
        bottom: padding_side(style.padding.bottom),
    };
    s.border = Rect {
        left: border_side(style.border_width.left),
        right: border_side(style.border_width.right),
        top: border_side(style.border_width.top),
        bottom: border_side(style.border_width.bottom),
    };
    // overflow：仅裁剪标记影响 taffy 的自动最小尺寸（flex/grid item）。
    let overflow = match style.overflow {
        OverflowValue::Visible => Overflow::Visible,
        OverflowValue::Hidden => Overflow::Hidden,
    };
    s.overflow = Point {
        x: overflow,
        y: overflow,
    };

    // Flexbox 容器/子项。
    s.flex_direction = match style.flex_direction {
        FlexDirectionValue::Row => FlexDirection::Row,
        FlexDirectionValue::RowReverse => FlexDirection::RowReverse,
        FlexDirectionValue::Column => FlexDirection::Column,
        FlexDirectionValue::ColumnReverse => FlexDirection::ColumnReverse,
    };
    s.flex_wrap = match style.flex_wrap {
        FlexWrapValue::NoWrap => FlexWrap::NoWrap,
        FlexWrapValue::Wrap => FlexWrap::Wrap,
        FlexWrapValue::WrapReverse => FlexWrap::WrapReverse,
    };
    s.flex_grow = style.flex_grow;
    s.flex_shrink = style.flex_shrink;
    s.flex_basis = map_dimension(style.flex_basis);
    s.align_items = Some(map_align_items(style.align_items));
    s.align_self = map_align_self(style.align_self);
    s.justify_content = Some(map_justify_content(style.justify_content));

    // Grid 容器。
    s.grid_template_columns = map_grid_tracks(&style.grid_template_columns);
    s.grid_template_rows = map_grid_tracks(&style.grid_template_rows);
    s.gap = Size {
        // taffy gap：width = column-gap，height = row-gap。
        width: gap_side(style.column_gap),
        height: gap_side(style.row_gap),
    };
    s.grid_auto_flow = match style.grid_auto_flow {
        GridAutoFlowValue::Row => GridAutoFlow::Row,
        GridAutoFlowValue::Column => GridAutoFlow::Column,
        GridAutoFlowValue::RowDense => GridAutoFlow::RowDense,
        GridAutoFlowValue::ColumnDense => GridAutoFlow::ColumnDense,
    };

    s
}

// ---------------------------------------------------------------------------
// 基础值映射
// ---------------------------------------------------------------------------

/// `display` → taffy `Display`，含必要 blockify（见模块文档偏差）。
fn map_display(d: DisplayValue) -> Display {
    match d {
        DisplayValue::Block
        | DisplayValue::ListItem
        | DisplayValue::InlineBlock
        | DisplayValue::Table
        | DisplayValue::InlineTable
        | DisplayValue::TableInternal(_) => Display::Block,
        DisplayValue::FlowRoot => Display::FlowRoot,
        DisplayValue::Flex | DisplayValue::InlineFlex => Display::Flex,
        DisplayValue::Grid | DisplayValue::InlineGrid => Display::Grid,
        DisplayValue::None => Display::None,
        // 偏差兜底：见模块文档。
        DisplayValue::Inline | DisplayValue::Contents => Display::Block,
    }
}

/// `width` / `height` / `flex-basis`：auto / 长度 / 百分比 → taffy `Dimension`。
fn map_dimension(v: SizeValue) -> Dimension {
    match v {
        SizeValue::Auto => sh::auto(),
        SizeValue::Length(f) => sh::length(f),
        SizeValue::Percent(p) => sh::percent(p),
    }
}

/// `min-*` / `max-*`：auto / 长度 / 百分比 → taffy `LengthPercentageAuto`。
fn map_lp_auto(v: SizeValue) -> LengthPercentageAuto {
    match v {
        SizeValue::Auto => sh::auto(),
        SizeValue::Length(f) => sh::length(f),
        SizeValue::Percent(p) => sh::percent(p),
    }
}

fn margin_side(v: MarginValue) -> LengthPercentageAuto {
    match v {
        MarginValue::Length(f) => sh::length(f),
        MarginValue::Percent(p) => sh::percent(p),
        MarginValue::Auto => sh::auto(),
    }
}

fn padding_side(v: PaddingValue) -> LengthPercentage {
    match v {
        PaddingValue::Length(f) => sh::length(f),
        PaddingValue::Percent(p) => sh::percent(p),
    }
}

fn inset_side(v: InsetValue) -> LengthPercentageAuto {
    match v {
        InsetValue::Length(f) => sh::length(f),
        InsetValue::Percent(p) => sh::percent(p),
        InsetValue::Auto => sh::auto(),
    }
}

/// border 宽度（computed 阶段 style 为 none/hidden 已归零）按长度处理。
fn border_side(v: f32) -> LengthPercentage {
    sh::length(v)
}

fn gap_side(v: GapValue) -> LengthPercentage {
    match v {
        GapValue::Length(f) => sh::length(f),
        GapValue::Percent(p) => sh::percent(p),
        // `normal` 在 flex/grid 中等价于 0。
        GapValue::Normal => sh::length(0.0),
    }
}

// ---------------------------------------------------------------------------
// flex / grid 对齐映射
// ---------------------------------------------------------------------------

fn map_align_items(v: AlignItemsValue) -> AlignItems {
    match v {
        AlignItemsValue::Stretch => AlignItems::STRETCH,
        AlignItemsValue::FlexStart => AlignItems::FLEX_START,
        AlignItemsValue::FlexEnd => AlignItems::FLEX_END,
        AlignItemsValue::Center => AlignItems::CENTER,
        AlignItemsValue::Baseline => AlignItems::BASELINE,
    }
}

fn map_align_self(v: AlignSelfValue) -> Option<AlignSelf> {
    match v {
        AlignSelfValue::Auto => None,
        AlignSelfValue::Stretch => Some(AlignSelf::STRETCH),
        AlignSelfValue::FlexStart => Some(AlignSelf::FLEX_START),
        AlignSelfValue::FlexEnd => Some(AlignSelf::FLEX_END),
        AlignSelfValue::Center => Some(AlignSelf::CENTER),
        AlignSelfValue::Baseline => Some(AlignSelf::BASELINE),
    }
}

fn map_justify_content(v: JustifyContentValue) -> JustifyContent {
    match v {
        JustifyContentValue::FlexStart => JustifyContent::FLEX_START,
        JustifyContentValue::FlexEnd => JustifyContent::FLEX_END,
        JustifyContentValue::Center => JustifyContent::CENTER,
        JustifyContentValue::SpaceBetween => JustifyContent::SPACE_BETWEEN,
        JustifyContentValue::SpaceAround => JustifyContent::SPACE_AROUND,
        JustifyContentValue::SpaceEvenly => JustifyContent::SPACE_EVENLY,
    }
}

// ---------------------------------------------------------------------------
// grid 轨道映射
// ---------------------------------------------------------------------------

/// `GridTrackList` → taffy 轨道序列。`S` 由目标字段（`Style::<DefaultCheapStr>`）推断。
fn map_grid_tracks<S: CheapCloneStr>(list: &GridTrackList) -> Vec<GridTemplateComponent<S>> {
    list.tracks.iter().map(map_grid_track).collect()
}

fn map_grid_track<S: CheapCloneStr>(t: &GridTrack) -> GridTemplateComponent<S> {
    let tsf = match t {
        GridTrack::Length(f) => sh::length(*f),
        GridTrack::Percent(p) => sh::percent(*p),
        GridTrack::Fr(f) => sh::fr(*f),
        GridTrack::Auto => sh::auto(),
        GridTrack::MinMax(a, b) => sh::minmax(map_track_inner_min(a), map_track_inner_max(b)),
    };
    GridTemplateComponent::Single(tsf)
}

/// `minmax()` 的 min 段：长度 / 百分比 / auto；`fr` 非法降级为 auto。
fn map_track_inner_min(t: &GridTrack) -> MinTrackSizingFunction {
    match t {
        GridTrack::Length(f) => sh::length(*f),
        GridTrack::Percent(p) => sh::percent(*p),
        GridTrack::Auto => sh::auto(),
        GridTrack::Fr(_) => sh::auto(),
        GridTrack::MinMax(a, _) => map_track_inner_min(a),
    }
}

/// `minmax()` 的 max 段：长度 / 百分比 / auto / `fr`。
fn map_track_inner_max(t: &GridTrack) -> MaxTrackSizingFunction {
    match t {
        GridTrack::Length(f) => sh::length(*f),
        GridTrack::Percent(p) => sh::percent(*p),
        GridTrack::Auto => sh::auto(),
        GridTrack::Fr(f) => sh::fr(*f),
        GridTrack::MinMax(_, b) => map_track_inner_max(b),
    }
}

// ---------------------------------------------------------------------------
// 单元测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use nexty_css::{ComputedStyle, Edges, GridTrack as GTrack, SizeValue};
    use taffy::{
        Dimension, ExpandedMinTrackSizingFunction, LengthPercentage, LengthPercentageAuto,
    };

    #[test]
    fn maps_length_percent_auto_sizes() {
        let mut s = ComputedStyle::initial();
        s.width = SizeValue::Length(120.0);
        s.height = SizeValue::Percent(0.5);
        s.min_width = SizeValue::Auto;
        let t = map_style(&s);
        assert_eq!(t.size.width, Dimension::length(120.0));
        assert_eq!(t.size.height, Dimension::percent(0.5));
        assert_eq!(t.min_size.width, LengthPercentageAuto::auto());
    }

    #[test]
    fn maps_box_sizing_and_position() {
        let mut s = ComputedStyle::initial();
        s.box_sizing = BoxSizingValue::ContentBox;
        s.position = PositionValue::Absolute;
        let t = map_style(&s);
        assert_eq!(t.box_sizing, BoxSizing::ContentBox);
        assert_eq!(t.position, Position::Absolute);
    }

    #[test]
    fn maps_border_edges() {
        let mut s = ComputedStyle::initial();
        s.border_width = Edges::splat(2.0);
        let t = map_style(&s);
        assert_eq!(t.border.left, LengthPercentage::length(2.0));
        assert_eq!(t.border.top, LengthPercentage::length(2.0));
    }

    #[test]
    fn maps_flex_basis_and_direction() {
        let mut s = ComputedStyle::initial();
        s.display = DisplayValue::Flex;
        s.flex_basis = SizeValue::Length(50.0);
        s.flex_direction = FlexDirectionValue::Column;
        s.flex_wrap = FlexWrapValue::Wrap;
        s.flex_grow = 2.0;
        s.flex_shrink = 0.0;
        let t = map_style(&s);
        assert_eq!(t.display, Display::Flex);
        assert_eq!(t.flex_basis, Dimension::length(50.0));
        assert_eq!(t.flex_direction, FlexDirection::Column);
        assert_eq!(t.flex_wrap, FlexWrap::Wrap);
        assert_eq!(t.flex_grow, 2.0);
        assert_eq!(t.flex_shrink, 0.0);
    }

    #[test]
    fn maps_grid_template_and_gap() {
        let mut s = ComputedStyle::initial();
        s.display = DisplayValue::Grid;
        s.grid_template_columns = GridTrackList {
            tracks: vec![GTrack::Length(100.0), GTrack::Fr(1.0)],
        };
        s.grid_template_rows = GridTrackList {
            tracks: vec![GTrack::MinMax(
                Box::new(GTrack::Auto),
                Box::new(GTrack::Length(40.0)),
            )],
        };
        s.column_gap = GapValue::Length(8.0);
        s.row_gap = GapValue::Normal;
        let t = map_style(&s);
        assert_eq!(t.display, Display::Grid);
        assert_eq!(t.grid_template_columns.len(), 2);
        assert_eq!(t.grid_template_rows.len(), 1);

        // 第一列：100px 固定轨道（min sizing function 为 Length(100)）。
        let GridTemplateComponent::Single(tsf) = &t.grid_template_columns[0] else {
            panic!("expected Single track component");
        };
        assert!(matches!(
            tsf.min_sizing_function().expand(),
            ExpandedMinTrackSizingFunction::Length(v) if (v - 100.0).abs() < 1e-6
        ));

        // gap：column 8px，row（normal）0。
        assert_eq!(t.gap.width, LengthPercentage::length(8.0));
        assert_eq!(t.gap.height, LengthPercentage::length(0.0));
    }

    #[test]
    fn maps_absolute_inset() {
        let mut s = ComputedStyle::initial();
        s.position = PositionValue::Absolute;
        s.inset = Edges {
            top: InsetValue::Length(10.0),
            right: InsetValue::Auto,
            bottom: InsetValue::Percent(0.25),
            left: InsetValue::Length(5.0),
        };
        let t = map_style(&s);
        assert_eq!(t.inset.top, LengthPercentageAuto::length(10.0));
        assert_eq!(t.inset.right, LengthPercentageAuto::auto());
        assert_eq!(t.inset.bottom, LengthPercentageAuto::percent(0.25));
        assert_eq!(t.inset.left, LengthPercentageAuto::length(5.0));
    }
}
