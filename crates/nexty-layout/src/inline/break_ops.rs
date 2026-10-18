//! 断行机会：UAX#14 结果 + CSS `word-break` / `overflow-wrap` 增删。
//!
//! CSS Text 3 §5.1 / §5.4：CSS 只允许在 typographic letter unit **之间**增加
//! 断点，不得切开字形簇；`break-all` 的断点参与 min-content，而
//! `overflow-wrap: break-word` 的紧急断点不参与（`anywhere` 参与）。

use unicode_segmentation::UnicodeSegmentation;

use super::uax14::{LineBreakClass, Unit, build_units, uax14_breaks};

/// CSS `word-break`（CSS Text 3 §5.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WordBreak {
    /// 按 UAX#14 的常规断点。
    #[default]
    Normal,
    /// 允许在单词内部断行（但不切开字形簇）。
    BreakAll,
    /// 禁止在单词内部断行。
    KeepAll,
}

/// CSS `overflow-wrap`（CSS Text 3 §5.4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverflowWrap {
    /// 只在 UAX#14 允许的位置断行。
    #[default]
    Normal,
    /// 溢出时可在任意字形簇之间断行；不参与 min-content。
    BreakWord,
    /// 溢出时可在任意字形簇之间断行；参与 min-content。
    Anywhere,
}

/// 断行选项。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BreakOptions {
    /// CSS `word-break`。
    pub word_break: WordBreak,
    /// CSS `overflow-wrap`。
    pub overflow_wrap: OverflowWrap,
}

/// 断点偏移正好落在两个 unit 的边界上时，返回两侧 unit 的下标。
fn unit_boundary(units: &[Unit], offset: usize) -> Option<(usize, usize)> {
    let next = units.partition_point(|unit| unit.start < offset);
    if next == 0 || next >= units.len() {
        return None;
    }
    if units[next].start != offset || units[next - 1].end != offset {
        return None;
    }
    Some((next - 1, next))
}

fn add_cluster_breaks(text: &str, units: &[Unit], breaks: &mut Vec<usize>, word_only: bool) {
    use LineBreakClass::{AL, HL, ID, NU};
    for (offset, _) in text.grapheme_indices(true) {
        if offset == 0 {
            continue;
        }
        let Some((previous, next)) = unit_boundary(units, offset) else {
            continue;
        };
        if word_only {
            let previous = units[previous].class;
            let next = units[next].class;
            let word = |class| matches!(class, AL | HL | NU | ID);
            if !(word(previous) && word(next)) {
                continue;
            }
        }
        breaks.push(offset);
    }
}

fn remove_word_internal_breaks(units: &[Unit], breaks: &mut Vec<usize>) {
    use LineBreakClass::{AL, HL, ID, NU};
    breaks.retain(|&offset| {
        let Some((previous, next)) = unit_boundary(units, offset) else {
            return true;
        };
        let previous = units[previous].class;
        let next = units[next].class;
        let word = |class| matches!(class, AL | HL | NU | ID);
        !(word(previous) && word(next))
    });
}

/// 按 UAX#14（默认解析）返回断行机会的字节偏移，升序、去重，含
/// `text.len()`（LB3 的 eot 断点）。
///
/// CSS `word-break` / `overflow-wrap` 只在此结果上增删：`break-all` 在
/// 相邻 typographic letter unit 之间加断点；`anywhere`/`break-word` 在
/// 每个字形簇边界加紧急断点（后者是否参与 min-content 由调用方决定）；
/// `keep-all` 移除单词内部的隐式断点。
#[must_use]
pub fn break_opportunities(text: &str, options: &BreakOptions) -> Vec<usize> {
    let units = build_units(text);
    let mut breaks = uax14_breaks(&units);
    match options.word_break {
        WordBreak::Normal => {}
        WordBreak::BreakAll => add_cluster_breaks(text, &units, &mut breaks, true),
        WordBreak::KeepAll => remove_word_internal_breaks(&units, &mut breaks),
    }
    if options.overflow_wrap != OverflowWrap::Normal {
        add_cluster_breaks(text, &units, &mut breaks, false);
    }
    breaks.push(text.len());
    breaks.sort_unstable();
    breaks.dedup();
    breaks
}
