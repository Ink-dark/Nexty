//! 文本断行集成测试：真实 `layout_document` 管线上的 `white-space`、`<br>`、
//! UAX#14 断点与 CSS `word-break` / `overflow-wrap`。
//!
//! 行为 ground truth：CSS Text 3 §3.1（white-space）、§5.1（word-break）、
//! §5.4（overflow-wrap）、UAX#14（断行机会）。几何断言只依赖行盒输出，
//! 字符归属用 `Glyph.cluster`（源文本字节偏移）反查。

use nexty_css::{Stylesheet, compute_document_styles, html_ua_stylesheet};
use nexty_dom::{Document, NodeKind};
use nexty_html::ParseOptions;
use nexty_layout::{Fragment, LineFragment, layout_document};
use nexty_text::ParleyTextShaper;
use std::collections::HashMap;
use unicode_segmentation::UnicodeSegmentation;

/// 解析 + 级联（含 UA 默认样式）+ 布局；body margin 归零。
fn layout(html: &str, css: &str, viewport_width: f32) -> (Document, Fragment) {
    let document = nexty_html::parse_document(html, ParseOptions::default());
    let sheet = Stylesheet::parse(&format!("body {{ margin: 0 }} {css}"));
    let styles = compute_document_styles(&document, &[&sheet], Some(html_ua_stylesheet()));
    let shaper = ParleyTextShaper::new();
    let fragment = layout_document(&document, &shaper, &styles, &HashMap::new(), viewport_width)
        .expect("document has a root element");
    (document, fragment)
}

/// 找到 `fragment` 子树中第一个局部名为 `name` 的元素的片段。
fn find<'a>(document: &Document, fragment: &'a Fragment, name: &str) -> Option<&'a Fragment> {
    if matches!(
        document.node(fragment.node),
        Some(NodeKind::Element(data)) if data.name == name
    ) {
        return Some(fragment);
    }
    fragment
        .children
        .iter()
        .find_map(|child| find(document, child, name))
}

fn glyph_count(line: &LineFragment) -> usize {
    line.runs.iter().map(|run| run.glyphs.len()).sum()
}

/// 行的自然宽度（glyph / 图片 / 原子盒右缘最大值）。
fn natural_width(line: &LineFragment) -> f32 {
    let mut width = 0.0_f32;
    for run in &line.runs {
        for glyph in &run.glyphs {
            width = width.max(glyph.x + glyph.advance);
        }
    }
    for image in &line.images {
        width = width.max(image.x + image.width);
    }
    for atomic in &line.boxes {
        width = width.max(atomic.border_box.x + atomic.border_box.width);
    }
    width
}

/// 行首字形在源文本中的字符（`cluster` 是整形输入的字节偏移）。
fn first_char(line: &LineFragment, text: &str) -> Option<char> {
    let run = line.runs.first()?;
    let glyph = run.glyphs.first()?;
    text.get(glyph.cluster as usize..)?.chars().next()
}

/// 行首字形的源文本字节偏移。
fn first_cluster(line: &LineFragment) -> Option<usize> {
    Some(line.runs.first()?.glyphs.first()?.cluster as usize)
}

// ---------------------------------------------------------------------------
// white-space：每个取值一个用例（CSS Text 3 §3.1）
// ---------------------------------------------------------------------------

/// `normal`：空格/制表/换行折叠为单个空格。
#[test]
fn white_space_normal_collapses_whitespace() {
    let (document, root) = layout("<p>a    b\nc\td</p>", "", 400.0);
    let p = find(&document, &root, "p").expect("p");
    assert_eq!(p.lines.len(), 1, "折叠后单行");
    assert_eq!(glyph_count(&p.lines[0]), 7, "a b c d → 4 字符 + 3 空格");
}

/// `nowrap`：折叠空白且不产生软断点（单行溢出）。
#[test]
fn white_space_nowrap_keeps_single_line() {
    let (document, root) = layout(
        "<p style=\"white-space: nowrap\">aaa bbb ccc ddd eee fff ggg</p>",
        "",
        60.0,
    );
    let p = find(&document, &root, "p").expect("p");
    assert_eq!(p.lines.len(), 1, "nowrap 忽略所有软断点");
    assert!(natural_width(&p.lines[0]) > 60.0, "单行溢出");
}

/// `pre`：空格与 `\n` 逐个保留；`\n` 是硬断行。
#[test]
fn white_space_pre_preserves_spaces_and_newlines() {
    let (document, root) = layout("<pre>a  b\nc</pre>", "", 400.0);
    let pre = find(&document, &root, "pre").expect("pre");
    assert_eq!(pre.lines.len(), 2, "一个 \\n → 两行");
    assert_eq!(glyph_count(&pre.lines[0]), 4, "a + 两个空格 + b 全部保留");
    assert_eq!(glyph_count(&pre.lines[1]), 1);
    assert_eq!(first_cluster(&pre.lines[1]), Some(0), "第二行从行首开始");
}

/// `<pre>` 行数 = `\n` 数 + 1（含尾随换行）。
#[test]
fn pre_line_count_equals_newline_count_plus_one() {
    let (document, root) = layout("<pre>a\nb\nc</pre>", "", 400.0);
    let pre = find(&document, &root, "pre").expect("pre");
    assert_eq!(pre.lines.len(), 3);

    let (document, root) = layout("<pre>a\n</pre>", "", 400.0);
    let pre = find(&document, &root, "pre").expect("pre");
    assert_eq!(pre.lines.len(), 2, "尾随换行保留空行");
}

/// `pre-wrap`：保留空白且允许软换行。
#[test]
fn white_space_pre_wrap_preserves_and_wraps() {
    let (document, root) = layout("<p style=\"white-space: pre-wrap\">aa    bb</p>", "", 400.0);
    let p = find(&document, &root, "p").expect("p");
    assert_eq!(p.lines.len(), 1);
    assert_eq!(
        glyph_count(&p.lines[0]),
        8,
        "两个字符 + 四个空格 + 两个字符"
    );

    // 保留的空格处可换行（窄容器）
    let (document, root) = layout(
        "<p style=\"white-space: pre-wrap\">aaaa    bbbb    cccc</p>",
        "",
        60.0,
    );
    let p = find(&document, &root, "p").expect("p");
    assert!(p.lines.len() > 1, "pre-wrap 允许软换行");
}

/// `pre-line`：空格折叠，`\n` 保留为硬断行。
#[test]
fn white_space_pre_line_collapses_spaces_preserves_breaks() {
    let (document, root) = layout("<p style=\"white-space: pre-line\">a   b\nc</p>", "", 400.0);
    let p = find(&document, &root, "p").expect("p");
    assert_eq!(p.lines.len(), 2, "\\n 保留为硬断行");
    assert_eq!(glyph_count(&p.lines[0]), 3, "多个空格折叠为一个");
    assert_eq!(glyph_count(&p.lines[1]), 1);
}

/// `break-spaces`：保留空格（含行尾参与宽度）且每个空格后可换行。
#[test]
fn white_space_break_spaces_preserves_and_wraps() {
    let (document, root) = layout(
        "<p style=\"white-space: break-spaces\">aa    bb</p>",
        "",
        400.0,
    );
    let p = find(&document, &root, "p").expect("p");
    assert_eq!(p.lines.len(), 1);
    assert_eq!(glyph_count(&p.lines[0]), 8, "空格全部保留");

    let (document, root) = layout(
        "<p style=\"white-space: break-spaces\">aa    bb    cc</p>",
        "",
        40.0,
    );
    let p = find(&document, &root, "p").expect("p");
    assert!(p.lines.len() > 1, "空格后可换行");
}

// ---------------------------------------------------------------------------
// <br> 硬断行
// ---------------------------------------------------------------------------

/// `<br>` 强制换行，第二行 x 从 0 起；不受 `nowrap` 影响。
#[test]
fn br_breaks_line_even_under_nowrap() {
    for style in ["", "white-space: nowrap"] {
        let (document, root) = layout(&format!("<p style=\"{style}\">a<br>b</p>"), "", 400.0);
        let p = find(&document, &root, "p").expect("p");
        assert_eq!(p.lines.len(), 2, "style={style:?} 时 <br> 仍硬断");
        let second = &p.lines[1];
        let x = second
            .runs
            .first()
            .and_then(|run| run.glyphs.first())
            .map(|glyph| glyph.x)
            .expect("second line glyph");
        assert!(x.abs() < 0.01, "第二行首字形 x 从 0 起，实际 {x}");
    }
}

/// 连续 `<br>` 产生空行（与 `\n` 数 + 1 的语义一致）。
#[test]
fn consecutive_br_produces_empty_lines() {
    let (document, root) = layout("<p>a<br><br>b</p>", "", 400.0);
    let p = find(&document, &root, "p").expect("p");
    assert_eq!(p.lines.len(), 3, "a / 空行 / b");
    assert!(p.lines[1].runs.is_empty());
}

// ---------------------------------------------------------------------------
// UAX#14 接入后的真实断行（CSS Text 3 §7）
// ---------------------------------------------------------------------------

/// 120 字中文在 200px 视口下折成多行，每行自然宽不超过容器 + 0.5。
#[test]
fn chinese_text_wraps_at_viewport_width() {
    let text = "汉".repeat(120);
    let (document, root) = layout(&format!("<p>{text}</p>"), "", 200.0);
    let p = find(&document, &root, "p").expect("p");
    assert!(
        p.lines.len() >= 3,
        "中文必须折行，实际 {} 行",
        p.lines.len()
    );
    for (index, line) in p.lines.iter().enumerate() {
        let width = natural_width(line);
        assert!(width <= 200.5, "第 {index} 行自然宽 {width} 超出 200 + 0.5");
    }
}

/// 行首不出现收尾标点（UAX#14 LB13/LB15d 的禁则）。
#[test]
fn closing_punctuation_never_starts_a_line() {
    let text = "一二三四五六七八九十甲乙，丙丁戊己庚辛壬癸子丑寅卯。辰巳午未申酉戌亥";
    let (document, root) = layout(&format!("<p>{text}</p>"), "", 200.0);
    let p = find(&document, &root, "p").expect("p");
    assert!(p.lines.len() >= 2);
    const FORBIDDEN: &[char] = &['，', '。', '、', '）', '！', '？', '：', '；', '》', '】'];
    for (index, line) in p.lines.iter().enumerate() {
        if let Some(first) = first_char(line, text) {
            assert!(
                !FORBIDDEN.contains(&first),
                "第 {index} 行以收尾标点 {first:?} 开头"
            );
        }
    }
}

/// `word-break: break-all`：无空白处也能断行；`normal` 同文本单行溢出。
#[test]
fn word_break_break_all_breaks_inside_word() {
    let text = "a".repeat(60);
    let (document, root) = layout(&format!("<p>{text}</p>"), "", 100.0);
    let p = find(&document, &root, "p").expect("p");
    assert_eq!(p.lines.len(), 1, "normal 下长单词不拆");

    let (document, root) = layout(
        &format!("<p style=\"word-break: break-all\">{text}</p>"),
        "",
        100.0,
    );
    let p = find(&document, &root, "p").expect("p");
    assert!(p.lines.len() > 1, "break-all 允许在字符间断行");
    for line in &p.lines {
        assert!(natural_width(line) <= 100.5, "断行后每行不超宽");
    }
}

/// `word-break: keep-all`：中文不折行（单行溢出）。
#[test]
fn word_break_keep_all_keeps_chinese_on_one_line() {
    let text = "汉".repeat(120);
    let (document, root) = layout(
        &format!("<p style=\"word-break: keep-all\">{text}</p>"),
        "",
        200.0,
    );
    let p = find(&document, &root, "p").expect("p");
    assert_eq!(p.lines.len(), 1, "keep-all 移除 CJK 内部断点");
    assert!(natural_width(&p.lines[0]) > 200.0);
}

/// `overflow-wrap: break-word` / `anywhere`：长 URL 溢出时按 cluster 断开。
#[test]
fn overflow_wrap_breaks_long_url() {
    let url = format!("https://example.com/{}", "x".repeat(80));
    for value in ["break-word", "anywhere"] {
        let (document, root) = layout(
            &format!("<p style=\"overflow-wrap: {value}\">{url}</p>"),
            "",
            120.0,
        );
        let p = find(&document, &root, "p").expect("p");
        assert!(p.lines.len() > 1, "overflow-wrap: {value} 应断开长 URL");
        for line in &p.lines {
            assert!(
                natural_width(line) <= 120.5,
                "overflow-wrap: {value} 断行后每行不超宽"
            );
        }
    }

    // normal 只有 URL 标点处的断点：纯 x 段仍溢出容器
    let (document, root) = layout(&format!("<p>{url}</p>"), "", 120.0);
    let p = find(&document, &root, "p").expect("p");
    assert!(
        p.lines.iter().any(|line| natural_width(line) > 120.0),
        "normal 下长 URL 段必须溢出"
    );
}

/// 断行不切开字形簇：组合符行首必须落在 grapheme 边界上。
#[test]
fn break_all_never_splits_clusters() {
    let text = "e\u{0301}".repeat(40);
    let grapheme_starts: Vec<usize> = text.grapheme_indices(true).map(|(i, _)| i).collect();
    let (document, root) = layout(
        &format!("<p style=\"word-break: break-all\">{text}</p>"),
        "",
        80.0,
    );
    let p = find(&document, &root, "p").expect("p");
    assert!(p.lines.len() > 1);
    for (index, line) in p.lines.iter().enumerate() {
        let Some(cluster) = first_cluster(line) else {
            continue;
        };
        assert!(
            grapheme_starts.contains(&cluster),
            "第 {index} 行行首 cluster {cluster} 落在字形簇内部"
        );
    }
}

/// `white-space: pre` 关闭软换行：长行溢出而不是折行。
#[test]
fn pre_disables_soft_wrapping() {
    let (document, root) = layout("<pre>aaaa bbbb cccc dddd</pre>", "", 40.0);
    let pre = find(&document, &root, "pre").expect("pre");
    assert_eq!(pre.lines.len(), 1, "pre 不软换行");
    assert!(natural_width(&pre.lines[0]) > 40.0);
}
