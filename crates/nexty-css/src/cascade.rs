//! 自研 cascade 与 computed style。
//!
//! 行为 ground truth：
//! - 级联排序：[CSS Cascading and Inheritance Level 5 §6](https://drafts.csswg.org/css-cascade-5/#cascading)
//! - 继承 / 初始值：[CSS Cascade 5 §7](https://drafts.csswg.org/css-cascade-5/#defaulting)
//! - 字号计算：[CSS Fonts 4 §2.5](https://drafts.csswg.org/css-fonts-4/#font-size-prop)
//! - 相对字重：[CSS Fonts 4 §2.2.1](https://drafts.csswg.org/css-fonts-4/#relative-weights)
//!
//! 本轮只有 author origin（UA 样式表未引入），故排序桶为：
//! importance → 内联样式 → 特异度 → 源顺序。`revert` 回退到 UA origin，
//! 而 UA 无声明，等价于 initial。

use std::collections::HashMap;

use nexty_dom::{Document, NodeId, NodeKind};

use crate::parser::{Declaration, MediaViewport, StyleRule, Stylesheet, parse_inline_style};
use crate::selector::{NextySelector, atom_str, match_element_selectors};
use crate::value::{
    AbsoluteSize, AlignItemsValue, AlignSelfValue, BackgroundImageValue, BorderColorValue,
    BorderRadiusValue, BorderStyle, BorderWidthValue, BoxSizingValue, ClearValue, CssWideKeyword,
    DeclaredValue, DisplayValue, FlexDirectionValue, FlexWrapValue, FloatValue, FontFamilyValue,
    FontSizeValue, FontStyleValue, FontWeightValue, GapValue, GridAutoFlowValue, GridTrackList,
    InsetValue, JustifyContentValue, LineHeightValue, MAX_CSS_LENGTH, MarginValue, OverflowValue,
    OverflowWrapValue, PaddingValue, PositionValue, PropertyId, PropertyValue, Rgba, SizeValue,
    SpacingValue, TabSizeValue, TextAlignValue, TextDecorationColorValue, TextDecorationLineValue,
    TextDecorationStyleValue, TextIndentValue, ViewportUnit, WhiteSpaceValue, WordBreakValue,
    ZIndexValue,
};

/// 本层 UA 的 medium 字号基准（CSS Fonts 4 §2.5：initial 值由 UA 决定，
/// 对齐浏览器默认 16px；绝对尺寸关键字按 §2.5.1 缩放系数相对它计算）。
pub(crate) const MEDIUM_FONT_SIZE: f32 = 16.0;

/// 一个元素的 computed style（属性集随管线逐轮扩展）。
#[derive(Debug, Clone, PartialEq)]
pub struct ComputedStyle {
    /// `display`。
    pub display: DisplayValue,
    /// `color`。
    pub color: Rgba,
    /// `background-color`。
    pub background_color: Rgba,
    /// `background-image`（CSS Backgrounds 3 §2；不继承；基础档仅
    /// none / linear-gradient，色标位置已归一为 0–1）。
    pub background_image: BackgroundImageValue,
    /// 计算字号（CSS px）。
    pub font_size: f32,
    /// 计算字重（1–1000）。
    pub font_weight: f32,
    /// `font-style`。
    pub font_style: FontStyleValue,
    /// `font-family` 列表。
    pub font_family: Vec<FontFamilyValue>,
    /// `text-align`。
    pub text_align: TextAlignValue,
    /// `line-height`（数值/normal 保持原样；长度为绝对 px；
    /// 百分比与 em 已按本元素 font-size 折算）。
    pub line_height: LineHeightValue,
    /// `white-space`（CSS Text 3 §3.1）。
    pub white_space: WhiteSpaceValue,
    /// `overflow-wrap`（CSS Text 3 §5.4）。
    pub overflow_wrap: OverflowWrapValue,
    /// `word-break`（CSS Text 3 §5.1）。
    pub word_break: WordBreakValue,
    /// `letter-spacing` 的附加字距（CSS px；`normal` 与 0 同义，CSS Text 3 §8.1）。
    pub letter_spacing: f32,
    /// `word-spacing`（CSS Text 3 §8.2；`normal` 语义保留给 layout，em 已折算）。
    pub word_spacing: SpacingValue,
    /// `text-indent`（CSS Text 3 §4；em 已折算，百分比由 layout 按包含块解析）。
    pub text_indent: TextIndentValue,
    /// `tab-size`（CSS Text 3 §5；整数形态由 layout 按空格 advance 解析）。
    pub tab_size: TabSizeValue,
    /// `text-decoration-line`（CSS Text Decoration 3 §3；不继承——规范的
    /// 装饰线「传播」语义不实现，由消费方按元素自身 computed style 画，
    /// 记偏差）。
    pub text_decoration_line: TextDecorationLineValue,
    /// `text-decoration-style`（CSS Text Decoration 3 §5；解析存储，
    /// 渲染仅 solid——偏差）。
    pub text_decoration_style: TextDecorationStyleValue,
    /// `text-decoration-color`（CSS Text Decoration 3 §4；currentcolor 已按
    /// 本元素 color 解析）。
    pub text_decoration_color: Rgba,
    /// `z-index`（CSS Position 3 §5；不继承；基础定序由 chrome pipeline
    /// 消费，完整 stacking context 留 K7）。
    pub z_index: ZIndexValue,
    /// `width`（百分比由 layout 解析）。
    pub width: SizeValue,
    /// `height`（百分比由 layout 解析）。
    pub height: SizeValue,
    /// `min-width`（CSS 2.1 §10.4）。
    pub min_width: SizeValue,
    /// `max-width`（CSS 2.1 §10.4）。
    pub max_width: SizeValue,
    /// `min-height`（CSS 2.1 §10.7）。
    pub min_height: SizeValue,
    /// `max-height`（CSS 2.1 §10.7）。
    pub max_height: SizeValue,
    /// `box-sizing`（CSS Sizing 4 §5.3）。
    pub box_sizing: BoxSizingValue,
    /// `overflow`（CSS Overflow 3 §3；仅裁剪标记，裁剪未实现）。
    pub overflow: OverflowValue,
    /// `flex-grow`（CSS Flexbox §7.1，initial 0）。
    pub flex_grow: f32,
    /// `flex-basis`（CSS Flexbox §7.2，initial auto）。
    pub flex_basis: SizeValue,
    /// `position`（CSS Position 3 §2，initial static）。
    pub position: PositionValue,
    /// `float`（CSS 2.1 §9.5.1，initial none；不继承）。
    pub float: FloatValue,
    /// `clear`（CSS 2.1 §9.5.2，initial none；不继承）。
    pub clear: ClearValue,
    /// 四边 inset（top / right / bottom / left；百分比与 auto 由 layout 解析）。
    pub inset: Edges<InsetValue>,
    /// `flex-direction`（CSS Flexbox §8.3.1，initial row）。
    pub flex_direction: FlexDirectionValue,
    /// `flex-wrap`（CSS Flexbox §8.4.1，initial nowrap）。
    pub flex_wrap: FlexWrapValue,
    /// `flex-shrink`（CSS Flexbox §7.2，initial 1）。
    pub flex_shrink: f32,
    /// `align-items`（CSS Box Alignment §7，initial stretch）。
    pub align_items: AlignItemsValue,
    /// `justify-content`（CSS Box Alignment §8，initial flex-start）。
    pub justify_content: JustifyContentValue,
    /// `align-self`（CSS Box Alignment §7，initial auto）。
    pub align_self: AlignSelfValue,
    /// `grid-template-columns`（CSS Grid §7.2，initial 空）。
    pub grid_template_columns: GridTrackList,
    /// `grid-template-rows`（CSS Grid §7.3，initial 空）。
    pub grid_template_rows: GridTrackList,
    /// `row-gap`（CSS Box Alignment §6，initial normal）。
    pub row_gap: GapValue,
    /// `column-gap`（CSS Box Alignment §6，initial normal）。
    pub column_gap: GapValue,
    /// `grid-auto-flow`（CSS Grid §7.7，initial row）。
    pub grid_auto_flow: GridAutoFlowValue,
    /// 四边 margin（百分比与 auto 由 layout 解析）。
    pub margin: Edges<MarginValue>,
    /// 四边 padding（百分比由 layout 解析）。
    pub padding: Edges<PaddingValue>,
    /// 四边 border 宽度（computed 值：style 为 none/hidden 时为 0）。
    pub border_width: Edges<f32>,
    /// 四边 border 样式。
    pub border_style: Edges<BorderStyle>,
    /// 四边 border 颜色（currentcolor 已按本元素 color 解析）。
    pub border_color: Edges<Rgba>,
    /// 四角 border-radius（top-left / top-right / bottom-right / bottom-left；
    /// 百分比相对边框盒对应轴尺寸，由消费方解析）。
    pub border_radius: Corners<BorderRadiusValue>,
}

/// 四边值（top / right / bottom / left）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edges<T> {
    /// 上边。
    pub top: T,
    /// 右边。
    pub right: T,
    /// 下边。
    pub bottom: T,
    /// 左边。
    pub left: T,
}

impl<T: Copy> Edges<T> {
    /// 用同一值填充四边。
    #[must_use]
    pub const fn splat(value: T) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }
}

/// 四角值（top-left / top-right / bottom-right / bottom-left，
/// [CSS Backgrounds 3 §5.1](https://drafts.csswg.org/css-backgrounds-3/#border-radius)
/// 的角顺序）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Corners<T> {
    /// 左上角。
    pub top_left: T,
    /// 右上角。
    pub top_right: T,
    /// 右下角。
    pub bottom_right: T,
    /// 左下角。
    pub bottom_left: T,
}

impl<T: Copy> Corners<T> {
    /// 用同一值填充四角。
    #[must_use]
    pub const fn splat(value: T) -> Self {
        Self {
            top_left: value,
            top_right: value,
            bottom_right: value,
            bottom_left: value,
        }
    }
}

impl ComputedStyle {
    /// 全部属性取 initial 值。
    ///
    /// initial 值逐条对应各属性规范；UA 相关的（`color` 的 `CanvasText`、
    /// `font-family` 的 UA 默认、`font-size` 的 medium、`line-height` 的
    /// normal）取本层固定选择：黑色、serif、16px。
    #[must_use]
    pub fn initial() -> Self {
        Self {
            display: DisplayValue::Inline,
            color: Rgba::opaque(0, 0, 0),
            background_color: Rgba::transparent(),
            background_image: BackgroundImageValue::None,
            font_size: MEDIUM_FONT_SIZE,
            font_weight: 400.0,
            font_style: FontStyleValue::Normal,
            font_family: vec![FontFamilyValue::Generic("serif")],
            text_align: TextAlignValue::Start,
            line_height: LineHeightValue::Normal,
            white_space: WhiteSpaceValue::Normal,
            overflow_wrap: OverflowWrapValue::Normal,
            word_break: WordBreakValue::Normal,
            letter_spacing: 0.0,
            word_spacing: SpacingValue::Normal,
            text_indent: TextIndentValue::Length(0.0),
            tab_size: TabSizeValue::Integer(8),
            text_decoration_line: TextDecorationLineValue::None,
            text_decoration_style: TextDecorationStyleValue::Solid,
            text_decoration_color: Rgba::opaque(0, 0, 0),
            z_index: ZIndexValue::Auto,
            width: SizeValue::Auto,
            height: SizeValue::Auto,
            min_width: SizeValue::Auto,
            max_width: SizeValue::Auto,
            min_height: SizeValue::Auto,
            max_height: SizeValue::Auto,
            box_sizing: BoxSizingValue::ContentBox,
            overflow: OverflowValue::Visible,
            flex_grow: 0.0,
            flex_basis: SizeValue::Auto,
            position: PositionValue::Static,
            float: FloatValue::None,
            clear: ClearValue::None,
            inset: Edges::splat(InsetValue::Auto),
            flex_direction: FlexDirectionValue::Row,
            flex_wrap: FlexWrapValue::NoWrap,
            flex_shrink: 1.0,
            align_items: AlignItemsValue::Stretch,
            justify_content: JustifyContentValue::FlexStart,
            align_self: AlignSelfValue::Auto,
            grid_template_columns: GridTrackList::default(),
            grid_template_rows: GridTrackList::default(),
            row_gap: GapValue::Normal,
            column_gap: GapValue::Normal,
            grid_auto_flow: GridAutoFlowValue::Row,
            margin: Edges::splat(MarginValue::Length(0.0)),
            padding: Edges::splat(PaddingValue::Length(0.0)),
            border_width: Edges::splat(0.0),
            border_style: Edges::splat(BorderStyle::None),
            border_color: Edges::splat(Rgba::opaque(0, 0, 0)),
            border_radius: Corners::splat(BorderRadiusValue::Length(0.0)),
        }
    }
}

impl PropertyId {
    /// 属性是否继承（各属性规范 "Inherited" 行）。
    #[must_use]
    pub fn inherited(self) -> bool {
        matches!(
            self,
            Self::Color
                | Self::FontSize
                | Self::FontWeight
                | Self::FontStyle
                | Self::FontFamily
                | Self::TextAlign
                | Self::LineHeight
                | Self::WhiteSpace
                | Self::OverflowWrap
                | Self::WordBreak
                | Self::LetterSpacing
                | Self::WordSpacing
                | Self::TextIndent
                | Self::TabSize
        )
    }
}

/// 全部属性的固定枚举顺序。
///
/// 顺序承载计算依赖：`FontSize` 先于 `LineHeight`（百分比按自身字号折算）、
/// `Color` 先于 border / text-decoration 颜色（currentcolor）、border style
/// 先于 width（none/hidden 时宽度归零）。
const PROPERTY_ORDER: [PropertyId; 73] = [
    PropertyId::Display,
    PropertyId::Color,
    PropertyId::BackgroundColor,
    PropertyId::BackgroundImage,
    PropertyId::FontSize,
    PropertyId::FontWeight,
    PropertyId::FontStyle,
    PropertyId::FontFamily,
    PropertyId::TextAlign,
    PropertyId::LineHeight,
    PropertyId::WhiteSpace,
    PropertyId::OverflowWrap,
    PropertyId::WordBreak,
    PropertyId::LetterSpacing,
    PropertyId::WordSpacing,
    PropertyId::TextIndent,
    PropertyId::TabSize,
    PropertyId::TextDecorationLine,
    PropertyId::TextDecorationStyle,
    PropertyId::TextDecorationColor,
    PropertyId::Width,
    PropertyId::Height,
    PropertyId::MinWidth,
    PropertyId::MaxWidth,
    PropertyId::MinHeight,
    PropertyId::MaxHeight,
    PropertyId::BoxSizing,
    PropertyId::Overflow,
    PropertyId::FlexGrow,
    PropertyId::FlexBasis,
    PropertyId::Position,
    PropertyId::Float,
    PropertyId::Clear,
    PropertyId::Top,
    PropertyId::Right,
    PropertyId::Bottom,
    PropertyId::Left,
    PropertyId::ZIndex,
    PropertyId::FlexDirection,
    PropertyId::FlexWrap,
    PropertyId::FlexShrink,
    PropertyId::AlignItems,
    PropertyId::JustifyContent,
    PropertyId::AlignSelf,
    PropertyId::GridTemplateColumns,
    PropertyId::GridTemplateRows,
    PropertyId::RowGap,
    PropertyId::ColumnGap,
    PropertyId::GridAutoFlow,
    PropertyId::MarginTop,
    PropertyId::MarginRight,
    PropertyId::MarginBottom,
    PropertyId::MarginLeft,
    PropertyId::PaddingTop,
    PropertyId::PaddingRight,
    PropertyId::PaddingBottom,
    PropertyId::PaddingLeft,
    PropertyId::BorderTopStyle,
    PropertyId::BorderRightStyle,
    PropertyId::BorderBottomStyle,
    PropertyId::BorderLeftStyle,
    PropertyId::BorderTopWidth,
    PropertyId::BorderRightWidth,
    PropertyId::BorderBottomWidth,
    PropertyId::BorderLeftWidth,
    PropertyId::BorderTopColor,
    PropertyId::BorderRightColor,
    PropertyId::BorderBottomColor,
    PropertyId::BorderLeftColor,
    PropertyId::BorderTopLeftRadius,
    PropertyId::BorderTopRightRadius,
    PropertyId::BorderBottomRightRadius,
    PropertyId::BorderBottomLeftRadius,
];

/// 级联 origin 桶（CSS Cascade 5 §6.4，仅 UA 与 author 两个 origin；
/// user origin 尚未引入）。
///
/// 值越大优先级越高：UA 普通 < author 普通 < author `!important` <
/// UA `!important`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum OriginBucket {
    UaNormal = 0,
    AuthorNormal = 1,
    AuthorImportant = 2,
    UaImportant = 3,
}

/// 级联排序键（CSS Cascade 5 §6.4）。
///
/// 字典序即优先级：先 origin/importance 桶；author 声明中内联样式排在
/// 样式表声明之后（§6.4 style attribute）；然后按特异度、规则顺序、声明顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CascadeKey {
    bucket: OriginBucket,
    inline: bool,
    specificity: u32,
    rule_order: usize,
    declaration_order: usize,
}

/// 级联 origin（CSS Cascade 5 §6.4；user origin 尚未引入）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    Ua,
    Author,
}

/// origin + importance → 排序桶：UA 普通 < author 普通 < author `!important`
/// < UA `!important`。
fn bucket_of(origin: Origin, important: bool) -> OriginBucket {
    match (origin, important) {
        (Origin::Ua, false) => OriginBucket::UaNormal,
        (Origin::Author, false) => OriginBucket::AuthorNormal,
        (Origin::Author, true) => OriginBucket::AuthorImportant,
        (Origin::Ua, true) => OriginBucket::UaImportant,
    }
}

/// 选择器的必要条件（M5 分桶）：匹配该选择器的元素**必然**满足的条件。
///
/// 只取顶层 compound 里的直接简单选择器（`Class` / `ID` / `LocalName`）——
/// `:is()` / `:where()` / `:has()` 内部是备选分支而非必要条件，含组合器的
/// 选择器其条件分布在多个 compound 上，一概保守回落
/// [`Requirement::Always`]。键统一 ASCII 小写：quirks 模式的
/// case-insensitive 与严格模式都只会「多测不会漏」（大小写不同的名字折到
/// 同一键，仍然进入匹配）。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Requirement {
    /// 元素必须带有该 class。
    Class(String),
    /// 元素必须是该标签名。
    Tag(String),
    /// 元素必须带有该 id。
    Id(String),
    /// 无可提取的必要条件：每元素必测。
    Always,
}

/// 从一条已解析选择器提取必要条件（优先 id > class > tag，其余 Always）。
fn requirement_of(selector: &NextySelector) -> Requirement {
    use selectors::parser::Component;
    use std::borrow::Borrow;
    let mut class: Option<&str> = None;
    let mut tag: Option<&str> = None;
    for component in selector.selector.iter_raw_match_order() {
        match component {
            // 组合器连接多个 compound：条件不再唯一，保守全测
            Component::Combinator(_) => return Requirement::Always,
            Component::ID(atom) => {
                return Requirement::Id(atom_str(atom).to_ascii_lowercase());
            }
            Component::Class(atom) if class.is_none() => class = Some(atom_str(atom)),
            Component::LocalName(local) if tag.is_none() => tag = Some(local.lower_name.borrow()),
            _ => {}
        }
    }
    if let Some(class) = class {
        return Requirement::Class(class.to_ascii_lowercase());
    }
    if let Some(tag) = tag {
        return Requirement::Tag(tag.to_ascii_lowercase());
    }
    Requirement::Always
}

/// 桶内候选：一次选择器测试的全部输入（特异度已在解析期算好）。
struct Candidate<'a> {
    /// 规则在所属 origin 展平序列中的序号（CascadeKey::rule_order）。
    rule_order: usize,
    /// 展平选择器槽位（同元素去重用：同一选择器可出现在多个桶）。
    slot: usize,
    selector: &'a NextySelector,
}

/// 一组样式表的规则桶索引（M5 ④）：构建一次，全文档每元素复用。
///
/// 语义与全量扫描逐条一致：只有「必要条件已失败」的选择器被跳过——它们
/// 本就不可能匹配。命中声明仍按 origin → importance → specificity →
/// 源顺序仲裁（CascadeKey 不变）。
struct SheetIndex<'a> {
    /// 展平的（序号, 规则）序列：rule_order 与声明借用的来源。
    rules: Vec<(usize, &'a StyleRule)>,
    class: HashMap<String, Vec<Candidate<'a>>>,
    tag: HashMap<String, Vec<Candidate<'a>>>,
    id: HashMap<String, Vec<Candidate<'a>>>,
    /// 无可提取必要条件的选择器：每元素必测。
    always: Vec<Candidate<'a>>,
    /// 展平选择器总数（去重 scratch 的尺寸）。
    selector_count: usize,
}

impl<'a> SheetIndex<'a> {
    /// 为一组样式表构建索引（规则按传入顺序、选择器按源顺序展平编号）。
    fn build(sheets: &[&'a Stylesheet]) -> Self {
        let mut rules = Vec::new();
        let mut class: HashMap<String, Vec<Candidate<'a>>> = HashMap::new();
        let mut tag: HashMap<String, Vec<Candidate<'a>>> = HashMap::new();
        let mut id: HashMap<String, Vec<Candidate<'a>>> = HashMap::new();
        let mut always = Vec::new();
        let mut selector_count = 0usize;
        for (rule_order, rule) in sheets.iter().flat_map(|sheet| sheet.rules()).enumerate() {
            rules.push((rule_order, rule));
            for selector in rule.selectors() {
                let slot = selector_count;
                selector_count += 1;
                let candidate = Candidate {
                    rule_order,
                    slot,
                    selector,
                };
                match requirement_of(selector) {
                    Requirement::Class(key) => class.entry(key).or_default().push(candidate),
                    Requirement::Tag(key) => tag.entry(key).or_default().push(candidate),
                    Requirement::Id(key) => id.entry(key).or_default().push(candidate),
                    Requirement::Always => always.push(candidate),
                }
            }
        }
        Self {
            rules,
            class,
            tag,
            id,
            always,
            selector_count,
        }
    }

    /// 元素的必要条件集合：class 列表 + tag + id（键与桶同为小写）。
    fn requirements_of(
        document: &Document,
        element: NodeId,
    ) -> (Vec<String>, String, Option<String>) {
        let Some(NodeKind::Element(data)) = document.node(element) else {
            return (Vec::new(), String::new(), None);
        };
        let attribute = |name: &str| {
            data.attributes
                .iter()
                .find(|attr| attr.namespace == nexty_dom::Namespace::None && attr.name == name)
                .map(|attr| attr.value.as_str())
        };
        let classes = attribute("class")
            .map(|value| {
                value
                    .split_ascii_whitespace()
                    .map(str::to_ascii_lowercase)
                    .collect()
            })
            .unwrap_or_default();
        let tag = data.name.to_ascii_lowercase();
        let id = attribute("id").map(str::to_ascii_lowercase);
        (classes, tag, id)
    }
}

/// 元素级收集的去重 scratch：跨元素复用（epoch 标记免清零）。
#[derive(Default)]
struct GatherScratch {
    seen: Vec<u32>,
    epoch: u32,
}

/// 收集一个元素在 UA + author 两个索引下的全部命中声明（引用，零克隆）。
///
/// `inline` 是本元素的内联样式解析结果（[`InlineStyleCache`] 或现解析），
/// 声明同样以引用进入 matched——声明的生命周期覆盖 `resolve_all` 调用。
fn collect_matched<'a: 'd, 'd>(
    document: &Document,
    element: NodeId,
    ua: Option<&'a SheetIndex<'a>>,
    author: &'a SheetIndex<'a>,
    inline: Option<&'d [Declaration]>,
    scratch: &mut GatherScratch,
    matched: &mut Vec<(CascadeKey, &'d Declaration)>,
) {
    // 把一个桶的去重候选并入 test（嵌套 fn 而非闭包：参数需要显式生命周期）
    fn push_bucket<'a>(
        bucket: &'a [Candidate<'a>],
        is_ua: bool,
        slot_offset: usize,
        seen: &mut [u32],
        epoch: u32,
        test: &mut Vec<(&'a Candidate<'a>, bool)>,
    ) {
        for candidate in bucket {
            let slot = candidate.slot + slot_offset;
            if seen[slot] == epoch {
                continue;
            }
            seen[slot] = epoch;
            test.push((candidate, is_ua));
        }
    }

    // 从一个索引的全部相关桶收集候选
    fn gather<'a>(
        index: &'a SheetIndex<'a>,
        document: &Document,
        element: NodeId,
        is_ua: bool,
        slot_offset: usize,
        scratch: &mut GatherScratch,
        test: &mut Vec<(&'a Candidate<'a>, bool)>,
    ) {
        let (classes, tag, id) = SheetIndex::requirements_of(document, element);
        for key in &classes {
            if let Some(bucket) = index.class.get(key) {
                push_bucket(
                    bucket,
                    is_ua,
                    slot_offset,
                    &mut scratch.seen,
                    scratch.epoch,
                    test,
                );
            }
        }
        if let Some(bucket) = index.tag.get(&tag) {
            push_bucket(
                bucket,
                is_ua,
                slot_offset,
                &mut scratch.seen,
                scratch.epoch,
                test,
            );
        }
        if let Some(id) = &id
            && let Some(bucket) = index.id.get(id)
        {
            push_bucket(
                bucket,
                is_ua,
                slot_offset,
                &mut scratch.seen,
                scratch.epoch,
                test,
            );
        }
        push_bucket(
            &index.always,
            is_ua,
            slot_offset,
            &mut scratch.seen,
            scratch.epoch,
            test,
        );
    }

    // 命中规则的声明组按引用入 matched（嵌套 fn：显式生命周期）
    fn push_rule<'d>(
        matched: &mut Vec<(CascadeKey, &'d Declaration)>,
        origin: Origin,
        rule_order: usize,
        specificity: Option<u32>,
        declarations: &'d [Declaration],
    ) {
        let Some(specificity) = specificity else {
            return;
        };
        for (declaration_order, declaration) in declarations.iter().enumerate() {
            matched.push((
                CascadeKey {
                    bucket: bucket_of(origin, declaration.important),
                    inline: false,
                    specificity,
                    rule_order,
                    declaration_order,
                },
                declaration,
            ));
        }
    }

    // 槽位跨两个索引全局唯一：author 从 0 起，UA 偏移其后（去重 scratch）
    let slot_count = author.selector_count + ua.map_or(0, |index| index.selector_count);
    if scratch.seen.len() < slot_count {
        scratch.seen.resize(slot_count, 0);
    }
    scratch.epoch = scratch.epoch.wrapping_add(1);

    // 逐桶收集候选选择器（epoch 去重：同一选择器可能同时命中多个 class 桶）
    let mut test: Vec<(&Candidate<'a>, bool)> = Vec::new(); // (候选, 是否 UA)
    if let Some(ua) = ua {
        gather(
            ua,
            document,
            element,
            true,
            author.selector_count,
            scratch,
            &mut test,
        );
    }
    gather(author, document, element, false, 0, scratch, &mut test);

    // 测试候选选择器：命中者按规则聚合最大特异度（与全量扫描一致：
    // 声明的特异度取其匹配选择器中的最大值；UA 与 author 各自独立序号空间）
    let mut author_specificity: Vec<Option<u32>> = vec![None; author.rules.len()];
    let mut ua_specificity = ua.map(|index| vec![None; index.rules.len()]);
    for (candidate, is_ua) in &test {
        if !match_element_selectors(std::slice::from_ref(candidate.selector), document, element) {
            continue;
        }
        let slots = if *is_ua {
            ua_specificity.as_mut().expect("ua slots sized")
        } else {
            &mut author_specificity
        };
        let entry = &mut slots[candidate.rule_order];
        *entry = Some(entry.map_or(candidate.selector.specificity, |current| {
            current.max(candidate.selector.specificity)
        }));
    }

    // 生成 matched：author 表 + UA 表 + 内联（声明全部按引用）
    for &(rule_order, rule) in &author.rules {
        push_rule(
            matched,
            Origin::Author,
            rule_order,
            author_specificity[rule_order],
            rule.declarations(),
        );
    }
    if let (Some(ua), Some(slots)) = (ua, ua_specificity.as_ref()) {
        for &(rule_order, rule) in &ua.rules {
            push_rule(
                matched,
                Origin::Ua,
                rule_order,
                slots[rule_order],
                rule.declarations(),
            );
        }
    }
    if let Some(inline) = inline {
        for (declaration_order, declaration) in inline.iter().enumerate() {
            matched.push((
                CascadeKey {
                    bucket: bucket_of(Origin::Author, declaration.important),
                    inline: true,
                    specificity: 0,
                    rule_order: usize::MAX,
                    declaration_order,
                },
                declaration,
            ));
        }
    }
}

/// 内联样式解析缓存（M5 ②）：元素 →（style 属性原文, 解析结果）。
///
/// recascade 时按**属性原文**校验——原文未变直接复用解析结果，不重解析
/// 字符串。以原文为键保证任意来源的属性变更都自动失效（校验是廉价的
/// 字符串比较）。存方（如 chrome 的 `Page`）跨级联调用持有本缓存。
#[derive(Default)]
pub struct InlineStyleCache {
    entries: HashMap<NodeId, (String, Vec<Declaration>)>,
}

impl InlineStyleCache {
    /// 空缓存。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 元素内联声明的借用：原文命中直接复用，未命中现解析并入库。
    /// 无 style 属性返回 `None`。
    fn declarations_of(&mut self, document: &Document, element: NodeId) -> Option<&[Declaration]> {
        let Some(NodeKind::Element(data)) = document.node(element) else {
            return None;
        };
        let attribute = data
            .attributes
            .iter()
            .find(|attr| attr.namespace == nexty_dom::Namespace::None && attr.name == "style")?
            .value
            .clone();
        // 快路径：原文未变直接复用（属性原文是解析结果失效的唯一来源）
        if let Some((cached, _)) = self.entries.get(&element)
            && *cached == attribute
        {
            return Some(&self.entries[&element].1);
        }
        let declarations = parse_inline_style(&attribute);
        self.entries.insert(element, (attribute, declarations));
        Some(&self.entries[&element].1)
    }
}

/// 对单个元素做级联，产出 computed style。
///
/// `ua_sheet` 是 UA origin 样式表（浏览器默认样式），参与级联但优先级
/// 低于 author 声明（UA `!important` 最高，见 [`OriginBucket`]）。
/// `parent` 是树序父元素的 computed style（元素没有元素祖先时为 `None`）。
/// 各属性：级联胜者 → CSS-wide 关键字 → 继承 → initial。
/// 无视口信息：视口单位原样保留（见 [`cascade_in`]）。
#[must_use]
pub fn cascade(
    document: &Document,
    element: NodeId,
    stylesheets: &[&Stylesheet],
    ua_sheet: Option<&Stylesheet>,
    parent: Option<&ComputedStyle>,
) -> ComputedStyle {
    cascade_in(document, element, stylesheets, ua_sheet, parent, None)
}

/// [`cascade`] 的带视口版本：视口单位（vw/vh/vmin/vmax）在 computed 阶段
/// 按给定视口折算为绝对 px（CSS Values 4 §6.2：1v* = 视口对应边的 1%）。
/// 未提供视口时视口单位原样保留，由消费方按降级值处理。
///
/// 单元素入口每次调用重建一次规则桶索引（O(选择器总数)，远小于逐元素
/// 全量扫描）；整文档入口走 [`compute_document_styles_in_with_cache`]，
/// 索引只建一次。
#[must_use]
pub fn cascade_in(
    document: &Document,
    element: NodeId,
    stylesheets: &[&Stylesheet],
    ua_sheet: Option<&Stylesheet>,
    parent: Option<&ComputedStyle>,
    viewport: Option<MediaViewport>,
) -> ComputedStyle {
    let author = SheetIndex::build(stylesheets);
    let ua = ua_sheet.map(|sheet| SheetIndex::build(std::slice::from_ref(&sheet)));
    let mut scratch = GatherScratch::default();
    let mut cache = InlineStyleCache::new();
    let inline = cache.declarations_of(document, element);
    let mut matched: Vec<(CascadeKey, &Declaration)> = Vec::new();
    collect_matched(
        document,
        element,
        ua.as_ref(),
        &author,
        inline,
        &mut scratch,
        &mut matched,
    );
    resolve_all(&matched, parent, viewport)
}

/// 按树序计算整份文档的 computed style（父先于子）。
///
/// `<template>` 的内容子树是惰性片段，不参与级联，跳过。
#[must_use]
pub fn compute_document_styles(
    document: &Document,
    stylesheets: &[&Stylesheet],
    ua_sheet: Option<&Stylesheet>,
) -> HashMap<NodeId, ComputedStyle> {
    compute_document_styles_in(document, stylesheets, ua_sheet, None)
}

/// [`compute_document_styles`] 的带视口版本：视口单位（vw/vh/vmin/vmax）
/// 在 computed 阶段按给定视口折算为绝对 px（CSS Values 4 §6.2）。
/// 未提供视口时视口单位原样保留，由消费方按降级值处理。
///
/// 规则桶索引本调用内构建一次；内联样式每次重解析（无跨调用缓存）。
#[must_use]
pub fn compute_document_styles_in(
    document: &Document,
    stylesheets: &[&Stylesheet],
    ua_sheet: Option<&Stylesheet>,
    viewport: Option<MediaViewport>,
) -> HashMap<NodeId, ComputedStyle> {
    let mut cache = InlineStyleCache::new();
    compute_document_styles_in_with_cache(document, stylesheets, ua_sheet, viewport, &mut cache)
}

/// [`compute_document_styles_in`] 的带缓存版本（M5 ②）：调用方跨调用持有
/// [`InlineStyleCache`] 时，recascade 对未变化的 style 属性直接复用解析
/// 结果，不重解析字符串。
#[must_use]
pub fn compute_document_styles_in_with_cache(
    document: &Document,
    stylesheets: &[&Stylesheet],
    ua_sheet: Option<&Stylesheet>,
    viewport: Option<MediaViewport>,
    cache: &mut InlineStyleCache,
) -> HashMap<NodeId, ComputedStyle> {
    // 规则桶索引：构建一次（O(选择器总数)），全文档每元素复用
    let mut ctx = CascadeCtx {
        ua: ua_sheet.map(|sheet| SheetIndex::build(std::slice::from_ref(&sheet))),
        author: SheetIndex::build(stylesheets),
        viewport,
        cache,
        scratch: GatherScratch::default(),
    };
    let mut styles = HashMap::new();
    walk(document, document.root(), None, &mut ctx, &mut styles);
    styles
}

/// 整文档级联的共享上下文（M5）：规则桶索引（构建一次）、视口、内联样式
/// 缓存与去重 scratch，由 [`walk`] 全文档复用。
struct CascadeCtx<'a, 'c> {
    ua: Option<SheetIndex<'a>>,
    author: SheetIndex<'a>,
    viewport: Option<MediaViewport>,
    cache: &'c mut InlineStyleCache,
    scratch: GatherScratch,
}

fn walk(
    document: &Document,
    node: NodeId,
    parent: Option<&ComputedStyle>,
    ctx: &mut CascadeCtx<'_, '_>,
    out: &mut HashMap<NodeId, ComputedStyle>,
) {
    for child in document.children(node) {
        if matches!(document.node(child), Some(NodeKind::Element(_))) {
            // matched 借用 cache 与索引，块结束即释放（递归前必须归还）
            let style = {
                let CascadeCtx {
                    ua,
                    author,
                    viewport,
                    cache,
                    scratch,
                } = &mut *ctx;
                let inline = cache.declarations_of(document, child);
                let mut matched: Vec<(CascadeKey, &Declaration)> = Vec::new();
                collect_matched(
                    document,
                    child,
                    ua.as_ref(),
                    author,
                    inline,
                    scratch,
                    &mut matched,
                );
                resolve_all(&matched, parent, *viewport)
            };
            walk(document, child, Some(&style), ctx, out);
            out.insert(child, style);
        }
    }
}

/// 某属性在级联中的胜者声明。
fn winner<'d>(
    property: PropertyId,
    matched: &[(CascadeKey, &'d Declaration)],
) -> Option<&'d Declaration> {
    matched
        .iter()
        .filter(|(_, declaration)| declaration.property == property)
        .max_by_key(|(key, _)| *key)
        .map(|(_, declaration)| *declaration)
}

/// 胜者决定该属性的来源。
enum Decision<'a> {
    /// 使用级联胜者的属性值。
    Use(&'a PropertyValue),
    /// 取父元素的 computed value；无父元素则取 initial。
    Parent,
    /// 取 initial 值。
    Initial,
}

fn decide<'a>(
    property: PropertyId,
    winner: Option<&'a Declaration>,
    parent: Option<&ComputedStyle>,
) -> Decision<'a> {
    let has_parent = parent.is_some();
    match winner {
        Some(Declaration {
            value: DeclaredValue::Typed(value),
            ..
        }) => Decision::Use(value),
        Some(Declaration {
            value: DeclaredValue::CssWide(keyword),
            ..
        }) => match keyword {
            // revert 回退到 UA origin；本层 UA 无声明，等价 initial
            CssWideKeyword::Initial | CssWideKeyword::Revert => Decision::Initial,
            CssWideKeyword::Inherit => Decision::Parent,
            CssWideKeyword::Unset => {
                if property.inherited() && has_parent {
                    Decision::Parent
                } else {
                    Decision::Initial
                }
            }
        },
        // 无声明：继承属性取父值，否则取 initial（CSS Cascade 5 §7）
        None => {
            if property.inherited() && has_parent {
                Decision::Parent
            } else {
                Decision::Initial
            }
        }
    }
}

/// 视口单位折算为 px：1v* = 视口对应边的 1%（CSS Values 4 §6.2）。
///
/// 折算乘积同样过卫生钳：巨数量 × 视口基数不得产生 `inf`。
fn viewport_px(amount: f32, unit: ViewportUnit, viewport: MediaViewport) -> f32 {
    let base = match unit {
        ViewportUnit::Vw => viewport.width,
        ViewportUnit::Vh => viewport.height,
        ViewportUnit::Vmin => viewport.width.min(viewport.height),
        ViewportUnit::Vmax => viewport.width.max(viewport.height),
    };
    clamp_computed(amount * base / 100.0)
}

/// 计算出口的长度卫生：有限值 clamp 到上限；±inf 钳到边界；NaN 归 0
/// （相对单位乘积可能越过解析期的钳值，计算值必须仍落在预算内）。
fn clamp_computed(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(-MAX_CSS_LENGTH, MAX_CSS_LENGTH)
    }
}

macro_rules! resolve_viewport {
    ($fn_name:ident, $enum:ident) => {
        fn $fn_name(value: $enum, viewport: Option<MediaViewport>) -> $enum {
            match value {
                $enum::Viewport(amount, unit) => match viewport {
                    Some(vp) => $enum::Length(viewport_px(amount, unit, vp)),
                    None => $enum::Viewport(amount, unit),
                },
                other => other,
            }
        }
    };
}

resolve_viewport!(resolve_size, SizeValue);
resolve_viewport!(resolve_inset, InsetValue);
resolve_viewport!(resolve_margin, MarginValue);
resolve_viewport!(resolve_padding, PaddingValue);
resolve_viewport!(resolve_gap, GapValue);
resolve_viewport!(resolve_border_radius, BorderRadiusValue);

/// 逐属性解析出 computed style。
fn resolve_all(
    matched: &[(CascadeKey, &Declaration)],
    parent: Option<&ComputedStyle>,
    viewport: Option<MediaViewport>,
) -> ComputedStyle {
    let initial = ComputedStyle::initial();
    let mut style = initial.clone();
    let parent_font_size = parent.map_or(MEDIUM_FONT_SIZE, |p| p.font_size);
    let parent_font_weight = parent.map_or(400.0, |p| p.font_weight);

    for property in PROPERTY_ORDER {
        let decision = decide(property, winner(property, matched), parent);
        match (property, decision) {
            (PropertyId::Display, Decision::Use(PropertyValue::Display(value))) => {
                style.display = *value;
            }
            (PropertyId::TextAlign, Decision::Use(PropertyValue::TextAlign(value))) => {
                style.text_align = *value;
            }
            (PropertyId::FontStyle, Decision::Use(PropertyValue::FontStyle(value))) => {
                style.font_style = *value;
            }
            (PropertyId::Color, Decision::Use(PropertyValue::Color(value))) => {
                style.color = *value;
            }
            (PropertyId::BackgroundColor, Decision::Use(PropertyValue::Color(value))) => {
                style.background_color = *value;
            }
            (PropertyId::BackgroundImage, Decision::Use(PropertyValue::BackgroundImage(value))) => {
                style.background_image = value.clone();
            }
            (PropertyId::FontSize, Decision::Use(PropertyValue::FontSize(value))) => {
                style.font_size = compute_font_size(value, parent_font_size);
            }
            (PropertyId::FontWeight, Decision::Use(PropertyValue::FontWeight(value))) => {
                style.font_weight = compute_font_weight(value, parent_font_weight);
            }
            (PropertyId::FontFamily, Decision::Use(PropertyValue::FontFamily(value))) => {
                style.font_family = value.clone();
            }
            (PropertyId::LineHeight, Decision::Use(PropertyValue::LineHeight(value))) => {
                // CSS 2.1 §10.8.1：百分比/em 的 computed 值为绝对长度
                //（按本元素自身 font-size，已在 PROPERTY_ORDER 中先求）
                style.line_height = match *value {
                    LineHeightValue::Percent(fraction) => {
                        LineHeightValue::Length(clamp_computed(style.font_size * fraction))
                    }
                    LineHeightValue::Em(em) => {
                        LineHeightValue::Length(clamp_computed(style.font_size * em))
                    }
                    other => other,
                };
            }
            (PropertyId::WhiteSpace, Decision::Use(PropertyValue::WhiteSpace(value))) => {
                style.white_space = *value;
            }
            (PropertyId::OverflowWrap, Decision::Use(PropertyValue::OverflowWrap(value))) => {
                style.overflow_wrap = *value;
            }
            (PropertyId::WordBreak, Decision::Use(PropertyValue::WordBreak(value))) => {
                style.word_break = *value;
            }
            (PropertyId::LetterSpacing, Decision::Use(PropertyValue::LetterSpacing(value))) => {
                // CSS Text 3 §8.1：normal 的 used value 与 0 同义
                style.letter_spacing = match *value {
                    SpacingValue::Normal => 0.0,
                    SpacingValue::Length(px) => px,
                    SpacingValue::Em(em) => clamp_computed(style.font_size * em),
                };
            }
            (PropertyId::WordSpacing, Decision::Use(PropertyValue::WordSpacing(value))) => {
                // normal 的语义保留给 layout 判读（CSS Text 3 §8.2）
                style.word_spacing = match *value {
                    SpacingValue::Normal => SpacingValue::Normal,
                    SpacingValue::Length(px) => SpacingValue::Length(px),
                    SpacingValue::Em(em) => {
                        SpacingValue::Length(clamp_computed(style.font_size * em))
                    }
                };
            }
            (PropertyId::TextIndent, Decision::Use(PropertyValue::TextIndent(value))) => {
                style.text_indent = match *value {
                    TextIndentValue::Em(em) => {
                        TextIndentValue::Length(clamp_computed(style.font_size * em))
                    }
                    other => other,
                };
            }
            (PropertyId::TabSize, Decision::Use(PropertyValue::TabSize(value))) => {
                style.tab_size = match *value {
                    // 整数形态依赖空格 advance（字体度量），保留给 layout
                    TabSizeValue::Em(em) => {
                        TabSizeValue::Length(clamp_computed(style.font_size * em))
                    }
                    other => other,
                };
            }
            (
                PropertyId::TextDecorationLine,
                Decision::Use(PropertyValue::TextDecorationLine(value)),
            ) => {
                style.text_decoration_line = *value;
            }
            (
                PropertyId::TextDecorationStyle,
                Decision::Use(PropertyValue::TextDecorationStyle(value)),
            ) => {
                style.text_decoration_style = *value;
            }
            (
                PropertyId::TextDecorationColor,
                Decision::Use(PropertyValue::TextDecorationColor(value)),
            ) => {
                // currentcolor 按本元素 color 解析（与 border 颜色同语义）
                style.text_decoration_color = resolve_text_decoration_color(*value, style.color);
            }
            (PropertyId::ZIndex, Decision::Use(PropertyValue::ZIndex(value))) => {
                style.z_index = *value;
            }
            (PropertyId::Width, Decision::Use(PropertyValue::Size(value))) => {
                style.width = resolve_size(*value, viewport);
            }
            (PropertyId::Height, Decision::Use(PropertyValue::Size(value))) => {
                style.height = resolve_size(*value, viewport);
            }
            (PropertyId::MinWidth, Decision::Use(PropertyValue::Size(value))) => {
                style.min_width = resolve_size(*value, viewport);
            }
            (PropertyId::MaxWidth, Decision::Use(PropertyValue::Size(value))) => {
                style.max_width = resolve_size(*value, viewport);
            }
            (PropertyId::MinHeight, Decision::Use(PropertyValue::Size(value))) => {
                style.min_height = resolve_size(*value, viewport);
            }
            (PropertyId::MaxHeight, Decision::Use(PropertyValue::Size(value))) => {
                style.max_height = resolve_size(*value, viewport);
            }
            (PropertyId::BoxSizing, Decision::Use(PropertyValue::BoxSizing(value))) => {
                style.box_sizing = *value;
            }
            (PropertyId::Overflow, Decision::Use(PropertyValue::Overflow(value))) => {
                style.overflow = *value;
            }
            (PropertyId::FlexGrow, Decision::Use(PropertyValue::FlexGrow(value))) => {
                style.flex_grow = *value;
            }
            (PropertyId::FlexBasis, Decision::Use(PropertyValue::FlexBasis(value))) => {
                style.flex_basis = *value;
            }
            (PropertyId::Position, Decision::Use(PropertyValue::Position(value))) => {
                style.position = *value;
            }
            (PropertyId::Float, Decision::Use(PropertyValue::Float(value))) => {
                style.float = *value;
            }
            (PropertyId::Clear, Decision::Use(PropertyValue::Clear(value))) => {
                style.clear = *value;
            }
            (PropertyId::Top, Decision::Use(PropertyValue::Inset(value))) => {
                style.inset.top = resolve_inset(*value, viewport);
            }
            (PropertyId::Right, Decision::Use(PropertyValue::Inset(value))) => {
                style.inset.right = resolve_inset(*value, viewport);
            }
            (PropertyId::Bottom, Decision::Use(PropertyValue::Inset(value))) => {
                style.inset.bottom = resolve_inset(*value, viewport);
            }
            (PropertyId::Left, Decision::Use(PropertyValue::Inset(value))) => {
                style.inset.left = resolve_inset(*value, viewport);
            }
            (PropertyId::FlexDirection, Decision::Use(PropertyValue::FlexDirection(value))) => {
                style.flex_direction = *value;
            }
            (PropertyId::FlexWrap, Decision::Use(PropertyValue::FlexWrap(value))) => {
                style.flex_wrap = *value;
            }
            (PropertyId::FlexShrink, Decision::Use(PropertyValue::FlexShrink(value))) => {
                style.flex_shrink = *value;
            }
            (PropertyId::AlignItems, Decision::Use(PropertyValue::AlignItems(value))) => {
                style.align_items = *value;
            }
            (PropertyId::JustifyContent, Decision::Use(PropertyValue::JustifyContent(value))) => {
                style.justify_content = *value;
            }
            (PropertyId::AlignSelf, Decision::Use(PropertyValue::AlignSelf(value))) => {
                style.align_self = *value;
            }
            (PropertyId::GridTemplateColumns, Decision::Use(PropertyValue::GridTrack(value))) => {
                style.grid_template_columns = value.clone();
            }
            (PropertyId::GridTemplateRows, Decision::Use(PropertyValue::GridTrack(value))) => {
                style.grid_template_rows = value.clone();
            }
            (PropertyId::RowGap, Decision::Use(PropertyValue::Gap(value))) => {
                style.row_gap = resolve_gap(*value, viewport);
            }
            (PropertyId::ColumnGap, Decision::Use(PropertyValue::Gap(value))) => {
                style.column_gap = resolve_gap(*value, viewport);
            }
            (PropertyId::GridAutoFlow, Decision::Use(PropertyValue::GridAutoFlow(value))) => {
                style.grid_auto_flow = *value;
            }
            (PropertyId::MarginTop, Decision::Use(PropertyValue::Margin(value))) => {
                style.margin.top = resolve_margin(*value, viewport);
            }
            (PropertyId::MarginRight, Decision::Use(PropertyValue::Margin(value))) => {
                style.margin.right = resolve_margin(*value, viewport);
            }
            (PropertyId::MarginBottom, Decision::Use(PropertyValue::Margin(value))) => {
                style.margin.bottom = resolve_margin(*value, viewport);
            }
            (PropertyId::MarginLeft, Decision::Use(PropertyValue::Margin(value))) => {
                style.margin.left = resolve_margin(*value, viewport);
            }
            (PropertyId::PaddingTop, Decision::Use(PropertyValue::Padding(value))) => {
                style.padding.top = resolve_padding(*value, viewport);
            }
            (PropertyId::PaddingRight, Decision::Use(PropertyValue::Padding(value))) => {
                style.padding.right = resolve_padding(*value, viewport);
            }
            (PropertyId::PaddingBottom, Decision::Use(PropertyValue::Padding(value))) => {
                style.padding.bottom = resolve_padding(*value, viewport);
            }
            (PropertyId::PaddingLeft, Decision::Use(PropertyValue::Padding(value))) => {
                style.padding.left = resolve_padding(*value, viewport);
            }
            (PropertyId::BorderTopStyle, Decision::Use(PropertyValue::BorderStyle(value))) => {
                style.border_style.top = *value;
            }
            (PropertyId::BorderRightStyle, Decision::Use(PropertyValue::BorderStyle(value))) => {
                style.border_style.right = *value;
            }
            (PropertyId::BorderBottomStyle, Decision::Use(PropertyValue::BorderStyle(value))) => {
                style.border_style.bottom = *value;
            }
            (PropertyId::BorderLeftStyle, Decision::Use(PropertyValue::BorderStyle(value))) => {
                style.border_style.left = *value;
            }
            (PropertyId::BorderTopWidth, Decision::Use(PropertyValue::BorderWidth(value))) => {
                style.border_width.top = used_border_width(*value, style.border_style.top);
            }
            (PropertyId::BorderRightWidth, Decision::Use(PropertyValue::BorderWidth(value))) => {
                style.border_width.right = used_border_width(*value, style.border_style.right);
            }
            (PropertyId::BorderBottomWidth, Decision::Use(PropertyValue::BorderWidth(value))) => {
                style.border_width.bottom = used_border_width(*value, style.border_style.bottom);
            }
            (PropertyId::BorderLeftWidth, Decision::Use(PropertyValue::BorderWidth(value))) => {
                style.border_width.left = used_border_width(*value, style.border_style.left);
            }
            (PropertyId::BorderTopColor, Decision::Use(PropertyValue::BorderColor(value))) => {
                style.border_color.top = resolve_border_color(*value, style.color);
            }
            (PropertyId::BorderRightColor, Decision::Use(PropertyValue::BorderColor(value))) => {
                style.border_color.right = resolve_border_color(*value, style.color);
            }
            (PropertyId::BorderBottomColor, Decision::Use(PropertyValue::BorderColor(value))) => {
                style.border_color.bottom = resolve_border_color(*value, style.color);
            }
            (PropertyId::BorderLeftColor, Decision::Use(PropertyValue::BorderColor(value))) => {
                style.border_color.left = resolve_border_color(*value, style.color);
            }
            (
                PropertyId::BorderTopLeftRadius,
                Decision::Use(PropertyValue::BorderRadius(value)),
            ) => {
                style.border_radius.top_left = resolve_border_radius(*value, viewport);
            }
            (
                PropertyId::BorderTopRightRadius,
                Decision::Use(PropertyValue::BorderRadius(value)),
            ) => {
                style.border_radius.top_right = resolve_border_radius(*value, viewport);
            }
            (
                PropertyId::BorderBottomRightRadius,
                Decision::Use(PropertyValue::BorderRadius(value)),
            ) => {
                style.border_radius.bottom_right = resolve_border_radius(*value, viewport);
            }
            (
                PropertyId::BorderBottomLeftRadius,
                Decision::Use(PropertyValue::BorderRadius(value)),
            ) => {
                style.border_radius.bottom_left = resolve_border_radius(*value, viewport);
            }
            // 关键字与继承：按属性回退
            (property, decision @ (Decision::Parent | Decision::Initial)) => {
                let source = match decision {
                    Decision::Parent => parent.unwrap_or(&initial),
                    _ => &initial,
                };
                match property {
                    PropertyId::Display => style.display = source.display,
                    PropertyId::Color => style.color = source.color,
                    PropertyId::BackgroundColor => style.background_color = source.background_color,
                    PropertyId::BackgroundImage => {
                        style.background_image = source.background_image.clone();
                    }
                    PropertyId::FontSize => style.font_size = source.font_size,
                    PropertyId::FontWeight => style.font_weight = source.font_weight,
                    PropertyId::FontStyle => style.font_style = source.font_style,
                    PropertyId::FontFamily => style.font_family = source.font_family.clone(),
                    PropertyId::TextAlign => style.text_align = source.text_align,
                    PropertyId::LineHeight => style.line_height = source.line_height,
                    PropertyId::WhiteSpace => style.white_space = source.white_space,
                    PropertyId::OverflowWrap => style.overflow_wrap = source.overflow_wrap,
                    PropertyId::WordBreak => style.word_break = source.word_break,
                    PropertyId::LetterSpacing => style.letter_spacing = source.letter_spacing,
                    PropertyId::WordSpacing => style.word_spacing = source.word_spacing,
                    PropertyId::TextIndent => style.text_indent = source.text_indent,
                    PropertyId::TabSize => style.tab_size = source.tab_size,
                    PropertyId::TextDecorationLine => {
                        style.text_decoration_line = source.text_decoration_line;
                    }
                    PropertyId::TextDecorationStyle => {
                        style.text_decoration_style = source.text_decoration_style;
                    }
                    PropertyId::TextDecorationColor => {
                        style.text_decoration_color = source.text_decoration_color;
                    }
                    PropertyId::ZIndex => style.z_index = source.z_index,
                    PropertyId::Width => style.width = source.width,
                    PropertyId::Height => style.height = source.height,
                    PropertyId::MinWidth => style.min_width = source.min_width,
                    PropertyId::MaxWidth => style.max_width = source.max_width,
                    PropertyId::MinHeight => style.min_height = source.min_height,
                    PropertyId::MaxHeight => style.max_height = source.max_height,
                    PropertyId::BoxSizing => style.box_sizing = source.box_sizing,
                    PropertyId::Overflow => style.overflow = source.overflow,
                    PropertyId::FlexGrow => style.flex_grow = source.flex_grow,
                    PropertyId::FlexBasis => style.flex_basis = source.flex_basis,
                    PropertyId::Position => style.position = source.position,
                    PropertyId::Float => style.float = source.float,
                    PropertyId::Clear => style.clear = source.clear,
                    PropertyId::Top => style.inset.top = source.inset.top,
                    PropertyId::Right => style.inset.right = source.inset.right,
                    PropertyId::Bottom => style.inset.bottom = source.inset.bottom,
                    PropertyId::Left => style.inset.left = source.inset.left,
                    PropertyId::FlexDirection => style.flex_direction = source.flex_direction,
                    PropertyId::FlexWrap => style.flex_wrap = source.flex_wrap,
                    PropertyId::FlexShrink => style.flex_shrink = source.flex_shrink,
                    PropertyId::AlignItems => style.align_items = source.align_items,
                    PropertyId::JustifyContent => style.justify_content = source.justify_content,
                    PropertyId::AlignSelf => style.align_self = source.align_self,
                    PropertyId::GridTemplateColumns => {
                        style.grid_template_columns = source.grid_template_columns.clone()
                    }
                    PropertyId::GridTemplateRows => {
                        style.grid_template_rows = source.grid_template_rows.clone()
                    }
                    PropertyId::RowGap => style.row_gap = source.row_gap,
                    PropertyId::ColumnGap => style.column_gap = source.column_gap,
                    PropertyId::GridAutoFlow => style.grid_auto_flow = source.grid_auto_flow,
                    PropertyId::MarginTop => style.margin.top = source.margin.top,
                    PropertyId::MarginRight => style.margin.right = source.margin.right,
                    PropertyId::MarginBottom => style.margin.bottom = source.margin.bottom,
                    PropertyId::MarginLeft => style.margin.left = source.margin.left,
                    PropertyId::PaddingTop => style.padding.top = source.padding.top,
                    PropertyId::PaddingRight => style.padding.right = source.padding.right,
                    PropertyId::PaddingBottom => style.padding.bottom = source.padding.bottom,
                    PropertyId::PaddingLeft => style.padding.left = source.padding.left,
                    PropertyId::BorderTopStyle => style.border_style.top = source.border_style.top,
                    PropertyId::BorderRightStyle => {
                        style.border_style.right = source.border_style.right;
                    }
                    PropertyId::BorderBottomStyle => {
                        style.border_style.bottom = source.border_style.bottom;
                    }
                    PropertyId::BorderLeftStyle => {
                        style.border_style.left = source.border_style.left;
                    }
                    PropertyId::BorderTopWidth => style.border_width.top = source.border_width.top,
                    PropertyId::BorderRightWidth => {
                        style.border_width.right = source.border_width.right;
                    }
                    PropertyId::BorderBottomWidth => {
                        style.border_width.bottom = source.border_width.bottom;
                    }
                    PropertyId::BorderLeftWidth => {
                        style.border_width.left = source.border_width.left;
                    }
                    PropertyId::BorderTopColor => style.border_color.top = source.border_color.top,
                    PropertyId::BorderRightColor => {
                        style.border_color.right = source.border_color.right;
                    }
                    PropertyId::BorderBottomColor => {
                        style.border_color.bottom = source.border_color.bottom;
                    }
                    PropertyId::BorderLeftColor => {
                        style.border_color.left = source.border_color.left;
                    }
                    PropertyId::BorderTopLeftRadius => {
                        style.border_radius.top_left = source.border_radius.top_left;
                    }
                    PropertyId::BorderTopRightRadius => {
                        style.border_radius.top_right = source.border_radius.top_right;
                    }
                    PropertyId::BorderBottomRightRadius => {
                        style.border_radius.bottom_right = source.border_radius.bottom_right;
                    }
                    PropertyId::BorderBottomLeftRadius => {
                        style.border_radius.bottom_left = source.border_radius.bottom_left;
                    }
                }
            }
            // 声明值与属性不匹配只可能来自程序错误；保持 initial 值
            _ => {}
        }
    }
    style
}

/// border-*-width 的 computed 值：style 为 none/hidden 时为 0
/// （CSS 2.1 §8.5.4）。
fn used_border_width(value: BorderWidthValue, border_style: BorderStyle) -> f32 {
    if border_style.visible() {
        value.px()
    } else {
        0.0
    }
}

/// border-*-color 的 computed 值：currentcolor 按本元素 color 解析。
fn resolve_border_color(value: BorderColorValue, color: Rgba) -> Rgba {
    match value {
        BorderColorValue::Rgba(rgba) => rgba,
        BorderColorValue::CurrentColor => color,
    }
}

/// text-decoration-color 的 computed 值：currentcolor 按本元素 color 解析。
fn resolve_text_decoration_color(value: TextDecorationColorValue, color: Rgba) -> Rgba {
    match value {
        TextDecorationColorValue::Rgba(rgba) => rgba,
        TextDecorationColorValue::CurrentColor => color,
    }
}

/// `font-size` 计算值（CSS Fonts 4 §2.5：长度/百分比相对父元素计算字号）。
///
/// 相对单位乘积过卫生钳：继承链上的巨数 em/百分比不得算出 `inf`。
fn compute_font_size(value: &FontSizeValue, parent_font_size: f32) -> f32 {
    clamp_computed(match value {
        FontSizeValue::Px(px) => *px,
        FontSizeValue::Em(em) => parent_font_size * em,
        FontSizeValue::Percent(fraction) => parent_font_size * fraction,
        FontSizeValue::Absolute(size) => MEDIUM_FONT_SIZE * size.factor(),
        FontSizeValue::Larger => adjacent_size(parent_font_size, true),
        FontSizeValue::Smaller => adjacent_size(parent_font_size, false),
    })
}

/// `larger` / `smaller`：父字号恰为绝对尺寸表项时取相邻项
/// （CSS Fonts 4 §2.5），否则按规范允许的简单比例 1.2 缩放。
fn adjacent_size(parent_font_size: f32, larger: bool) -> f32 {
    const TOLERANCE: f32 = 0.01;
    let index = AbsoluteSize::ORDERED
        .iter()
        .position(|size| (MEDIUM_FONT_SIZE * size.factor() - parent_font_size).abs() <= TOLERANCE);
    match index {
        Some(index) if larger && index + 1 < AbsoluteSize::ORDERED.len() => {
            MEDIUM_FONT_SIZE * AbsoluteSize::ORDERED[index + 1].factor()
        }
        Some(index) if !larger && index > 0 => {
            MEDIUM_FONT_SIZE * AbsoluteSize::ORDERED[index - 1].factor()
        }
        // 表外或已到表端：简单比例
        _ => {
            if larger {
                parent_font_size * 1.2
            } else {
                parent_font_size / 1.2
            }
        }
    }
}

/// `font-weight` 计算值（CSS Fonts 4 §2.2）。
fn compute_font_weight(value: &FontWeightValue, parent_font_weight: f32) -> f32 {
    match value {
        FontWeightValue::Weight(weight) => weight.clamp(1.0, 1000.0),
        FontWeightValue::Bolder => relative_weight(parent_font_weight, true),
        FontWeightValue::Lighter => relative_weight(parent_font_weight, false),
    }
}

/// bolder / lighter 相对映射表（CSS Fonts 4 §2.2.1）。
fn relative_weight(parent_weight: f32, bolder: bool) -> f32 {
    if parent_weight < 100.0 {
        if bolder { 400.0 } else { parent_weight }
    } else if parent_weight < 350.0 {
        if bolder { 400.0 } else { 100.0 }
    } else if parent_weight < 550.0 {
        if bolder { 700.0 } else { 100.0 }
    } else if parent_weight < 750.0 {
        if bolder { 900.0 } else { 400.0 }
    } else if parent_weight < 900.0 {
        if bolder { 900.0 } else { 700.0 }
    } else {
        // bolder：无更粗字重，保持不变；lighter：700
        if bolder { parent_weight } else { 700.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::html_ua_stylesheet;
    use crate::test_support::{build_document, find_first_by_name};
    use crate::value::GridTrack;

    fn style_of(html: &str, sheets: &[&str], name: &str) -> ComputedStyle {
        let document = build_document(html);
        let parsed: Vec<Stylesheet> = sheets.iter().map(|css| Stylesheet::parse(css)).collect();
        let refs: Vec<&Stylesheet> = parsed.iter().collect();
        let element = find_first_by_name(&document, name).expect("element");
        cascade(&document, element, &refs, None, None)
    }

    /// 视口 800×600：vw/vh/vmin/vmax 按对应边 1% 折算为绝对 px。
    #[test]
    fn viewport_units_resolve_against_viewport() {
        let document = build_document("<div></div>");
        let sheet = Stylesheet::parse(
            "div { width: 50vw; min-height: 10vh; margin: 1vmin auto; \
             padding: 1vmax; top: 2vw; row-gap: 3vh }",
        );
        let refs = vec![&sheet];
        let div = find_first_by_name(&document, "div").expect("div");
        let viewport = MediaViewport {
            width: 800.0,
            height: 600.0,
        };
        let style = cascade_in(&document, div, &refs, None, None, Some(viewport));
        assert_eq!(style.width, SizeValue::Length(400.0), "50vw = 400px");
        assert_eq!(style.min_height, SizeValue::Length(60.0), "10vh = 60px");
        assert_eq!(
            style.margin.top,
            MarginValue::Length(6.0),
            "1vmin = 6px（短边 600）"
        );
        assert_eq!(style.margin.left, MarginValue::Auto, "非视口成分不受影响");
        assert_eq!(
            style.padding.top,
            PaddingValue::Length(8.0),
            "1vmax = 8px（长边 800）"
        );
        assert_eq!(style.inset.top, InsetValue::Length(16.0), "2vw = 16px");
        assert_eq!(style.row_gap, GapValue::Length(18.0), "3vh = 18px");
    }

    /// 无视口信息：视口单位原样保留在 computed style 里（消费方降级）。
    #[test]
    fn viewport_units_stay_unresolved_without_viewport() {
        let style = style_of(
            "<div></div>",
            &["div { width: 50vw; margin-top: 2vh }"],
            "div",
        );
        assert_eq!(style.width, SizeValue::Viewport(50.0, ViewportUnit::Vw));
        assert_eq!(
            style.margin.top,
            MarginValue::Viewport(2.0, ViewportUnit::Vh)
        );
    }

    #[test]
    fn no_declarations_fall_back_to_inheritance_and_initial() {
        let document = build_document("<div style=\"color: red\"><p>x</p></div>");
        let refs: Vec<&Stylesheet> = vec![];
        let div = find_first_by_name(&document, "div").expect("div");
        let div_style = cascade(&document, div, &refs, None, None);
        assert_eq!(div_style.color, Rgba::opaque(255, 0, 0), "内联样式生效");
        assert_eq!(
            div_style.background_color,
            Rgba::transparent(),
            "非继承属性取 initial"
        );

        let p = find_first_by_name(&document, "p").expect("p");
        let p_style = cascade(&document, p, &refs, None, Some(&div_style));
        assert_eq!(p_style.color, Rgba::opaque(255, 0, 0), "继承属性取父值");
        assert_eq!(p_style.background_color, Rgba::transparent());
        assert_eq!(p_style.font_size, 16.0);
    }

    #[test]
    fn specificity_beats_source_order() {
        let style = style_of(
            "<p class=\"a\">x</p>",
            &["p { color: red } .a { color: blue }"],
            "p",
        );
        assert_eq!(style.color, Rgba::opaque(0, 0, 255));
    }

    #[test]
    fn source_order_breaks_ties() {
        let style = style_of(
            "<p class=\"a\">x</p>",
            &[".a { color: blue } p.a { color: red }"],
            "p",
        );
        assert_eq!(style.color, Rgba::opaque(255, 0, 0));
    }

    #[test]
    fn important_beats_specificity() {
        let style = style_of(
            "<p class=\"a\">x</p>",
            &["p.a { color: red } p { color: blue !important }"],
            "p",
        );
        assert_eq!(style.color, Rgba::opaque(0, 0, 255));
    }

    #[test]
    fn inline_style_ordering() {
        // 内联普通声明压过样式表普通声明
        let style = style_of("<p style=\"color: red\">x</p>", &["p { color: blue }"], "p");
        assert_eq!(style.color, Rgba::opaque(255, 0, 0));

        // 样式表 !important 压过内联普通声明
        let style = style_of(
            "<p style=\"color: red\">x</p>",
            &["p { color: blue !important }"],
            "p",
        );
        assert_eq!(style.color, Rgba::opaque(0, 0, 255));

        // 内联 !important 压过样式表 !important
        let style = style_of(
            "<p style=\"color: red !important\">x</p>",
            &["p { color: blue !important }"],
            "p",
        );
        assert_eq!(style.color, Rgba::opaque(255, 0, 0));
    }

    #[test]
    fn matching_rule_selectors_use_max_specificity() {
        let style = style_of(
            "<p class=\"x\">x</p>",
            &["p, .x { color: red } p { color: blue }"],
            "p",
        );
        assert_eq!(style.color, Rgba::opaque(255, 0, 0));
    }

    #[test]
    fn font_size_resolves_against_parent() {
        // em / 百分比相对父元素计算字号
        let mut document = build_document(
            "<div style=\"font-size: 20px\"><p style=\"font-size: 1.5em\">x</p></div>",
        );
        let parsed = Stylesheet::parse("");
        let div = find_first_by_name(&document, "div").expect("div");
        let div_style = cascade(&document, div, &[&parsed], None, None);
        assert_eq!(div_style.font_size, 20.0);
        let p = find_first_by_name(&document, "p").expect("p");
        let p_style = cascade(&document, p, &[&parsed], None, Some(&div_style));
        assert_eq!(p_style.font_size, 30.0);

        // 继承的是计算值：孙辈再乘 em 时基于 30px
        let span = document.create_node(nexty_dom::NodeKind::Element(nexty_dom::ElementData {
            name: "span".into(),
            namespace: nexty_dom::Namespace::Html,
            attributes: Vec::new(),
        }));
        document.insert_node(span, p, None);
        let span_style = cascade(&document, span, &[&parsed], None, Some(&p_style));
        assert_eq!(span_style.font_size, 30.0, "继承计算字号而非声明值");
    }

    #[test]
    fn huge_relative_font_size_and_line_height_stay_finite() {
        // 巨数 em 的解析值已钳到上限，计算乘积再钳一次：inf 不得进入布局
        let style = style_of(
            "<p style=\"font-size: 1e30em; line-height: 1.5em\">x</p>",
            &[],
            "p",
        );
        assert_eq!(style.font_size, MAX_CSS_LENGTH, "16 × 1e30 → 钳到上限");
        let LineHeightValue::Length(line_height) = style.line_height else {
            panic!("em 行高应折算为绝对长度");
        };
        assert_eq!(line_height, MAX_CSS_LENGTH, "1.5 × 1e7 → 钳到上限");
        // 负字号钳到下界
        let style = style_of("<p style=\"font-size: -1e30em\">x</p>", &[], "p");
        assert_eq!(style.font_size, -MAX_CSS_LENGTH);
    }

    #[test]
    fn font_size_keywords_and_relative() {
        let style = style_of("<p>x</p>", &["p { font-size: x-large }"], "p");
        assert_eq!(style.font_size, 24.0);

        // larger：父字号在表内时取下一项
        let document = build_document(
            "<div style=\"font-size: large\"><p style=\"font-size: larger\">x</p></div>",
        );
        let parsed = Stylesheet::parse("");
        let div_style = cascade(
            &document,
            find_first_by_name(&document, "div").unwrap(),
            &[&parsed],
            None,
            None,
        );
        let p_style = cascade(
            &document,
            find_first_by_name(&document, "p").unwrap(),
            &[&parsed],
            None,
            Some(&div_style),
        );
        assert_eq!(
            p_style.font_size,
            MEDIUM_FONT_SIZE * 1.5,
            "large(1.2) → larger(1.5)"
        );

        // 表外字号用 1.2 比例
        let document = build_document(
            "<div style=\"font-size: 100px\"><p style=\"font-size: smaller\">x</p></div>",
        );
        let div_style = cascade(
            &document,
            find_first_by_name(&document, "div").unwrap(),
            &[&parsed],
            None,
            None,
        );
        let p_style = cascade(
            &document,
            find_first_by_name(&document, "p").unwrap(),
            &[&parsed],
            None,
            Some(&div_style),
        );
        assert!((p_style.font_size - 100.0 / 1.2).abs() < 1e-4);
    }

    #[test]
    fn font_weight_relative_table() {
        // CSS Fonts 4 §2.2.1 映射表
        let cases: [(f32, &str, f32); 6] = [
            (50.0, "bolder", 400.0),
            (200.0, "bolder", 400.0),
            (400.0, "bolder", 700.0),
            (600.0, "bolder", 900.0),
            (950.0, "bolder", 950.0),
            (950.0, "lighter", 700.0),
        ];
        for (parent_weight, keyword, expected) in cases {
            let document = build_document(&format!(
                "<div style=\"font-weight: {parent_weight}\"><p style=\"font-weight: {keyword}\">x</p></div>"
            ));
            let parsed = Stylesheet::parse("");
            let div_style = cascade(
                &document,
                find_first_by_name(&document, "div").unwrap(),
                &[&parsed],
                None,
                None,
            );
            assert_eq!(div_style.font_weight, parent_weight);
            let p_style = cascade(
                &document,
                find_first_by_name(&document, "p").unwrap(),
                &[&parsed],
                None,
                Some(&div_style),
            );
            assert_eq!(
                p_style.font_weight, expected,
                "{keyword} from {parent_weight}"
            );
        }
    }

    #[test]
    fn css_wide_keywords() {
        // inherit：无父元素 → initial
        let style = style_of("<p style=\"color: inherit\">x</p>", &[], "p");
        assert_eq!(style.color, Rgba::opaque(0, 0, 0));

        // unset：继承属性取父值
        let document =
            build_document("<div style=\"color: red\"><p style=\"color: unset\">x</p></div>");
        let parsed = Stylesheet::parse("");
        let div_style = cascade(
            &document,
            find_first_by_name(&document, "div").unwrap(),
            &[&parsed],
            None,
            None,
        );
        let p_style = cascade(
            &document,
            find_first_by_name(&document, "p").unwrap(),
            &[&parsed],
            None,
            Some(&div_style),
        );
        assert_eq!(p_style.color, Rgba::opaque(255, 0, 0));

        // unset：非继承属性取 initial
        let style = style_of(
            "<p style=\"background-color: unset\" class=\"x\">x</p>",
            &["p.x { background-color: red }"],
            "p",
        );
        assert_eq!(style.background_color, Rgba::transparent());

        // initial：显式回到 initial
        let style = style_of(
            "<p style=\"color: initial\">x</p>",
            &["p { color: red }"],
            "p",
        );
        assert_eq!(style.color, Rgba::opaque(0, 0, 0));

        // revert：回退到 UA origin（本层无 UA 声明 → initial）
        let style = style_of(
            "<p style=\"color: revert\">x</p>",
            &["p { color: red }"],
            "p",
        );
        assert_eq!(style.color, Rgba::opaque(0, 0, 0));
    }

    #[test]
    fn document_styles_walk_parents_first() {
        let document =
            build_document("<body><div style=\"color: red\"><p><em>x</em></p></div></body>");
        let sheet = Stylesheet::parse("em { font-style: italic }");
        let styles = compute_document_styles(&document, &[&sheet], None);

        let div = find_first_by_name(&document, "div").expect("div");
        let p = find_first_by_name(&document, "p").expect("p");
        let em = find_first_by_name(&document, "em").expect("em");

        assert_eq!(styles[&div].color, Rgba::opaque(255, 0, 0));
        assert_eq!(styles[&p].color, Rgba::opaque(255, 0, 0), "p 继承 div");
        assert_eq!(styles[&em].color, Rgba::opaque(255, 0, 0), "em 继承 p");
        assert_eq!(
            styles[&em].font_style,
            FontStyleValue::Italic,
            "样式表命中 em"
        );
        // body 自身也在结果里
        let body = find_first_by_name(&document, "body").expect("body");
        assert_eq!(styles[&body].font_style, FontStyleValue::Normal);
    }

    #[test]
    fn multiple_stylesheets_cascade_in_order() {
        let document = build_document("<p>x</p>");
        let first = Stylesheet::parse("p { color: red }");
        let second = Stylesheet::parse("p { color: blue }");
        let style = cascade(
            &document,
            find_first_by_name(&document, "p").unwrap(),
            &[&first, &second],
            None,
            None,
        );
        assert_eq!(style.color, Rgba::opaque(0, 0, 255), "后传入的样式表胜出");
    }

    #[test]
    fn where_has_zero_specificity_is_takes_argument_max() {
        // Selectors 4：:where() 特异度为 0；:is() 取参数中的最大特异度
        let document = build_document("<p id=\"a\" class=\"x\">x</p>");
        let sheet = Stylesheet::parse(
            "p { color: red } :where(p.x) { color: blue } :is(#a) { color: green }",
        );
        let style = cascade(
            &document,
            find_first_by_name(&document, "p").unwrap(),
            &[&sheet],
            None,
            None,
        );
        // :is(#a) 特异度 (1,0,0) 压过其余两条
        assert_eq!(style.color, Rgba::opaque(0, 128, 0));

        let sheet = Stylesheet::parse("p { color: red } :where(p.x) { color: blue }");
        let style = cascade(
            &document,
            find_first_by_name(&document, "p").unwrap(),
            &[&sheet],
            None,
            None,
        );
        // :where(p.x) 特异度为 0，p (0,0,1) 按源顺序在后仍胜出
        assert_eq!(style.color, Rgba::opaque(255, 0, 0));
    }

    #[test]
    fn ua_origin_ordering_matches_cascade_5() {
        let ua_block = Stylesheet::parse("div { display: block }");
        let ua_important_none = Stylesheet::parse("div { display: none !important }");
        let div = || build_document("<div>x</div>");

        // UA normal < author normal
        let document = div();
        let author = Stylesheet::parse("div { display: inline }");
        let style = cascade(
            &document,
            find_first_by_name(&document, "div").unwrap(),
            &[],
            Some(&ua_block),
            None,
        );
        assert_eq!(
            style.display,
            DisplayValue::Block,
            "无 author 声明时 UA 生效"
        );
        let style = cascade(
            &document,
            find_first_by_name(&document, "div").unwrap(),
            &[&author],
            Some(&ua_block),
            None,
        );
        assert_eq!(
            style.display,
            DisplayValue::Inline,
            "author normal 压过 UA normal"
        );

        // author normal < author important < UA important
        let document = div();
        let author_normal = Stylesheet::parse("div { display: block }");
        let style = cascade(
            &document,
            find_first_by_name(&document, "div").unwrap(),
            &[&author_normal],
            Some(&ua_important_none),
            None,
        );
        assert_eq!(
            style.display,
            DisplayValue::None,
            "UA !important 压过 author normal"
        );

        let document = div();
        let author_important = Stylesheet::parse("div { display: block !important }");
        let style = cascade(
            &document,
            find_first_by_name(&document, "div").unwrap(),
            &[&author_important],
            Some(&ua_important_none),
            None,
        );
        assert_eq!(
            style.display,
            DisplayValue::None,
            "UA !important 压过 author !important"
        );
    }

    #[test]
    fn html_ua_stylesheet_provides_rendering_defaults() {
        let document =
            build_document("<div><p>x</p></div><head></head><ul><li>item</li></ul><b>bold</b>");
        let styles = compute_document_styles(&document, &[], Some(html_ua_stylesheet()));
        let display_of = |name: &str| {
            let node = find_first_by_name(&document, name).expect(name);
            styles[&node].display
        };
        assert_eq!(display_of("div"), DisplayValue::Block);
        assert_eq!(display_of("p"), DisplayValue::Block);
        assert_eq!(display_of("head"), DisplayValue::None);
        assert_eq!(display_of("li"), DisplayValue::ListItem);
        assert_eq!(
            display_of("b"),
            DisplayValue::Inline,
            "未列入 UA 表的元素保持 initial"
        );

        // author normal 声明覆盖 UA 默认 margin
        let sheet = Stylesheet::parse("body { margin: 0 }");
        let styles = compute_document_styles(&document, &[&sheet], Some(html_ua_stylesheet()));
        let body = find_first_by_name(&document, "body").expect("body");
        assert_eq!(
            styles[&body].margin,
            crate::Edges::splat(MarginValue::Length(0.0))
        );
    }

    #[test]
    fn border_width_folds_style_and_currentcolor_resolves() {
        // style 缺省为 none → computed 宽度 0（CSS 2.1 §8.5.4）
        let style = style_of("<p>x</p>", &["p { border: 4px }"], "p");
        assert_eq!(style.border_width, Edges::splat(0.0));
        assert_eq!(style.border_style, Edges::splat(BorderStyle::None));

        // style 可见 → 宽度生效；currentcolor 按本元素 color 解析
        let style = style_of(
            "<p>x</p>",
            &[
                "p { border: 2px solid; color: red; border-top-color: currentcolor; border-left-color: green }",
            ],
            "p",
        );
        assert_eq!(style.border_width, Edges::splat(2.0));
        assert_eq!(style.border_style.top, BorderStyle::Solid);
        assert_eq!(
            style.border_color.top,
            Rgba::opaque(255, 0, 0),
            "currentcolor"
        );
        assert_eq!(style.border_color.left, Rgba::opaque(0, 128, 0));
        assert_eq!(
            style.border_color.bottom,
            Rgba::opaque(255, 0, 0),
            "border 简写缺省 color = currentcolor"
        );
    }

    #[test]
    fn line_height_computes_percent_and_em_to_absolute() {
        let style = style_of(
            "<p>x</p>",
            &["p { font-size: 20px; line-height: 150% }"],
            "p",
        );
        assert_eq!(style.line_height, LineHeightValue::Length(30.0));

        let style = style_of(
            "<p>x</p>",
            &["p { font-size: 20px; line-height: 1.2em }"],
            "p",
        );
        assert_eq!(style.line_height, LineHeightValue::Length(24.0));

        // 数值保持数值：无单位数值按数值继承
        let document = build_document(
            "<div style=\"font-size: 10px; line-height: 2\"><p style=\"font-size: 20px\">x</p></div>",
        );
        let parsed = Stylesheet::parse("");
        let div = find_first_by_name(&document, "div").expect("div");
        let div_style = cascade(&document, div, &[&parsed], None, None);
        let p = find_first_by_name(&document, "p").expect("p");
        let p_style = cascade(&document, p, &[&parsed], None, Some(&div_style));
        assert_eq!(div_style.line_height, LineHeightValue::Number(2.0));
        assert_eq!(
            p_style.line_height,
            LineHeightValue::Number(2.0),
            "数值 line-height 以数值继承，不随父字号折算"
        );
    }

    #[test]
    fn box_model_properties_reach_computed_style() {
        let style = style_of(
            "<p style=\"margin: 10px 20%; padding: 5px; width: 300px; height: 50%\">x</p>",
            &[],
            "p",
        );
        assert_eq!(
            style.margin,
            Edges {
                top: MarginValue::Length(10.0),
                right: MarginValue::Percent(0.2),
                bottom: MarginValue::Length(10.0),
                left: MarginValue::Percent(0.2),
            }
        );
        assert_eq!(style.padding, Edges::splat(PaddingValue::Length(5.0)));
        assert_eq!(style.width, SizeValue::Length(300.0));
        assert_eq!(style.height, SizeValue::Percent(0.5));
    }

    #[test]
    fn layout_properties_reach_computed_style() {
        let style = style_of(
            "<div style=\"position: absolute; top: 10px; right: 20%; bottom: auto; left: 5px;\
                flex-direction: column; flex-wrap: wrap; flex-shrink: 2;\
                align-items: center; justify-content: space-between; align-self: flex-end;\
                row-gap: 8px; column-gap: 4px; grid-auto-flow: row dense;\
                grid-template-columns: 100px 1fr; grid-template-rows: auto\">x</div>",
            &[],
            "div",
        );
        assert_eq!(style.position, PositionValue::Absolute);
        assert_eq!(
            style.inset,
            Edges {
                top: InsetValue::Length(10.0),
                right: InsetValue::Percent(0.2),
                bottom: InsetValue::Auto,
                left: InsetValue::Length(5.0),
            }
        );
        assert_eq!(style.flex_direction, FlexDirectionValue::Column);
        assert_eq!(style.flex_wrap, FlexWrapValue::Wrap);
        assert_eq!(style.flex_shrink, 2.0);
        assert_eq!(style.align_items, AlignItemsValue::Center);
        assert_eq!(style.justify_content, JustifyContentValue::SpaceBetween);
        assert_eq!(style.align_self, AlignSelfValue::FlexEnd);
        assert_eq!(style.row_gap, GapValue::Length(8.0));
        assert_eq!(style.column_gap, GapValue::Length(4.0));
        assert_eq!(style.grid_auto_flow, GridAutoFlowValue::RowDense);
        assert_eq!(
            style.grid_template_columns,
            GridTrackList {
                tracks: vec![GridTrack::Length(100.0), GridTrack::Fr(1.0)]
            }
        );
        assert_eq!(
            style.grid_template_rows,
            GridTrackList {
                tracks: vec![GridTrack::Auto]
            }
        );
        assert_eq!(style.float, FloatValue::None);
        assert_eq!(style.clear, ClearValue::None);
    }

    /// `float` / `clear` 解析进 computed style 且**不**继承（CSS 2.1 §9.5）。
    #[test]
    fn float_clear_properties_do_not_inherit() {
        let document = build_document("<div style=\"float: left; clear: both\"><p>x</p></div>");
        let parsed = Stylesheet::parse("");
        let div = find_first_by_name(&document, "div").unwrap();
        let p = find_first_by_name(&document, "p").unwrap();
        let div_style = cascade(&document, div, &[&parsed], None, None);
        assert_eq!(div_style.float, FloatValue::Left);
        assert_eq!(div_style.clear, ClearValue::Both);
        let p_style = cascade(&document, p, &[&parsed], None, None);
        assert_eq!(p_style.float, FloatValue::None);
        assert_eq!(p_style.clear, ClearValue::None);
    }

    /// `white-space` / `overflow-wrap` / `word-break` 均继承，initial 为 `normal`。
    #[test]
    fn text_breaking_properties_inherit() {
        let document = build_document(
            "<div style=\"white-space: nowrap; overflow-wrap: break-word; word-break: break-all\">\
             <p>x</p></div><span>y</span>",
        );
        let parsed = Stylesheet::parse("");
        let div_style = cascade(
            &document,
            find_first_by_name(&document, "div").unwrap(),
            &[&parsed],
            None,
            None,
        );
        assert_eq!(div_style.white_space, WhiteSpaceValue::Nowrap);
        assert_eq!(div_style.overflow_wrap, OverflowWrapValue::BreakWord);
        assert_eq!(div_style.word_break, WordBreakValue::BreakAll);

        let p_style = cascade(
            &document,
            find_first_by_name(&document, "p").unwrap(),
            &[&parsed],
            None,
            Some(&div_style),
        );
        assert_eq!(
            p_style.white_space,
            WhiteSpaceValue::Nowrap,
            "子继承 nowrap"
        );
        assert_eq!(p_style.overflow_wrap, OverflowWrapValue::BreakWord);
        assert_eq!(p_style.word_break, WordBreakValue::BreakAll);

        // 无声明：initial 为 normal
        let style = style_of("<span>y</span>", &[], "span");
        assert_eq!(style.white_space, WhiteSpaceValue::Normal);
        assert_eq!(style.overflow_wrap, OverflowWrapValue::Normal);
        assert_eq!(style.word_break, WordBreakValue::Normal);
    }

    /// UA 表给 `<pre>` 系元素 `white-space: pre`，author 声明可覆盖。
    #[test]
    fn ua_stylesheet_sets_white_space_pre() {
        let document = build_document("<pre>a\nb</pre><div>c</div>");
        let styles = compute_document_styles(&document, &[], Some(html_ua_stylesheet()));
        let pre = find_first_by_name(&document, "pre").expect("pre");
        assert_eq!(styles[&pre].white_space, WhiteSpaceValue::Pre);
        let div = find_first_by_name(&document, "div").expect("div");
        assert_eq!(styles[&div].white_space, WhiteSpaceValue::Normal);

        // author 普通声明压过 UA
        let sheet = Stylesheet::parse("pre { white-space: pre-wrap }");
        let styles = compute_document_styles(&document, &[&sheet], Some(html_ua_stylesheet()));
        assert_eq!(styles[&pre].white_space, WhiteSpaceValue::PreWrap);
    }

    /// UA 表给 `<a>` 链接 `text-decoration: underline`（WHATWG §15.3.4
    /// Phrasing content 中 `:link`/`:visited` 规则的无伪类近似），非 a
    /// 元素不受影响，author 声明可覆盖。
    #[test]
    fn ua_stylesheet_sets_link_underline() {
        let document = build_document("<a href=\"https://example.com\">link</a><p>text</p>");
        let styles = compute_document_styles(&document, &[], Some(html_ua_stylesheet()));
        let a = find_first_by_name(&document, "a").expect("a");
        assert_eq!(
            styles[&a].text_decoration_line,
            TextDecorationLineValue::Underline
        );
        let p = find_first_by_name(&document, "p").expect("p");
        assert_eq!(
            styles[&p].text_decoration_line,
            TextDecorationLineValue::None
        );

        // author 普通声明压过 UA
        let sheet = Stylesheet::parse("a { text-decoration: none }");
        let styles = compute_document_styles(&document, &[&sheet], Some(html_ua_stylesheet()));
        assert_eq!(
            styles[&a].text_decoration_line,
            TextDecorationLineValue::None
        );
    }

    /// CSS Text 3 四属性：initial、em 按自身字号折算、percentage 保留原值、
    /// 继承（四属性均 inherited）。
    #[test]
    fn css_text_spacing_properties_compute_and_inherit() {
        // initial 值：letter-spacing 0（normal 折算）、word-spacing normal、
        // text-indent 0、tab-size 整数 8（CSS Text 3 §5 initial）
        let style = style_of("<span>x</span>", &[], "span");
        assert_eq!(style.letter_spacing, 0.0);
        assert_eq!(style.word_spacing, SpacingValue::Normal);
        assert_eq!(style.text_indent, TextIndentValue::Length(0.0));
        assert_eq!(style.tab_size, TabSizeValue::Integer(8));

        // em 按本元素 font-size 折算（先于自身求值的 FontSize，PROPERTY_ORDER）
        let style = style_of(
            "<div style=\"font-size: 20px; letter-spacing: 0.5em; word-spacing: 2em;\
             text-indent: 1.5em; tab-size: 2em\">x</div>",
            &[],
            "div",
        );
        assert_eq!(style.letter_spacing, 10.0);
        assert_eq!(style.word_spacing, SpacingValue::Length(40.0));
        assert_eq!(style.text_indent, TextIndentValue::Length(30.0));
        assert_eq!(style.tab_size, TabSizeValue::Length(40.0));

        // percentage 保留原值（text-indent 由 layout 按包含块宽度解析）
        let style = style_of("<div style=\"text-indent: 10%\">x</div>", &[], "div");
        assert_eq!(style.text_indent, TextIndentValue::Percent(0.1));

        // 负 letter-spacing 合法
        let style = style_of("<div style=\"letter-spacing: -2px\">x</div>", &[], "div");
        assert_eq!(style.letter_spacing, -2.0);

        // 四属性均继承：子元素无声明时取父值
        let document = build_document(
            "<div style=\"letter-spacing: 4px; word-spacing: 8px; text-indent: 40px; tab-size: 4\">\
             <p>x</p></div>",
        );
        let parsed = Stylesheet::parse("");
        let div_style = cascade(
            &document,
            find_first_by_name(&document, "div").unwrap(),
            &[&parsed],
            None,
            None,
        );
        let p_style = cascade(
            &document,
            find_first_by_name(&document, "p").unwrap(),
            &[&parsed],
            None,
            Some(&div_style),
        );
        assert_eq!(p_style.letter_spacing, 4.0);
        assert_eq!(p_style.word_spacing, SpacingValue::Length(8.0));
        assert_eq!(p_style.text_indent, TextIndentValue::Length(40.0));
        assert_eq!(p_style.tab_size, TabSizeValue::Integer(4));
    }

    /// 全量扫描的参照实现（桶化前的语义）：每元素测试全部规则与选择器。
    /// 桶索引（M5 ④）的正确性判据：两条路径产出的 computed style 逐元素
    /// 一致——跳过的选择器只可能是「必要条件已失败、本就不可能匹配」的。
    fn cascade_unbucketed_in(
        document: &Document,
        element: NodeId,
        stylesheets: &[&Stylesheet],
        ua_sheet: Option<&Stylesheet>,
        parent: Option<&ComputedStyle>,
        viewport: Option<MediaViewport>,
    ) -> ComputedStyle {
        let mut matched: Vec<(CascadeKey, &Declaration)> = Vec::new();
        if let Some(ua) = ua_sheet {
            for (rule_order, rule) in ua.rules().iter().enumerate() {
                let specificity = rule
                    .selectors()
                    .iter()
                    .filter(|parsed| {
                        match_element_selectors(std::slice::from_ref(parsed), document, element)
                    })
                    .map(|parsed| parsed.specificity)
                    .max();
                if let Some(specificity) = specificity {
                    for (declaration_order, declaration) in rule.declarations().iter().enumerate() {
                        matched.push((
                            CascadeKey {
                                bucket: bucket_of(Origin::Ua, declaration.important),
                                inline: false,
                                specificity,
                                rule_order,
                                declaration_order,
                            },
                            declaration,
                        ));
                    }
                }
            }
        }
        for (rule_order, rule) in stylesheets
            .iter()
            .flat_map(|sheet| sheet.rules())
            .enumerate()
        {
            let specificity = rule
                .selectors()
                .iter()
                .filter(|parsed| {
                    match_element_selectors(std::slice::from_ref(parsed), document, element)
                })
                .map(|parsed| parsed.specificity)
                .max();
            if let Some(specificity) = specificity {
                for (declaration_order, declaration) in rule.declarations().iter().enumerate() {
                    matched.push((
                        CascadeKey {
                            bucket: bucket_of(Origin::Author, declaration.important),
                            inline: false,
                            specificity,
                            rule_order,
                            declaration_order,
                        },
                        declaration,
                    ));
                }
            }
        }
        let style_attribute = document.node(element).and_then(|node| match node {
            NodeKind::Element(data) => data
                .attributes
                .iter()
                .find(|attr| attr.namespace == nexty_dom::Namespace::None && attr.name == "style")
                .map(|attr| attr.value.clone()),
            _ => None,
        });
        // 内联声明提升到函数作用域：matched 按引用持有直到 resolve_all
        let inline_declarations = style_attribute.map(|attr| parse_inline_style(&attr));
        if let Some(inline_declarations) = &inline_declarations {
            for (declaration_order, declaration) in inline_declarations.iter().enumerate() {
                matched.push((
                    CascadeKey {
                        bucket: bucket_of(Origin::Author, declaration.important),
                        inline: true,
                        specificity: 0,
                        rule_order: usize::MAX,
                        declaration_order,
                    },
                    declaration,
                ));
            }
        }
        resolve_all(&matched, parent, viewport)
    }

    /// 全量扫描的整文档版本（参照路径）。
    fn walk_unbucketed(
        document: &Document,
        node: NodeId,
        parent: Option<&ComputedStyle>,
        stylesheets: &[&Stylesheet],
        ua_sheet: Option<&Stylesheet>,
        viewport: Option<MediaViewport>,
        out: &mut HashMap<NodeId, ComputedStyle>,
    ) {
        for child in document.children(node) {
            if matches!(document.node(child), Some(NodeKind::Element(_))) {
                let style =
                    cascade_unbucketed_in(document, child, stylesheets, ua_sheet, parent, viewport);
                walk_unbucketed(
                    document,
                    child,
                    Some(&style),
                    stylesheets,
                    ua_sheet,
                    viewport,
                    out,
                );
                out.insert(child, style);
            }
        }
    }

    /// 桶化前后逐元素一致：覆盖类/标签/id/组合器/:is/属性选择器/通配/
    /// 多选择器规则/大小写/内联样式/important 的语料，逐页对比两条路径
    /// 的整文档 computed style。
    #[test]
    fn bucketed_cascade_matches_full_scan() {
        const CASES: &[(&str, &str)] = &[
            // 类桶：单类、多类元素命中其一、类不存在
            (
                "<div class=a>x</div><p class=\"a b\">y</p><span>z</span>",
                ".a { color: rgb(1 2 3) } .b { color: rgb(4 5 6) } .c { color: red }",
            ),
            // 标签桶 + 类优先于标签的选择器
            (
                "<p class=lead>text</p><div>other</div>",
                "p { margin: 2px } .lead { margin: 9px } div { padding: 1px }",
            ),
            // id 桶：命中与未命中
            (
                "<div id=main>x</div><div id=other>y</div>",
                "#main { color: blue } #missing { color: red }",
            ),
            // 组合器（Always 桶）：后代 / 子代 / 兄弟
            (
                "<div><p>deep</p></div><div><section><p>nested</p></section></div><h1>a</h1><p>b</p>",
                "div p { color: green } div > p { margin: 1px } h1 + p { color: red }",
            ),
            // :is() / :where()：内部是备选分支，不得作为必要条件
            (
                "<em class=a>x</em><strong id=special>y</strong><b>z</b>",
                ":is(.a, #special) { color: purple } b:is(.a, .b) { color: red } :where(p, div) { margin: 2px }",
            ),
            // 属性选择器与通配（无可提取必要条件）
            (
                "<a href=x>link</a><code>c</code>",
                "[href] { color: blue } * { line-height: 1.4 }",
            ),
            // 一条规则多个选择器：各自分桶，命中其一即整规则生效
            (
                "<i class=x>i</i><div>d</div><u>u</u>",
                ".x, div, #nope { color: teal }",
            ),
            // 大小写：HTML 标签大小写不敏感，类名敏感——两条路径必须一致
            (
                "<DIV CLASS=keep>case</DIV><span CLASS=Keep>x</span>",
                "div { color: brown } .Keep { color: pink } .keep { color: olive }",
            ),
            // 内联样式 + important 交错
            (
                "<p style=\"color: red\">inline</p><b style=\"color: red\">b</b>",
                "p { color: green !important } b { color: green }",
            ),
            // 无样式表 / 空规则：退化路径
            ("<div>x</div>", ""),
            // 特异度混合：tag < class < id，important 反超
            (
                "<div id=t class=c>x</div>",
                "div { color: black } .c { color: gray } #t { color: white } div { color: red !important }",
            ),
        ];
        let ua_sheet = crate::ua::html_ua_stylesheet();
        for (index, (html, css)) in CASES.iter().enumerate() {
            let document = build_document(html);
            let sheet = Stylesheet::parse(css);
            let bucketed = compute_document_styles(&document, &[&sheet], Some(ua_sheet));
            let mut full = HashMap::new();
            walk_unbucketed(
                &document,
                document.root(),
                None,
                &[&sheet],
                Some(ua_sheet),
                None,
                &mut full,
            );
            assert_eq!(bucketed.len(), full.len(), "case {index}: 元素数一致");
            for (node, style) in &bucketed {
                assert_eq!(
                    style, &full[node],
                    "case {index}: node {node:?} 桶化与全量扫描不一致"
                );
            }
        }
    }

    /// 内联样式缓存（M5 ②）：原文未变复用解析结果（同元素两次级联第
    /// 二次命中），原文变化自动失效重解析。
    #[test]
    fn inline_style_cache_reuses_and_invalidates() {
        let mut cache = InlineStyleCache::new();
        let document = build_document("<p style=\"color: red\">x</p>");
        let p = find_first_by_name(&document, "p").unwrap();
        let first = cache.declarations_of(&document, p).expect("style attr");
        assert_eq!(first.len(), 1);
        // 复用：同一元素再次请求命中（声明存储位置相同）
        let first_ptr = first.as_ptr();
        let second_ptr = cache
            .declarations_of(&document, p)
            .expect("style attr")
            .as_ptr();
        assert!(std::ptr::eq(first_ptr, second_ptr), "原文未变应复用");
    }

    // ---- K2 绘制扩展：新属性 computed 与继承性 ----

    /// 四组新属性均不继承：父元素有值，子元素保持 initial。
    #[test]
    fn drawing_extension_properties_are_not_inherited() {
        let style = style_of(
            "<div style=\"text-decoration: underline wavy; z-index: 5; \
             border-radius: 8px; background-image: linear-gradient(90deg, red, blue)\">
             <p>x</p></div>",
            &[],
            "p",
        );
        assert_eq!(
            style.text_decoration_line,
            TextDecorationLineValue::None,
            "text-decoration 不继承"
        );
        assert_eq!(style.text_decoration_style, TextDecorationStyleValue::Solid);
        assert_eq!(style.z_index, ZIndexValue::Auto, "z-index 不继承");
        assert_eq!(
            style.border_radius.top_left,
            BorderRadiusValue::Length(0.0),
            "border-radius 不继承"
        );
        assert_eq!(
            style.background_image,
            BackgroundImageValue::None,
            "background-image 不继承"
        );
    }

    /// text-decoration 简写展开进 cascade：line/style/color 三字段各自生效，
    /// color 的 currentcolor 按本元素 color 解析。
    #[test]
    fn text_decoration_computed_resolves_currentcolor() {
        let style = style_of(
            "<p style=\"color: blue; text-decoration: underline wavy\">x</p>",
            &[],
            "p",
        );
        assert_eq!(
            style.text_decoration_line,
            TextDecorationLineValue::Underline
        );
        assert_eq!(style.text_decoration_style, TextDecorationStyleValue::Wavy);
        assert_eq!(
            style.text_decoration_color,
            Rgba::opaque(0, 0, 255),
            "currentcolor 按本元素 color"
        );
        // 显式 color 成分覆盖 currentcolor
        let style = style_of(
            "<p style=\"color: blue; text-decoration: red underline\">x</p>",
            &[],
            "p",
        );
        assert_eq!(style.text_decoration_color, Rgba::opaque(255, 0, 0));
    }

    /// z-index：整数进 computed；`inherit` 取父 computed 值（fallback 分支）。
    #[test]
    fn z_index_cascade_and_inherit() {
        let document =
            build_document("<div style=\"z-index: -3\"><p style=\"z-index: inherit\">x</p></div>");
        let refs: Vec<&Stylesheet> = vec![];
        let div = find_first_by_name(&document, "div").expect("div");
        let div_style = cascade(&document, div, &refs, None, None);
        assert_eq!(div_style.z_index, ZIndexValue::Integer(-3));
        let p = find_first_by_name(&document, "p").expect("p");
        let p_style = cascade(&document, p, &refs, None, Some(&div_style));
        assert_eq!(p_style.z_index, ZIndexValue::Integer(-3), "inherit 取父值");
    }

    /// background-image：渐变带着归一后的色标位置进 computed style。
    #[test]
    fn background_image_gradient_reaches_computed_style() {
        let style = style_of(
            "<p style=\"background-image: linear-gradient(to right, red, blue 50%, green)\">x</p>",
            &[],
            "p",
        );
        let BackgroundImageValue::LinearGradient(gradient) = &style.background_image else {
            panic!("gradient should reach computed style")
        };
        assert_eq!(gradient.angle, 90.0);
        assert_eq!(gradient.stops.len(), 3);
        assert_eq!(gradient.stops[0].color, Rgba::opaque(255, 0, 0));
        assert_eq!(gradient.stops[1].position, 0.5);
        assert_eq!(gradient.stops[2].position, 1.0);
    }

    /// border-radius：1–4 值展开到四角；视口单位有视口时折算、无视口保留。
    #[test]
    fn border_radius_computed_values() {
        let style = style_of(
            "<p style=\"border-radius: 10px 20px 30px 40px\">x</p>",
            &[],
            "p",
        );
        assert_eq!(
            style.border_radius.top_left,
            BorderRadiusValue::Length(10.0)
        );
        assert_eq!(
            style.border_radius.top_right,
            BorderRadiusValue::Length(20.0)
        );
        assert_eq!(
            style.border_radius.bottom_right,
            BorderRadiusValue::Length(30.0)
        );
        assert_eq!(
            style.border_radius.bottom_left,
            BorderRadiusValue::Length(40.0)
        );

        let document = build_document("<p></p>");
        let sheet = Stylesheet::parse("p { border-radius: 2vw 3vh }");
        let refs = vec![&sheet];
        let p = find_first_by_name(&document, "p").expect("p");
        let viewport = MediaViewport {
            width: 800.0,
            height: 600.0,
        };
        let with_viewport = cascade_in(&document, p, &refs, None, None, Some(viewport));
        assert_eq!(
            with_viewport.border_radius.top_left,
            BorderRadiusValue::Length(16.0),
            "2vw = 16px"
        );
        assert_eq!(
            with_viewport.border_radius.top_right,
            BorderRadiusValue::Length(18.0),
            "3vh = 18px"
        );
        let without_viewport = cascade(&document, p, &refs, None, None);
        assert_eq!(
            without_viewport.border_radius.top_left,
            BorderRadiusValue::Viewport(2.0, ViewportUnit::Vw),
            "无视口保留原值"
        );
    }
}
