//! inline 断行一致性测试（tests-first）。
//!
//! 本文件针对本轮规划的三分离 API 编写：`cluster_map` → `break_ops` →
//! `span_split`。这些 API 与 `nexty_text::Glyph::cluster` 尚未实现，因此本测试
//! 目前**预期编译失败**；实现落地后应逐条转绿。
//!
//! # ground truth
//!
//! - UAX #14《Unicode Line Breaking Algorithm》：断行机会。语料为官方
//!   `LineBreakTest-18.0.0.txt`（见 `tests/data/README.md`），每行用 `÷` / `×`
//!   标记每个字符边界是否存在断行机会，注释里的 `[n.n]` 给出决定该位置的规则
//!   编号。
//! - UAX #29 §3.1：扩展字形簇（extended grapheme cluster）。CSS Text 3 §5.5
//!   要求断行不得切开 typographic character unit，因此 cluster 边界必须落在
//!   字形簇边界上。
//! - CSS Text 3 §5.1：`word-break`；§5.4：`overflow-wrap`；§5.5：断行细节。
//! - CSS Sizing 3：min-content intrinsic size。
//!
//! # 与需求描述的差异（以规范原文为准）
//!
//! 需求描述有两条与规范冲突，本文件按规范原文写，实现不得按冲突描述收敛：
//!
//! 1. 「`word-break: break-all` 允许 cluster 内切断」——CSS Text 3 §5.5：
//!    “CSS never allows soft wrap opportunities within typographic character
//!    units. Thus, the CM and SG Unicode line breaking classes must always be
//!    honored.” `break-all` 只在 typographic letter unit **之间**增加断点
//!    （§5.1），不得切开 `e` + U+0301 这类字形簇。
//! 2. 「`break-all` 不影响 min-content」——CSS Text 3 §5.1 末段：“The effects
//!    of word-break are taken into account when computing intrinsic sizes.”
//!    因此 `break-all` 的断点参与 min-content 计算（min-content 收窄到最宽
//!    cluster）；真正**不**参与 min-content 的是 `overflow-wrap: break-word`
//!    （§5.4），`anywhere` 才参与。
//!
//! 若项目决定偏离规范，应先改这份测试与对应 ADR，而不是让实现与测试各自
//! 表述不同语义。
//!
//! # 被测 API 契约
//!
//! - `cluster_map(text, &ShapedText) -> Vec<Span>`：把一次整形结果的 glyph
//!   cluster 映射为源文本字节区间。区间无缝覆盖 `0..text.len()`、非空、边界
//!   落在 UAX#29 字形簇边界上。整形后端未合并的 ZWJ / 旗帜 / 印度文连写必须
//!   在映射时与字形簇取并集，不能按字符切。
//! - `break_opportunities(text, &BreakOptions) -> Vec<usize>`：按 UAX#14
//!   返回升序、去重的断行字节偏移，含 `text.len()`。默认选项与
//!   `LineBreakTest.txt` 的默认规则一致。
//! - `min_content_width(text, &ShapedText, &BreakOptions) -> f32`：按
//!   min-content 的断行机会计算最窄宽度，CSS px。
//! - `split_spans(text, &[Span], &[usize]) -> Vec<Span>`：只在 cluster
//!   边界切分，输出无缝覆盖输入；逐段拼回必须与输入逐字节相同。
//!
//! 运行：`cargo test -p nexty-layout --test inline_conformance`。

#![cfg(feature = "inline")]

use std::collections::BTreeSet;
use std::fs;
use std::ops::RangeInclusive;
use std::path::Path;

use harfrust::{FontRef, ShapeOptions, ShaperData, UnicodeBuffer};
use nexty_layout::{
    BreakOptions, OverflowWrap, Span, WordBreak, break_opportunities, cluster_map,
    min_content_width, split_spans,
};
use nexty_text::{FontResolver, Glyph, ShapedText};
use unicode_segmentation::UnicodeSegmentation;

/// 测试用字号（CSS px），只影响 min-content 的绝对数值，不影响断点集合。
const FONT_SIZE: f32 = 16.0;

/// 钉死的 UAX #14 官方语料；版本与 SHA-256 见 `tests/data/README.md`。
const LINE_BREAK_TEST: &str = "tests/data/LineBreakTest-18.0.0.txt";
const LINE_BREAK_TEST_HEADER: &str = "# LineBreakTest-18.0.0.txt";
const LINE_BREAK_TEST_CASES: usize = 19_346;

// ---------------------------------------------------------------------------
// 整形 oracle
// ---------------------------------------------------------------------------

/// 用 harfrust 独立整形，构造一个带 glyph cluster 的 [`ShapedText`]。
///
/// harfrust 只作测试 oracle；生产整形仍在 nexty-text。字形位置按
/// `FONT_SIZE / units_per_em` 归一到 CSS px，与 nexty-text 的坐标约定一致。
///
/// 不用 rustybuzz：RUSTSEC-2026-0206 已标记其无人维护，官方替代即 harfrust
/// （本仓经 parley 已在依赖树中）。
fn shape_with_harfrust(text: &str) -> ShapedText {
    let resolver = FontResolver::new();
    let families = ["sans-serif".to_string()];
    let font = resolver
        .resolve_font(&families)
        .expect("inline conformance tests need a system sans-serif font");

    let (glyphs, width) = {
        let font_ref = FontRef::from_index(font.data().as_ref(), font.index())
            .expect("resolved system font must parse as an OpenType face");
        let shaper_data = ShaperData::new(&font_ref);
        let shaper = shaper_data.shaper(&font_ref).build();
        let units_per_em = shaper.units_per_em().max(1) as f32;
        let scale = FONT_SIZE / units_per_em;

        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.guess_segment_properties();
        let output = shaper.shape(buffer, ShapeOptions::new());

        let mut pen_x = 0.0;
        let glyphs = output
            .glyph_infos()
            .iter()
            .zip(output.glyph_positions())
            .map(|(info, position)| {
                let glyph = Glyph {
                    id: info.glyph_id,
                    x: pen_x + position.x_offset as f32 * scale,
                    y: position.y_offset as f32 * scale,
                    advance: position.x_advance as f32 * scale,
                    font: 0,
                    // UAX#14 的断点必须按 glyph cluster 落位；这里把 harfrust
                    // 的源文本字节偏移直接带进来。
                    cluster: info.cluster,
                };
                pen_x += glyph.advance;
                glyph
            })
            .collect::<Vec<_>>();

        (glyphs, pen_x)
    };

    ShapedText {
        glyphs,
        fonts: vec![font],
        width,
        // 行高与断点/宽度断言无关；取 1.2 倍字号即可。
        height: FONT_SIZE * 1.2,
    }
}

// ---------------------------------------------------------------------------
// span 不变量
// ---------------------------------------------------------------------------

fn span_pairs(spans: &[Span]) -> Vec<(usize, usize)> {
    spans
        .iter()
        .map(|span| (span.byte_start, span.byte_end))
        .collect()
}

/// 断言 spans 无缝覆盖整段文本，且逐段拼回与输入完全一致。
///
/// 这是需求 1（覆盖不变量）与需求 4（跨模块无字节丢失/重复）的共同断言。
fn assert_spans_cover(text: &str, spans: &[Span], what: &str) {
    if text.is_empty() {
        assert!(spans.is_empty(), "{what}: empty text must produce no spans");
        return;
    }

    assert!(!spans.is_empty(), "{what}: no spans for {text:?}");
    assert_eq!(
        spans[0].byte_start,
        0,
        "{what}: first span must start at byte 0: {:?}",
        span_pairs(spans)
    );
    for pair in spans.windows(2) {
        assert_eq!(
            pair[0].byte_end,
            pair[1].byte_start,
            "{what}: spans must be seamless (spans[i].byte_end == spans[i+1].byte_start): {:?}",
            span_pairs(spans)
        );
    }
    assert_eq!(
        spans.last().expect("non-empty").byte_end,
        text.len(),
        "{what}: last span must cover text.len() = {}: {:?}",
        text.len(),
        span_pairs(spans)
    );

    for span in spans {
        assert!(
            span.byte_start < span.byte_end,
            "{what}: span must be non-empty: {span:?}"
        );
        assert!(
            span.byte_end <= text.len(),
            "{what}: span past end of text: {span:?}"
        );
        assert!(
            text.is_char_boundary(span.byte_start) && text.is_char_boundary(span.byte_end),
            "{what}: span must lie on UTF-8 char boundaries: {span:?}"
        );
    }

    let rebuilt: String = spans
        .iter()
        .map(|span| &text[span.byte_start..span.byte_end])
        .collect();
    assert_eq!(
        rebuilt.as_str(),
        text,
        "{what}: concatenating spans must reproduce the input byte-for-byte"
    );
}

fn grapheme_boundaries(text: &str) -> BTreeSet<usize> {
    let mut boundaries = BTreeSet::new();
    boundaries.insert(0);
    for (offset, grapheme) in text.grapheme_indices(true) {
        boundaries.insert(offset);
        boundaries.insert(offset + grapheme.len());
    }
    boundaries
}

/// CSS Text 3 §5.5：断行不得切开 typographic character unit。
/// 这里用 UAX#29 扩展字形簇作为该 unit 的下界。
fn assert_grapheme_aligned(text: &str, spans: &[Span], what: &str) {
    let boundaries = grapheme_boundaries(text);
    for span in spans {
        assert!(
            boundaries.contains(&span.byte_start) && boundaries.contains(&span.byte_end),
            "{what}: span {span:?} splits a UAX#29 grapheme cluster in {text:?}; \
             grapheme boundaries are {boundaries:?}"
        );
    }
}

fn cluster_boundaries(spans: &[Span]) -> BTreeSet<usize> {
    spans
        .iter()
        .flat_map(|span| [span.byte_start, span.byte_end])
        .collect()
}

// ---------------------------------------------------------------------------
// 需求 1：glyph cluster 映射
// ---------------------------------------------------------------------------

/// 语料与期望的 cluster 字节区间。
///
/// 期望值来自 UAX#29 扩展字形簇（不是“一个 char 一个 cluster”）：
/// - `cafe\u{301}`：c / a / f / e+◌́，4 个 cluster；
/// - `e\u{301}\u{327}`：e + 两个组合附加符，1 个 cluster；
/// - ZWJ 家庭 emoji：7 个码点，1 个 cluster；
/// - 区域指示符旗帜：2 个码点，1 个 cluster；
/// - 天城文 conjunct क्ष：GB9c（Indic conjunct），1 个 cluster；
/// - U+FB00 ﬀ：预组合连字，1 个 cluster。
const CLUSTER_CASES: &[(&str, &[(usize, usize)])] = &[
    ("cafe\u{301}", &[(0, 1), (1, 2), (2, 3), (3, 6)]),
    ("e\u{301}\u{327}", &[(0, 5)]),
    ("👨\u{200D}👩\u{200D}👧\u{200D}👦", &[(0, 25)]),
    ("🇨🇳", &[(0, 8)]),
    ("क्ष", &[(0, 9)]),
    ("ﬀ", &[(0, 3)]),
];

#[test]
fn glyph_cluster_map_is_seamless_and_never_splits_graphemes() {
    for &(text, expected) in CLUSTER_CASES {
        let shaped = shape_with_harfrust(text);
        let spans = cluster_map(text, &shaped);

        // 需求 1：无缝覆盖，spans[i].byte_end == spans[i+1].byte_start，
        // 末 cluster 覆盖到 text.len()。
        assert_spans_cover(text, &spans, "cluster_map");
        // CSS Text 3 §5.5 + UAX#14 LB9/LB10：不得切开字形簇 / 组合序列。
        assert_grapheme_aligned(text, &spans, "cluster_map");
        assert_eq!(
            span_pairs(&spans).as_slice(),
            expected,
            "cluster_map({text:?}) must respect UAX#29 clusters; \
             a 1-char = 1-cluster implementation would return {} spans",
            text.chars().count()
        );

        // 需求 1 的反例守卫：多码点 cluster 必须严格少于 char 数。
        if text.chars().count() > expected.len() {
            assert!(
                spans.len() < text.chars().count(),
                "cluster_map split {text:?} per char: {} spans for {} chars",
                spans.len(),
                text.chars().count()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 需求 2：UAX#14 断行机会（官方 LineBreakTest.txt）
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct OfficialCase {
    /// 语料文件中的 1-based 行号，失败时用于定位。
    line: usize,
    /// 反序列化后的文本。
    text: String,
    /// 期望的断行字节偏移（含 text.len()）。
    expected: Vec<usize>,
    /// 注释中出现的 UAX#14 规则主编号（`[9.0]` → 9）。
    rules: Vec<u16>,
}

fn load_official_cases() -> Vec<OfficialCase> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(LINE_BREAK_TEST);
    let corpus = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));

    let header = corpus.lines().next().unwrap_or_default();
    assert_eq!(
        header, LINE_BREAK_TEST_HEADER,
        "unexpected LineBreakTest version; update tests/data/README.md, the pinned \
         SHA-256, and this assertion together"
    );

    let cases: Vec<OfficialCase> = corpus
        .lines()
        .enumerate()
        .filter_map(|(index, line)| parse_official_case(line, index + 1))
        .collect();
    assert_eq!(
        cases.len(),
        LINE_BREAK_TEST_CASES,
        "LineBreakTest case count changed; update the pinned version deliberately"
    );
    cases
}

/// 解析一行 `× 2757 × 0020 ÷ 2757 ÷ # ...`。
///
/// marker/codepoint 交错出现：marker 描述它**后面**的位置。行首 marker
/// （恒为 `×`）不是内部断点，跳过；行尾 marker（恒为 `÷`）对应 text.len()。
fn parse_official_case(raw: &str, line: usize) -> Option<OfficialCase> {
    let raw = raw.trim_end_matches('\r');
    if raw.is_empty() || raw.starts_with('#') {
        return None;
    }
    let (data, comment) = raw.split_once('#').unwrap_or((raw, ""));
    let tokens: Vec<&str> = data.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }
    assert_eq!(
        tokens.len() % 2,
        1,
        "LineBreakTest line {line}: expected marker/codepoint/marker alternation, got {tokens:?}"
    );

    let mut text = String::new();
    let mut expected = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if index % 2 == 0 {
            assert!(
                *token == "÷" || *token == "×",
                "LineBreakTest line {line}: {token:?} is not a break marker"
            );
            if index > 0 && *token == "÷" {
                expected.push(text.len());
            }
        } else {
            let codepoint = u32::from_str_radix(token, 16).unwrap_or_else(|_| {
                panic!("LineBreakTest line {line}: invalid hex codepoint {token:?}")
            });
            let character = char::from_u32(codepoint).unwrap_or_else(|| {
                panic!("LineBreakTest line {line}: U+{codepoint:04X} is not a Unicode scalar value")
            });
            text.push(character);
        }
    }

    let mut rules: Vec<u16> = comment
        .split('[')
        .skip(1)
        .filter_map(|rest| rest.split_once(']').map(|(rule, _)| rule))
        .filter_map(|rule| rule.split('.').next()?.parse().ok())
        .collect();
    rules.sort_unstable();
    rules.dedup();

    Some(OfficialCase {
        line,
        text,
        expected,
        rules,
    })
}

fn assert_break_conformance(case: &OfficialCase) {
    let actual = break_opportunities(&case.text, &BreakOptions::default());
    assert_eq!(
        actual, case.expected,
        "UAX#14 LineBreakTest line {} (rules {:?}) failed for {:?}",
        case.line, case.rules, case.text
    );
}

/// 全量官方语料：19,346 条，逐条比对 `÷` / `×` 标记。
#[test]
fn uax14_official_line_break_test() {
    for case in load_official_cases() {
        assert_break_conformance(&case);
    }
}

/// 按规则组过滤同一份钉死语料。
///
/// 官方 LineBreakTest.txt 在每个断点后用 `[n.n]` 标注规则编号，所以规则组
/// 过滤是精确的；组间允许重叠（一条用例可同时覆盖多条规则），全量入口另行
/// 覆盖所有规则。三个入口对应需求里的 LB4-8 / LB9-10 / LB21-31。
fn run_rule_group(label: &str, majors: RangeInclusive<u16>) {
    let cases = load_official_cases();
    let selected: Vec<&OfficialCase> = cases
        .iter()
        .filter(|case| case.rules.iter().any(|rule| majors.contains(rule)))
        .collect();
    assert!(
        selected.len() > 100,
        "{label}: only {} cases tagged with rules {majors:?}; corpus or rule parser is wrong",
        selected.len()
    );
    for case in selected {
        assert_break_conformance(case);
    }
}

#[test]
fn uax14_rules_lb4_to_lb8() {
    run_rule_group("LB4-8", 4..=8);
}

#[test]
fn uax14_rules_lb9_to_lb10() {
    run_rule_group("LB9-10", 9..=10);
}

#[test]
fn uax14_rules_lb21_to_lb31() {
    run_rule_group("LB21-31", 21..=31);
}

// ---------------------------------------------------------------------------
// 需求 3：强制断点语义差异（两个独立用例）
// ---------------------------------------------------------------------------

/// `word-break: break-all`：在 typographic letter unit 之间增加断点，但绝不
/// 切开 cluster；其效果参与 intrinsic size 计算。
///
/// CSS Text 3 §5.1（break-all 把 AL/NU/SA 当作 ID 处理）与 §5.1 末段
/// “The effects of word-break are taken into account when computing intrinsic
/// sizes.”；§5.5 与 UAX#14 LB9 禁止切开组合序列。
#[test]
fn word_break_break_all_breaks_between_clusters_not_inside_them() {
    let clustered = "e\u{301}x";
    let shaped = shape_with_harfrust(clustered);
    let break_all = BreakOptions {
        word_break: WordBreak::BreakAll,
        ..BreakOptions::default()
    };
    let breaks = break_opportunities(clustered, &break_all);

    assert!(
        breaks.contains(&3),
        "break-all must offer a break after the e+U+0301 cluster: {breaks:?}"
    );
    assert_eq!(
        breaks.last().copied(),
        Some(clustered.len()),
        "break opportunities are byte offsets and include end-of-text: {breaks:?}"
    );
    assert!(
        !breaks.contains(&1),
        "break-all must not split the e+U+0301 grapheme (UAX#14 LB9, CSS Text 3 §5.5): {breaks:?}"
    );

    let clusters = cluster_map(clustered, &shaped);
    let boundaries = cluster_boundaries(&clusters);
    for offset in &breaks {
        assert!(
            boundaries.contains(offset),
            "break-all offset {offset} is inside a cluster; cluster boundaries: {boundaries:?}"
        );
    }

    // 独立的两条规范约束：break-all 的断点参与 min-content。
    let word = "abcdefghij";
    let word_shaped = shape_with_harfrust(word);
    let normal_min = min_content_width(word, &word_shaped, &BreakOptions::default());
    let break_all_min = min_content_width(word, &word_shaped, &break_all);
    assert!(
        break_all_min < normal_min,
        "break-all min-content ({break_all_min}) must be narrower than normal ({normal_min}); \
         CSS Text 3 §5.1: word-break effects participate in intrinsic sizes"
    );
}

/// `overflow-wrap: anywhere`：紧急断点参与 min-content；作为对照，
/// `break-word` 的紧急断点不参与 min-content。
///
/// CSS Text 3 §5.4：“Soft wrap opportunities introduced by anywhere are
/// considered when calculating min-content intrinsic sizes.”，而 break-word
/// 是“As for anywhere except that ... not considered”。
#[test]
fn overflow_wrap_anywhere_participates_in_min_content() {
    let text = "abcdefghijklmnopqrstuvwxyz";
    let shaped = shape_with_harfrust(text);
    let normal = BreakOptions::default();
    let anywhere = BreakOptions {
        overflow_wrap: OverflowWrap::Anywhere,
        ..BreakOptions::default()
    };
    let break_word = BreakOptions {
        overflow_wrap: OverflowWrap::BreakWord,
        ..BreakOptions::default()
    };

    let normal_min = min_content_width(text, &shaped, &normal);
    let anywhere_min = min_content_width(text, &shaped, &anywhere);
    let break_word_min = min_content_width(text, &shaped, &break_word);

    assert!(
        anywhere_min < normal_min,
        "anywhere min-content ({anywhere_min}) must be narrower than normal ({normal_min})"
    );
    assert!(
        (break_word_min - normal_min).abs() <= 0.5,
        "break-word must not affect min-content: break-word={break_word_min}, normal={normal_min}"
    );
}

// ---------------------------------------------------------------------------
// 需求 4：跨模块不变量（span 切分逐段拼回原文本）
// ---------------------------------------------------------------------------

#[test]
fn span_split_reassembles_input_without_byte_loss_or_duplication() {
    let texts = [
        "cafe\u{301}",
        "e\u{301}\u{327}",
        "👨\u{200D}👩\u{200D}👧\u{200D}👦",
        "🇨🇳",
        "क्ष",
        "ﬀ",
        "中a\u{301}🇨🇳bc",
        "a b\tc\nd",
    ];
    let option_sets = [
        BreakOptions::default(),
        BreakOptions {
            word_break: WordBreak::BreakAll,
            ..BreakOptions::default()
        },
        BreakOptions {
            word_break: WordBreak::KeepAll,
            ..BreakOptions::default()
        },
        BreakOptions {
            overflow_wrap: OverflowWrap::Anywhere,
            ..BreakOptions::default()
        },
        BreakOptions {
            overflow_wrap: OverflowWrap::BreakWord,
            ..BreakOptions::default()
        },
    ];

    for text in texts {
        let shaped = shape_with_harfrust(text);
        let clusters = cluster_map(text, &shaped);
        assert_spans_cover(text, &clusters, "cluster_map");
        assert_grapheme_aligned(text, &clusters, "cluster_map");
        let boundaries = cluster_boundaries(&clusters);

        for (option_index, options) in option_sets.iter().enumerate() {
            let breaks = break_opportunities(text, options);
            for offset in &breaks {
                assert!(
                    boundaries.contains(offset),
                    "option set {option_index}: break offset {offset} splits a cluster \
                     in {text:?}; cluster boundaries: {boundaries:?}"
                );
            }

            let spans = split_spans(text, &clusters, &breaks);
            assert_spans_cover(text, &spans, "split_spans");
            assert_grapheme_aligned(text, &spans, "split_spans");
            for span in &spans {
                assert!(
                    boundaries.contains(&span.byte_start) && boundaries.contains(&span.byte_end),
                    "split_spans produced {span:?}, which does not align to clusters {clusters:?}"
                );
            }

            // 需求 4 的显式断言：逐段拼回原文本，不丢字节、不重复字节。
            let rebuilt: String = spans
                .iter()
                .map(|span| &text[span.byte_start..span.byte_end])
                .collect();
            assert_eq!(
                rebuilt.as_str(),
                text,
                "option set {option_index}: split_spans lost or duplicated bytes for {text:?}: \
                 {spans:?}"
            );
        }
    }
}
