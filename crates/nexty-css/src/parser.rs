//! 样式表解析：`cssparser` 的语法引擎 + 逐属性值解析。
//!
//! 行为 ground truth：[CSS Syntax Module Level 3 §5](https://drafts.csswg.org/css-syntax-3/#parsing)。
//!
//! - 顶层按 rule list 消费：样式规则（qualified rule）产出 [`StyleRule`]，
//!   at-rule 由 cssparser 按规范错误恢复整块跳过（§5.4）；文档开头
//!   的 `@charset` 自动忽略。
//! - 样式规则 prelude 是选择器列表：任一选择器无效 → 整条规则丢弃（§5.3.2）。
//! - 声明块逐条解析：单条声明无效只丢弃该条，其余保留（§5.5）。
//! - 未知属性按无效声明处理；未知 at-rule（`@import` / `@supports` / …）整块跳过。
//! - `@media` 按视口条件求值（[`Stylesheet::parse_with_viewport`]）：命中的
//!   规则按源顺序就地展开参与级联，未命中或未提供视口的整块丢弃。
//! - `@font-face` 按简化档解析为 [`FontFaceRule`]（CSS Fonts 4 §4），由
//!   [`Stylesheet::font_faces`] 按源顺序携带；必需描述符 `font-family`
//!   缺失或非法 → 整条规则丢弃；`src` 单项非法 → 跳过该源，其余保留。
//!
//! 已知偏差：`@supports` 等其余 at-rule 不求值（整体跳过）；媒体特性只认
//! `min/max-width/height`（px）；嵌套 at-rule 不支持（含 `@media` 内的
//! `@font-face`）。
//!
//! `@font-face` 简化档偏差：描述符只认 `font-family`（必需） / `src` /
//! `font-weight`（单值） / `font-style` / `font-display`，其余
//! （`unicode-range` / `font-stretch` / `font-variant` 等）按无效描述符丢弃；
//! `font-style` 只认 `normal` / `italic`（`oblique` 不识别）；`font-weight`
//! 单值数字或 `normal` / `bold` 关键字，非整数字重四舍五入为整数存储
//! （下游 face 权重类型为整数，实际字体 OS/2 权重亦为整数）；`font-display`
//! 只解析存储，加载行为恒「就绪后重排一次」（swap 简化，五种取值行为相同）；
//! `src` 的 `tech()` 提示与裸字符串形式不识别（含 `tech()` 的源按无效跳过），
//! `url()` 为空视为无效源，`format()` 提示只存不校验（woff/woff2 等格式
//! 判断由字体加载层决策）；未加引号的多词族名（`font-family: My Font`、
//! `local(Helvetica Neue)`）不支持，引号形式可用。
//!
//! **Feature**：本模块受 `stylesheets` feature 控制（由 lib.rs 装配）。`StyleRule`
//! 的 `selectors` / `specificity` 字段由 `cascade` 模块读取，故关闭 cascade 时
//! 它们暂无读取方——此时按模块整体抑制 dead_code。

#![cfg_attr(not(feature = "cascade"), allow(dead_code))]

use crate::selector::{NextySelector, parse_selector_list};
use crate::value::{
    BackgroundImageValue, BorderColorValue, BorderStyle, BorderWidthValue, DeclaredValue,
    PropertyId, PropertyValue, Rgba, TextDecorationColorValue, TextDecorationLineValue,
    TextDecorationStyleValue, parse_background_image_value, parse_border_color_value,
    parse_border_radius_value, parse_border_style_value, parse_border_width_value, parse_color,
    parse_css_wide_keyword, parse_declared_value, parse_font_shorthand, parse_gap_value,
    parse_inset_value, parse_margin_value, parse_padding_value, parse_text_decoration_color_value,
    parse_text_decoration_line_value, parse_text_decoration_style_value,
};
use cssparser::{
    AtRuleParser, DeclarationParser, ParseError, Parser, QualifiedRuleParser, RuleBodyItemParser,
    RuleBodyParser, StyleSheetParser, Token,
};

/// 一份已解析的样式表，规则保持源顺序。
#[derive(Debug, Clone, Default)]
pub struct Stylesheet {
    rules: Vec<StyleRule>,
    /// `@font-face` 规则（源顺序；必需描述符非法的规则已丢弃）。
    font_faces: Vec<FontFaceRule>,
}

/// `@media` 求值的视口条件（CSS px）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MediaViewport {
    /// 视口宽。
    pub width: f32,
    /// 视口高。
    pub height: f32,
}

impl Stylesheet {
    /// 解析样式表文本。
    ///
    /// 输入按 UTF-8 处理（调用方负责按 HTTP/文档编码解码）。解析错误按
    /// CSS Syntax §5 的恢复规则就地吞掉：无效的规则与声明不会出现在结果里，
    /// 也不上报。`@media` 整块跳过（未提供求值条件）。
    #[must_use]
    pub fn parse(css: &str) -> Stylesheet {
        Self::parse_inner(css, None)
    }

    /// 解析样式表文本并按视口条件求值 `@media`。
    ///
    /// 命中的 `@media` 内层规则按源顺序就地展开参与级联；未命中的整块丢弃
    /// （CSS Conditional 3：不匹配的 condition 不贡献任何规则）。
    #[must_use]
    pub fn parse_with_viewport(css: &str, viewport: MediaViewport) -> Stylesheet {
        Self::parse_inner(css, Some(viewport))
    }

    fn parse_inner(css: &str, viewport: Option<MediaViewport>) -> Stylesheet {
        let mut rule_parser = StylesheetRuleParser { viewport };
        let mut input = Parser::new(css);
        let mut rules = Vec::new();
        let mut font_faces = Vec::new();
        // at-rule 产出的是规则组（@media 展开 / 单条 @font-face），
        // 普通规则是单元素组
        for group in StyleSheetParser::new(&mut input, &mut rule_parser).flatten() {
            match group {
                TopLevelRule::Style(style_rules) => rules.extend(style_rules),
                TopLevelRule::FontFace(rule) => font_faces.push(rule),
            }
        }
        Stylesheet { rules, font_faces }
    }

    /// 按源顺序返回全部样式规则。
    #[must_use]
    pub fn rules(&self) -> &[StyleRule] {
        &self.rules
    }

    /// 按源顺序返回全部 `@font-face` 规则。
    ///
    /// 供下游编排字体子资源抓取与注册（K4）：`family` 参与族名匹配，
    /// `sources` 按序尝试。不参与级联。
    #[must_use]
    pub fn font_faces(&self) -> &[FontFaceRule] {
        &self.font_faces
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

/// 一条 `@font-face` 规则（[CSS Fonts 4 §4](https://drafts.csswg.org/css-fonts-4/#font-face-rule)
/// 简化档，偏差见模块文档）。
///
/// 字段语义与下游字体描述符（`FontFaceDescriptor { family: String,
/// weight: u16, style: FontStyle::{Normal, Italic} }`）一一对应：可选描述符
/// 未指定时为 `None`，按描述符 initial 值映射（weight → 400、style → normal）。
#[derive(Debug, Clone, PartialEq)]
pub struct FontFaceRule {
    /// `font-family` 描述符（必需；缺失或非法的规则已在解析期丢弃）。
    pub family: String,
    /// `src` 描述符：按源顺序排列、按序尝试的源列表（非法项已跳过；
    /// 描述符存在但全部源无效时为空列表——下游按「永远不可用 → 回退
    /// 系统字体」处理）。
    pub sources: Vec<FontFaceSource>,
    /// `font-weight` 描述符（[CSS Fonts 4](https://drafts.csswg.org/css-fonts-4/#descdef-font-face-font-weight)，
    /// 单值 1–1000；`None` = 未指定，语义默认 400）。
    pub weight: Option<u16>,
    /// `font-style` 描述符（`None` = 未指定，语义默认 normal）。
    pub style: Option<FontFaceStyle>,
    /// `font-display` 描述符（只解析存储；行为恒 swap 简化——偏差）。
    pub display: Option<FontFaceDisplay>,
}

/// `src` 描述符的一个源（[CSS Fonts 4](https://drafts.csswg.org/css-fonts-4/#descdef-font-face-src)
/// 简化档）：按序尝试，前一个失败才轮到下一个。
#[derive(Debug, Clone, PartialEq)]
pub enum FontFaceSource {
    /// 外部资源：`url(…)` + 可选 `format(…)` 提示（提示只存不校验）。
    Url {
        /// 资源地址（cssparser 已去转义；相对地址由抓取方按文档 base 解析）。
        url: String,
        /// `format()` 提示字符串（无提示时为空列表）。
        format_hints: Vec<String>,
    },
    /// 本地字体：`local(…)` 引用的已安装字体名。
    Local {
        /// 本地字体名（`<string>` 或单个 `<custom-ident>`）。
        name: String,
    },
}

/// `font-style` 描述符取值（`normal` / `italic`；`oblique` 不识别——偏差）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontFaceStyle {
    /// `normal`（描述符 initial）。
    Normal,
    /// `italic`。
    Italic,
}

/// `font-display` 描述符取值
/// （[CSS Fonts 4](https://drafts.csswg.org/css-fonts-4/#descdef-font-face-font-display)）。
///
/// 当前只解析存储：加载行为恒「就绪后重排一次」（swap 简化——偏差）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontFaceDisplay {
    /// `auto`（描述符 initial）。
    Auto,
    /// `block`。
    Block,
    /// `swap`。
    Swap,
    /// `fallback`。
    Fallback,
    /// `optional`。
    Optional,
}

/// 顶层规则解析产物：样式规则组（qualified 规则单元素、`@media` 展开多
/// 元素）或一条 `@font-face` 规则。cssparser 要求两个 trait 的产出同型，
/// 故用枚举承载后在 [`Stylesheet::parse_inner`] 分流。
enum TopLevelRule {
    Style(Vec<StyleRule>),
    FontFace(FontFaceRule),
}

/// 顶层 at-rule 的 prelude：`@media` 查询或无 prelude 的 `@font-face`。
enum AtRulePrelude {
    Media(MediaQuery),
    FontFace,
}

/// 顶层样式表规则解析：接受样式规则、`@media`（带视口条件时求值）
/// 与 `@font-face`。
struct StylesheetRuleParser {
    /// `@media` 求值条件；`None` 时 `@media` 不命中（整块丢弃）。
    viewport: Option<MediaViewport>,
}

impl<'i> QualifiedRuleParser<'i> for StylesheetRuleParser {
    type Prelude = Vec<NextySelector>;
    type QualifiedRule = TopLevelRule;
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
        Ok(TopLevelRule::Style(vec![StyleRule {
            selectors: prelude,
            selector_text,
            declarations: parse_declaration_block(input),
        }]))
    }
}

// 未知 at-rule（@import / @supports / …）走默认拒绝，
// StyleSheetParser 按规范把整块消费掉。
impl<'i> AtRuleParser<'i> for StylesheetRuleParser {
    type Prelude = AtRulePrelude;
    type AtRule = TopLevelRule;
    type Error = ();

    fn parse_prelude(
        &mut self,
        name: cssparser::CowRcStr<'i>,
        input: &mut Parser<'i>,
    ) -> Result<Self::Prelude, ParseError<Self::Error>> {
        if name.eq_ignore_ascii_case("media") {
            parse_media_query(input)
                .map(AtRulePrelude::Media)
                .ok_or_else(ParseError::<()>::unexpected_token)
        } else if name.eq_ignore_ascii_case("font-face") {
            // @font-face 无 prelude（CSS Fonts 4 §4）；带 prelude 的规则整条丢弃
            input
                .expect_exhausted()
                .map_err(|_| ParseError::<()>::unexpected_token())?;
            Ok(AtRulePrelude::FontFace)
        } else {
            Err(ParseError::unexpected_token())
        }
    }

    fn parse_block<'t>(
        &mut self,
        prelude: Self::Prelude,
        _start: &cssparser::ParserState,
        input: &mut Parser<'i>,
    ) -> Result<Self::AtRule, ParseError<Self::Error>> {
        match prelude {
            AtRulePrelude::Media(query) => {
                // 内层按 rule list 解析（块必须整体消费后再决定去留）；
                // 嵌套 at-rule 在内层解析器里按未知拒绝
                let mut inner = MediaQueryRuleParser;
                let mut rules = Vec::new();
                for group in RuleBodyParser::new(input, &mut inner).flatten() {
                    rules.extend(group);
                }
                let matched = match self.viewport {
                    Some(viewport) => query.matches(viewport),
                    None => false,
                };
                Ok(TopLevelRule::Style(if matched {
                    rules
                } else {
                    Vec::new()
                }))
            }
            AtRulePrelude::FontFace => {
                // 描述符块：逐条解析，非法描述符就地丢弃（CSS Fonts 4 §4）
                let mut descriptors = FontFaceDescriptorsParser;
                let mut items = Vec::new();
                for item in RuleBodyParser::new(input, &mut descriptors).flatten() {
                    items.push(item);
                }
                // 必需描述符 font-family 缺失 → 整条规则丢弃
                parse_font_face_rule(items)
                    .map(TopLevelRule::FontFace)
                    .ok_or_else(ParseError::<()>::unexpected_token)
            }
        }
    }
}

/// `@media` 块内层规则解析：只接受样式规则，声明与嵌套 at-rule 拒绝。
struct MediaQueryRuleParser;

impl<'i> DeclarationParser<'i> for MediaQueryRuleParser {
    type Declaration = Vec<StyleRule>;
    type Error = ();
}

impl<'i> QualifiedRuleParser<'i> for MediaQueryRuleParser {
    type Prelude = Vec<NextySelector>;
    type QualifiedRule = Vec<StyleRule>;
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
        Ok(vec![StyleRule {
            selectors: prelude,
            selector_text,
            declarations: parse_declaration_block(input),
        }])
    }
}

// 嵌套 at-rule（@media 内再套 @media / @supports 等）不支持，默认拒绝。
impl<'i> AtRuleParser<'i> for MediaQueryRuleParser {
    type Prelude = ();
    type AtRule = Vec<StyleRule>;
    type Error = ();
}

impl<'i> RuleBodyItemParser<'i, Vec<StyleRule>, ()> for MediaQueryRuleParser {
    // @media 块内不解析声明（顶层声明本身非法），只解析样式规则
    fn parse_declarations(&self) -> bool {
        false
    }

    fn parse_qualified(&self) -> bool {
        true
    }
}

/// 一条媒体查询的解析结果（CSS Conditional 3 §3 / CSS3 Media Queries 语法子集）。
#[derive(Debug)]
struct MediaQuery {
    /// 查询是否合法；非法查询按 "not all" 处理（恒不命中）。
    valid: bool,
    /// `not` 前缀：命中条件取反。
    negate: bool,
    /// 媒体类型是否可接受（空 / all / screen）。
    type_ok: bool,
    /// `and` 连接的特性条件。
    features: Vec<MediaFeature>,
}

impl MediaQuery {
    /// 该查询在给定视口下是否命中。
    fn matches(&self, viewport: MediaViewport) -> bool {
        let matched = self.valid
            && self.type_ok
            && self
                .features
                .iter()
                .all(|feature| feature.matches(viewport));
        if self.negate { !matched } else { matched }
    }
}

/// 单个媒体特性：min/max-width/height（px）。
#[derive(Debug, Clone, Copy, PartialEq)]
enum MediaFeature {
    MinWidth(f32),
    MaxWidth(f32),
    MinHeight(f32),
    MaxHeight(f32),
}

impl MediaFeature {
    fn matches(&self, viewport: MediaViewport) -> bool {
        match *self {
            MediaFeature::MinWidth(value) => viewport.width >= value,
            MediaFeature::MaxWidth(value) => viewport.width <= value,
            MediaFeature::MinHeight(value) => viewport.height >= value,
            MediaFeature::MaxHeight(value) => viewport.height <= value,
        }
    }
}

/// 解析 `@media` 的 prelude：`[only | not]? <type>? [and <expression>]*`。
///
/// 不认识的前缀 / 类型 / 特性 → `None`（整条规则丢弃，CSS Conditional 3：
/// 未知查询等价 `not all`）。
fn parse_media_query(input: &mut Parser<'_>) -> Option<MediaQuery> {
    let mut query = MediaQuery {
        valid: true,
        negate: false,
        type_ok: true,
        features: Vec::new(),
    };
    let mut expect_expression = false;

    loop {
        let token = match input.next() {
            Ok(token) => token,
            // EOF：查询正常结束
            Err(_) => return Some(query),
        };
        match token {
            Token::Ident(name) if name.eq_ignore_ascii_case("only") => {}
            Token::Ident(name) if name.eq_ignore_ascii_case("not") => query.negate = true,
            Token::Ident(name) if name.eq_ignore_ascii_case("and") => {
                if !expect_expression {
                    return None; // and 出现在不该出现的位置
                }
                expect_expression = false;
            }
            Token::Ident(name) => {
                // 媒体类型：空缺视为 all；print/speech 等不命中。
                // 类型之后允许 `and <expression>`
                query.type_ok =
                    name.eq_ignore_ascii_case("all") || name.eq_ignore_ascii_case("screen");
                expect_expression = true;
            }
            Token::Function(_) => return None,
            Token::ParenthesisBlock => {
                let feature = input
                    .parse_nested_block(|input| {
                        parse_media_feature(input).map_err(|_| ParseError::<()>::unexpected_token())
                    })
                    .ok()?;
                query.features.push(feature);
                expect_expression = true;
            }
            _ => return None,
        }
    }
}

/// 解析一个媒体特性表达式：`(min-width: 400px)` 等。
fn parse_media_feature(input: &mut Parser<'_>) -> Result<MediaFeature, ()> {
    let name = match input.next().map_err(|_| ())? {
        Token::Ident(name) => name.to_string(),
        _ => return Err(()),
    };
    input.expect_colon().map_err(|_| ())?;
    let value = match input.next().map_err(|_| ())? {
        // px 数值；无单位数字按 px（CSS 层面其实不允许，宽松处理）
        Token::Dimension { value, unit, .. } if unit.eq_ignore_ascii_case("px") => *value,
        Token::Number { value, .. } => *value,
        _ => return Err(()),
    };
    input.expect_exhausted().map_err(|_| ())?;
    if name.eq_ignore_ascii_case("min-width") {
        Ok(MediaFeature::MinWidth(value))
    } else if name.eq_ignore_ascii_case("max-width") {
        Ok(MediaFeature::MaxWidth(value))
    } else if name.eq_ignore_ascii_case("min-height") {
        Ok(MediaFeature::MinHeight(value))
    } else if name.eq_ignore_ascii_case("max-height") {
        Ok(MediaFeature::MaxHeight(value))
    } else {
        Err(())
    }
}

// ---- @font-face 描述符解析（CSS Fonts 4 §4） ----

/// `@font-face` 块内单项解析结果（描述符产出；非法项由 cssparser 错误恢复丢弃）。
enum FontFaceItem {
    Family(String),
    Sources(Vec<FontFaceSource>),
    Weight(u16),
    Style(FontFaceStyle),
    Display(FontFaceDisplay),
}

/// `@font-face` 块内描述符解析器：只接受描述符声明，拒绝嵌套规则与 at-rule。
struct FontFaceDescriptorsParser;

impl<'i> DeclarationParser<'i> for FontFaceDescriptorsParser {
    type Declaration = FontFaceItem;
    type Error = ();

    fn parse_value<'t>(
        &mut self,
        name: cssparser::CowRcStr<'i>,
        input: &mut Parser<'i>,
        _declaration_start: &cssparser::ParserState,
    ) -> Result<FontFaceItem, ParseError<Self::Error>> {
        let item = if name.eq_ignore_ascii_case("font-family") {
            parse_font_face_family(input).map(FontFaceItem::Family)
        } else if name.eq_ignore_ascii_case("src") {
            Ok(FontFaceItem::Sources(parse_font_face_sources(input)))
        } else if name.eq_ignore_ascii_case("font-weight") {
            parse_font_face_weight(input).map(FontFaceItem::Weight)
        } else if name.eq_ignore_ascii_case("font-style") {
            parse_font_face_style(input).map(FontFaceItem::Style)
        } else if name.eq_ignore_ascii_case("font-display") {
            parse_font_face_display(input).map(FontFaceItem::Display)
        } else {
            // 未知 / 记偏差不解析的描述符（unicode-range、font-stretch、
            // font-variant、font-feature-settings…）按无效描述符丢弃
            Err(())
        };
        item.map_err(|_| ParseError::<()>::unexpected_token())
    }
}

impl<'i> QualifiedRuleParser<'i> for FontFaceDescriptorsParser {
    type Prelude = ();
    type QualifiedRule = FontFaceItem;
    type Error = ();
}

impl<'i> AtRuleParser<'i> for FontFaceDescriptorsParser {
    type Prelude = ();
    type AtRule = FontFaceItem;
    type Error = ();
}

impl<'i> RuleBodyItemParser<'i, FontFaceItem, ()> for FontFaceDescriptorsParser {
    // @font-face 块内只解析描述符声明（属性声明语法相同，但语义是描述符）
    fn parse_declarations(&self) -> bool {
        true
    }

    fn parse_qualified(&self) -> bool {
        false
    }
}

/// 组装 `@font-face` 规则：同一描述符多次出现取最后一条；必需的
/// `font-family` 缺失（含描述符全部非法）→ 整条规则丢弃（返回 `None`）。
fn parse_font_face_rule(items: Vec<FontFaceItem>) -> Option<FontFaceRule> {
    let mut rule = FontFaceRule {
        family: String::new(),
        sources: Vec::new(),
        weight: None,
        style: None,
        display: None,
    };
    for item in items {
        match item {
            FontFaceItem::Family(family) => rule.family = family,
            FontFaceItem::Sources(sources) => rule.sources = sources,
            FontFaceItem::Weight(weight) => rule.weight = Some(weight),
            FontFaceItem::Style(style) => rule.style = Some(style),
            FontFaceItem::Display(display) => rule.display = Some(display),
        }
    }
    if rule.family.is_empty() {
        None
    } else {
        Some(rule)
    }
}

/// 族名不能取的关键字：CSS-wide 关键字（描述符禁用）与 `default`（CSS Fonts 4
/// 将其与 CSS-wide 关键字一并排除在族名之外）。
fn is_reserved_family_name(name: &str) -> bool {
    ["initial", "inherit", "unset", "revert", "default"]
        .iter()
        .any(|keyword| name.eq_ignore_ascii_case(keyword))
}

/// `font-family` 描述符（必需；简化档）：`<string>` 或单个 `<custom-ident>`，
/// 空族名非法；未加引号的多词族名不支持（偏差）。
fn parse_font_face_family(input: &mut Parser<'_>) -> Result<String, ()> {
    let family = match input.next().map_err(|_| ())? {
        Token::QuotedString(family) => family.to_string(),
        Token::Ident(family) if !is_reserved_family_name(family) => family.to_string(),
        _ => return Err(()),
    };
    if family.is_empty() {
        return Err(());
    }
    input.expect_exhausted().map_err(|_| ())?;
    Ok(family)
}

/// `font-weight` 描述符（单值简化档）：`<number [1,1000]>` 或 `normal`（400） /
/// `bold`（700）关键字；相对关键字 `bolder` / `lighter` 描述符不适用，按无效
/// 丢弃。非整数字重四舍五入为整数存储（偏差：下游 face 权重为整数）。
fn parse_font_face_weight(input: &mut Parser<'_>) -> Result<u16, ()> {
    let weight = match input.next().map_err(|_| ())? {
        Token::Number { value, .. } if (1.0..=1000.0).contains(value) => value.round() as u16,
        Token::Ident(name) if name.eq_ignore_ascii_case("normal") => 400,
        Token::Ident(name) if name.eq_ignore_ascii_case("bold") => 700,
        _ => return Err(()),
    };
    input.expect_exhausted().map_err(|_| ())?;
    Ok(weight)
}

/// `font-style` 描述符（简化档）：`normal` / `italic`；`oblique` 不识别（偏差）。
fn parse_font_face_style(input: &mut Parser<'_>) -> Result<FontFaceStyle, ()> {
    let style = match input.next().map_err(|_| ())? {
        Token::Ident(name) if name.eq_ignore_ascii_case("normal") => FontFaceStyle::Normal,
        Token::Ident(name) if name.eq_ignore_ascii_case("italic") => FontFaceStyle::Italic,
        _ => return Err(()),
    };
    input.expect_exhausted().map_err(|_| ())?;
    Ok(style)
}

/// `font-display` 描述符：五关键字只解析存储（行为恒 swap 简化——偏差）。
fn parse_font_face_display(input: &mut Parser<'_>) -> Result<FontFaceDisplay, ()> {
    let display = match input.next().map_err(|_| ())? {
        Token::Ident(name) if name.eq_ignore_ascii_case("auto") => FontFaceDisplay::Auto,
        Token::Ident(name) if name.eq_ignore_ascii_case("block") => FontFaceDisplay::Block,
        Token::Ident(name) if name.eq_ignore_ascii_case("swap") => FontFaceDisplay::Swap,
        Token::Ident(name) if name.eq_ignore_ascii_case("fallback") => FontFaceDisplay::Fallback,
        Token::Ident(name) if name.eq_ignore_ascii_case("optional") => FontFaceDisplay::Optional,
        _ => return Err(()),
    };
    input.expect_exhausted().map_err(|_| ())?;
    Ok(display)
}

/// `src` 描述符：逗号分隔有序列表，逐项解析；单项非法 → 跳过该源，其余
/// 保留（全部无效 → 空列表，规则保留）。项内只认 `url(…)`（含带引号的
/// `url("…")`）可选尾随一个 `format(…)`，或 `local(…)`。
fn parse_font_face_sources(input: &mut Parser<'_>) -> Vec<FontFaceSource> {
    let mut sources = Vec::new();
    // 待定项：已解析出一个源，等逗号或结束才收录；项内出现其他 token 则整项作废
    let mut pending: Option<FontFaceSource> = None;
    loop {
        let token = match input.next() {
            Ok(token) => token,
            Err(_) => {
                // src 描述符结束（parse_until_after 已限定到 `;` 前）
                if let Some(source) = pending.take() {
                    sources.push(source);
                }
                break;
            }
        };
        match token {
            Token::Comma => {
                if let Some(source) = pending.take() {
                    sources.push(source);
                }
            }
            Token::Function(name) if name.eq_ignore_ascii_case("format") => {
                // format() 只能尾随同项的 url()：挂到待定项，或整项作废
                let attachable = matches!(
                    &pending,
                    Some(FontFaceSource::Url { format_hints, .. }) if format_hints.is_empty()
                );
                let hints = if attachable {
                    input
                        .parse_nested_block(|input| parse_format_hints(input))
                        .ok()
                } else {
                    // 位置非法的 format()：仍需消费其块再跳到逗号
                    let _ = input.parse_nested_block(|_input| {
                        Err::<(), _>(ParseError::<()>::unexpected_token())
                    });
                    None
                };
                match (hints, &mut pending) {
                    (Some(hints), Some(FontFaceSource::Url { format_hints, .. })) => {
                        *format_hints = hints;
                    }
                    _ => {
                        pending = None;
                        skip_to_comma(input);
                    }
                }
            }
            token => match parse_font_face_source(token.clone(), input) {
                Ok(source) if pending.is_none() => pending = Some(source),
                _ => {
                    // 无效项，或同项出现第二个源：整项作废，跳到下一项
                    pending = None;
                    skip_to_comma(input);
                }
            },
        }
    }
    sources
}

/// 解析一个源项的首个 token（`token` 为已消费项的克隆，与 parser 解引用）：
/// `url(…)` / `url("…")` / `local(…)`。空 `url()` 与裸字符串等其余形式按无效处理。
fn parse_font_face_source(token: Token<'_>, input: &mut Parser<'_>) -> Result<FontFaceSource, ()> {
    match token {
        Token::UnquotedUrl(url) if !url.is_empty() => Ok(FontFaceSource::Url {
            url: url.to_string(),
            format_hints: Vec::new(),
        }),
        Token::Function(name) if name.eq_ignore_ascii_case("url") => {
            let url = input
                .parse_nested_block(|input| parse_quoted_url(input))
                .map_err(|_| ())?;
            if url.is_empty() {
                return Err(());
            }
            Ok(FontFaceSource::Url {
                url,
                format_hints: Vec::new(),
            })
        }
        Token::Function(name) if name.eq_ignore_ascii_case("local") => {
            let name = input
                .parse_nested_block(|input| parse_local_name(input))
                .map_err(|_| ())?;
            Ok(FontFaceSource::Local { name })
        }
        _ => Err(()),
    }
}

/// `url("…")` 带引号形式：函数块内恰好一个非空 `<string>`。
fn parse_quoted_url(input: &mut Parser<'_>) -> Result<String, ParseError<()>> {
    let url = match input.next() {
        Ok(Token::QuotedString(url)) => url.to_string(),
        _ => return Err(ParseError::unexpected_token()),
    };
    input
        .expect_exhausted()
        .map_err(|_| ParseError::unexpected_token())?;
    Ok(url)
}

/// `local(…)` 内容：非空 `<string>` 或单个 `<custom-ident>`
/// （未加引号的多词族名不支持——偏差）。
fn parse_local_name(input: &mut Parser<'_>) -> Result<String, ParseError<()>> {
    let name = match input.next() {
        Ok(Token::QuotedString(name)) => name.to_string(),
        Ok(Token::Ident(name)) if !is_reserved_family_name(name) => name.to_string(),
        _ => return Err(ParseError::unexpected_token()),
    };
    if name.is_empty() {
        return Err(ParseError::unexpected_token());
    }
    input
        .expect_exhausted()
        .map_err(|_| ParseError::unexpected_token())?;
    Ok(name)
}

/// `format(<string>#)` 提示：只接受带引号字符串（规范语法），逗号分隔；
/// 至少一项，含其他 token 则整个源项无效。
fn parse_format_hints(input: &mut Parser<'_>) -> Result<Vec<String>, ParseError<()>> {
    let mut hints = Vec::new();
    loop {
        match input.next() {
            Ok(Token::QuotedString(hint)) => hints.push(hint.to_string()),
            Ok(Token::Comma) => continue,
            Err(_) => break,
            Ok(_) => return Err(ParseError::unexpected_token()),
        }
    }
    if hints.is_empty() {
        return Err(ParseError::unexpected_token());
    }
    Ok(hints)
}

/// 跳过一个源项的剩余 token（到逗号或 src 描述符结束）；嵌套块整体消费，
/// 避免块内逗号提前截断。
fn skip_to_comma(input: &mut Parser<'_>) {
    loop {
        match input.next() {
            Err(_) | Ok(Token::Comma) => break,
            Ok(
                Token::Function(_)
                | Token::ParenthesisBlock
                | Token::SquareBracketBlock
                | Token::CurlyBracketBlock,
            ) => {
                let _ = input.parse_nested_block(|_input| {
                    Err::<(), _>(ParseError::<()>::unexpected_token())
                });
            }
            _ => {}
        }
    }
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
    // font 简写：缺省成分重置为 initial（CSS Fonts 4 §3.3），longhand 组见
    // font_longhands；small-caps / font-width 成分在值层接受后丢弃。
    if name.eq_ignore_ascii_case("font") {
        if let Ok(keyword) = input.try_parse(parse_css_wide_keyword) {
            let important = input.try_parse(cssparser::parse_important).is_ok();
            if input.expect_exhausted().is_err() {
                return Some(Err(()));
            }
            return Some(Ok((
                font_longhands()
                    .into_iter()
                    .map(|property| (property, DeclaredValue::CssWide(keyword)))
                    .collect(),
                important,
            )));
        }
        let expanded = parse_font_shorthand(input)
            .map(|values| {
                values
                    .into_iter()
                    .map(|(property, value)| (property, DeclaredValue::Typed(value)))
                    .collect::<Vec<_>>()
            })
            .map_err(|_| ());
        let important = input.try_parse(cssparser::parse_important).is_ok();
        if input.expect_exhausted().is_err() {
            return Some(Err(()));
        }
        return Some(expanded.map(|declarations| (declarations, important)));
    }

    // gap 简写：row-gap || column-gap（两值：行优先，缺省列取行）。
    if name.eq_ignore_ascii_case("gap") {
        if let Ok(keyword) = input.try_parse(parse_css_wide_keyword) {
            let important = input.try_parse(cssparser::parse_important).is_ok();
            if input.expect_exhausted().is_err() {
                return Some(Err(()));
            }
            let longhands = [PropertyId::RowGap, PropertyId::ColumnGap];
            return Some(Ok((
                longhands
                    .into_iter()
                    .map(|property| (property, DeclaredValue::CssWide(keyword)))
                    .collect(),
                important,
            )));
        }
        return Some(parse_gap_shorthand(input));
    }

    // text-decoration 简写：<'text-decoration-line'> || <'text-decoration-style'>
    // || <'text-decoration-color'>（CSS Text Decoration 3 §3.3 基础档；缺省成分
    // 重置为 initial——line none / style solid / color currentcolor。thickness
    // 成分与 `blink` 不识别，整条无效——偏差）。
    if name.eq_ignore_ascii_case("text-decoration") {
        if let Ok(keyword) = input.try_parse(parse_css_wide_keyword) {
            let important = input.try_parse(cssparser::parse_important).is_ok();
            if input.expect_exhausted().is_err() {
                return Some(Err(()));
            }
            let longhands = [
                PropertyId::TextDecorationLine,
                PropertyId::TextDecorationStyle,
                PropertyId::TextDecorationColor,
            ];
            return Some(Ok((
                longhands
                    .into_iter()
                    .map(|property| (property, DeclaredValue::CssWide(keyword)))
                    .collect(),
                important,
            )));
        }
        let mut line = None;
        let mut style = None;
        let mut color = None;
        loop {
            if line.is_none()
                && let Ok(value) = input.try_parse(parse_text_decoration_line_value)
            {
                line = Some(value);
                continue;
            }
            if style.is_none()
                && let Ok(value) = input.try_parse(parse_text_decoration_style_value)
            {
                style = Some(value);
                continue;
            }
            if color.is_none()
                && let Ok(value) = input.try_parse(parse_text_decoration_color_value)
            {
                color = Some(value);
                continue;
            }
            break;
        }
        if line.is_none() && style.is_none() && color.is_none() {
            return Some(Err(()));
        }
        let important = input.try_parse(cssparser::parse_important).is_ok();
        if input.expect_exhausted().is_err() {
            return Some(Err(()));
        }
        return Some(Ok((
            vec![
                (
                    PropertyId::TextDecorationLine,
                    DeclaredValue::Typed(PropertyValue::TextDecorationLine(
                        line.unwrap_or(TextDecorationLineValue::None),
                    )),
                ),
                (
                    PropertyId::TextDecorationStyle,
                    DeclaredValue::Typed(PropertyValue::TextDecorationStyle(
                        style.unwrap_or(TextDecorationStyleValue::Solid),
                    )),
                ),
                (
                    PropertyId::TextDecorationColor,
                    DeclaredValue::Typed(PropertyValue::TextDecorationColor(
                        color.unwrap_or(TextDecorationColorValue::CurrentColor),
                    )),
                ),
            ],
            important,
        )));
    }

    // background 简写：[ <bg-image> || <bg-color> ]（CSS Backgrounds 3 §2 的
    // 两 longhand 基础子集；position / size / repeat / attachment / origin /
    // clip 成分与多层背景不识别，整条无效——偏差）。缺省成分重置为 initial
    // （image none / color transparent）。
    if name.eq_ignore_ascii_case("background") {
        if let Ok(keyword) = input.try_parse(parse_css_wide_keyword) {
            let important = input.try_parse(cssparser::parse_important).is_ok();
            if input.expect_exhausted().is_err() {
                return Some(Err(()));
            }
            let longhands = [PropertyId::BackgroundImage, PropertyId::BackgroundColor];
            return Some(Ok((
                longhands
                    .into_iter()
                    .map(|property| (property, DeclaredValue::CssWide(keyword)))
                    .collect(),
                important,
            )));
        }
        let mut image = None;
        let mut color = None;
        loop {
            if image.is_none()
                && let Ok(value) = input.try_parse(parse_background_image_value)
            {
                image = Some(value);
                continue;
            }
            if color.is_none()
                && let Ok(value) = input.try_parse(parse_color)
            {
                color = Some(value);
                continue;
            }
            break;
        }
        if image.is_none() && color.is_none() {
            return Some(Err(()));
        }
        let important = input.try_parse(cssparser::parse_important).is_ok();
        if input.expect_exhausted().is_err() {
            return Some(Err(()));
        }
        return Some(Ok((
            vec![
                (
                    PropertyId::BackgroundImage,
                    DeclaredValue::Typed(PropertyValue::BackgroundImage(
                        image.unwrap_or(BackgroundImageValue::None),
                    )),
                ),
                (
                    PropertyId::BackgroundColor,
                    DeclaredValue::Typed(PropertyValue::Color(
                        color.unwrap_or_else(Rgba::transparent),
                    )),
                ),
            ],
            important,
        )));
    }

    // border-radius 简写：<length-percentage [0,∞]>{1,4}（CSS Backgrounds 3
    // §5.1 基础档；`/` 椭圆语法不识别，整条无效——偏差）。
    if name.eq_ignore_ascii_case("border-radius") {
        if let Ok(keyword) = input.try_parse(parse_css_wide_keyword) {
            let important = input.try_parse(cssparser::parse_important).is_ok();
            if input.expect_exhausted().is_err() {
                return Some(Err(()));
            }
            return Some(Ok((
                border_radius_longhands()
                    .into_iter()
                    .map(|property| (property, DeclaredValue::CssWide(keyword)))
                    .collect(),
                important,
            )));
        }
        let corners = match expand_box_values(input, parse_border_radius_value) {
            Ok(values) => values,
            Err(()) => return Some(Err(())),
        };
        let important = input.try_parse(cssparser::parse_important).is_ok();
        if input.expect_exhausted().is_err() {
            return Some(Err(()));
        }
        return Some(Ok((
            zip_edges(
                corners,
                border_radius_longhands(),
                PropertyValue::BorderRadius,
            ),
            important,
        )));
    }

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
                Some(BoxEdges::Inset) => inset_longhands(),
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
        BoxEdges::Inset => expand_box_values(input, parse_inset_value)
            .map(|values| zip_edges(values, inset_longhands(), PropertyValue::Inset)),
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
    Inset,
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
    } else if name.eq_ignore_ascii_case("inset") {
        Some(BoxEdges::Inset)
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

/// `inset` 简写的四边 longhand（top / right / bottom / left）。
fn inset_longhands() -> Vec<PropertyId> {
    vec![
        PropertyId::Top,
        PropertyId::Right,
        PropertyId::Bottom,
        PropertyId::Left,
    ]
}

/// `border-radius` 简写的四角 longhand（顺时针：TL / TR / BR / BL）。
fn border_radius_longhands() -> Vec<PropertyId> {
    vec![
        PropertyId::BorderTopLeftRadius,
        PropertyId::BorderTopRightRadius,
        PropertyId::BorderBottomRightRadius,
        PropertyId::BorderBottomLeftRadius,
    ]
}

/// `gap` 简写：`<row-gap> <column-gap>?`（CSS Box Alignment §6）。
///
/// 单值同时作用于行与列；双值行领先、列取第二项。`normal` 不是合法 gap 值，
/// 由 `parse_gap` 按无效声明丢弃。
fn parse_gap_shorthand(
    input: &mut Parser<'_>,
) -> Result<(Vec<(PropertyId, DeclaredValue)>, bool), ()> {
    let row = parse_gap_value(input)?;
    let column = input.try_parse(parse_gap_value).unwrap_or(row);
    let important = input.try_parse(cssparser::parse_important).is_ok();
    if input.expect_exhausted().is_err() {
        return Err(());
    }
    Ok((
        vec![
            (
                PropertyId::RowGap,
                DeclaredValue::Typed(PropertyValue::Gap(row)),
            ),
            (
                PropertyId::ColumnGap,
                DeclaredValue::Typed(PropertyValue::Gap(column)),
            ),
        ],
        important,
    ))
}

fn border_longhands() -> Vec<PropertyId> {
    [
        border_width_longhands(),
        border_style_longhands(),
        border_color_longhands(),
    ]
    .concat()
}

/// `font` 简写的 longhand 组（CSS Fonts 4 §3.3：简写重置全部子属性，
/// 故缺省成分也产声明、值取 initial）。
fn font_longhands() -> Vec<PropertyId> {
    vec![
        PropertyId::FontStyle,
        PropertyId::FontWeight,
        PropertyId::FontSize,
        PropertyId::LineHeight,
        PropertyId::FontFamily,
    ]
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

    // ---- T7：@media 按视口求值（CSS Conditional 3） ----

    /// 窄视口命中 max-width 分支；宽视口命中 min-width 分支。
    #[test]
    fn media_query_matches_by_viewport_width() {
        let css = "@media (max-width: 500px) { p { color: red } }\
                   @media (min-width: 600px) { p { color: blue } }";

        let narrow = Stylesheet::parse_with_viewport(
            css,
            MediaViewport {
                width: 300.0,
                height: 600.0,
            },
        );
        assert_eq!(rule_texts(&narrow), vec!["p"], "窄视口只命中 max-width 块");
        assert_eq!(
            declaration_of(&narrow, 0, PropertyId::Color).value,
            DeclaredValue::Typed(PropertyValue::Color(Rgba::opaque(255, 0, 0)))
        );

        let wide = Stylesheet::parse_with_viewport(
            css,
            MediaViewport {
                width: 900.0,
                height: 600.0,
            },
        );
        assert_eq!(rule_texts(&wide), vec!["p"], "宽视口只命中 min-width 块");
        assert_eq!(
            declaration_of(&wide, 0, PropertyId::Color).value,
            DeclaredValue::Typed(PropertyValue::Color(Rgba::opaque(0, 0, 255)))
        );
    }

    /// 不匹配的规则不参与级联；边界值按闭区间语义（<= / >=）。
    #[test]
    fn non_matching_media_rules_are_dropped() {
        let css = "@media (min-width: 600px) { p { color: red } }\
                   p { display: none }";

        let narrow = Stylesheet::parse_with_viewport(
            css,
            MediaViewport {
                width: 599.0,
                height: 0.0,
            },
        );
        assert_eq!(rule_texts(&narrow), vec!["p"], "599px 不命中 600px 下限");

        // 边界：600px 恰好命中 min-width: 600px
        let boundary = Stylesheet::parse_with_viewport(
            css,
            MediaViewport {
                width: 600.0,
                height: 0.0,
            },
        );
        assert_eq!(
            rule_texts(&boundary),
            vec!["p", "p"],
            "600px 命中 min-width"
        );
    }

    /// and 组合、not、媒体类型与非法查询的求值语义。
    #[test]
    fn media_query_combinators_and_invalid_queries() {
        let viewport = MediaViewport {
            width: 800.0,
            height: 400.0,
        };
        // and 组合全命中
        let sheet = Stylesheet::parse_with_viewport(
            "@media screen and (min-width: 400px) and (max-height: 500px) { p { color: red } }",
            viewport,
        );
        assert_eq!(rule_texts(&sheet), vec!["p"]);
        // not 取反
        let sheet = Stylesheet::parse_with_viewport(
            "@media not screen and (min-width: 400px) { p { color: red } }",
            viewport,
        );
        assert!(sheet.rules().is_empty(), "not screen 命中的查询被取反");
        // print 类型不命中
        let sheet = Stylesheet::parse_with_viewport("@media print { p { color: red } }", viewport);
        assert!(sheet.rules().is_empty());
        // 非法特性 → 恒不命中（not all）
        let sheet = Stylesheet::parse_with_viewport(
            "@media (resolution: 300dpi) { p { color: red } }",
            viewport,
        );
        assert!(sheet.rules().is_empty());
        // 命中的块与不命中的块交错时保持源顺序
        let sheet = Stylesheet::parse_with_viewport(
            "@media (max-width: 100px) { a { color: red } }\
             b { color: blue }\
             @media (min-width: 100px) { c { color: green } }",
            viewport,
        );
        assert_eq!(rule_texts(&sheet), vec!["b", "c"]);
    }

    /// 命中的 @media 规则真实参与级联。
    #[test]
    fn media_rules_participate_in_cascade() {
        use crate::{compute_document_styles, html_ua_stylesheet, test_support};

        let document = test_support::build_document("<p>x</p>");
        let sheet = Stylesheet::parse_with_viewport(
            "@media (min-width: 600px) { p { color: blue } }",
            MediaViewport {
                width: 800.0,
                height: 600.0,
            },
        );
        let styles = compute_document_styles(&document, &[&sheet], Some(html_ua_stylesheet()));
        let p = test_support::find_first_by_name(&document, "p").expect("p");
        assert_eq!(styles[&p].color, Rgba::opaque(0, 0, 255), "命中块参与级联");
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

    // ---- font 简写（CSS Fonts 4 §3.3） ----

    use crate::value::{FontFamilyValue, FontStyleValue, FontWeightValue};

    #[test]
    fn font_shorthand_expands_with_line_height() {
        let sheet =
            Stylesheet::parse(r#"p { font: italic bold 12px/1.5 "Helvetica Neue", serif }"#);
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::FontStyle).value,
            DeclaredValue::Typed(PropertyValue::FontStyle(FontStyleValue::Italic))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::FontWeight).value,
            DeclaredValue::Typed(PropertyValue::FontWeight(FontWeightValue::Weight(700.0)))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::FontSize).value,
            DeclaredValue::Typed(PropertyValue::FontSize(FontSizeValue::Px(12.0)))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::LineHeight).value,
            DeclaredValue::Typed(PropertyValue::LineHeight(LineHeightValue::Number(1.5)))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::FontFamily).value,
            DeclaredValue::Typed(PropertyValue::FontFamily(vec![
                FontFamilyValue::Family("Helvetica Neue".to_string()),
                FontFamilyValue::Generic("serif"),
            ]))
        );
    }

    /// 缺省成分按简写语义重置为 initial（style=normal、weight=400、
    /// line-height=normal）。
    #[test]
    fn font_shorthand_resets_missing_longhands_to_initial() {
        let sheet = Stylesheet::parse("p { font: 12px serif }");
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::FontStyle).value,
            DeclaredValue::Typed(PropertyValue::FontStyle(FontStyleValue::Normal))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::FontWeight).value,
            DeclaredValue::Typed(PropertyValue::FontWeight(FontWeightValue::Weight(400.0)))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::FontSize).value,
            DeclaredValue::Typed(PropertyValue::FontSize(FontSizeValue::Px(12.0)))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::LineHeight).value,
            DeclaredValue::Typed(PropertyValue::LineHeight(LineHeightValue::Normal))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::FontFamily).value,
            DeclaredValue::Typed(PropertyValue::FontFamily(vec![FontFamilyValue::Generic(
                "serif"
            )]))
        );
    }

    /// `normal` 占位（style/variant/weight/width 槽位）与数值字重。
    #[test]
    fn font_shorthand_accepts_normal_placeholders_and_numeric_weight() {
        let sheet = Stylesheet::parse("p { font: normal normal 400 1em sans-serif }");
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::FontStyle).value,
            DeclaredValue::Typed(PropertyValue::FontStyle(FontStyleValue::Normal))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::FontWeight).value,
            DeclaredValue::Typed(PropertyValue::FontWeight(FontWeightValue::Weight(400.0)))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::FontSize).value,
            DeclaredValue::Typed(PropertyValue::FontSize(FontSizeValue::Em(1.0)))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::FontFamily).value,
            DeclaredValue::Typed(PropertyValue::FontFamily(vec![FontFamilyValue::Generic(
                "sans-serif"
            )]))
        );
    }

    /// `||` 成分任意顺序；small-caps / font-width 无 longhand，接受后丢弃。
    #[test]
    fn font_shorthand_components_in_any_order_with_discarded_width() {
        let sheet =
            Stylesheet::parse("p { font: small-caps condensed bold oblique 14px monospace }");
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::FontStyle).value,
            DeclaredValue::Typed(PropertyValue::FontStyle(FontStyleValue::Oblique(None)))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::FontWeight).value,
            DeclaredValue::Typed(PropertyValue::FontWeight(FontWeightValue::Weight(700.0)))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::FontSize).value,
            DeclaredValue::Typed(PropertyValue::FontSize(FontSizeValue::Px(14.0)))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::FontFamily).value,
            DeclaredValue::Typed(PropertyValue::FontFamily(vec![FontFamilyValue::Generic(
                "monospace"
            )]))
        );
    }

    /// `!important` 作用于全部 longhand（族名解析在 `!` 前停止）。
    #[test]
    fn font_shorthand_important_applies_to_all_longhands() {
        let sheet = Stylesheet::parse("p { font: 12px serif !important }");
        let declarations = &sheet.rules()[0].declarations();
        assert_eq!(declarations.len(), 5);
        assert!(declarations.iter().all(|declaration| declaration.important));
    }

    /// CSS-wide 关键字作用于全部 longhand。
    #[test]
    fn font_shorthand_css_wide_keyword() {
        let sheet = Stylesheet::parse("p { font: inherit }");
        let declarations = &sheet.rules()[0].declarations();
        assert_eq!(declarations.len(), 5);
        assert!(declarations.iter().all(
            |declaration| declaration.value == DeclaredValue::CssWide(CssWideKeyword::Inherit)
        ));
    }

    /// 非法值整条丢弃：缺 size、缺 family、系统字体关键字、值后残留垃圾。
    #[test]
    fn font_shorthand_invalid_values_are_dropped() {
        for css in [
            "p { font: 12px }",         // 缺 family
            "p { font: bold serif }",   // 缺 size
            "p { font: caption }",      // 系统字体关键字
            "p { font: 12px serif 2 }", // 族名中混入非 ident token
            "p { font: 12px/ }",        // 行高缺失
        ] {
            assert!(
                Stylesheet::parse(css).rules()[0].declarations().is_empty(),
                "{css} should drop the declaration"
            );
        }
    }

    /// font-family longhand 本身带 `!important`（族名解析在 `!` 前停止）。
    #[test]
    fn font_family_longhand_with_important() {
        let sheet = Stylesheet::parse(r#"p { font-family: "Helvetica Neue", serif !important }"#);
        assert!(declaration_of(&sheet, 0, PropertyId::FontFamily).important);
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::FontFamily).value,
            DeclaredValue::Typed(PropertyValue::FontFamily(vec![
                FontFamilyValue::Family("Helvetica Neue".to_string()),
                FontFamilyValue::Generic("serif"),
            ]))
        );
    }

    // ---- K2 绘制扩展：text-decoration / background / border-radius 简写 ----

    use crate::value::BorderRadiusValue;

    #[test]
    fn text_decoration_shorthand_expands_with_resets() {
        // 单成分：缺省成分重置为 initial（line none / style solid / color currentcolor）
        let sheet = Stylesheet::parse("p { text-decoration: underline }");
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::TextDecorationLine).value,
            DeclaredValue::Typed(PropertyValue::TextDecorationLine(
                TextDecorationLineValue::Underline
            ))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::TextDecorationStyle).value,
            DeclaredValue::Typed(PropertyValue::TextDecorationStyle(
                TextDecorationStyleValue::Solid
            ))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::TextDecorationColor).value,
            DeclaredValue::Typed(PropertyValue::TextDecorationColor(
                TextDecorationColorValue::CurrentColor
            ))
        );

        // 三成分任意顺序（|| 语义）
        let sheet = Stylesheet::parse("p { text-decoration: wavy red underline }");
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::TextDecorationLine).value,
            DeclaredValue::Typed(PropertyValue::TextDecorationLine(
                TextDecorationLineValue::Underline
            ))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::TextDecorationStyle).value,
            DeclaredValue::Typed(PropertyValue::TextDecorationStyle(
                TextDecorationStyleValue::Wavy
            ))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::TextDecorationColor).value,
            DeclaredValue::Typed(PropertyValue::TextDecorationColor(
                TextDecorationColorValue::Rgba(Rgba::opaque(255, 0, 0))
            ))
        );

        // 仅 color 成分合法（line 重置为 none）
        let sheet = Stylesheet::parse("p { text-decoration: red }");
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::TextDecorationLine).value,
            DeclaredValue::Typed(PropertyValue::TextDecorationLine(
                TextDecorationLineValue::None
            ))
        );

        // CSS-wide 关键字作用于全部 longhand
        let sheet = Stylesheet::parse("p { text-decoration: inherit !important }");
        let declarations = sheet.rules()[0].declarations();
        assert_eq!(declarations.len(), 3);
        assert!(declarations.iter().all(|declaration| declaration.important
            && declaration.value == DeclaredValue::CssWide(CssWideKeyword::Inherit)));

        // thickness 成分与 blink 不识别 → 整条声明丢弃（偏差）
        assert!(
            Stylesheet::parse("p { text-decoration: underline 2px }").rules()[0]
                .declarations()
                .is_empty()
        );
        assert!(
            Stylesheet::parse("p { text-decoration: blink }").rules()[0]
                .declarations()
                .is_empty()
        );
    }

    #[test]
    fn background_shorthand_expands_with_resets() {
        // 纯色：image 重置为 none
        let sheet = Stylesheet::parse("p { background: red }");
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::BackgroundColor).value,
            DeclaredValue::Typed(PropertyValue::Color(Rgba::opaque(255, 0, 0)))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::BackgroundImage).value,
            DeclaredValue::Typed(PropertyValue::BackgroundImage(BackgroundImageValue::None))
        );

        // 渐变 + 颜色任意顺序；缺省 color 重置为 transparent
        for css in [
            "p { background: linear-gradient(90deg, red, blue) #0f0 }",
            "p { background: #0f0 linear-gradient(90deg, red, blue) }",
        ] {
            let sheet = Stylesheet::parse(css);
            assert_eq!(
                declaration_of(&sheet, 0, PropertyId::BackgroundColor).value,
                DeclaredValue::Typed(PropertyValue::Color(Rgba::opaque(0, 255, 0))),
                "{css}"
            );
            let image = declaration_of(&sheet, 0, PropertyId::BackgroundImage)
                .value
                .clone();
            let DeclaredValue::Typed(PropertyValue::BackgroundImage(
                BackgroundImageValue::LinearGradient(gradient),
            )) = image
            else {
                panic!("{css} should carry a linear-gradient")
            };
            assert_eq!(gradient.angle, 90.0);
            assert_eq!(gradient.stops.len(), 2);
        }

        // 纯渐变：color 重置为 transparent
        let sheet = Stylesheet::parse("p { background: linear-gradient(red, blue) }");
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::BackgroundColor).value,
            DeclaredValue::Typed(PropertyValue::Color(Rgba::transparent()))
        );

        // CSS-wide 关键字
        let sheet = Stylesheet::parse("p { background: initial }");
        let declarations = sheet.rules()[0].declarations();
        assert_eq!(declarations.len(), 2);
        assert!(declarations.iter().all(
            |declaration| declaration.value == DeclaredValue::CssWide(CssWideKeyword::Initial)
        ));

        // position / repeat 等成分不识别 → 整条声明丢弃（偏差）；空值非法
        assert!(
            Stylesheet::parse("p { background: url(x.png) no-repeat }").rules()[0]
                .declarations()
                .is_empty()
        );
        assert!(
            Stylesheet::parse("p { background: no-repeat red }").rules()[0]
                .declarations()
                .is_empty()
        );
        assert!(
            Stylesheet::parse("p { background: }").rules()[0]
                .declarations()
                .is_empty()
        );
    }

    #[test]
    fn border_radius_shorthand_expands_clockwise() {
        // 单值：四角同值
        let sheet = Stylesheet::parse("p { border-radius: 10px }");
        assert_eq!(sheet.rules()[0].declarations().len(), 4);
        for property in [
            PropertyId::BorderTopLeftRadius,
            PropertyId::BorderTopRightRadius,
            PropertyId::BorderBottomRightRadius,
            PropertyId::BorderBottomLeftRadius,
        ] {
            assert_eq!(
                declaration_of(&sheet, 0, property).value,
                DeclaredValue::Typed(PropertyValue::BorderRadius(BorderRadiusValue::Length(10.0)))
            );
        }

        // 双值：TL/BR 取第一，TR/BL 取第二
        let sheet = Stylesheet::parse("p { border-radius: 10px 20px }");
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::BorderTopLeftRadius).value,
            DeclaredValue::Typed(PropertyValue::BorderRadius(BorderRadiusValue::Length(10.0)))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::BorderTopRightRadius).value,
            DeclaredValue::Typed(PropertyValue::BorderRadius(BorderRadiusValue::Length(20.0)))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::BorderBottomRightRadius).value,
            DeclaredValue::Typed(PropertyValue::BorderRadius(BorderRadiusValue::Length(10.0)))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::BorderBottomLeftRadius).value,
            DeclaredValue::Typed(PropertyValue::BorderRadius(BorderRadiusValue::Length(20.0)))
        );

        // 三值：TR/BL 取第二；四值顺时针
        let sheet = Stylesheet::parse("p { border-radius: 10px 20px 30px }");
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::BorderBottomLeftRadius).value,
            DeclaredValue::Typed(PropertyValue::BorderRadius(BorderRadiusValue::Length(20.0)))
        );
        let sheet = Stylesheet::parse("p { border-radius: 10% 20px 30% 40px }");
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::BorderTopLeftRadius).value,
            DeclaredValue::Typed(PropertyValue::BorderRadius(BorderRadiusValue::Percent(0.1)))
        );
        assert_eq!(
            declaration_of(&sheet, 0, PropertyId::BorderBottomLeftRadius).value,
            DeclaredValue::Typed(PropertyValue::BorderRadius(BorderRadiusValue::Length(40.0)))
        );

        // CSS-wide 关键字
        let sheet = Stylesheet::parse("p { border-radius: unset }");
        let declarations = sheet.rules()[0].declarations();
        assert_eq!(declarations.len(), 4);
        assert!(
            declarations
                .iter()
                .all(|declaration| declaration.value
                    == DeclaredValue::CssWide(CssWideKeyword::Unset))
        );

        // `/` 椭圆语法不识别 → 整条声明丢弃（偏差）
        assert!(
            Stylesheet::parse("p { border-radius: 10px / 20px }").rules()[0]
                .declarations()
                .is_empty()
        );
        assert!(
            Stylesheet::parse("p { border-radius: 10px 20px / 5px }").rules()[0]
                .declarations()
                .is_empty()
        );
    }

    // ---- K4 T1：@font-face 解析（CSS Fonts 4 §4） ----

    fn url_source(url: &str, hints: &[&str]) -> FontFaceSource {
        FontFaceSource::Url {
            url: url.to_string(),
            format_hints: hints.iter().map(|hint| hint.to_string()).collect(),
        }
    }

    fn local_source(name: &str) -> FontFaceSource {
        FontFaceSource::Local {
            name: name.to_string(),
        }
    }

    #[test]
    fn font_face_parses_descriptors_with_defaults() {
        let sheet =
            Stylesheet::parse("@font-face { font-family: \"My Font\"; src: url(fonts/a.ttf); }");
        // @font-face 不进样式规则列表
        assert!(sheet.rules().is_empty());
        let faces = sheet.font_faces();
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0].family, "My Font");
        assert_eq!(faces[0].sources, vec![url_source("fonts/a.ttf", &[])]);
        // 可选描述符未指定 → None（下游按 initial 值映射：400 / normal / auto）
        assert_eq!(faces[0].weight, None);
        assert_eq!(faces[0].style, None);
        assert_eq!(faces[0].display, None);
    }

    #[test]
    fn font_face_keeps_source_order_across_url_and_local() {
        // 多 src：逗号分隔有序列表，按序尝试语义由顺序承载
        let sheet = Stylesheet::parse(
            "@font-face { font-family: Fallback; src: local(Arial), \
             url(a.woff2) format(\"woff2\"), url(b.ttf) format(\"truetype\"), \
             url(\"quoted.otf\"), local(\"My Font\"); }",
        );
        let sources = &sheet.font_faces()[0].sources;
        assert_eq!(
            sources,
            &[
                local_source("Arial"),
                url_source("a.woff2", &["woff2"]),
                url_source("b.ttf", &["truetype"]),
                url_source("quoted.otf", &[]),
                local_source("My Font"),
            ]
        );
    }

    #[test]
    fn font_face_without_required_family_is_dropped() {
        // 缺 font-family → 整条规则丢弃，后续规则不受影响
        let sheet = Stylesheet::parse("@font-face { src: url(a.ttf) } p { color: red }");
        assert!(sheet.font_faces().is_empty());
        assert_eq!(sheet.rules().len(), 1);
        // font-family 非法：CSS-wide 关键字
        assert!(
            Stylesheet::parse("@font-face { font-family: inherit; src: url(a.ttf) }")
                .font_faces()
                .is_empty()
        );
        // font-family 非法：未加引号的多词族名（偏差：不支持）
        assert!(
            Stylesheet::parse("@font-face { font-family: My Font; src: url(a.ttf) }")
                .font_faces()
                .is_empty()
        );
        // font-family 非法：空字符串 / 非标识符
        assert!(
            Stylesheet::parse("@font-face { font-family: \"\" }")
                .font_faces()
                .is_empty()
        );
        assert!(
            Stylesheet::parse("@font-face { font-family: 42 }")
                .font_faces()
                .is_empty()
        );
        // 无块的 @font-face 与带 prelude 的 @font-face 整条丢弃
        assert!(Stylesheet::parse("@font-face;").font_faces().is_empty());
        assert!(
            Stylesheet::parse("@font-face bogus { font-family: X }")
                .font_faces()
                .is_empty()
        );
    }

    #[test]
    fn font_face_invalid_sources_are_skipped_individually() {
        let sheet = Stylesheet::parse(
            "@font-face { font-family: X; src: url(good.ttf), bogus, \
             url(\"quoted.woff\") format(\"woff\"), local(), format(\"woff2\"), \
             \"bare-string.ttf\", url(last.ttf); }",
        );
        let sources = &sheet.font_faces()[0].sources;
        // bogus（非源 token）、local()（空内容）、位置非法的 format()、
        // 裸字符串四项被跳过，其余按源顺序保留
        assert_eq!(
            sources,
            &[
                url_source("good.ttf", &[]),
                url_source("quoted.woff", &["woff"]),
                url_source("last.ttf", &[]),
            ]
        );
    }

    #[test]
    fn font_face_with_all_invalid_sources_keeps_rule_with_empty_list() {
        let sheet = Stylesheet::parse("@font-face { font-family: X; src: bogus, local() }");
        let faces = sheet.font_faces();
        assert_eq!(faces.len(), 1);
        assert!(faces[0].sources.is_empty());
    }

    #[test]
    fn font_face_weight_number_and_keyword_forms() {
        let sheet = Stylesheet::parse(
            "@font-face { font-family: A; font-weight: 700 } \
             @font-face { font-family: B; font-weight: bold } \
             @font-face { font-family: C; font-weight: normal } \
             @font-face { font-family: D; font-weight: 550.5 } \
             @font-face { font-family: E; font-weight: bolder } \
             @font-face { font-family: G; font-weight: 1001 } \
             @font-face { font-family: H; font-weight: 0 }",
        );
        let faces = sheet.font_faces();
        // weight 是可选描述符：非法值只丢描述符，规则保留
        assert_eq!(faces.len(), 7);
        assert_eq!(faces[0].weight, Some(700));
        assert_eq!(faces[1].weight, Some(700), "bold = 700");
        assert_eq!(faces[2].weight, Some(400), "normal = 400");
        assert_eq!(faces[3].weight, Some(551), "非整数四舍五入（偏差）");
        assert_eq!(faces[4].weight, None, "bolder 描述符不适用");
        assert_eq!(faces[5].weight, None, "1001 越界");
        assert_eq!(faces[6].weight, None, "0 越界");
    }

    #[test]
    fn font_face_format_hints_are_parsed_and_stored() {
        // format() 多提示：CSS Fonts 4 允许逗号分隔多个 <string>
        let sheet = Stylesheet::parse(
            "@font-face { font-family: X; src: url(a.woff2) format(\"woff2\"), \
             url(b.otf) format(\"opentype\", \"otf\"), url(c.ttf) format(), \
             url(d.ttf) format(\"truetype\") }",
        );
        let sources = &sheet.font_faces()[0].sources;
        assert_eq!(sources[0], url_source("a.woff2", &["woff2"]));
        assert_eq!(
            sources[1],
            url_source("b.otf", &["opentype", "otf"]),
            "多提示按序存储"
        );
        assert_eq!(sources[2], url_source("d.ttf", &["truetype"]));
        // 空 format() 与未加引号的提示（规范语法是 <string>#）使该源无效
        let sheet = Stylesheet::parse(
            "@font-face { font-family: X; src: url(c.ttf) format(), url(d.ttf) format(truetype) }",
        );
        assert!(sheet.font_faces()[0].sources.is_empty());
    }

    #[test]
    fn font_face_style_and_display_forms() {
        let sheet = Stylesheet::parse(
            "@font-face { font-family: A; font-style: italic; font-display: swap } \
             @font-face { font-family: B; font-style: normal; font-display: optional } \
             @font-face { font-family: C; font-style: oblique; font-display: block } \
             @font-face { font-family: D; font-style: oblique 30deg; font-display: fallback } \
             @font-face { font-family: E; font-display: auto }",
        );
        let faces = sheet.font_faces();
        assert_eq!(faces.len(), 5, "可选描述符非法只丢描述符");
        assert_eq!(faces[0].style, Some(FontFaceStyle::Italic));
        assert_eq!(faces[0].display, Some(FontFaceDisplay::Swap));
        assert_eq!(faces[1].style, Some(FontFaceStyle::Normal));
        assert_eq!(faces[1].display, Some(FontFaceDisplay::Optional));
        assert_eq!(faces[2].style, None, "oblique 不识别（偏差）");
        assert_eq!(faces[2].display, Some(FontFaceDisplay::Block));
        assert_eq!(faces[3].style, None, "oblique 带角度同样不识别");
        assert_eq!(faces[3].display, Some(FontFaceDisplay::Fallback));
        assert_eq!(faces[4].display, Some(FontFaceDisplay::Auto));
    }

    #[test]
    fn font_face_family_string_and_ident_forms() {
        let sheet = Stylesheet::parse(
            "@font-face { font-family: Custom; src: url(a.ttf) } \
             @font-face { font-family: \"With Space\"; src: url(b.ttf) } \
             @font-face { font-family: initial; src: url(c.ttf) } \
             @font-face { font-family: default; src: url(d.ttf) }",
        );
        let faces = sheet.font_faces();
        assert_eq!(faces.len(), 2, "CSS-wide 关键字与 default 不能作族名");
        assert_eq!(faces[0].family, "Custom");
        assert_eq!(faces[1].family, "With Space");
    }

    #[test]
    fn font_face_duplicate_descriptors_last_wins() {
        let sheet = Stylesheet::parse(
            "@font-face { font-family: A; font-family: B; font-weight: 300; \
             font-weight: 700; src: url(one.ttf); src: url(two.ttf) format(\"t2\"); }",
        );
        let face = &sheet.font_faces()[0];
        assert_eq!(face.family, "B");
        assert_eq!(face.weight, Some(700));
        assert_eq!(face.sources, vec![url_source("two.ttf", &["t2"])]);
    }

    #[test]
    fn font_face_descriptor_with_important_is_dropped() {
        // 描述符不接受 !important：带该标志的描述符按无效丢弃
        let sheet = Stylesheet::parse("@font-face { font-family: X !important } p { color: red }");
        assert!(
            sheet.font_faces().is_empty(),
            "必需描述符带 !important → 丢整条"
        );
        assert_eq!(sheet.rules().len(), 1);
        let sheet = Stylesheet::parse("@font-face { font-family: X; font-weight: 700 !important }");
        assert_eq!(
            sheet.font_faces()[0].weight,
            None,
            "可选描述符带 !important 丢弃"
        );
        let sheet = Stylesheet::parse("@font-face { font-family: X; src: url(a.ttf) !important }");
        assert!(
            sheet.font_faces()[0].sources.is_empty(),
            "src 带 !important 整个描述符无效"
        );
    }

    #[test]
    fn font_face_unknown_descriptors_are_dropped() {
        // unicode-range / font-stretch / font-variant 等记偏差不解析：
        // 按无效描述符丢弃，规则保留
        let sheet = Stylesheet::parse(
            "@font-face { font-family: X; unicode-range: U+0-7F; font-stretch: condensed; \
             font-variant: small-caps; src: url(a.ttf) }",
        );
        let faces = sheet.font_faces();
        assert_eq!(faces.len(), 1);
        assert_eq!(faces[0].sources, vec![url_source("a.ttf", &[])]);
    }

    #[test]
    fn font_faces_keep_source_order_and_do_not_disturb_style_rules() {
        let sheet = Stylesheet::parse(
            "@font-face { font-family: A; src: url(a.ttf) } \
             p { color: red } \
             @font-face { font-family: B; font-family: C; src: url(b.ttf) } \
             q { color: blue } \
             @font-face { src: url(c.ttf) }",
        );
        assert_eq!(rule_texts(&sheet), vec!["p", "q"]);
        let faces = sheet.font_faces();
        assert_eq!(faces.len(), 2, "缺 family 的第三条丢弃");
        assert_eq!(faces[0].family, "A");
        assert_eq!(faces[1].family, "C", "重复描述符最后生效");
    }

    #[test]
    fn font_face_inside_media_is_not_supported() {
        // 嵌套 at-rule 不支持（既有偏差）：@media 内的 @font-face 整块跳过
        let sheet = Stylesheet::parse_with_viewport(
            "@media screen { @font-face { font-family: X; src: url(a.ttf) } }",
            MediaViewport {
                width: 800.0,
                height: 600.0,
            },
        );
        assert!(sheet.font_faces().is_empty());
        assert!(sheet.rules().is_empty());
    }
}
