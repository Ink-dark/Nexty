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
use crate::value::{DeclaredValue, PropertyId, parse_declared_value};
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
    for declaration in RuleBodyParser::new(input, &mut declaration_parser).flatten() {
        declarations.push(declaration);
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
struct DeclarationListParser;

impl<'i> DeclarationParser<'i> for DeclarationListParser {
    type Declaration = Declaration;
    type Error = ();

    fn parse_value<'t>(
        &mut self,
        name: cssparser::CowRcStr<'i>,
        input: &mut Parser<'i>,
        _declaration_start: &cssparser::ParserState,
    ) -> Result<Declaration, ParseError<Self::Error>> {
        let Some(property) = PropertyId::from_name(&name) else {
            // 未知属性（含 custom property）按无效声明处理
            return Err(ParseError::<()>::unexpected_token());
        };
        let (value, important) = parse_declared_value(property, input)
            .map_err(|_| ParseError::<()>::unexpected_token())?;
        Ok(Declaration {
            property,
            value,
            important,
        })
    }
}

impl<'i> QualifiedRuleParser<'i> for DeclarationListParser {
    type Prelude = ();
    type QualifiedRule = Declaration;
    type Error = ();
}

impl<'i> AtRuleParser<'i> for DeclarationListParser {
    type Prelude = ();
    type AtRule = Declaration;
    type Error = ();
}

impl<'i> RuleBodyItemParser<'i, Declaration, ()> for DeclarationListParser {
    fn parse_declarations(&self) -> bool {
        true
    }

    fn parse_qualified(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::{CssWideKeyword, FontSizeValue, PropertyValue, Rgba};

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
}
