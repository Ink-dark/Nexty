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

use crate::parser::{Declaration, Stylesheet, parse_inline_style};
use crate::selector::match_element_selectors;
use crate::value::{
    AbsoluteSize, BorderColorValue, BorderStyle, BorderWidthValue, BoxSizingValue, CssWideKeyword,
    DeclaredValue, DisplayValue, FontFamilyValue, FontSizeValue, FontStyleValue, FontWeightValue,
    LineHeightValue, MarginValue, OverflowValue, PaddingValue, PropertyId, PropertyValue, Rgba,
    SizeValue, TextAlignValue,
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
            font_size: MEDIUM_FONT_SIZE,
            font_weight: 400.0,
            font_style: FontStyleValue::Normal,
            font_family: vec![FontFamilyValue::Generic("serif")],
            text_align: TextAlignValue::Start,
            line_height: LineHeightValue::Normal,
            width: SizeValue::Auto,
            height: SizeValue::Auto,
            min_width: SizeValue::Auto,
            max_width: SizeValue::Auto,
            min_height: SizeValue::Auto,
            max_height: SizeValue::Auto,
            box_sizing: BoxSizingValue::ContentBox,
            overflow: OverflowValue::Visible,
            margin: Edges::splat(MarginValue::Length(0.0)),
            padding: Edges::splat(PaddingValue::Length(0.0)),
            border_width: Edges::splat(0.0),
            border_style: Edges::splat(BorderStyle::None),
            border_color: Edges::splat(Rgba::opaque(0, 0, 0)),
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
        )
    }
}

/// 全部属性的固定枚举顺序。
///
/// 顺序承载计算依赖：`FontSize` 先于 `LineHeight`（百分比按自身字号折算）、
/// `Color` 先于 border 颜色（currentcolor）、border style 先于 width
/// （none/hidden 时宽度归零）。
const PROPERTY_ORDER: [PropertyId; 37] = [
    PropertyId::Display,
    PropertyId::Color,
    PropertyId::BackgroundColor,
    PropertyId::FontSize,
    PropertyId::FontWeight,
    PropertyId::FontStyle,
    PropertyId::FontFamily,
    PropertyId::TextAlign,
    PropertyId::LineHeight,
    PropertyId::Width,
    PropertyId::Height,
    PropertyId::MinWidth,
    PropertyId::MaxWidth,
    PropertyId::MinHeight,
    PropertyId::MaxHeight,
    PropertyId::BoxSizing,
    PropertyId::Overflow,
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

/// 收集一个 origin 样式表中命中元素的全部声明。
fn collect_origin_declarations(
    document: &Document,
    element: NodeId,
    sheet: &Stylesheet,
    origin: Origin,
    matched: &mut Vec<(CascadeKey, Declaration)>,
) {
    for (rule_order, rule) in sheet.rules().iter().enumerate() {
        // 规则内多个选择器都能命中时，取特异度最高者
        // （CSS Cascade 5：声明的特异度取其匹配选择器中的最大值）。
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
                        bucket: bucket_of(origin, declaration.important),
                        inline: false,
                        specificity,
                        rule_order,
                        declaration_order,
                    },
                    declaration.clone(),
                ));
            }
        }
    }
}

/// 对单个元素做级联，产出 computed style。
///
/// `ua_sheet` 是 UA origin 样式表（浏览器默认样式），参与级联但优先级
/// 低于 author 声明（UA `!important` 最高，见 [`OriginBucket`]）。
/// `parent` 是树序父元素的 computed style（元素没有元素祖先时为 `None`）。
/// 各属性：级联胜者 → CSS-wide 关键字 → 继承 → initial。
#[must_use]
pub fn cascade(
    document: &Document,
    element: NodeId,
    stylesheets: &[&Stylesheet],
    ua_sheet: Option<&Stylesheet>,
    parent: Option<&ComputedStyle>,
) -> ComputedStyle {
    let mut matched: Vec<(CascadeKey, Declaration)> = Vec::new();

    if let Some(ua) = ua_sheet {
        collect_origin_declarations(document, element, ua, Origin::Ua, &mut matched);
    }
    // 规则顺序跨样式表连续编号：样式表按传入顺序、规则按源顺序
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
                    declaration.clone(),
                ));
            }
        }
    }

    // 内联 style 属性：author origin，普通声明排在全部样式表声明之后
    if let Some(style_attribute) = style_attribute_of(document, element) {
        for (declaration_order, declaration) in
            parse_inline_style(&style_attribute).into_iter().enumerate()
        {
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

    resolve_all(&matched, parent)
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
    let mut styles = HashMap::new();
    walk(
        document,
        document.root(),
        None,
        stylesheets,
        ua_sheet,
        &mut styles,
    );
    styles
}

fn walk(
    document: &Document,
    node: NodeId,
    parent: Option<&ComputedStyle>,
    stylesheets: &[&Stylesheet],
    ua_sheet: Option<&Stylesheet>,
    out: &mut HashMap<NodeId, ComputedStyle>,
) {
    for child in document.children(node) {
        if matches!(document.node(child), Some(NodeKind::Element(_))) {
            let style = cascade(document, child, stylesheets, ua_sheet, parent);
            walk(document, child, Some(&style), stylesheets, ua_sheet, out);
            out.insert(child, style);
        }
    }
}

/// 元素的 style 属性值。
fn style_attribute_of(document: &Document, element: NodeId) -> Option<String> {
    match document.node(element) {
        Some(NodeKind::Element(data)) => data
            .attributes
            .iter()
            .find(|attr| attr.namespace == nexty_dom::Namespace::None && attr.name == "style")
            .map(|attr| attr.value.clone()),
        _ => None,
    }
}

/// 某属性在级联中的胜者声明。
fn winner(property: PropertyId, matched: &[(CascadeKey, Declaration)]) -> Option<&Declaration> {
    matched
        .iter()
        .filter(|(_, declaration)| declaration.property == property)
        .max_by_key(|(key, _)| *key)
        .map(|(_, declaration)| declaration)
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

/// 逐属性解析出 computed style。
fn resolve_all(
    matched: &[(CascadeKey, Declaration)],
    parent: Option<&ComputedStyle>,
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
                        LineHeightValue::Length(style.font_size * fraction)
                    }
                    LineHeightValue::Em(em) => LineHeightValue::Length(style.font_size * em),
                    other => other,
                };
            }
            (PropertyId::Width, Decision::Use(PropertyValue::Size(value))) => {
                style.width = *value;
            }
            (PropertyId::Height, Decision::Use(PropertyValue::Size(value))) => {
                style.height = *value;
            }
            (PropertyId::MinWidth, Decision::Use(PropertyValue::Size(value))) => {
                style.min_width = *value;
            }
            (PropertyId::MaxWidth, Decision::Use(PropertyValue::Size(value))) => {
                style.max_width = *value;
            }
            (PropertyId::MinHeight, Decision::Use(PropertyValue::Size(value))) => {
                style.min_height = *value;
            }
            (PropertyId::MaxHeight, Decision::Use(PropertyValue::Size(value))) => {
                style.max_height = *value;
            }
            (PropertyId::BoxSizing, Decision::Use(PropertyValue::BoxSizing(value))) => {
                style.box_sizing = *value;
            }
            (PropertyId::Overflow, Decision::Use(PropertyValue::Overflow(value))) => {
                style.overflow = *value;
            }
            (PropertyId::MarginTop, Decision::Use(PropertyValue::Margin(value))) => {
                style.margin.top = *value;
            }
            (PropertyId::MarginRight, Decision::Use(PropertyValue::Margin(value))) => {
                style.margin.right = *value;
            }
            (PropertyId::MarginBottom, Decision::Use(PropertyValue::Margin(value))) => {
                style.margin.bottom = *value;
            }
            (PropertyId::MarginLeft, Decision::Use(PropertyValue::Margin(value))) => {
                style.margin.left = *value;
            }
            (PropertyId::PaddingTop, Decision::Use(PropertyValue::Padding(value))) => {
                style.padding.top = *value;
            }
            (PropertyId::PaddingRight, Decision::Use(PropertyValue::Padding(value))) => {
                style.padding.right = *value;
            }
            (PropertyId::PaddingBottom, Decision::Use(PropertyValue::Padding(value))) => {
                style.padding.bottom = *value;
            }
            (PropertyId::PaddingLeft, Decision::Use(PropertyValue::Padding(value))) => {
                style.padding.left = *value;
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
                    PropertyId::FontSize => style.font_size = source.font_size,
                    PropertyId::FontWeight => style.font_weight = source.font_weight,
                    PropertyId::FontStyle => style.font_style = source.font_style,
                    PropertyId::FontFamily => style.font_family = source.font_family.clone(),
                    PropertyId::TextAlign => style.text_align = source.text_align,
                    PropertyId::LineHeight => style.line_height = source.line_height,
                    PropertyId::Width => style.width = source.width,
                    PropertyId::Height => style.height = source.height,
                    PropertyId::MinWidth => style.min_width = source.min_width,
                    PropertyId::MaxWidth => style.max_width = source.max_width,
                    PropertyId::MinHeight => style.min_height = source.min_height,
                    PropertyId::MaxHeight => style.max_height = source.max_height,
                    PropertyId::BoxSizing => style.box_sizing = source.box_sizing,
                    PropertyId::Overflow => style.overflow = source.overflow,
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

/// `font-size` 计算值（CSS Fonts 4 §2.5：长度/百分比相对父元素计算字号）。
fn compute_font_size(value: &FontSizeValue, parent_font_size: f32) -> f32 {
    match value {
        FontSizeValue::Px(px) => *px,
        FontSizeValue::Em(em) => parent_font_size * em,
        FontSizeValue::Percent(fraction) => parent_font_size * fraction,
        FontSizeValue::Absolute(size) => MEDIUM_FONT_SIZE * size.factor(),
        FontSizeValue::Larger => adjacent_size(parent_font_size, true),
        FontSizeValue::Smaller => adjacent_size(parent_font_size, false),
    }
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

    fn style_of(html: &str, sheets: &[&str], name: &str) -> ComputedStyle {
        let document = build_document(html);
        let parsed: Vec<Stylesheet> = sheets.iter().map(|css| Stylesheet::parse(css)).collect();
        let refs: Vec<&Stylesheet> = parsed.iter().collect();
        let element = find_first_by_name(&document, name).expect("element");
        cascade(&document, element, &refs, None, None)
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
}
