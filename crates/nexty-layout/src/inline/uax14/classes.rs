//! UAX#14 的 Line_Break 类、字符属性与 LB9/LB10 单元切分。
//!
//! 三分离边界：本模块只做「码点 → 类 / 属性」与「文本 → 单元」的映射，
//! 不含断行规则（`rules.rs`）与数据解析（`data.rs`）。

use super::data::tables;

/// UAX#14 的 Line_Break 类（Unicode 18.0.0，含 AK/AP/AS/VF/VI/HH）。
#[allow(clippy::upper_case_acronyms)] // 类名沿用 UAX#14 的缩写（ZWJ/BK/...）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineBreakClass {
    BK,
    CR,
    LF,
    CM,
    NL,
    SG,
    WJ,
    ZW,
    GL,
    SP,
    ZWJ,
    B2,
    BA,
    BB,
    HY,
    CB,
    CL,
    CP,
    EX,
    IN,
    NS,
    OP,
    QU,
    IS,
    NU,
    PO,
    PR,
    SY,
    AI,
    AL,
    CJ,
    EB,
    EM,
    H2,
    H3,
    HL,
    ID,
    JL,
    JV,
    JT,
    RI,
    SA,
    XX,
    AK,
    AP,
    AS,
    VF,
    VI,
    HH,
}

impl LineBreakClass {
    pub(super) fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "BK" => Self::BK,
            "CR" => Self::CR,
            "LF" => Self::LF,
            "CM" => Self::CM,
            "NL" => Self::NL,
            "SG" => Self::SG,
            "WJ" => Self::WJ,
            "ZW" => Self::ZW,
            "GL" => Self::GL,
            "SP" => Self::SP,
            "ZWJ" => Self::ZWJ,
            "B2" => Self::B2,
            "BA" => Self::BA,
            "BB" => Self::BB,
            "HY" => Self::HY,
            "CB" => Self::CB,
            "CL" => Self::CL,
            "CP" => Self::CP,
            "EX" => Self::EX,
            "IN" => Self::IN,
            "NS" => Self::NS,
            "OP" => Self::OP,
            "QU" => Self::QU,
            "IS" => Self::IS,
            "NU" => Self::NU,
            "PO" => Self::PO,
            "PR" => Self::PR,
            "SY" => Self::SY,
            "AI" => Self::AI,
            "AL" => Self::AL,
            "CJ" => Self::CJ,
            "EB" => Self::EB,
            "EM" => Self::EM,
            "H2" => Self::H2,
            "H3" => Self::H3,
            "HL" => Self::HL,
            "ID" => Self::ID,
            "JL" => Self::JL,
            "JV" => Self::JV,
            "JT" => Self::JT,
            "RI" => Self::RI,
            "SA" => Self::SA,
            "XX" => Self::XX,
            "AK" => Self::AK,
            "AP" => Self::AP,
            "AS" => Self::AS,
            "VF" => Self::VF,
            "VI" => Self::VI,
            "HH" => Self::HH,
            _ => return None,
        })
    }
}

/// `QU` 类字符的引号归属（UAX#14 的 Pi / Pf 区分）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuoteKind {
    None,
    Pi,
    Pf,
}

/// 单个字符解析后的断行属性。
#[derive(Clone, Copy)]
pub(crate) struct CharProps {
    pub(super) class: LineBreakClass,
    pub(super) east_asian: bool,
    pub(super) quote: QuoteKind,
    pub(super) dotted_circle: bool,
    /// LB9：该字符并入前一个 unit（CM/ZWJ，或 SA 类中的组合符）。
    /// VF/VI 等有独立断行类的组合符**不**并入（它们参与 LB28a）。
    pub(super) lb9_joins_previous: bool,
    /// LB30b 的“potential emoji”：未分配的 Extended_Pictographic。
    pub(super) potential_emoji: bool,
}

/// 解析单个字符的断行属性。
pub(crate) fn char_props(character: char) -> CharProps {
    let tables = tables();
    let codepoint = character as u32;
    let raw = tables.line_break.get(codepoint);
    let quote = if raw == LineBreakClass::QU {
        if tables.pi.contains(codepoint) {
            QuoteKind::Pi
        } else if tables.pf.contains(codepoint) {
            QuoteKind::Pf
        } else {
            QuoteKind::None
        }
    } else {
        QuoteKind::None
    };
    // LB1 默认解析：AI/SA/SG/XX → AL，CJ → NS；CB 保持未解析（LB20）。
    let class = match raw {
        LineBreakClass::AI | LineBreakClass::SA | LineBreakClass::SG | LineBreakClass::XX => {
            LineBreakClass::AL
        }
        LineBreakClass::CJ => LineBreakClass::NS,
        other => other,
    };
    CharProps {
        class,
        east_asian: tables.east_asian.contains(codepoint),
        quote,
        dotted_circle: codepoint == 0x25CC,
        // LB9 的“组合字符序列”：CM/ZWJ 按 Line_Break 类并入；SA 类里的
        // 泰文/缅甸文组合符（如 U+0E31 SA_Mn）也要按 GC=Mark 并入。
        // VF/VI 之类有独立断行类的组合符不并入，否则 LB28a 会失效。
        lb9_joins_previous: raw == LineBreakClass::CM
            || (raw == LineBreakClass::SA && tables.combining.contains(codepoint)),
        // 已分配的 ExtPict（如 U+2757）不是 LB30b 的 emoji base；只有
        // 未分配的 ExtPictUnassigned 才算“potential emoji”。
        potential_emoji: tables.extended_pictographic.contains(codepoint)
            && tables.unassigned.contains(codepoint),
    }
}

/// 断行规则的最小作用单元：一个基字符连同其 LB9 组合序列。
#[derive(Clone, Copy)]
pub(crate) struct Unit {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) class: LineBreakClass,
    pub(crate) east_asian: bool,
    pub(crate) quote: QuoteKind,
    pub(crate) dotted_circle: bool,
    pub(crate) potential_emoji: bool,
    /// LB8a：unit 的最后一个字符是 ZWJ。
    pub(crate) no_break_after: bool,
}

/// 按 LB9/LB10 把文本切成单元（字节区间 `start..end`）。
pub(crate) fn build_units(text: &str) -> Vec<Unit> {
    let mut units: Vec<Unit> = Vec::new();
    for (offset, character) in text.char_indices() {
        let props = char_props(character);
        let end = offset + character.len_utf8();
        if props.lb9_joins_previous || props.class == LineBreakClass::ZWJ {
            if let Some(unit) = units.last_mut()
                && !matches!(
                    unit.class,
                    LineBreakClass::BK
                        | LineBreakClass::CR
                        | LineBreakClass::LF
                        | LineBreakClass::NL
                        | LineBreakClass::SP
                        | LineBreakClass::ZW
                )
            {
                // LB9：X (CM|ZWJ)* 视为 X；只更新 unit 末尾与 LB8a 标记。
                unit.end = end;
                unit.no_break_after = props.class == LineBreakClass::ZWJ;
                continue;
            }
            // LB10：孤立的 CM/ZWJ 按 AL 处理。
            units.push(Unit {
                start: offset,
                end,
                class: LineBreakClass::AL,
                east_asian: props.east_asian,
                quote: QuoteKind::None,
                dotted_circle: false,
                potential_emoji: false,
                no_break_after: props.class == LineBreakClass::ZWJ,
            });
            continue;
        }
        units.push(Unit {
            start: offset,
            end,
            class: props.class,
            east_asian: props.east_asian,
            quote: props.quote,
            dotted_circle: props.dotted_circle,
            potential_emoji: props.potential_emoji,
            no_break_after: false,
        });
    }
    units
}
