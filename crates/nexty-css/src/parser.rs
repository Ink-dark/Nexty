//! 样式表解析：`cssparser` 的语法引擎 + 逐属性值解析。
//!
//! 行为 ground truth：[CSS Syntax Module Level 3 §5](https://drafts.csswg.org/css-syntax-3/#parsing)。
//!
//! - 顶层按 rule list 消费：样式规则（qualified rule）产出 [`StyleRule`]，
//!   at-rule 一律拒绝并由 cssparser 按规范错误恢复整块跳过（§5.4）；文档开头
//!   的 `@charset` 自动忽略。
//! - 样式规则 prelude 是选择器列表：任一选择器无效 → 整条规则丢弃（§5.3.2）。
//! - 声明块逐条解析：单条声明无效只丢弃该条，其余保留（§5.5）。
//! - 未知属性按无效声明处理；未知 at-rule（`@media` / `@import` / …）整块跳过。
//!
//! 已知偏差：`@media` / `@supports` 等内层样式规则本轮不求值（整体跳过，
//! 待后续轮次按需展开）。

use crate::selector::{NextySelector, parse_selector_list};
use crate::value::{
    BorderColorValue, BorderStyle, BorderWidthValue, DeclaredValue, PropertyId, PropertyValue,
    parse_border_color_value, parse_border_style_value, parse_border_width_value,
    parse_css_wide_keyword, parse_declared_value, parse_margin_value, parse_padding_value,
};
use cssparser::{
    AtRuleParser, DeclarationParser, ParseError, Parser, QualifiedRuleParser, RuleBodyItemParser,
    RuleBodyParser, StyleSheetParser,
};

/// 一份已解析的样式表，规则保持源顺序。
#[derive(Debug, Clone, Default)]
pub struct Stylesheet {
    rules: Vec<StyleRule>,
}

impl Stylesheet {
    /// 解析样式表文本。
    ///
    /// 输入按 UTF-8 处理（调用方负责按 HTTP/文档编码解码）。解析错误按
    /// CSS Syntax §5 的恢复规则就地吞掉：无效的规则与声明不会出现在结果里，
    /// 也不上报。
    #[must_use]
    pub fn parse(css: &str) -> Stylesheet {
        let mut rule_parser = StylesheetRuleParser;
        let mut input = Parser::new(css);
        let mut rules = Vec::new();
        for rule in StyleSheetParser::new(&mut input, &mut rule_parser).flatten() {
            rules.push(rule);
        }
        Stylesheet { rules }
    }

    /// 按源顺序返回全部样式规则。
    #[must_use]
    pub fn rules(&self) -> &[StyleRule] {
        &self.rules
    }
}

/// 一条样式规则：选择器列表 + 声明块。
#[derive(Debug, Clone)]
pub struct StyleRule {
    selectors: Vec<NextySelector>,
    selector_text: String,
    declarations: Vec<Declaration>,
}

impl StyleRule {
    /// 规则的选择器文本（序列化形式，与源文本等价但可能经过规范化）。
    #[must_use]
    pub fn selector_text(&self) -> &str {
        &self.selector_text
    }

    /// 声明列表（保持源顺序）。
    #[must_use]
    pub fn declarations(&self) -> &[Declaration] {
        &self.declarations
    }

    /// 供 cascade 使用的已解析选择器（含逐条特异度）。
    pub(crate) fn selectors(&self) -> &[NextySelector] {
        &self.selectors
    }
}

/// 一条属性声明。
#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    /// 属性。
    pub property: PropertyId,
    /// 解析后的声明值。
    pub value: DeclaredValue,
    /// 是否带 `!important`。
    pub important: bool,
}

/// 顶层样式表规则解析：只接受样式规则（qualified rule）。
struct StylesheetRuleParser;

impl<'i> QualifiedRuleParser<'i> for StylesheetRuleParser {
    type Prelude = Vec<NextySelector>;
    type QualifiedRule = StyleRule;
    type Error = ();

    fn parse_prelude(
        &mut self,
        input: &mut Parser<'i>,
    ) -> Result<Self::Prelude, ParseError<Self::Error>> {
        parse_selector_list(input).map_err(|_| ParseError::<()>::unexpected_token())
    }

    fn parse_block<'t>(
        &mut self,
        prelude: Self::Prelude,
        _start: &cssparser::ParserState,
        input: &mut Parser<'i>,
    ) -> Result<Self::QualifiedRule, ParseError<Self::Error>> {
        let selector_text = prelude
            .iter()
            .map(NextySelector::to_css_string)
            .collect::<Vec<_>>()
            .join(", ");
        Ok(StyleRule {
            selectors: prelude,
            selector_text,
            declarations: parse_declaration_block(input),
        })
    }
}

// 顶层不接受任何 at-rule：默认实现全部拒绝，
// StyleSheetParser 按规范把无效 at-rule 整块消费掉。
impl<'i> AtRuleParser<'i> for StylesheetRuleParser {
    type Prelude = ();
    type AtRule = StyleRule;
    type Error = ();
}

/// 解析一个声明块（`{ ... }` 内容或内联 style 属性值）。
///
/// 无效声明（含未知属性）就地跳过，其余保留（CSS Syntax §5.5）。
pub(crate) fn parse_declaration_block(input: &mut Parser<'_>) -> Vec<Declaration> {
    let mut declaration_parser = DeclarationListParser;
    let mut declarations = Vec::new();
    for group in RuleBodyParser::new(input, &mut declaration_parser).flatten() {
        declarations.extend(group);
    }
    declarations
}

/// 解析一段内联 style 属性为声明列表。
#[must_use]
pub fn parse_inline_style(style_attribute: &str) -> Vec<Declaration> {
    let mut input = Parser::new(style_attribute);
    parse_declaration_block(&mut input)
}

/// 声明块内容解析器：只接受声明，拒绝嵌套规则与 at-rule。
///
/// 一条声明可展开为多条（简写属性按 CSS Cascade 5 §8 展开为 longhand，
/// 保持书写顺序）。
struct DeclarationListParser;

/// [`DeclarationParser::Declaration`] 的承载类型：一条简写可产出多条声明。
type DeclarationGroup = Vec<Declaration>;

impl<'i> DeclarationParser<'i> for DeclarationListParser {
    type Declaration = DeclarationGroup;
    type Error = ();

    fn parse_value<'t>(
        &mut self,
        name: cssparser::CowRcStr<'i>,
        input: &mut Parser<'i>,
        _declaration_start: &cssparser::ParserState,
    ) -> Result<DeclarationGroup, ParseError<Self::Error>> {
        // 简写属性：展开为 longhand（CSS Cascade 5 §8 shorthand expansion）
        if let Some(expanded) = expand_shorthand(&name, input) {
            let (values, important) = expanded.map_err(|_| ParseError::<()>::unexpected_token())?;
            return Ok(values
                .into_iter()
                .map(|(property, value)| Declaration {
                    property,
                    value,
                    important,
                })
                .collect());
        }
        let Some(property) = PropertyId::from_name(&name) else {
            // 未知属性（含 custom property）按无效声明处理
            return Err(ParseError::<()>::unexpected_token());
        };
        let (value, important) = parse_declared_value(property, input)
            .map_err(|_| ParseError::<()>::unexpected_token())?;
        Ok(vec![Declaration {
            property,
            value,
            important,
        }])
    }
}

impl<'i> QualifiedRuleParser<'i> for DeclarationListParser {
    type Prelude = ();
    type QualifiedRule = DeclarationGroup;
    type Error = ();
}

impl<'i> AtRuleParser<'i> for DeclarationListParser {
    type Prelude = ();
    type AtRule = DeclarationGroup;
    type Error = ();
}

impl<'i> RuleBodyItemParser<'i, DeclarationGroup, ()> for DeclarationListParser {
    fn parse_declarations(&self) -> bool {
        true
    }

    fn parse_qualified(&self) -> bool {
        false
    }
}

/// 简写展开结果：展开出的 (longhand, 值) 列表与 `!important` 标记。
type ShorthandExpansion = (Vec<(PropertyId, DeclaredValue)>, bool);

/// 简写属性展开（CSS Cascade 5 §8）。
///
/// 返回 `None` 表示 `name` 不是本层支持的简写；`Some(Err)` 表示简写值无效
/// （整条声明丢弃）。CSS-wide 关键字作用于全部 longhand。
fn expand_shorthand(name: &str, input: &mut Parser<'_>) -> Option<Result<ShorthandExpansion, ()>> {
    // 先确认是简写名，再试 CSS-wide 关键字（否则关键字会被误吞且不复位）
    let is_border = name.eq_ignore_ascii_case("border");
    let edges = box_shorthand_edges(name);
    if !is_border && edges.is_none() {
        return None;
    }

    if let Ok(keyword) = input.try_parse(parse_css_wide_keyword) {
        let important = input.try_parse(cssparser::parse_important).is_ok();
        if input.expect_exhausted().is_err() {
            return Some(Err(()));
        }
        let longhands = if is_border {
            border_longhands()
        } else {
            match edges {
                Some(BoxEdges::Margin) => margin_longhands(),
                Some(BoxEdges::Padding) => padding_longhands(),
                Some(BoxEdges::BorderWidth) => border_width_longhands(),
                Some(BoxEdges::BorderStyle) => border_style_longhands(),
                Some(BoxEdges::BorderColor) => border_color_longhands(),
                None => border_longhands(),
            }
        };
        return Some(Ok((
            longhands
                .into_iter()
                .map(|property| (property, DeclaredValue::CssWide(keyword)))
                .collect(),
            important,
        )));
    }

    if is_border {
        return Some(parse_border_shorthand(input));
    }
    let expanded = match edges? {
        BoxEdges::Margin => expand_box_values(input, parse_margin_value)
            .map(|values| zip_edges(values, margin_longhands(), PropertyValue::Margin)),
        BoxEdges::Padding => expand_box_values(input, parse_padding_value)
            .map(|values| zip_edges(values, padding_longhands(), PropertyValue::Padding)),
        BoxEdges::BorderWidth => expand_box_values(input, parse_border_width_value)
            .map(|values| zip_edges(values, border_width_longhands(), PropertyValue::BorderWidth)),
        BoxEdges::BorderStyle => expand_box_values(input, parse_border_style_value)
            .map(|values| zip_edges(values, border_style_longhands(), PropertyValue::BorderStyle)),
        BoxEdges::BorderColor => expand_box_values(input, parse_border_color_value)
            .map(|values| zip_edges(values, border_color_longhands(), PropertyValue::BorderColor)),
    }
    .ok()?;
    let important = input.try_parse(cssparser::parse_important).is_ok();
    if input.expect_exhausted().is_err() {
        return Some(Err(()));
    }
    Some(Ok((expanded, important)))
}

/// 四边值与对应 longhand 组装为声明（统一包成 [`DeclaredValue::Typed`]）。
fn zip_edges<T>(
    values: (T, T, T, T),
    longhands: Vec<PropertyId>,
    wrap: impl Fn(T) -> PropertyValue,
) -> Vec<(PropertyId, DeclaredValue)> {
    let (top, right, bottom, left) = values;
    [top, right, bottom, left]
        .into_iter()
        .zip(longhands)
        .map(|(value, property)| (property, DeclaredValue::Typed(wrap(value))))
        .collect()
}

/// 盒模型四边简写的属性组。
#[derive(Debug, Clone, Copy)]
enum BoxEdges {
    Margin,
    Padding,
    BorderWidth,
    BorderStyle,
    BorderColor,
}

fn box_shorthand_edges(name: &str) -> Option<BoxEdges> {
    if name.eq_ignore_ascii_case("margin") {
        Some(BoxEdges::Margin)
    } else if name.eq_ignore_ascii_case("padding") {
        Some(BoxEdges::Padding)
    } else if name.eq_ignore_ascii_case("border-width") {
        Some(BoxEdges::BorderWidth)
    } else if name.eq_ignore_ascii_case("border-style") {
        Some(BoxEdges::BorderStyle)
    } else if name.eq_ignore_ascii_case("border-color") {
        Some(BoxEdges::BorderColor)
    } else {
        None
    }
}

/// 1–4 值的盒模型展开（CSS 2.1 §8.3：顺时针 top → right → bottom → left）。
fn expand_box_values<T: Clone>(
    input: &mut Parser<'_>,
    parse_one: impl Fn(&mut Parser<'_>) -> Result<T, ()>,
) -> Result<(T, T, T, T), ()> {
    let top = parse_one(input)?;
    let second = input.try_parse(|i| parse_one(i)).ok();
    let third = input.try_parse(|i| parse_one(i)).ok();
    let fourth = input.try_parse(|i| parse_one(i)).ok();
    match (second, third, fourth) {
        (None, None, None) => Ok((top.clone(), top.clone(), top.clone(), top)),
        (Some(horizontal), None, None) => Ok((top.clone(), horizontal.clone(), top, horizontal)),
        (Some(horizontal), Some(bottom), None) => Ok((top, horizontal.clone(), bottom, horizontal)),
        (Some(right), Some(bottom), Some(left)) => Ok((top, right, bottom, left)),
        _ => Err(()),
    }
}

/// `border` 简写：`<line-width> || <line-style> || <color>`（任意顺序、
/// 至少一项；缺省项取 initial：medium / none / currentcolor）。
fn parse_border_shorthand(
    input: &mut Parser<'_>,
) -> Result<(Vec<(PropertyId, DeclaredValue)>, bool), ()> {
    let mut width = None;
    let mut style = None;
    let mut color = None;
    loop {
        if width.is_none()
            && let Ok(value) = input.try_parse(parse_border_width_value)
        {
            width = Some(value);
            continue;
        }
        if style.is_none()
            && let Ok(value) = input.try_parse(parse_border_style_value)
        {
            style = Some(value);
            continue;
        }
        if color.is_none()
            && let Ok(value) = input.try_parse(parse_border_color_value)
        {
            color = Some(value);
            continue;
        }
        break;
    }
    if width.is_none() && style.is_none() && color.is_none() {
        return Err(());
    }
    let important = input.try_parse(cssparser::parse_important).is_ok();
    if input.expect_exhausted().is_err() {
        return Err(());
    }

    let width = width.unwrap_or(BorderWidthValue::Medium);
    let style = style.unwrap_or(BorderStyle::None);
    let color = color.unwrap_or(BorderColorValue::CurrentColor);

    let widths = border_width_longhands();
    let styles = border_style_longhands();
    let colors = border_color_longhands();
    let mut declarations = Vec::with_capacity(12);
    for property in widths {
        declarations.push((
            property,
            DeclaredValue::Typed(PropertyValue::BorderWidth(width)),
        ));
    }
    for property in styles {
        declarations.push((
            property,
            DeclaredValue::Typed(PropertyValue::BorderStyle(style)),
        ));
    }
    for property in colors {
        declarations.push((
            property,
            DeclaredValue::Typed(PropertyValue::BorderColor(color)),
        ));
    }
    Ok((declarations, important))
}

fn margin_longhands() -> Vec<PropertyId> {
    vec![
        PropertyId::MarginTop,
        PropertyId::MarginRight,
        PropertyId::MarginBottom,
        PropertyId::MarginLeft,
    ]
}

fn padding_longhands() -> Vec<PropertyId> {
    vec![
        PropertyId::PaddingTop,
        PropertyId::PaddingRight,
        PropertyId::PaddingBottom,
        PropertyId::PaddingLeft,
    ]
}

fn border_width_longhands() -> Vec<PropertyId> {
    vec![
        PropertyId::BorderTopWidth,
        PropertyId::BorderRightWidth,
        PropertyId::BorderBottomWidth,
        PropertyId::BorderLeftWidth,
    ]
}

fn border_style_longhands() -> Vec<PropertyId> {
    vec![
        PropertyId::BorderTopStyle,
        PropertyId::BorderRightStyle,
        PropertyId::BorderBottomStyle,
        PropertyId::BorderLeftStyle,
    ]
}

fn border_color_longhands() -> Vec<PropertyId> {
    vec![
        PropertyId::BorderTopColor,
        PropertyId::BorderRightColor,
        PropertyId::BorderBottomColor,
        PropertyId::BorderLeftColor,
    ]
}

fn border_longhands() -> Vec<PropertyId> {
    [
        border_width_longhands(),
        border_style_longhands(),
        border_color_longhands(),
    ]
    .concat()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::{
        BorderColorValue, BorderStyle, BorderWidthValue, CssWideKeyword, FontSizeValue,
        LineHeightValue, MarginValue, PaddingValue, PropertyValue, Rgba, SizeValue,
    };

    fn rule_texts(sheet: &Stylesheet) -> Vec<&str> {
        sheet.rules().iter().map(StyleRule::selector_text).collect()
    }

    fn declaration_of(sheet: &Stylesheet, rule_index: usize, property: PropertyId) -> &Declaration {
        sheet.rules()[rule_index]
            .declarations()
            .iter()
            .find(|declaration| declaration.property == property)
            .unwrap_or_else(|| panic!("missing declaration {property:?}"))
    }

    #[test]
    fn parses_rules_in_source_order() {
        let sheet =
            Stylesheet::parse("p { color: red; } .a { font-size: 16px } #b { display: none; }");
        assert_eq!(rule_texts(&sheet), vec!["p", ".a", "#b"]);
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::Color).value,
            DeclaredValue::Typed(PropertyValue::Color(Rgba::opaque(255, 0, 0)))
        );
        assert_eq!(
            declaration_of(&sheet, 1, PropertyId::FontSize).value,
            DeclaredValue::Typed(PropertyValue::FontSize(FontSizeValue::Px(16.0)))
        );
    }

    #[test]
    fn selector_list_keeps_rule_once_with_serialized_text() {
        let sheet = Stylesheet::parse("h1, h2, .title { color: blue; }");
        assert_eq!(rule_texts(&sheet), vec!["h1, h2, .title"]);
    }

    #[test]
    fn invalid_selector_drops_whole_rule() {
        // CSS Syntax §5.3.2：qualified rule prelude 无效 → 整条规则丢弃，
        // 但后续规则不受影响
        let sheet =
            Stylesheet::parse("p > { color: red } p { color: blue } , {} p b { display: none }");
        assert_eq!(rule_texts(&sheet), vec!["p", "p b"]);
    }

    #[test]
    fn at_rules_are_skipped_including_blocks() {
        // @charset 由 StyleSheetParser 直接忽略；未知 at-rule 整块跳过。
        // 注意：顶层游离分号按现行 CSS Syntax §5.5.3 并入下一条 qualified rule
        // 的 prelude（"semicolons don't end them"），故这里用空白分隔规则。
        let sheet = Stylesheet::parse(
            "@charset \"utf-8\";\
             @media screen { p { color: red } div { color: green } }\
             @import url(\"x.css\");\
             @unknown foo { p { color: pink } }\
             p { color: blue }",
        );
        assert_eq!(rule_texts(&sheet), vec!["p"]);
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::Color).value,
            DeclaredValue::Typed(PropertyValue::Color(Rgba::opaque(0, 0, 255)))
        );
    }

    #[test]
    fn invalid_declarations_are_recovered_individually() {
        // CSS Syntax §5.5：单条声明无效 → 跳过该条，保留其余
        let sheet = Stylesheet::parse(
            "p { color: red;; colr: blue; font-size: ; display: none; color: ; grid-area: x }",
        );
        let declarations = &sheet.rules()[0].declarations;
        let properties: Vec<PropertyId> = declarations.iter().map(|d| d.property).collect();
        assert_eq!(properties, vec![PropertyId::Color, PropertyId::Display]);
    }

    #[test]
    fn unknown_property_is_dropped_but_custom_property_syntax_recovered() {
        let sheet = Stylesheet::parse("p { --custom: 1px solid; color: green }");
        assert_eq!(sheet.rules()[0].declarations().len(), 1);
        assert_eq!(
            sheet.rules()[0].declarations()[0].property,
            PropertyId::Color
        );
    }

    #[test]
    fn important_flag_is_parsed() {
        let sheet = Stylesheet::parse("p { color: red !important; font-size: 10px }");
        assert!(declaration_of(&sheet, 0, PropertyId::Color).important);
        assert!(!declaration_of(&sheet, 0, PropertyId::FontSize).important);
    }

    #[test]
    fn css_wide_keywords_survive_parsing() {
        let sheet = Stylesheet::parse("p { color: inherit; font-size: initial !important }");
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::Color).value,
            DeclaredValue::CssWide(CssWideKeyword::Inherit)
        );
        let font_size = declaration_of(&sheet, 0, PropertyId::FontSize);
        assert_eq!(
            font_size.value,
            DeclaredValue::CssWide(CssWideKeyword::Initial)
        );
        assert!(font_size.important);
    }

    #[test]
    fn comments_and_cdo_cdc_are_ignored() {
        let sheet = Stylesheet::parse(
            "/* header */ <!-- p { color: red } --> /* tail */ p em { color: blue }",
        );
        assert_eq!(rule_texts(&sheet), vec!["p", "p em"]);
    }

    #[test]
    fn inline_style_parses_declarations() {
        let declarations = parse_inline_style("color: red; font-size: 2em !important; bogus: 1");
        assert_eq!(declarations.len(), 2);
        assert_eq!(declarations[0].property, PropertyId::Color);
        assert!(!declarations[0].important);
        assert_eq!(declarations[1].property, PropertyId::FontSize);
        assert!(declarations[1].important);
    }

    #[test]
    fn empty_and_garbage_input_yield_no_rules() {
        assert!(Stylesheet::parse("").rules().is_empty());
        assert!(Stylesheet::parse("}{{{{").rules().is_empty());
        assert!(Stylesheet::parse("@").rules().is_empty());
    }

    fn property_values(sheet: &Stylesheet, rule: usize) -> Vec<(PropertyId, &DeclaredValue)> {
        sheet.rules()[rule]
            .declarations()
            .iter()
            .map(|declaration| (declaration.property, &declaration.value))
            .collect()
    }

    fn length(value: f32) -> DeclaredValue {
        DeclaredValue::Typed(PropertyValue::Margin(MarginValue::Length(value)))
    }

    #[test]
    fn margin_shorthand_expands_clockwise() {
        let one = Stylesheet::parse("p { margin: 10px }");
        let values = property_values(&one, 0);
        assert_eq!(values.len(), 4);
        for (property, value) in values {
            assert!(matches!(
                property,
                PropertyId::MarginTop
                    | PropertyId::MarginRight
                    | PropertyId::MarginBottom
                    | PropertyId::MarginLeft
            ));
            assert_eq!(value, &length(10.0), "{property:?}");
        }

        let two = Stylesheet::parse("p { margin: 1px 2px }");
        let values = property_values(&two, 0);
        assert_eq!(
            values,
            vec![
                (PropertyId::MarginTop, &length(1.0)),
                (PropertyId::MarginRight, &length(2.0)),
                (PropertyId::MarginBottom, &length(1.0)),
                (PropertyId::MarginLeft, &length(2.0)),
            ]
        );

        let three = Stylesheet::parse("p { margin: 1px 2px 3px }");
        let values = property_values(&three, 0);
        assert_eq!(values[1].1, &length(2.0), "三值时左右取第二个");
        assert_eq!(values[2].1, &length(3.0));

        let four = Stylesheet::parse("p { margin: 1px 2px 3px 4px }");
        let values = property_values(&four, 0);
        assert_eq!(
            values,
            vec![
                (PropertyId::MarginTop, &length(1.0)),
                (PropertyId::MarginRight, &length(2.0)),
                (PropertyId::MarginBottom, &length(3.0)),
                (PropertyId::MarginLeft, &length(4.0)),
            ]
        );
    }

    #[test]
    fn margin_accepts_negative_auto_percent() {
        let sheet = Stylesheet::parse("p { margin: -5px auto 50% }");
        let values = property_values(&sheet, 0);
        assert_eq!(values[0].1, &length(-5.0), "margin 可为负");
        assert_eq!(
            values[1].1,
            &DeclaredValue::Typed(PropertyValue::Margin(MarginValue::Auto))
        );
        assert_eq!(
            values[2].1,
            &DeclaredValue::Typed(PropertyValue::Margin(MarginValue::Percent(0.5)))
        );
    }

    #[test]
    fn padding_rejects_auto_and_negative() {
        let sheet = Stylesheet::parse("p { padding: 10px 5%; bogus: x }");
        let values = property_values(&sheet, 0);
        assert_eq!(values.len(), 4);
        assert_eq!(
            values[0].1,
            &DeclaredValue::Typed(PropertyValue::Padding(PaddingValue::Length(10.0)))
        );
        assert_eq!(
            values[3].1,
            &DeclaredValue::Typed(PropertyValue::Padding(PaddingValue::Percent(0.05)))
        );

        // auto / 负值 → 整条声明丢弃（规则保留，声明为空）
        assert!(
            Stylesheet::parse("p { padding: auto }").rules()[0]
                .declarations()
                .is_empty()
        );
        assert!(
            Stylesheet::parse("p { padding: -5px }").rules()[0]
                .declarations()
                .is_empty()
        );
    }

    #[test]
    fn border_shorthand_expands_to_twelve_longhands() {
        let sheet = Stylesheet::parse("p { border: 1px solid red }");
        let declarations = &sheet.rules()[0].declarations();
        assert_eq!(declarations.len(), 12);
        assert!(
            declarations
                .iter()
                .filter(|declaration| matches!(
                    declaration.property,
                    PropertyId::BorderTopWidth
                        | PropertyId::BorderRightWidth
                        | PropertyId::BorderBottomWidth
                        | PropertyId::BorderLeftWidth
                ))
                .all(|declaration| declaration.value
                    == DeclaredValue::Typed(PropertyValue::BorderWidth(BorderWidthValue::Length(
                        1.0
                    ))))
        );
        assert!(declarations.iter().all(|declaration| !matches!(
            declaration.property,
            PropertyId::BorderTopColor
        ) || declaration.value
            == DeclaredValue::Typed(PropertyValue::BorderColor(BorderColorValue::Rgba(
                Rgba::opaque(255, 0, 0)
            )))));

        // 缺省成分取 initial：medium / none / currentcolor
        let sheet = Stylesheet::parse("p { border: solid }");
        let declarations = &sheet.rules()[0].declarations();
        assert_eq!(declarations.len(), 12);
        assert!(declarations.iter().any(|declaration| declaration.value
            == DeclaredValue::Typed(PropertyValue::BorderWidth(BorderWidthValue::Medium))));
        assert!(declarations.iter().any(|declaration| declaration.value
            == DeclaredValue::Typed(PropertyValue::BorderColor(BorderColorValue::CurrentColor))));
    }

    #[test]
    fn side_shorthands_expand_independently() {
        let sheet = Stylesheet::parse(
            "p { border-width: 1px 2px; border-style: dashed dotted; border-color: currentcolor }",
        );
        let declarations = &sheet.rules()[0].declarations();
        assert_eq!(declarations.len(), 12);
        assert!(declarations.iter().any(|declaration| declaration.value
            == DeclaredValue::Typed(PropertyValue::BorderWidth(BorderWidthValue::Length(2.0)))));
        assert!(declarations.iter().any(|declaration| declaration.value
            == DeclaredValue::Typed(PropertyValue::BorderStyle(BorderStyle::Dotted))));
        assert!(declarations.iter().any(|declaration| declaration.value
            == DeclaredValue::Typed(PropertyValue::BorderColor(BorderColorValue::CurrentColor))));
    }

    #[test]
    fn css_wide_keyword_on_shorthand_applies_to_all_longhands() {
        let sheet = Stylesheet::parse("p { margin: inherit !important }");
        let declarations = &sheet.rules()[0].declarations();
        assert_eq!(declarations.len(), 4);
        for declaration in declarations.iter() {
            assert_eq!(
                declaration.value,
                DeclaredValue::CssWide(CssWideKeyword::Inherit)
            );
            assert!(declaration.important);
        }
    }

    #[test]
    fn width_height_line_height_parse() {
        let sheet = Stylesheet::parse("p { width: 50%; height: auto; line-height: 1.5 }");
        let values = property_values(&sheet, 0);
        assert_eq!(values.len(), 3);
        assert_eq!(
            values[0].1,
            &DeclaredValue::Typed(PropertyValue::Size(SizeValue::Percent(0.5)))
        );
        assert_eq!(
            values[1].1,
            &DeclaredValue::Typed(PropertyValue::Size(SizeValue::Auto))
        );
        assert_eq!(
            values[2].1,
            &DeclaredValue::Typed(PropertyValue::LineHeight(LineHeightValue::Number(1.5)))
        );

        // 负宽度、负行高、未知值 → 声明无效（丢弃，规则保留）
        for css in [
            "p { width: -5px }",
            "p { line-height: -1 }",
            "p { line-height: 20zz }",
        ] {
            assert!(
                Stylesheet::parse(css).rules()[0].declarations().is_empty(),
                "{css} should drop the declaration"
            );
        }
    }
}
