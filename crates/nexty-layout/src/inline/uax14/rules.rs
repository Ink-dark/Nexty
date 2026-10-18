//! UAX#14 断行规则（Unicode 18.0.0，TR14 §6）：LB1–LB31。
//!
//! 规则顺序即规范顺序：`uax14_break_between` 返回「两个相邻 unit 之间是否存在
//! 断行机会」，未命中任何禁止规则即 LB31 的「其余位置允许断行」。
//!
//! 三分离边界：本模块只依赖 `classes`（unit 与类），不感知 CSS 属性与整形。

use super::classes::{LineBreakClass, QuoteKind, Unit};

/// 从 `index` 起向左跳过 SP，返回第一个非 SP unit 的下标。
pub(super) fn last_non_sp(units: &[Unit], mut index: usize) -> Option<usize> {
    loop {
        if units[index].class != LineBreakClass::SP {
            return Some(index);
        }
        if index == 0 {
            return None;
        }
        index -= 1;
    }
}

/// 从边界的左侧匹配 `NU (SY|IS)* (CL|CP)?`。
fn numeric_before(units: &[Unit], index: usize) -> bool {
    let mut cursor = index as isize;
    if cursor >= 0
        && matches!(
            units[cursor as usize].class,
            LineBreakClass::CL | LineBreakClass::CP
        )
    {
        cursor -= 1;
    }
    while cursor >= 0
        && matches!(
            units[cursor as usize].class,
            LineBreakClass::SY | LineBreakClass::IS
        )
    {
        cursor -= 1;
    }
    cursor >= 0 && units[cursor as usize].class == LineBreakClass::NU
}

/// `units[index]` 与 `units[index + 1]` 之间是否允许断行（UAX#14 §6）。
pub(crate) fn uax14_break_between(units: &[Unit], index: usize) -> bool {
    use LineBreakClass::*;

    let previous = units[index];
    let next = units[index + 1];

    // LB4 / LB5 / LB6 / LB7 / LB8 / LB8a
    if previous.class == BK {
        return true;
    }
    if previous.class == CR && next.class == LF {
        return false;
    }
    if matches!(previous.class, CR | LF | NL) {
        return true;
    }
    if matches!(next.class, BK | CR | LF | NL) {
        return false;
    }
    if matches!(next.class, SP | ZW) {
        return false;
    }
    if let Some(last) = last_non_sp(units, index)
        && units[last].class == ZW
    {
        return true;
    }
    if previous.no_break_after {
        return false;
    }

    // LB11 / LB12 / LB12a
    if next.class == WJ || previous.class == WJ {
        return false;
    }
    if previous.class == GL {
        return false;
    }
    if next.class == GL && !matches!(previous.class, SP | HY | HH) {
        return false;
    }

    // LB14（OP SP* ×）与 LB15a/b 先判；LB15c 是允许断行的例外，必须在
    // LB13 之前判，否则 LB13 的 × IS 会把它吃掉。
    if let Some(last) = last_non_sp(units, index)
        && units[last].class == OP
    {
        return false;
    }
    if let Some(last) = last_non_sp(units, index)
        && units[last].quote == QuoteKind::Pi
    {
        let qualifies = last == 0
            || matches!(
                units[last - 1].class,
                BK | CR | LF | NL | OP | QU | GL | SP | ZW
            );
        if qualifies {
            return false;
        }
    }
    if next.quote == QuoteKind::Pf {
        let after = units.get(index + 2).map(|unit| unit.class);
        let prohibited = match after {
            None => true,
            Some(class) => matches!(
                class,
                SP | GL | WJ | CL | QU | CP | EX | IS | SY | BK | CR | LF | NL | ZW
            ),
        };
        if prohibited {
            return false;
        }
    }
    // LB15c：SP ÷ IS NU（覆盖 LB13/LB15d 的禁止）。
    if previous.class == SP
        && next.class == IS
        && units.get(index + 2).is_some_and(|unit| unit.class == NU)
    {
        return true;
    }

    // LB13
    if matches!(next.class, CL | CP | EX | SY) {
        return false;
    }
    // LB15d
    if next.class == IS {
        return false;
    }
    // LB16
    if next.class == NS
        && let Some(last) = last_non_sp(units, index)
        && matches!(units[last].class, CL | CP)
    {
        return false;
    }
    // LB17
    if next.class == B2
        && let Some(last) = last_non_sp(units, index)
        && units[last].class == B2
    {
        return false;
    }
    // LB18
    if previous.class == SP {
        return true;
    }

    // LB19
    if next.class == QU && next.quote != QuoteKind::Pi {
        return false;
    }
    if previous.class == QU && previous.quote != QuoteKind::Pf {
        return false;
    }
    // LB19a
    if next.class == QU && !previous.east_asian {
        return false;
    }
    if next.class == QU {
        let after_non_east_asian = units.get(index + 2).is_none_or(|unit| !unit.east_asian);
        if after_non_east_asian {
            return false;
        }
    }
    if previous.class == QU && !next.east_asian {
        return false;
    }
    if previous.class == QU {
        let before_non_east_asian = index == 0 || !units[index - 1].east_asian;
        if before_non_east_asian {
            return false;
        }
    }

    // LB20
    if next.class == CB || previous.class == CB {
        return true;
    }
    // LB20a
    if matches!(previous.class, HY | HH) && matches!(next.class, AL | HL) {
        let word_initial = index == 0
            || matches!(
                units[index - 1].class,
                BK | CR | LF | NL | SP | ZW | CB | GL
            );
        if word_initial {
            return false;
        }
    }
    // LB21
    if matches!(next.class, BA | HH | HY | NS) || previous.class == BB {
        return false;
    }
    // LB21a
    if matches!(previous.class, HY | HH)
        && next.class != HL
        && index > 0
        && units[index - 1].class == HL
    {
        return false;
    }
    // LB21b
    if previous.class == SY && next.class == HL {
        return false;
    }
    // LB22
    if next.class == IN {
        return false;
    }
    // LB23
    if matches!(previous.class, AL | HL) && next.class == NU {
        return false;
    }
    if previous.class == NU && matches!(next.class, AL | HL) {
        return false;
    }
    // LB23a
    if previous.class == PR && matches!(next.class, ID | EB | EM) {
        return false;
    }
    if matches!(previous.class, ID | EB | EM) && next.class == PO {
        return false;
    }
    // LB24
    if matches!(previous.class, PR | PO) && matches!(next.class, AL | HL) {
        return false;
    }
    if matches!(previous.class, AL | HL) && matches!(next.class, PR | PO) {
        return false;
    }
    // LB25
    if matches!(next.class, PO | PR) && numeric_before(units, index) {
        return false;
    }
    if previous.class == PO || previous.class == PR {
        if next.class == NU {
            return false;
        }
        if next.class == OP {
            let after_op = units.get(index + 2).map(|unit| unit.class);
            if after_op == Some(NU) {
                return false;
            }
            if after_op == Some(IS) && units.get(index + 3).is_some_and(|unit| unit.class == NU) {
                return false;
            }
        }
    }
    if matches!(previous.class, HY | IS) && next.class == NU {
        return false;
    }
    if next.class == NU && matches!(previous.class, NU | SY | IS) && numeric_before(units, index) {
        return false;
    }
    // LB26
    if previous.class == JL && matches!(next.class, JL | JV | H2 | H3) {
        return false;
    }
    if matches!(previous.class, JV | H2) && matches!(next.class, JV | JT) {
        return false;
    }
    if matches!(previous.class, JT | H3) && next.class == JT {
        return false;
    }
    // LB27
    if matches!(previous.class, JL | JV | JT | H2 | H3) && next.class == PO {
        return false;
    }
    if previous.class == PR && matches!(next.class, JL | JV | JT | H2 | H3) {
        return false;
    }
    // LB28
    if matches!(previous.class, AL | HL) && matches!(next.class, AL | HL) {
        return false;
    }
    // LB28a
    let previous_brahmic = matches!(previous.class, AK | AS) || previous.dotted_circle;
    let next_brahmic = matches!(next.class, AK | AS) || next.dotted_circle;
    if previous.class == AP && (matches!(next.class, AK | AS) || next.dotted_circle) {
        return false;
    }
    if previous_brahmic && matches!(next.class, VF | VI) {
        return false;
    }
    if previous.class == VI
        && (next.class == AK || next.dotted_circle)
        && index > 0
        && (matches!(units[index - 1].class, AK | AS) || units[index - 1].dotted_circle)
    {
        return false;
    }
    if previous_brahmic && next_brahmic && units.get(index + 2).is_some_and(|unit| unit.class == VF)
    {
        return false;
    }
    // LB29
    if previous.class == IS && matches!(next.class, AL | HL) {
        return false;
    }
    // LB30
    if matches!(previous.class, AL | HL | NU) && next.class == OP && !next.east_asian {
        return false;
    }
    if previous.class == CP && !previous.east_asian && matches!(next.class, AL | HL | NU) {
        return false;
    }
    // LB30a
    if previous.class == RI && next.class == RI {
        let mut count = 0usize;
        let mut cursor = index as isize;
        while cursor >= 0 && units[cursor as usize].class == RI {
            count += 1;
            cursor -= 1;
        }
        if count % 2 == 1 {
            return false;
        }
    }
    // LB30b（“emoji base (or potential emoji)”）：ID_ExtPictUnassigned
    // 这类潜在 emoji 即使 Line_Break=ID 也要与 EM 粘连。
    if (previous.class == EB || previous.potential_emoji) && next.class == EM {
        return false;
    }
    // LB31
    true
}

/// 全部断行机会的字节偏移（不含 `text.len()` 的 eot 断点）。
pub(crate) fn uax14_breaks(units: &[Unit]) -> Vec<usize> {
    let mut breaks = Vec::new();
    for index in 0..units.len().saturating_sub(1) {
        if uax14_break_between(units, index) {
            breaks.push(units[index + 1].start);
        }
    }
    breaks
}

/// 两段相邻文本之间的边界是否允许断行（行内元素边界的近似）。
///
/// 取左段末尾两个字符与右段开头两个字符拼成上下文后跑 UAX#14；只判左右
/// 交界处。上下文有限是已知近似：跨边界的数字/引号规则（LB15c/LB25 等）
/// 需要更长的左文，这里不回溯整段行内流——边界两侧的样式与内容分属不同
/// 盒子，跨盒子回溯会把断行成本拖成整段平方。
///
/// 返回值：`true` 允许软断行；`false` 禁止（含右段以组合符开头、边界落在
/// LB9 单元内部的情况）。
pub(crate) fn breaks_between_text(left: &str, right: &str) -> bool {
    if left.is_empty() || right.is_empty() {
        return true;
    }
    let tail: Vec<char> = left.chars().rev().take(2).collect();
    let head: Vec<char> = right.chars().take(2).collect();
    let mut context = String::new();
    for character in tail.into_iter().rev() {
        context.push(character);
    }
    let left_len = context.len();
    for character in head {
        context.push(character);
    }
    let units = super::classes::build_units(&context);
    match units.iter().position(|unit| unit.end == left_len) {
        Some(index) => uax14_break_between(&units, index),
        // 边界落在 LB9 单元内部（右侧以组合符开头）：不可断。
        None => false,
    }
}
