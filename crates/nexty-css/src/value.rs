//! 属性值模型与逐属性解析。
//!
//! 行为 ground truth：
//! - CSS-wide 关键字：[CSS Cascading and Inheritance Level 5 §6](https://drafts.csswg.org/css-cascade-5/#defaulting-keywords)
//! - 颜色：[CSS Color Module Level 4](https://drafts.csswg.org/css-color-4/)（hex / rgb() / hsl() / 命名色）
//! - 字号：[CSS Fonts Module Level 4 §2.5](https://drafts.csswg.org/css-fonts-4/#font-size-prop)
//! - 字重：[CSS Fonts Module Level 4 §2.2](https://drafts.csswg.org/css-fonts-4/#font-weight-numeric)
//! - 字体族：[CSS Fonts Module Level 4 §2.3](https://drafts.csswg.org/css-fonts-4/#font-family-prop)
//! - display：[CSS Display Module Level 3](https://drafts.csswg.org/css-display-3/#the-display-properties)
//! - text-align：[CSS Text Module Level 4](https://drafts.csswg.org/css-text-4/#text-align-property)
//!
//! 已知偏差（按需再补，见本轮 goal.md 非目标）：`rem` / `ex` / `ch` 等
//! 相对单位、`currentcolor`、`color-mix()` 等函数色、`display` 的多关键字
//! 语法与 ruby 内部盒、`var()` 均视为无效声明（浏览器会接受其中的合法值）。

use cssparser::{ParseError, Parser, Token, parse_important};

/// 非预乘 sRGB 颜色。
///
/// 对应 CSS Color 4 的 [`<color>`](https://drafts.csswg.org/css-color-4/#color-syntax)
/// 解析结果；alpha 与各通道均量化到 8bit。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    /// 红通道。
    pub r: u8,
    /// 绿通道。
    pub g: u8,
    /// 蓝通道。
    pub b: u8,
    /// alpha 通道。
    pub a: u8,
}

impl Rgba {
    /// 不透明颜色。
    #[must_use]
    pub const fn opaque(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 0xff }
    }

    /// 全透明黑，即 CSS 关键字 `transparent`
    /// （[CSS Color 4 §5.5](https://drafts.csswg.org/css-color-4/#transparent-color)）。
    #[must_use]
    pub const fn transparent() -> Self {
        Self {
            r: 0,
            g: 0,
            b: 0,
            a: 0,
        }
    }
}

/// 级联引擎认识的最小属性集合。
///
/// 只覆盖当前管线需要的属性；新属性随消费方（layout / text / paint）落地时加入，
/// 不预先铺满。属性元数据（inherited / initial）见 `cascade` 模块。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PropertyId {
    /// `display`。
    Display,
    /// `color`。
    Color,
    /// `background-color`。
    BackgroundColor,
    /// `font-size`。
    FontSize,
    /// `font-weight`。
    FontWeight,
    /// `font-style`。
    FontStyle,
    /// `font-family`。
    FontFamily,
    /// `text-align`。
    TextAlign,
}

impl PropertyId {
    /// 属性的规范名（小写）。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            PropertyId::Display => "display",
            PropertyId::Color => "color",
            PropertyId::BackgroundColor => "background-color",
            PropertyId::FontSize => "font-size",
            PropertyId::FontWeight => "font-weight",
            PropertyId::FontStyle => "font-style",
            PropertyId::FontFamily => "font-family",
            PropertyId::TextAlign => "text-align",
        }
    }

    /// 按属性名查 [`PropertyId`]，ASCII case-insensitive
    /// （CSS Syntax：声明名匹配大小写不敏感）。
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        [
            Self::Display,
            Self::Color,
            Self::BackgroundColor,
            Self::FontSize,
            Self::FontWeight,
            Self::FontStyle,
            Self::FontFamily,
            Self::TextAlign,
        ]
        .into_iter()
        .find(|id| id.as_str().eq_ignore_ascii_case(name))
    }
}

/// CSS-wide 关键字
/// （[CSS Cascade 5 §6.1](https://drafts.csswg.org/css-cascade-5/#defaulting-keywords)）。
///
/// 对任何属性都合法，求值语义由 cascade 模块实现。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CssWideKeyword {
    /// `initial`。
    Initial,
    /// `inherit`。
    Inherit,
    /// `unset`。
    Unset,
    /// `revert`。
    Revert,
}

/// 一条声明按属性解析后的值。
#[derive(Debug, Clone, PartialEq)]
pub enum DeclaredValue {
    /// CSS-wide 关键字（对任意属性合法）。
    CssWide(CssWideKeyword),
    /// 按属性语法解析出的值。
    Typed(PropertyValue),
}

/// 各属性解析后的值。
///
/// 每个变体对应 [`PropertyId`] 中一个属性的可接受语法；不在集合内的值按
/// 无效声明处理（整体丢弃，声明级恢复，见 CSS Syntax §5.5）。
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    /// `display` 关键字。
    Display(DisplayValue),
    /// `text-align` 关键字。
    TextAlign(TextAlignValue),
    /// `font-style` 关键字。
    FontStyle(FontStyleValue),
    /// 颜色值。
    Color(Rgba),
    /// `font-size` 值。
    FontSize(FontSizeValue),
    /// `font-weight` 值。
    FontWeight(FontWeightValue),
    /// `font-family` 字体族列表。
    FontFamily(Vec<FontFamilyValue>),
}

/// `display` 的单关键字值
/// （[CSS Display 3 §2](https://drafts.csswg.org/css-display-3/#the-display-properties)）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayValue {
    /// `inline`（initial）。
    Inline,
    /// `block`。
    Block,
    /// `list-item`。
    ListItem,
    /// `inline-block`。
    InlineBlock,
    /// `flow-root`。
    FlowRoot,
    /// `flex`。
    Flex,
    /// `inline-flex`。
    InlineFlex,
    /// `grid`。
    Grid,
    /// `inline-grid`。
    InlineGrid,
    /// `table`。
    Table,
    /// `inline-table`。
    InlineTable,
    /// 表格内部盒（`table-row-group` / `table-header-group` /
    /// `table-footer-group` / `table-row` / `table-cell` /
    /// `table-column-group` / `table-column` / `table-caption`）。
    TableInternal(&'static str),
    /// `contents`。
    Contents,
    /// `none`。
    None,
}

/// `text-align` 值
/// （[CSS Text 4](https://drafts.csswg.org/css-text-4/#text-align-property)）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlignValue {
    /// `start`（initial）。
    Start,
    /// `end`。
    End,
    /// `left`。
    Left,
    /// `right`。
    Right,
    /// `center`。
    Center,
    /// `justify`。
    Justify,
    /// `match-parent`。
    MatchParent,
    /// `justify-all`。
    JustifyAll,
}

/// `font-style` 值
/// （[CSS Fonts 4 §2.1](https://drafts.csswg.org/css-fonts-4/#font-style-prop)）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FontStyleValue {
    /// `normal`（initial）。
    Normal,
    /// `italic`。
    Italic,
    /// `oblique`，可选倾斜角（角度制；`oblique none` 为 `None`）。
    Oblique(Option<f32>),
}

/// `font-size` 的绝对尺寸关键字及其缩放系数
/// （[CSS Fonts 4 §2.5.1](https://drafts.csswg.org/css-fonts-4/#absolute-size-mapping)：
/// medium 为基准 1，其余为规范给出的 scaling factor）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AbsoluteSize {
    /// `xx-small`（3/5）。
    XxSmall,
    /// `x-small`（3/4）。
    XSmall,
    /// `small`（8/9）。
    Small,
    /// `medium`（1）。
    Medium,
    /// `large`（6/5）。
    Large,
    /// `x-large`（3/2）。
    XLarge,
    /// `xx-large`（2/1）。
    XxLarge,
    /// `xxx-large`（3/1）。
    XxxLarge,
}

impl AbsoluteSize {
    /// 相对 medium 的缩放系数。
    #[must_use]
    pub fn factor(self) -> f32 {
        match self {
            AbsoluteSize::XxSmall => 3.0 / 5.0,
            AbsoluteSize::XSmall => 3.0 / 4.0,
            AbsoluteSize::Small => 8.0 / 9.0,
            AbsoluteSize::Medium => 1.0,
            AbsoluteSize::Large => 6.0 / 5.0,
            AbsoluteSize::XLarge => 1.5,
            AbsoluteSize::XxLarge => 2.0,
            AbsoluteSize::XxxLarge => 3.0,
        }
    }

    /// 绝对尺寸关键字的有序表，供 `larger` / `smaller` 取相邻项。
    pub const ORDERED: [AbsoluteSize; 8] = [
        AbsoluteSize::XxSmall,
        AbsoluteSize::XSmall,
        AbsoluteSize::Small,
        AbsoluteSize::Medium,
        AbsoluteSize::Large,
        AbsoluteSize::XLarge,
        AbsoluteSize::XxLarge,
        AbsoluteSize::XxxLarge,
    ];
}

/// `font-size` 值
/// （[CSS Fonts 4 §2.5](https://drafts.csswg.org/css-fonts-4/#font-size-prop)）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FontSizeValue {
    /// 绝对长度（已折算为 CSS px）。
    Px(f32),
    /// 相对父元素计算字号的 em。
    Em(f32),
    /// 相对父元素计算字号的百分比，以小数存储（`120%` → `1.2`）。
    Percent(f32),
    /// 绝对尺寸关键字。
    Absolute(AbsoluteSize),
    /// `larger`。
    Larger,
    /// `smaller`。
    Smaller,
}

/// `font-weight` 值
/// （[CSS Fonts 4 §2.2](https://drafts.csswg.org/css-fonts-4/#font-weight-numeric)）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FontWeightValue {
    /// 数值字重，1–1000（`normal` = 400、`bold` = 700 已在解析期折算）。
    Weight(f32),
    /// `bolder`：相对继承值。
    Bolder,
    /// `lighter`：相对继承值。
    Lighter,
}

/// 字体族列表中的一项
/// （[CSS Fonts 4 §2.3](https://drafts.csswg.org/css-fonts-4/#font-family-prop)）。
///
/// 带引号的 `"serif"` 是名为 serif 的普通字体族，只有不带引号的
/// generic 关键字才映射到 [`FontFamilyValue`] 的 generic 变体。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontFamilyValue {
    /// generic 族（`serif` / `sans-serif` / `cursive` / `fantasy` /
    /// `monospace` / `system-ui` / `ui-serif` / `ui-sans-serif` /
    /// `ui-monospace` / `ui-rounded` / `math` / `emoji` / `fangsong`）。
    Generic(&'static str),
    /// 具名字体族。
    Family(String),
}

/// 把声明值解析为 [`DeclaredValue`]：先识别 CSS-wide 关键字，再按属性语法解析。
///
/// `input` 已由调用方限定在本条声明的值范围内（分号 / 块尾之前）。
/// 返回值连同该声明是否带 `!important`（CSS Cascade 5 §5.1）；返回 `Err`
/// 表示整条声明无效。
pub(crate) fn parse_declared_value(
    property: PropertyId,
    input: &mut Parser<'_>,
) -> Result<(DeclaredValue, bool), ()> {
    if let Ok(keyword) = input.try_parse(parse_css_wide_keyword) {
        let important = input.try_parse(parse_important).is_ok();
        // CSS-wide 关键字不允许再拼接其他值
        // （CSS Cascade 5：关键字是完整的 <declaration-value>）。
        if input.expect_exhausted().is_err() {
            return Err(());
        }
        return Ok((DeclaredValue::CssWide(keyword), important));
    }
    let value = parse_property_value(property, input)?;
    let important = input.try_parse(parse_important).is_ok();
    if input.expect_exhausted().is_err() {
        return Err(());
    }
    Ok((DeclaredValue::Typed(value), important))
}

/// 解析 CSS-wide 关键字（ASCII case-insensitive）。
fn parse_css_wide_keyword(input: &mut Parser<'_>) -> Result<CssWideKeyword, ()> {
    let Token::Ident(name) = input.next().map_err(|_| ())? else {
        return Err(());
    };
    if name.eq_ignore_ascii_case("initial") {
        Ok(CssWideKeyword::Initial)
    } else if name.eq_ignore_ascii_case("inherit") {
        Ok(CssWideKeyword::Inherit)
    } else if name.eq_ignore_ascii_case("unset") {
        Ok(CssWideKeyword::Unset)
    } else if name.eq_ignore_ascii_case("revert") {
        Ok(CssWideKeyword::Revert)
    } else {
        Err(())
    }
}

/// 按属性语法解析值。
fn parse_property_value(property: PropertyId, input: &mut Parser<'_>) -> Result<PropertyValue, ()> {
    match property {
        PropertyId::Display => parse_display(input).map(PropertyValue::Display),
        PropertyId::TextAlign => parse_text_align(input).map(PropertyValue::TextAlign),
        PropertyId::FontStyle => parse_font_style(input).map(PropertyValue::FontStyle),
        PropertyId::Color => parse_color(input).map(PropertyValue::Color),
        PropertyId::FontSize => parse_font_size(input).map(PropertyValue::FontSize),
        PropertyId::FontWeight => parse_font_weight(input).map(PropertyValue::FontWeight),
        PropertyId::FontFamily => parse_font_family(input).map(PropertyValue::FontFamily),
        PropertyId::BackgroundColor => parse_color(input).map(PropertyValue::Color),
    }
}

fn parse_display(input: &mut Parser<'_>) -> Result<DisplayValue, ()> {
    let Token::Ident(name) = input.next().map_err(|_| ())? else {
        return Err(());
    };
    if name.eq_ignore_ascii_case("inline") {
        return Ok(DisplayValue::Inline);
    }
    if name.eq_ignore_ascii_case("block") {
        return Ok(DisplayValue::Block);
    }
    if name.eq_ignore_ascii_case("list-item") {
        return Ok(DisplayValue::ListItem);
    }
    if name.eq_ignore_ascii_case("inline-block") {
        return Ok(DisplayValue::InlineBlock);
    }
    if name.eq_ignore_ascii_case("flow-root") {
        return Ok(DisplayValue::FlowRoot);
    }
    if name.eq_ignore_ascii_case("flex") {
        return Ok(DisplayValue::Flex);
    }
    if name.eq_ignore_ascii_case("inline-flex") {
        return Ok(DisplayValue::InlineFlex);
    }
    if name.eq_ignore_ascii_case("grid") {
        return Ok(DisplayValue::Grid);
    }
    if name.eq_ignore_ascii_case("inline-grid") {
        return Ok(DisplayValue::InlineGrid);
    }
    if name.eq_ignore_ascii_case("table") {
        return Ok(DisplayValue::Table);
    }
    if name.eq_ignore_ascii_case("inline-table") {
        return Ok(DisplayValue::InlineTable);
    }
    if name.eq_ignore_ascii_case("contents") {
        return Ok(DisplayValue::Contents);
    }
    if name.eq_ignore_ascii_case("none") {
        return Ok(DisplayValue::None);
    }
    const INTERNAL: [(&str, &str); 8] = [
        ("table-row-group", "table-row-group"),
        ("table-header-group", "table-header-group"),
        ("table-footer-group", "table-footer-group"),
        ("table-row", "table-row"),
        ("table-cell", "table-cell"),
        ("table-column-group", "table-column-group"),
        ("table-column", "table-column"),
        ("table-caption", "table-caption"),
    ];
    for (keyword, label) in INTERNAL {
        if name.eq_ignore_ascii_case(keyword) {
            return Ok(DisplayValue::TableInternal(label));
        }
    }
    Err(())
}

fn parse_text_align(input: &mut Parser<'_>) -> Result<TextAlignValue, ()> {
    let Token::Ident(name) = input.next().map_err(|_| ())? else {
        return Err(());
    };
    if name.eq_ignore_ascii_case("start") {
        return Ok(TextAlignValue::Start);
    }
    if name.eq_ignore_ascii_case("end") {
        return Ok(TextAlignValue::End);
    }
    if name.eq_ignore_ascii_case("left") {
        return Ok(TextAlignValue::Left);
    }
    if name.eq_ignore_ascii_case("right") {
        return Ok(TextAlignValue::Right);
    }
    if name.eq_ignore_ascii_case("center") {
        return Ok(TextAlignValue::Center);
    }
    if name.eq_ignore_ascii_case("justify") {
        return Ok(TextAlignValue::Justify);
    }
    if name.eq_ignore_ascii_case("match-parent") {
        return Ok(TextAlignValue::MatchParent);
    }
    if name.eq_ignore_ascii_case("justify-all") {
        return Ok(TextAlignValue::JustifyAll);
    }
    Err(())
}

fn parse_font_style(input: &mut Parser<'_>) -> Result<FontStyleValue, ()> {
    let Token::Ident(name) = input.next().map_err(|_| ())? else {
        return Err(());
    };
    if name.eq_ignore_ascii_case("normal") {
        return Ok(FontStyleValue::Normal);
    }
    if name.eq_ignore_ascii_case("italic") {
        return Ok(FontStyleValue::Italic);
    }
    if name.eq_ignore_ascii_case("oblique") {
        // oblique [ <angle> | none ]?
        let angle = input.try_parse(parse_angle).ok();
        if angle.is_none()
            && (input.try_parse(|i| i.expect_ident_matching("none").map_err(|_| ()))).is_err()
            && input.expect_exhausted().is_err()
        {
            return Err(());
        }
        return Ok(FontStyleValue::Oblique(angle));
    }
    Err(())
}

/// 解析角度并折算为角度制。
fn parse_angle(input: &mut Parser<'_>) -> Result<f32, ()> {
    match input.next().map_err(|_| ())? {
        Token::Dimension { value, unit, .. } => {
            if unit.eq_ignore_ascii_case("deg") {
                Ok(*value)
            } else if unit.eq_ignore_ascii_case("grad") {
                Ok(*value * 360.0 / 400.0)
            } else if unit.eq_ignore_ascii_case("rad") {
                Ok(value.to_degrees())
            } else if unit.eq_ignore_ascii_case("turn") {
                Ok(*value * 360.0)
            } else {
                Err(())
            }
        }
        _ => Err(()),
    }
}

/// `<length-percentage>` 折算为 px；返回 `None` 表示该单位不受支持。
fn length_to_px(value: f32, unit: &str) -> Option<f32> {
    if unit.eq_ignore_ascii_case("px") {
        Some(value)
    } else if unit.eq_ignore_ascii_case("pt") {
        // 1pt = 1/72in，1in = 96px（CSS Values 4 绝对长度定义）
        Some(value * 96.0 / 72.0)
    } else if unit.eq_ignore_ascii_case("pc") {
        Some(value * 16.0)
    } else if unit.eq_ignore_ascii_case("in") {
        Some(value * 96.0)
    } else if unit.eq_ignore_ascii_case("cm") {
        Some(value * 96.0 / 2.54)
    } else if unit.eq_ignore_ascii_case("mm") {
        Some(value * 96.0 / 25.4)
    } else if unit.eq_ignore_ascii_case("q") {
        Some(value * 96.0 / 101.6)
    } else {
        None
    }
}

fn parse_font_size(input: &mut Parser<'_>) -> Result<FontSizeValue, ()> {
    match input.next().map_err(|_| ())? {
        // <length> 为 0 时单位可省略（CSS Values 4）
        Token::Number { value: 0.0, .. } => Ok(FontSizeValue::Px(0.0)),
        Token::Number { .. } => Err(()),
        Token::Percentage { unit_value, .. } => Ok(FontSizeValue::Percent(*unit_value)),
        Token::Dimension { value, unit, .. } => {
            if unit.eq_ignore_ascii_case("em") {
                Ok(FontSizeValue::Em(*value))
            } else {
                length_to_px(*value, unit).map(FontSizeValue::Px).ok_or(())
            }
        }
        Token::Ident(name) => {
            if name.eq_ignore_ascii_case("xx-small") {
                Ok(FontSizeValue::Absolute(AbsoluteSize::XxSmall))
            } else if name.eq_ignore_ascii_case("x-small") {
                Ok(FontSizeValue::Absolute(AbsoluteSize::XSmall))
            } else if name.eq_ignore_ascii_case("small") {
                Ok(FontSizeValue::Absolute(AbsoluteSize::Small))
            } else if name.eq_ignore_ascii_case("medium") {
                Ok(FontSizeValue::Absolute(AbsoluteSize::Medium))
            } else if name.eq_ignore_ascii_case("large") {
                Ok(FontSizeValue::Absolute(AbsoluteSize::Large))
            } else if name.eq_ignore_ascii_case("x-large") {
                Ok(FontSizeValue::Absolute(AbsoluteSize::XLarge))
            } else if name.eq_ignore_ascii_case("xx-large") {
                Ok(FontSizeValue::Absolute(AbsoluteSize::XxLarge))
            } else if name.eq_ignore_ascii_case("xxx-large") {
                Ok(FontSizeValue::Absolute(AbsoluteSize::XxxLarge))
            } else if name.eq_ignore_ascii_case("larger") {
                Ok(FontSizeValue::Larger)
            } else if name.eq_ignore_ascii_case("smaller") {
                Ok(FontSizeValue::Smaller)
            } else {
                Err(())
            }
        }
        _ => Err(()),
    }
}

fn parse_font_weight(input: &mut Parser<'_>) -> Result<FontWeightValue, ()> {
    match input.next().map_err(|_| ())? {
        Token::Number { value, .. } => {
            // CSS Fonts 4：<number [1,1000]>，越界无效
            if (1.0..=1000.0).contains(value) {
                Ok(FontWeightValue::Weight(*value))
            } else {
                Err(())
            }
        }
        Token::Ident(name) => {
            if name.eq_ignore_ascii_case("normal") {
                Ok(FontWeightValue::Weight(400.0))
            } else if name.eq_ignore_ascii_case("bold") {
                Ok(FontWeightValue::Weight(700.0))
            } else if name.eq_ignore_ascii_case("bolder") {
                Ok(FontWeightValue::Bolder)
            } else if name.eq_ignore_ascii_case("lighter") {
                Ok(FontWeightValue::Lighter)
            } else {
                Err(())
            }
        }
        _ => Err(()),
    }
}

fn parse_font_family(input: &mut Parser<'_>) -> Result<Vec<FontFamilyValue>, ()> {
    input
        .parse_comma_separated(|input| {
            parse_family_item(input).map_err(|_| ParseError::<()>::unexpected_token())
        })
        .map_err(|_| ())
}

/// 解析一个字体族名：`<string>` 或一串 `<custom-ident>`（以空白分隔）。
fn parse_family_item(input: &mut Parser<'_>) -> Result<FontFamilyValue, ()> {
    let mut tokens = Vec::new();
    while let Ok(token) = input.next_including_whitespace() {
        tokens.push(token.clone());
    }

    // 去掉首尾空白，中部空白折叠为单个空格。
    let mut parts: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    let mut quoted = false;
    for token in &tokens {
        match token {
            Token::WhiteSpace(_) => {
                if let Some(word) = current.take() {
                    parts.push(word);
                }
            }
            Token::Ident(name) => {
                current.get_or_insert_with(String::new).push_str(name);
            }
            Token::QuotedString(text) => {
                if current.is_some() || !parts.is_empty() {
                    // 字符串只能单独成族名（CSS Fonts 4 §2.3 语法）
                    return Err(());
                }
                quoted = true;
                parts.push(text.to_string());
            }
            _ => return Err(()),
        }
    }
    if let Some(word) = current.take() {
        parts.push(word);
    }

    if parts.is_empty() {
        return Err(());
    }
    // 带引号的 "serif" 是名为 serif 的普通族名，不映射 generic
    if parts.len() == 1
        && !quoted
        && let Some(generic) = generic_family(&parts[0])
    {
        return Ok(FontFamilyValue::Generic(generic));
    }
    Ok(FontFamilyValue::Family(parts.join(" ")))
}

/// 无引号的 generic 族关键字
/// （CSS Fonts 4 §2.3：serif、sans-serif、cursive、fantasy、monospace、
/// system-ui、math、emoji、fangsong、ui-serif、ui-sans-serif、ui-monospace、
/// ui-rounded）。
fn generic_family(name: &str) -> Option<&'static str> {
    const GENERICS: [&str; 13] = [
        "serif",
        "sans-serif",
        "cursive",
        "fantasy",
        "monospace",
        "system-ui",
        "math",
        "emoji",
        "fangsong",
        "ui-serif",
        "ui-sans-serif",
        "ui-monospace",
        "ui-rounded",
    ];
    GENERICS
        .into_iter()
        .find(|generic| generic.eq_ignore_ascii_case(name))
}

/// 解析 `<color>`（[CSS Color 4 §5](https://drafts.csswg.org/css-color-4/#color-syntax)）：
/// hex、`rgb()` / `rgba()`、`hsl()` / `hsla()`（现代与 legacy 逗号语法）、
/// 命名色与 `transparent`。
pub(crate) fn parse_color(input: &mut Parser<'_>) -> Result<Rgba, ()> {
    let token = input.next().map_err(|_| ())?;
    match token {
        // `#fff` 这类合法标识符形式的 hash 会词法为 IDHash，与 Hash 同义
        Token::Hash(hash) | Token::IDHash(hash) => parse_hex(hash),
        Token::Ident(name) => {
            if name.eq_ignore_ascii_case("transparent") {
                Ok(Rgba::transparent())
            } else if name.eq_ignore_ascii_case("currentcolor") {
                // 依赖元素上下文，本轮不支持（见模块文档偏差）
                Err(())
            } else {
                named_color(name).ok_or(())
            }
        }
        Token::Function(name) => {
            if name.eq_ignore_ascii_case("rgb") || name.eq_ignore_ascii_case("rgba") {
                input
                    .parse_nested_block(|input| {
                        parse_rgb_arguments(input).map_err(|_| ParseError::<()>::unexpected_token())
                    })
                    .map_err(|_| ())
            } else if name.eq_ignore_ascii_case("hsl") || name.eq_ignore_ascii_case("hsla") {
                input
                    .parse_nested_block(|input| {
                        parse_hsl_arguments(input).map_err(|_| ParseError::<()>::unexpected_token())
                    })
                    .map_err(|_| ())
            } else {
                Err(())
            }
        }
        _ => Err(()),
    }
}

/// CSS Color 4 §6.1 命名色表（148 项，由规范表格生成）。
const NAMED_COLORS: [(&str, u8, u8, u8); 148] = [
    ("aliceblue", 0xf0, 0xf8, 0xff),
    ("antiquewhite", 0xfa, 0xeb, 0xd7),
    ("aqua", 0x00, 0xff, 0xff),
    ("aquamarine", 0x7f, 0xff, 0xd4),
    ("azure", 0xf0, 0xff, 0xff),
    ("beige", 0xf5, 0xf5, 0xdc),
    ("bisque", 0xff, 0xe4, 0xc4),
    ("black", 0x00, 0x00, 0x00),
    ("blanchedalmond", 0xff, 0xeb, 0xcd),
    ("blue", 0x00, 0x00, 0xff),
    ("blueviolet", 0x8a, 0x2b, 0xe2),
    ("brown", 0xa5, 0x2a, 0x2a),
    ("burlywood", 0xde, 0xb8, 0x87),
    ("cadetblue", 0x5f, 0x9e, 0xa0),
    ("chartreuse", 0x7c, 0xfc, 0x00),
    ("chocolate", 0xd2, 0x69, 0x1e),
    ("coral", 0xff, 0x7f, 0x50),
    ("cornflowerblue", 0x64, 0x95, 0xed),
    ("cornsilk", 0xff, 0xf8, 0xdc),
    ("crimson", 0xdc, 0x14, 0x3c),
    ("cyan", 0x00, 0xff, 0xff),
    ("darkblue", 0x00, 0x00, 0x8b),
    ("darkcyan", 0x00, 0x8b, 0x8b),
    ("darkgoldenrod", 0xb8, 0x86, 0x0b),
    ("darkgray", 0xa9, 0xa9, 0xa9),
    ("darkgreen", 0x00, 0x80, 0x00),
    ("darkgrey", 0xa9, 0xa9, 0xa9),
    ("darkkhaki", 0xbd, 0xb7, 0x6b),
    ("darkmagenta", 0x8b, 0x00, 0x8b),
    ("darkolivegreen", 0x55, 0x6b, 0x2f),
    ("darkorange", 0xff, 0x8c, 0x00),
    ("darkorchid", 0x99, 0x32, 0xcc),
    ("darkred", 0x8b, 0x00, 0x00),
    ("darksalmon", 0xe9, 0x96, 0x7a),
    ("darkseagreen", 0x8f, 0xbc, 0x8f),
    ("darkslateblue", 0x48, 0x3d, 0x8b),
    ("darkslategray", 0x2f, 0x4f, 0x4f),
    ("darkslategrey", 0x2f, 0x4f, 0x4f),
    ("darkturquoise", 0x00, 0xce, 0xd1),
    ("darkviolet", 0x94, 0x00, 0xd3),
    ("deeppink", 0xff, 0x14, 0x93),
    ("deepskyblue", 0x00, 0xbf, 0xff),
    ("dimgray", 0x69, 0x69, 0x69),
    ("dimgrey", 0x69, 0x69, 0x69),
    ("dodgerblue", 0x1e, 0x90, 0xff),
    ("firebrick", 0xb2, 0x22, 0x22),
    ("floralwhite", 0xff, 0xfa, 0xf0),
    ("forestgreen", 0x22, 0x8b, 0x22),
    ("fuchsia", 0xff, 0x00, 0xff),
    ("gainsboro", 0xdc, 0xdc, 0xdc),
    ("ghostwhite", 0xf8, 0xf8, 0xff),
    ("gold", 0xff, 0xd7, 0x00),
    ("goldenrod", 0xda, 0xa5, 0x20),
    ("gray", 0x80, 0x80, 0x80),
    ("green", 0x00, 0x80, 0x00),
    ("greenyellow", 0xad, 0xff, 0x2f),
    ("grey", 0x80, 0x80, 0x80),
    ("honeydew", 0xf0, 0xff, 0xf0),
    ("hotpink", 0xff, 0x69, 0xb4),
    ("indianred", 0xcd, 0x5c, 0x5c),
    ("indigo", 0x4b, 0x00, 0x82),
    ("ivory", 0xff, 0xff, 0xf0),
    ("khaki", 0xf0, 0xe6, 0x8c),
    ("lavender", 0xe6, 0xe6, 0xfa),
    ("lavenderblush", 0xff, 0xf0, 0xf5),
    ("lawngreen", 0x7c, 0xfc, 0x00),
    ("lemonchiffon", 0xff, 0xfa, 0xcd),
    ("lightblue", 0xad, 0xd8, 0xe6),
    ("lightcoral", 0xf0, 0x80, 0x80),
    ("lightcyan", 0xe0, 0xff, 0xff),
    ("lightgoldenrodyellow", 0xfa, 0xfa, 0xd2),
    ("lightgray", 0xd3, 0xd3, 0xd3),
    ("lightgreen", 0x90, 0xee, 0x90),
    ("lightgrey", 0xd3, 0xd3, 0xd3),
    ("lightpink", 0xff, 0xb6, 0xc1),
    ("lightsalmon", 0xff, 0xa0, 0x7a),
    ("lightseagreen", 0x20, 0xb2, 0xaa),
    ("lightskyblue", 0x87, 0xce, 0xfa),
    ("lightslategray", 0x77, 0x88, 0x99),
    ("lightslategrey", 0x77, 0x88, 0x99),
    ("lightsteelblue", 0xb0, 0xc4, 0xde),
    ("lightyellow", 0xff, 0xff, 0xe0),
    ("lime", 0x00, 0xff, 0x00),
    ("limegreen", 0x32, 0xcd, 0x32),
    ("linen", 0xfa, 0xf0, 0xe6),
    ("magenta", 0xff, 0x00, 0xff),
    ("maroon", 0x80, 0x00, 0x00),
    ("mediumaquamarine", 0x66, 0xcd, 0xaa),
    ("mediumblue", 0x00, 0x00, 0xcd),
    ("mediumorchid", 0xba, 0x55, 0xd3),
    ("mediumpurple", 0x93, 0x70, 0xdb),
    ("mediumseagreen", 0x3c, 0xb3, 0x71),
    ("mediumslateblue", 0x7b, 0x68, 0xee),
    ("mediumspringgreen", 0x00, 0xfa, 0x9a),
    ("mediumturquoise", 0x48, 0xd1, 0xcc),
    ("mediumvioletred", 0xc7, 0x15, 0x85),
    ("midnightblue", 0x19, 0x19, 0x70),
    ("mintcream", 0xf5, 0xff, 0xfa),
    ("mistyrose", 0xff, 0xe4, 0xe1),
    ("moccasin", 0xff, 0xe4, 0xb5),
    ("navajowhite", 0xff, 0xde, 0xad),
    ("navy", 0x00, 0x00, 0x80),
    ("oldlace", 0xfd, 0xf5, 0xe6),
    ("olive", 0x80, 0x80, 0x00),
    ("olivedrab", 0x6b, 0x8e, 0x23),
    ("orange", 0xff, 0xa5, 0x00),
    ("orangered", 0xff, 0x45, 0x00),
    ("orchid", 0xda, 0x70, 0xd6),
    ("palegoldenrod", 0xee, 0xe8, 0xaa),
    ("palegreen", 0x98, 0xfb, 0x98),
    ("paleturquoise", 0xaf, 0xee, 0xee),
    ("palevioletred", 0xdb, 0x70, 0x93),
    ("papayawhip", 0xff, 0xef, 0xd5),
    ("peachpuff", 0xff, 0xda, 0xb9),
    ("peru", 0xcd, 0x85, 0x3f),
    ("pink", 0xff, 0xc0, 0xcb),
    ("plum", 0xdd, 0xa0, 0xdd),
    ("powderblue", 0xb0, 0xe0, 0xe6),
    ("purple", 0x80, 0x00, 0x80),
    ("rebeccapurple", 0x66, 0x33, 0x99),
    ("red", 0xff, 0x00, 0x00),
    ("rosybrown", 0xbc, 0x8f, 0x8f),
    ("royalblue", 0x41, 0x69, 0xe1),
    ("saddlebrown", 0x8b, 0x45, 0x13),
    ("salmon", 0xfa, 0x80, 0x72),
    ("sandybrown", 0xf4, 0xa4, 0x60),
    ("seagreen", 0x2e, 0x8b, 0x57),
    ("seashell", 0xff, 0xf5, 0xee),
    ("sienna", 0xa0, 0x52, 0x2d),
    ("silver", 0xc0, 0xc0, 0xc0),
    ("skyblue", 0x87, 0xce, 0xeb),
    ("slateblue", 0x6a, 0x5a, 0xcd),
    ("slategray", 0x70, 0x80, 0x90),
    ("slategrey", 0x70, 0x80, 0x90),
    ("snow", 0xff, 0xfa, 0xfa),
    ("springgreen", 0x00, 0xff, 0x7f),
    ("steelblue", 0x46, 0x82, 0xb4),
    ("tan", 0xd2, 0xb4, 0x8c),
    ("teal", 0x00, 0x80, 0x80),
    ("thistle", 0xd8, 0xbf, 0xd8),
    ("tomato", 0xff, 0x63, 0x47),
    ("turquoise", 0x40, 0xe0, 0xd0),
    ("violet", 0xee, 0x82, 0xee),
    ("wheat", 0xf5, 0xde, 0xb3),
    ("white", 0xff, 0xff, 0xff),
    ("whitesmoke", 0xf5, 0xf5, 0xf5),
    ("yellow", 0xff, 0xff, 0x00),
    ("yellowgreen", 0x9a, 0xcd, 0x32),
];

fn named_color(name: &str) -> Option<Rgba> {
    NAMED_COLORS
        .iter()
        .find(|(keyword, ..)| keyword.eq_ignore_ascii_case(name))
        .map(|&(_, r, g, b)| Rgba::opaque(r, g, b))
}

/// hex 记法：`#rgb` `#rgba` `#rrggbb` `#rrggbbaa`（CSS Color 4 §5.2）。
fn parse_hex(hash: &str) -> Result<Rgba, ()> {
    fn hex_digit(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    }
    let bytes = hash.as_bytes();
    let parse_pair = |slice: &[u8]| -> Option<u8> {
        Some(hex_digit(*slice.first()?)? * 16 + hex_digit(*slice.get(1)?)?)
    };
    match bytes.len() {
        3 => {
            let digits: Option<Vec<u8>> = bytes.iter().map(|&c| hex_digit(c)).collect();
            let d = digits.ok_or(())?;
            Ok(Rgba::opaque(d[0] * 17, d[1] * 17, d[2] * 17))
        }
        4 => {
            let digits: Option<Vec<u8>> = bytes.iter().map(|&c| hex_digit(c)).collect();
            let d = digits.ok_or(())?;
            Ok(Rgba {
                r: d[0] * 17,
                g: d[1] * 17,
                b: d[2] * 17,
                a: d[3] * 17,
            })
        }
        6 => Ok(Rgba::opaque(
            parse_pair(&bytes[0..2]).ok_or(())?,
            parse_pair(&bytes[2..4]).ok_or(())?,
            parse_pair(&bytes[4..6]).ok_or(())?,
        )),
        8 => Ok(Rgba {
            r: parse_pair(&bytes[0..2]).ok_or(())?,
            g: parse_pair(&bytes[2..4]).ok_or(())?,
            b: parse_pair(&bytes[4..6]).ok_or(())?,
            a: parse_pair(&bytes[6..8]).ok_or(())?,
        }),
        _ => Err(()),
    }
}

/// 颜色分量：数字或百分比，可选 `none` 视为本轮不支持。
enum Component {
    Number(f32),
    Percentage(f32),
}

fn parse_component(input: &mut Parser<'_>) -> Result<Component, ()> {
    match input.next().map_err(|_| ())? {
        Token::Number { value, .. } => Ok(Component::Number(*value)),
        Token::Percentage { unit_value, .. } => Ok(Component::Percentage(*unit_value * 100.0)),
        _ => Err(()),
    }
}

/// alpha 分量：数字（0–1）或百分比（0–100%）。
fn parse_alpha(input: &mut Parser<'_>) -> Result<f32, ()> {
    let component = parse_component(input)?;
    Ok(match component {
        Component::Number(value) => value,
        Component::Percentage(value) => value / 100.0,
    })
}

/// 分量 → 8bit 通道：数字按 0–255、百分比按 0–100% 折算后 clamp 并四舍五入
/// （CSS Color 4：rgb() 分量超出范围时 clamp 到设备无关 0–255）。
fn component_to_byte(component: Component) -> u8 {
    let raw = match component {
        Component::Number(value) => value,
        Component::Percentage(value) => value * 255.0 / 100.0,
    };
    raw.clamp(0.0, 255.0).round() as u8
}

/// rgb() / rgba() 的参数：现代空白语法（可带 `/ alpha`）与 legacy 逗号语法
/// （CSS Color 4 §5.3）。
fn parse_rgb_arguments(input: &mut Parser<'_>) -> Result<Rgba, ()> {
    let first = parse_component(input)?;
    let comma = input.try_parse(Parser::expect_comma).is_ok();

    let (red, green, blue, alpha) = if comma {
        let green = parse_component(input)?;
        input.expect_comma().map_err(|_| ())?;
        let blue = parse_component(input)?;
        let alpha = if input.try_parse(Parser::expect_comma).is_ok() {
            parse_alpha(input)?
        } else {
            1.0
        };
        (first, green, blue, alpha)
    } else {
        let green = parse_component(input)?;
        let blue = parse_component(input)?;
        let alpha = if input.try_parse(|i| i.expect_delim('/')).is_ok() {
            parse_alpha(input)?
        } else {
            1.0
        };
        (first, green, blue, alpha)
    };

    if input.expect_exhausted().is_err() {
        return Err(());
    }
    Ok(Rgba {
        r: component_to_byte(red),
        g: component_to_byte(green),
        b: component_to_byte(blue),
        a: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
    })
}

/// hsl() / hsla() 的参数（CSS Color 4 §7）。
fn parse_hsl_arguments(input: &mut Parser<'_>) -> Result<Rgba, ()> {
    let hue = parse_hue(input)?;
    let comma = input.try_parse(Parser::expect_comma).is_ok();
    let (saturation, lightness, alpha) = if comma {
        let saturation = parse_percentage(input)?;
        input.expect_comma().map_err(|_| ())?;
        let lightness = parse_percentage(input)?;
        let alpha = if input.try_parse(Parser::expect_comma).is_ok() {
            parse_alpha(input)?
        } else {
            1.0
        };
        (saturation, lightness, alpha)
    } else {
        let saturation = parse_percentage(input)?;
        let lightness = parse_percentage(input)?;
        let alpha = if input.try_parse(|i| i.expect_delim('/')).is_ok() {
            parse_alpha(input)?
        } else {
            1.0
        };
        (saturation, lightness, alpha)
    };
    if input.expect_exhausted().is_err() {
        return Err(());
    }
    Ok(hsl_to_rgb(
        hue,
        saturation / 100.0,
        lightness / 100.0,
        alpha,
    ))
}

/// `<hue>`：裸数字（度）或角度 dimension。
fn parse_hue(input: &mut Parser<'_>) -> Result<f32, ()> {
    match input.next().map_err(|_| ())? {
        Token::Number { value, .. } => Ok(*value),
        Token::Dimension { value, unit, .. } => {
            if unit.eq_ignore_ascii_case("deg") {
                Ok(*value)
            } else if unit.eq_ignore_ascii_case("grad") {
                Ok(*value * 360.0 / 400.0)
            } else if unit.eq_ignore_ascii_case("rad") {
                Ok(value.to_degrees())
            } else if unit.eq_ignore_ascii_case("turn") {
                Ok(*value * 360.0)
            } else {
                Err(())
            }
        }
        _ => Err(()),
    }
}

/// 百分比的数值（`50%` → `50.0`），与 [`Component::Percentage`] 一致。
fn parse_percentage(input: &mut Parser<'_>) -> Result<f32, ()> {
    match input.next().map_err(|_| ())? {
        Token::Percentage { unit_value, .. } => Ok(*unit_value * 100.0),
        _ => Err(()),
    }
}

/// HSL → sRGB（CSS Color 4 §7.1 的转换算法）。
fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32, alpha: f32) -> Rgba {
    let t2 = if lightness <= 0.5 {
        lightness * (1.0 + saturation)
    } else {
        lightness + saturation - lightness * saturation
    };
    let t1 = 2.0 * lightness - t2;
    let channel = |offset: f32| -> u8 {
        let h = (hue.rem_euclid(360.0) + offset).rem_euclid(360.0);
        let value = if h < 60.0 {
            t1 + (t2 - t1) * h / 60.0
        } else if h < 180.0 {
            t2
        } else if h < 240.0 {
            t1 + (t2 - t1) * (240.0 - h) / 60.0
        } else {
            t1
        };
        (value.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    Rgba {
        r: channel(120.0),
        g: channel(0.0),
        b: channel(-120.0),
        a: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
    }
}

#[allow(clippy::too_many_lines)]
#[cfg(test)]
mod tests {
    use super::*;

    /// 单值解析入口：解析整段文本后要求耗尽。
    fn parse_typed(property: PropertyId, text: &str) -> Result<PropertyValue, ()> {
        let mut parser = Parser::new(text);
        let value = parse_property_value(property, &mut parser)?;
        parser.expect_exhausted().map_err(|_| ())?;
        Ok(value)
    }

    fn parse_one(property: PropertyId, text: &str) -> PropertyValue {
        parse_typed(property, text).expect("value should parse")
    }

    fn parse_err(property: PropertyId, text: &str) {
        assert!(
            parse_typed(property, text).is_err(),
            "{text} should not parse"
        );
    }

    #[test]
    fn property_id_lookup_is_ascii_case_insensitive() {
        assert_eq!(
            PropertyId::from_name("FONT-SIZE"),
            Some(PropertyId::FontSize)
        );
        assert_eq!(PropertyId::from_name("color"), Some(PropertyId::Color));
        assert_eq!(PropertyId::from_name("grid-area"), None);
        assert_eq!(PropertyId::FontSize.as_str(), "font-size");
    }

    #[test]
    fn color_hex_forms() {
        assert_eq!(
            parse_one(PropertyId::Color, "#fff"),
            PropertyValue::Color(Rgba::opaque(255, 255, 255))
        );
        assert_eq!(
            parse_one(PropertyId::Color, "#0000"),
            PropertyValue::Color(Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 0
            })
        );
        assert_eq!(
            parse_one(PropertyId::Color, "#336699"),
            PropertyValue::Color(Rgba::opaque(0x33, 0x66, 0x99))
        );
        assert_eq!(
            parse_one(PropertyId::Color, "#33669980"),
            PropertyValue::Color(Rgba {
                r: 0x33,
                g: 0x66,
                b: 0x99,
                a: 0x80
            })
        );
        parse_err(PropertyId::Color, "#ff");
        parse_err(PropertyId::Color, "#ffggzz");
    }

    #[test]
    fn color_named_and_transparent() {
        assert_eq!(
            parse_one(PropertyId::Color, "rebeccapurple"),
            PropertyValue::Color(Rgba::opaque(0x66, 0x33, 0x99))
        );
        assert_eq!(
            parse_one(PropertyId::Color, "BLACK"),
            PropertyValue::Color(Rgba::opaque(0, 0, 0))
        );
        assert_eq!(
            parse_one(PropertyId::Color, "transparent"),
            PropertyValue::Color(Rgba::transparent())
        );
        parse_err(PropertyId::Color, "notacolor");
        parse_err(PropertyId::Color, "currentcolor");
    }

    #[test]
    fn color_rgb_legacy_and_modern() {
        assert_eq!(
            parse_one(PropertyId::Color, "rgb(255, 0, 128)"),
            PropertyValue::Color(Rgba::opaque(255, 0, 128))
        );
        assert_eq!(
            parse_one(PropertyId::Color, "rgba(255, 0, 128, 0.5)"),
            PropertyValue::Color(Rgba {
                r: 255,
                g: 0,
                b: 128,
                a: 128
            })
        );
        assert_eq!(
            parse_one(PropertyId::Color, "rgb(255 0 128)"),
            PropertyValue::Color(Rgba::opaque(255, 0, 128))
        );
        assert_eq!(
            parse_one(PropertyId::Color, "rgb(255 0 128 / 50%)"),
            PropertyValue::Color(Rgba {
                r: 255,
                g: 0,
                b: 128,
                a: 128
            })
        );
        assert_eq!(
            parse_one(PropertyId::Color, "rgb(50% 0% 100%)"),
            PropertyValue::Color(Rgba::opaque(128, 0, 255))
        );
        // 超范围 clamp（CSS Color 4）
        assert_eq!(
            parse_one(PropertyId::Color, "rgb(-20 300 128)"),
            PropertyValue::Color(Rgba::opaque(0, 255, 128))
        );
        parse_err(PropertyId::Color, "rgb(255 0)");
        parse_err(PropertyId::Color, "rgb(255, 0, 128 0.5)");
    }

    #[test]
    fn color_hsl() {
        assert_eq!(
            parse_one(PropertyId::Color, "hsl(120, 100%, 50%)"),
            PropertyValue::Color(Rgba::opaque(0, 255, 0))
        );
        assert_eq!(
            parse_one(PropertyId::Color, "hsl(120 100% 50%)"),
            PropertyValue::Color(Rgba::opaque(0, 255, 0))
        );
        assert_eq!(
            parse_one(PropertyId::Color, "hsla(0, 0%, 0%, 0.5)"),
            PropertyValue::Color(Rgba {
                r: 0,
                g: 0,
                b: 0,
                a: 128
            })
        );
        // 色相取模
        assert_eq!(
            parse_one(PropertyId::Color, "hsl(480 100% 50%)"),
            PropertyValue::Color(Rgba::opaque(0, 255, 0))
        );
        parse_err(PropertyId::Color, "hsl(120 50 50%)");
    }

    #[test]
    fn font_size_forms() {
        assert_eq!(
            parse_one(PropertyId::FontSize, "16px"),
            PropertyValue::FontSize(FontSizeValue::Px(16.0))
        );
        assert_eq!(
            parse_one(PropertyId::FontSize, "12pt"),
            PropertyValue::FontSize(FontSizeValue::Px(16.0))
        );
        assert_eq!(
            parse_one(PropertyId::FontSize, "1.5em"),
            PropertyValue::FontSize(FontSizeValue::Em(1.5))
        );
        assert_eq!(
            parse_one(PropertyId::FontSize, "120%"),
            PropertyValue::FontSize(FontSizeValue::Percent(1.2))
        );
        assert_eq!(
            parse_one(PropertyId::FontSize, "0"),
            PropertyValue::FontSize(FontSizeValue::Px(0.0))
        );
        assert_eq!(
            parse_one(PropertyId::FontSize, "x-large"),
            PropertyValue::FontSize(FontSizeValue::Absolute(AbsoluteSize::XLarge))
        );
        assert_eq!(
            parse_one(PropertyId::FontSize, "larger"),
            PropertyValue::FontSize(FontSizeValue::Larger)
        );
        parse_err(PropertyId::FontSize, "16");
        parse_err(PropertyId::FontSize, "2rem");
        parse_err(PropertyId::FontSize, "medium small");
    }

    #[test]
    fn absolute_size_table_matches_spec_factors() {
        // CSS Fonts 4 §2.5.1 scaling factors
        assert_eq!(AbsoluteSize::XxSmall.factor(), 0.6);
        assert_eq!(AbsoluteSize::XSmall.factor(), 0.75);
        assert_eq!(AbsoluteSize::Small.factor(), 8.0 / 9.0);
        assert_eq!(AbsoluteSize::Medium.factor(), 1.0);
        assert_eq!(AbsoluteSize::Large.factor(), 1.2);
        assert_eq!(AbsoluteSize::XLarge.factor(), 1.5);
        assert_eq!(AbsoluteSize::XxLarge.factor(), 2.0);
        assert_eq!(AbsoluteSize::XxxLarge.factor(), 3.0);
    }

    #[test]
    fn font_weight_forms() {
        assert_eq!(
            parse_one(PropertyId::FontWeight, "400"),
            PropertyValue::FontWeight(FontWeightValue::Weight(400.0))
        );
        assert_eq!(
            parse_one(PropertyId::FontWeight, "normal"),
            PropertyValue::FontWeight(FontWeightValue::Weight(400.0))
        );
        assert_eq!(
            parse_one(PropertyId::FontWeight, "bold"),
            PropertyValue::FontWeight(FontWeightValue::Weight(700.0))
        );
        assert_eq!(
            parse_one(PropertyId::FontWeight, "bolder"),
            PropertyValue::FontWeight(FontWeightValue::Bolder)
        );
        parse_err(PropertyId::FontWeight, "0");
        parse_err(PropertyId::FontWeight, "1001");
        parse_err(PropertyId::FontWeight, "bold 700");
    }

    #[test]
    fn font_family_forms() {
        assert_eq!(
            parse_one(PropertyId::FontFamily, "serif"),
            PropertyValue::FontFamily(vec![FontFamilyValue::Generic("serif")])
        );
        assert_eq!(
            parse_one(PropertyId::FontFamily, "SANS-SERIF"),
            PropertyValue::FontFamily(vec![FontFamilyValue::Generic("sans-serif")])
        );
        // 带引号的 serif 是普通族名，不是 generic
        assert_eq!(
            parse_one(PropertyId::FontFamily, "\"serif\""),
            PropertyValue::FontFamily(vec![FontFamilyValue::Family("serif".into())])
        );
        assert_eq!(
            parse_one(PropertyId::FontFamily, "Times New Roman, Arial, sans-serif"),
            PropertyValue::FontFamily(vec![
                FontFamilyValue::Family("Times New Roman".into()),
                FontFamilyValue::Family("Arial".into()),
                FontFamilyValue::Generic("sans-serif"),
            ])
        );
        parse_err(PropertyId::FontFamily, "");
        parse_err(PropertyId::FontFamily, "Times \"New\" Roman");
        parse_err(PropertyId::FontFamily, "12px");
    }

    #[test]
    fn keyword_properties() {
        assert_eq!(
            parse_one(PropertyId::Display, "none"),
            PropertyValue::Display(DisplayValue::None)
        );
        assert_eq!(
            parse_one(PropertyId::Display, "FLEX"),
            PropertyValue::Display(DisplayValue::Flex)
        );
        assert_eq!(
            parse_one(PropertyId::Display, "table-cell"),
            PropertyValue::Display(DisplayValue::TableInternal("table-cell"))
        );
        parse_err(PropertyId::Display, "ruby");
        parse_err(PropertyId::Display, "block flex");

        assert_eq!(
            parse_one(PropertyId::TextAlign, "center"),
            PropertyValue::TextAlign(TextAlignValue::Center)
        );
        assert_eq!(
            parse_one(PropertyId::TextAlign, "match-parent"),
            PropertyValue::TextAlign(TextAlignValue::MatchParent)
        );
        parse_err(PropertyId::TextAlign, "middle");

        assert_eq!(
            parse_one(PropertyId::FontStyle, "italic"),
            PropertyValue::FontStyle(FontStyleValue::Italic)
        );
        assert_eq!(
            parse_one(PropertyId::FontStyle, "oblique"),
            PropertyValue::FontStyle(FontStyleValue::Oblique(None))
        );
        assert_eq!(
            parse_one(PropertyId::FontStyle, "oblique 30deg"),
            PropertyValue::FontStyle(FontStyleValue::Oblique(Some(30.0)))
        );
        parse_err(PropertyId::FontStyle, "oblique 30px");
    }

    #[test]
    fn declared_value_css_wide_keywords() {
        fn parse_declared(text: &str) -> Result<(DeclaredValue, bool), ()> {
            let mut parser = Parser::new(text);
            parse_declared_value(PropertyId::Color, &mut parser)
        }
        assert_eq!(
            parse_declared("inherit"),
            Ok((DeclaredValue::CssWide(CssWideKeyword::Inherit), false))
        );
        assert_eq!(
            parse_declared("INITIAL !important"),
            Ok((DeclaredValue::CssWide(CssWideKeyword::Initial), true))
        );
        assert_eq!(
            parse_declared("unset"),
            Ok((DeclaredValue::CssWide(CssWideKeyword::Unset), false))
        );
        assert_eq!(
            parse_declared("revert"),
            Ok((DeclaredValue::CssWide(CssWideKeyword::Revert), false))
        );
        // CSS-wide 关键字后不允许拼接其他值
        assert!(parse_declared("inherit red").is_err());
        assert_eq!(
            parse_declared("#fff"),
            Ok((
                DeclaredValue::Typed(PropertyValue::Color(Rgba::opaque(255, 255, 255))),
                false
            ))
        );
        assert_eq!(
            parse_declared("#fff !important"),
            Ok((
                DeclaredValue::Typed(PropertyValue::Color(Rgba::opaque(255, 255, 255))),
                true
            ))
        );
        assert!(parse_declared("#fff important").is_err());
        assert!(parse_declared("#fff ! bogus").is_err());
    }
}
