//! UCD 数据表（Unicode 18.0.0）：解析与压缩区间查询。
//!
//! 数据来源、版本与 SHA-256 见 `src/data/README.md`。运行时首次使用解析为
//! 压缩区间表（`OnceLock`）；解析时断言文件版本串，避免误升级数据文件。
//!
//! 三分离边界：本模块只负责「码点 → 属性」的原始数据，不含任何断行规则
//! （规则在 `rules.rs`，类定义在 `classes.rs`）。

use std::sync::OnceLock;

use super::classes::LineBreakClass;

/// 码点空间上限（U+10FFFF 之上留一格，便于 `char as u32` 直接索引）。
const LIMIT: usize = 0x11_0000;

const LINE_BREAK_DATA: &str = include_str!("../../data/DerivedLineBreak-18.0.0.txt");
const EAST_ASIAN_DATA: &str = include_str!("../../data/EastAsianWidth-18.0.0.txt");
const PI_PF_DATA: &str = include_str!("../../data/PiPf-18.0.0.txt");
const COMBINING_DATA: &str = include_str!("../../data/Combining-18.0.0.txt");
const EXT_PICT_DATA: &str = include_str!("../../data/ExtPict-18.0.0.txt");
const UNASSIGNED_DATA: &str = include_str!("../../data/Unassigned-18.0.0.txt");

/// 码点 → Line_Break 类的区间表（二分查找）。
pub(super) struct RangeTable {
    pub(super) ranges: Vec<(u32, u32, LineBreakClass)>,
    pub(super) default: LineBreakClass,
}

impl RangeTable {
    pub(super) fn get(&self, codepoint: u32) -> LineBreakClass {
        let index = self
            .ranges
            .partition_point(|&(start, _, _)| start <= codepoint);
        if index > 0 {
            let (_, end, class) = self.ranges[index - 1];
            if codepoint <= end {
                return class;
            }
        }
        self.default
    }
}

/// 布尔属性的合并区间表。
#[derive(Default)]
pub(super) struct BoolRanges {
    pub(super) ranges: Vec<(u32, u32)>,
}

impl BoolRanges {
    pub(super) fn new(mut ranges: Vec<(u32, u32)>) -> Self {
        ranges.sort_unstable();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(ranges.len());
        for (start, end) in ranges {
            if let Some(last) = merged.last_mut()
                && start <= last.1.saturating_add(1)
            {
                last.1 = last.1.max(end);
            } else {
                merged.push((start, end));
            }
        }
        Self { ranges: merged }
    }

    pub(super) fn contains(&self, codepoint: u32) -> bool {
        let index = self
            .ranges
            .partition_point(|&(start, _)| start <= codepoint);
        index > 0 && codepoint <= self.ranges[index - 1].1
    }
}

/// 全部 UCD 属性表。
pub(super) struct Tables {
    pub(super) line_break: RangeTable,
    pub(super) east_asian: BoolRanges,
    pub(super) pi: BoolRanges,
    pub(super) pf: BoolRanges,
    pub(super) combining: BoolRanges,
    pub(super) extended_pictographic: BoolRanges,
    pub(super) unassigned: BoolRanges,
}

static TABLES: OnceLock<Tables> = OnceLock::new();

/// 进程内共享的属性表（首次调用解析）。
pub(super) fn tables() -> &'static Tables {
    TABLES.get_or_init(parse_tables)
}

fn parse_tables() -> Tables {
    assert!(
        LINE_BREAK_DATA.contains("18.0.0")
            && EAST_ASIAN_DATA.contains("18.0.0")
            && COMBINING_DATA.contains("18.0.0")
            && EXT_PICT_DATA.contains("18.0.0")
            && UNASSIGNED_DATA.contains("18.0.0"),
        "line-break data files must stay pinned to Unicode 18.0.0"
    );
    let (pi, pf) = parse_pi_pf();
    Tables {
        line_break: parse_line_break(),
        east_asian: parse_east_asian(),
        pi,
        pf,
        combining: parse_bool_property(COMBINING_DATA),
        extended_pictographic: parse_bool_property(EXT_PICT_DATA),
        unassigned: parse_bool_property(UNASSIGNED_DATA),
    }
}

/// 解析只含单一布尔属性的 `range ; Name` 数据文件（所有行都入表）。
fn parse_bool_property(data: &str) -> BoolRanges {
    let ranges = data
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(parse_property_line)
        .map(|(range, _)| range)
        .collect();
    BoolRanges::new(ranges)
}

fn parse_line_break() -> RangeTable {
    let mut dense = vec![LineBreakClass::XX; LIMIT];
    // @missing 提供未列出码点的默认类（XX/ID/PR）。先铺默认值，
    // 再用显式数据行覆盖。
    for line in LINE_BREAK_DATA.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("# @missing:") else {
            continue;
        };
        let Some((range, name)) = parse_property_line(rest) else {
            continue;
        };
        if let Some(class) = missing_class(name) {
            fill_line_break(&mut dense, range, class);
        }
    }
    for line in LINE_BREAK_DATA.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((range, name)) = parse_property_line(line) else {
            continue;
        };
        let class = LineBreakClass::from_name(name)
            .unwrap_or_else(|| panic!("unknown Line_Break class {name:?}"));
        fill_line_break(&mut dense, range, class);
    }
    RangeTable {
        ranges: compress_line_break(&dense),
        default: LineBreakClass::XX,
    }
}

fn parse_east_asian() -> BoolRanges {
    let mut dense = vec![false; LIMIT];
    for line in EAST_ASIAN_DATA.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("# @missing:") else {
            continue;
        };
        let Some((range, name)) = parse_property_line(rest) else {
            continue;
        };
        fill_bool(&mut dense, range, is_east_asian(name));
    }
    for line in EAST_ASIAN_DATA.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((range, name)) = parse_property_line(line) else {
            continue;
        };
        fill_bool(&mut dense, range, is_east_asian(name));
    }
    BoolRanges::new(compress_bool(&dense))
}

fn parse_pi_pf() -> (BoolRanges, BoolRanges) {
    let mut pi = Vec::new();
    let mut pf = Vec::new();
    for line in PI_PF_DATA.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((range, name)) = parse_property_line(line) else {
            continue;
        };
        match name {
            "Pi" => pi.push(range),
            "Pf" => pf.push(range),
            _ => {}
        }
    }
    (BoolRanges::new(pi), BoolRanges::new(pf))
}

fn parse_property_line(text: &str) -> Option<((u32, u32), &str)> {
    let (range_part, rest) = text.split_once(';')?;
    let name = rest.split('#').next()?.trim();
    let range_part = range_part.trim();
    let (start, end) = match range_part.split_once("..") {
        Some((start, end)) => (
            u32::from_str_radix(start.trim(), 16).ok()?,
            u32::from_str_radix(end.trim(), 16).ok()?,
        ),
        None => {
            let value = u32::from_str_radix(range_part, 16).ok()?;
            (value, value)
        }
    };
    Some(((start, end), name))
}

fn missing_class(name: &str) -> Option<LineBreakClass> {
    Some(match name {
        "Unknown" => LineBreakClass::XX,
        "Prefix_Numeric" => LineBreakClass::PR,
        "Ideographic" => LineBreakClass::ID,
        "Alphabetic" => LineBreakClass::AL,
        "Ambiguous" => LineBreakClass::AI,
        "Combining_Mark" => LineBreakClass::CM,
        "Contingent_Break" => LineBreakClass::CB,
        "Complex_Context" => LineBreakClass::SA,
        "Conditional_Japanese_Starter" => LineBreakClass::CJ,
        "Exclamation" => LineBreakClass::EX,
        "Hyphen" => LineBreakClass::HY,
        "Infix_Separator" => LineBreakClass::IS,
        "Numeric" => LineBreakClass::NU,
        "Postfix_Numeric" => LineBreakClass::PO,
        "Variation_Selector" => LineBreakClass::CM,
        _ => return None,
    })
}

fn is_east_asian(name: &str) -> bool {
    matches!(name, "F" | "W" | "H")
}

fn fill_line_break(dense: &mut [LineBreakClass], (start, end): (u32, u32), class: LineBreakClass) {
    let start = start as usize;
    let end = (end as usize).min(dense.len() - 1);
    if start <= end {
        dense[start..=end].fill(class);
    }
}

fn fill_bool(dense: &mut [bool], (start, end): (u32, u32), value: bool) {
    let start = start as usize;
    let end = (end as usize).min(dense.len() - 1);
    if start <= end {
        dense[start..=end].fill(value);
    }
}

fn compress_line_break(dense: &[LineBreakClass]) -> Vec<(u32, u32, LineBreakClass)> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    for index in 1..=dense.len() {
        if index == dense.len() || dense[index] != dense[start] {
            ranges.push((start as u32, (index - 1) as u32, dense[start]));
            start = index;
        }
    }
    ranges
}

fn compress_bool(dense: &[bool]) -> Vec<(u32, u32)> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    for index in 1..=dense.len() {
        if index == dense.len() || dense[index] != dense[start] {
            if dense[start] {
                ranges.push((start as u32, (index - 1) as u32));
            }
            start = index;
        }
    }
    ranges
}
