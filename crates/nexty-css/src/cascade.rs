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
    AbsoluteSize, CssWideKeyword, DeclaredValue, DisplayValue, FontFamilyValue, FontSizeValue,
    FontStyleValue, FontWeightValue, PropertyId, PropertyValue, Rgba, TextAlignValue,
};

/// 本层 UA 的 medium 字号基准（CSS Fonts 4 §2.5：initial 值由 UA 决定，
/// 对齐浏览器默认 16px；绝对尺寸关键字按 §2.5.1 缩放系数相对它计算）。
pub(crate) const MEDIUM_FONT_SIZE: f32 = 16.0;

/// 一个元素的 computed style（本轮最小属性集）。
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
}

impl ComputedStyle {
    /// 全部属性取 initial 值。
    ///
    /// initial 值逐条对应各属性规范；UA 相关的（`color` 的 `CanvasText`、
    /// `font-family` 的 UA 默认、`font-size` 的 medium）取本层固定选择：
    /// 黑色、serif、16px。
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
        )
    }
}

/// 全部属性的固定枚举顺序。
const PROPERTY_ORDER: [PropertyId; 8] = [
    PropertyId::Display,
    PropertyId::Color,
    PropertyId::BackgroundColor,
    PropertyId::FontSize,
    PropertyId::FontWeight,
    PropertyId::FontStyle,
    PropertyId::FontFamily,
    PropertyId::TextAlign,
];

/// 级联排序键（CSS Cascade 5 §6.4，仅 author origin）。
///
/// 字典序即优先级：`!important` 压过普通声明；同为普通声明时内联样式排在
/// 样式表声明之后（§6.4 style attribute）；然后按特异度、规则顺序、声明顺序。
/// `bool` 的 `Ord` 恰好满足 false < true。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CascadeKey {
    important: bool,
    inline: bool,
    specificity: u32,
    rule_order: usize,
    declaration_order: usize,
}

/// 对单个元素做级联，产出 computed style。
///
/// `parent` 是树序父元素的 computed style（元素没有元素祖先时为 `None`）。
/// 各属性：级联胜者 → CSS-wide 关键字 → 继承 → initial。
#[must_use]
pub fn cascade(
    document: &Document,
    element: NodeId,
    stylesheets: &[&Stylesheet],
    parent: Option<&ComputedStyle>,
) -> ComputedStyle {
    let mut matched: Vec<(CascadeKey, Declaration)> = Vec::new();

    // 规则顺序跨样式表连续编号：样式表按传入顺序、规则按源顺序
    for (rule_order, rule) in stylesheets
        .iter()
        .flat_map(|sheet| sheet.rules())
        .enumerate()
    {
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
                        important: declaration.important,
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
                    important: declaration.important,
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
) -> HashMap<NodeId, ComputedStyle> {
    let mut styles = HashMap::new();
    walk(document, document.root(), None, stylesheets, &mut styles);
    styles
}

fn walk(
    document: &Document,
    node: NodeId,
    parent: Option<&ComputedStyle>,
    stylesheets: &[&Stylesheet],
    out: &mut HashMap<NodeId, ComputedStyle>,
) {
    for child in document.children(node) {
        if matches!(document.node(child), Some(NodeKind::Element(_))) {
            let style = cascade(document, child, stylesheets, parent);
            walk(document, child, Some(&style), stylesheets, out);
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
                }
            }
            // 声明值与属性不匹配只可能来自程序错误；保持 initial 值
            _ => {}
        }
    }
    style
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
    use crate::test_support::{build_document, find_first_by_name};

    fn style_of(html: &str, sheets: &[&str], name: &str) -> ComputedStyle {
        let document = build_document(html);
        let parsed: Vec<Stylesheet> = sheets.iter().map(|css| Stylesheet::parse(css)).collect();
        let refs: Vec<&Stylesheet> = parsed.iter().collect();
        let element = find_first_by_name(&document, name).expect("element");
        cascade(&document, element, &refs, None)
    }

    #[test]
    fn no_declarations_fall_back_to_inheritance_and_initial() {
        let document = build_document("<div style=\"color: red\"><p>x</p></div>");
        let refs: Vec<&Stylesheet> = vec![];
        let div = find_first_by_name(&document, "div").expect("div");
        let div_style = cascade(&document, div, &refs, None);
        assert_eq!(div_style.color, Rgba::opaque(255, 0, 0), "内联样式生效");
        assert_eq!(
            div_style.background_color,
            Rgba::transparent(),
            "非继承属性取 initial"
        );

        let p = find_first_by_name(&document, "p").expect("p");
        let p_style = cascade(&document, p, &refs, Some(&div_style));
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
        let div_style = cascade(&document, div, &[&parsed], None);
        assert_eq!(div_style.font_size, 20.0);
        let p = find_first_by_name(&document, "p").expect("p");
        let p_style = cascade(&document, p, &[&parsed], Some(&div_style));
        assert_eq!(p_style.font_size, 30.0);

        // 继承的是计算值：孙辈再乘 em 时基于 30px
        let span = document.create_node(nexty_dom::NodeKind::Element(nexty_dom::ElementData {
            name: "span".into(),
            namespace: nexty_dom::Namespace::Html,
            attributes: Vec::new(),
        }));
        document.insert_node(span, p, None);
        let span_style = cascade(&document, span, &[&parsed], Some(&p_style));
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
        );
        let p_style = cascade(
            &document,
            find_first_by_name(&document, "p").unwrap(),
            &[&parsed],
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
        );
        let p_style = cascade(
            &document,
            find_first_by_name(&document, "p").unwrap(),
            &[&parsed],
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
            );
            assert_eq!(div_style.font_weight, parent_weight);
            let p_style = cascade(
                &document,
                find_first_by_name(&document, "p").unwrap(),
                &[&parsed],
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
        );
        let p_style = cascade(
            &document,
            find_first_by_name(&document, "p").unwrap(),
            &[&parsed],
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
        let styles = compute_document_styles(&document, &[&sheet]);

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
        );
        // :is(#a) 特异度 (1,0,0) 压过其余两条
        assert_eq!(style.color, Rgba::opaque(0, 128, 0));

        let sheet = Stylesheet::parse("p { color: red } :where(p.x) { color: blue }");
        let style = cascade(
            &document,
            find_first_by_name(&document, "p").unwrap(),
            &[&sheet],
            None,
        );
        // :where(p.x) 特异度为 0，p (0,0,1) 按源顺序在后仍胜出
        assert_eq!(style.color, Rgba::opaque(255, 0, 0));
    }
}
