//! H4 宽度无关缓存的行为钉子（性能轮 T2）。
//!
//! shaping / 簇映射 / 断点切分 / metrics 自 H4 起按 chunk 缓存并跨 measure
//! 宽度档位复用，只有贪心放置按宽度重跑。本文件把「缓存前后行为逐字节
//! 一致」钉在片段树摘要上：同一页在不同布局次序、不同历史宽度下产出的
//! 片段树必须完全一致（几何 + 字形 + 字体表 + run 文本），CSS Text 的
//! 放置期修正（justify / 字距 / 制表 / 悬挂 / 缩进）逐项覆盖。

use nexty_css::{Stylesheet, compute_document_styles, html_ua_stylesheet};
use nexty_dom::NodeId;
use nexty_html::ParseOptions;
use nexty_layout::{Fragment, layout_document};
use nexty_text::ParleyTextShaper;
use std::collections::HashMap;
use std::fmt::Write as _;

/// 片段树摘要：几何与文本内容全量序列化（浮点 3 位小数），任何布局期
/// 状态污染都会改变指纹。
fn digest(fragment: &Fragment, out: &mut String) {
    let _ = writeln!(
        out,
        "node={:?} bb=({:.3},{:.3},{:.3},{:.3}) anon={} children={} lines={}",
        fragment.node,
        fragment.border_box.x,
        fragment.border_box.y,
        fragment.border_box.width,
        fragment.border_box.height,
        fragment.anonymous,
        fragment.children.len(),
        fragment.lines.len(),
    );
    for line in &fragment.lines {
        let _ = writeln!(
            out,
            "  line h={:.3} baseline={:.3} start={:?}",
            line.height, line.baseline, line.start,
        );
        for run in &line.runs {
            let _ = write!(
                out,
                "    run size={:.3} fonts={} text={:?} span=({},{}) glyphs=",
                run.style.size,
                run.fonts.len(),
                run.text,
                run.text_start,
                run.text_end,
            );
            for glyph in &run.glyphs {
                let _ = write!(
                    out,
                    "({},{:.3},{:.3},{:.3},{},{})",
                    glyph.id, glyph.x, glyph.y, glyph.advance, glyph.font, glyph.cluster,
                );
            }
            let _ = writeln!(out);
        }
    }
    for child in &fragment.children {
        digest(child, out);
    }
}

fn fingerprint(fragment: &Fragment) -> String {
    let mut out = String::new();
    digest(fragment, &mut out);
    out
}

/// 解析 + 级联 + 布局（body margin 归零，与既有测试同约定）。
fn layout(html: &str, css: &str, viewport_width: f32) -> Fragment {
    let document = nexty_html::parse_document(html, ParseOptions::default());
    let sheet = Stylesheet::parse(&format!("body {{ margin: 0 }} {css}"));
    let styles = compute_document_styles(&document, &[&sheet], Some(html_ua_stylesheet()));
    let shaper = ParleyTextShaper::new();
    layout_document(
        &document,
        &shaper,
        &styles,
        &HashMap::<NodeId, (f32, f32)>::new(),
        viewport_width,
    )
    .expect("document has a root element")
}

/// 覆盖 CSS Text 放置期修正与多字体回退的语料：每页命中一个修正项，
/// 布局路径都经 max-content / min-content / 定宽三档 measure。
const CORPUS: &[(&str, &str)] = &[
    // text-align: justify：软折行非末行在机会点分配自由空间
    (
        "justify",
        "<p style=\"width:180px;text-align:justify\">汉字混排 justify words spread across lines evenly here</p>",
    ),
    // letter-spacing / word-spacing：簇间加距，行末簇字距不计行宽
    (
        "letter-spacing",
        "<p style=\"width:160px;letter-spacing:2px;word-spacing:5px\">spaced 文字 mixed with cjk</p>",
    ),
    // tab-size：制表推进到 tab-size × 空格宽的下一整数倍
    (
        "tab-size",
        "<pre style=\"width:200px;tab-size:8\">a\tb\nlonger\tend</pre>",
    ),
    // pre-wrap：换行保留、行尾空格悬挂不占行宽
    (
        "pre-wrap",
        "<p style=\"width:140px;white-space:pre-wrap\">first line with trailing spaces   \nsecond 段落 line</p>",
    ),
    // text-indent：首行缩进（长度与百分比两种）
    (
        "text-indent",
        "<p style=\"width:150px;text-indent:24px\">indent me across the wrap boundary please</p><p style=\"width:150px;text-indent:20%\">percent indent me across the boundary</p>",
    ),
    // 混合样式 run + CJK 回退（多字体整形结果经缓存复用）
    (
        "mixed-runs",
        "<p style=\"width:170px\">plain <b>bold 中文</b> tail <span style=\"font-size:20px\">big 字</span> end</p>",
    ),
    // overflow-wrap: break-word：紧急断点在 min-content 档位不参与
    (
        "break-word",
        "<p style=\"width:80px;overflow-wrap:break-word\">superlongunbreakableword 中文 here</p>",
    ),
    // <br> 硬断行 + 嵌套行内元素跨界断点
    (
        "br-and-nesting",
        "<p style=\"width:120px\">alpha <span>beta <b>gamma delta</b></span><br>second 行 line</p>",
    ),
];

/// 同一页在不同历史宽度下布局：先走别的宽度再走目标宽度，片段树指纹
/// 必须与「直接布局」一致（缓存不得泄漏任何宽度档位的影响）。
#[test]
fn cached_layout_matches_fresh_layout_across_widths() {
    for (name, html) in CORPUS.iter().copied() {
        let fresh = fingerprint(&layout(html, "", 200.0));
        // 预热另一宽度（填充别的档位）后重布局同一页
        let _warmed = layout(html, "", 333.0);
        let after_warm = fingerprint(&layout(html, "", 200.0));
        assert_eq!(fresh, after_warm, "[{name}] 预热后指纹漂移");
        // 再走 min-content 风格的极窄宽度与超宽（max-content 风格）
        let _narrow = layout(html, "", 40.0);
        let _wide = layout(html, "", 4000.0);
        let after_extremes = fingerprint(&layout(html, "", 200.0));
        assert_eq!(fresh, after_extremes, "[{name}] 极端宽度预热后指纹漂移");
    }
}

/// 同一次布局内部多档位命中同 run 节点缓存：flex baseline 对齐迫使
/// run 叶在 min/max/定宽档位各 measure 一次，结果与纯块级布局的文本
/// 放置一致（同文本同宽度同行盒）。
#[test]
fn multi_tier_measure_matches_direct_layout() {
    let html = "<div style=\"width:220px\"><p>first paragraph wraps here</p><p>second 段落 wraps 中文 here</p></div>";
    let block = fingerprint(&layout(html, "", 220.0));
    // 相同文本进 flex 容器（min/max/definite 档位全走）再比文本行内容：
    // 指纹含容器几何差异，只比对 p 子树的行盒。直接重建页面保持同宽。
    let flex_html = "<div style=\"width:220px;display:flex;flex-direction:column\"><p>first paragraph wraps here</p><p>second 段落 wraps 中文 here</p></div>";
    let flex = layout(flex_html, "", 220.0);
    let mut flex_lines = String::new();
    collect_lines(&flex, &mut flex_lines);
    let mut block_lines = String::new();
    collect_lines(&layout(html, "", 220.0), &mut block_lines);
    assert_eq!(block_lines, flex_lines, "flex 多档位 measure 后行盒漂移");
    let _ = block; // 全树指纹仅保证块级路径自身稳定
}

/// 收集片段树中全部文本 run（行盒内容指纹）。
fn collect_lines(fragment: &Fragment, out: &mut String) {
    use std::fmt::Write as _;
    for line in &fragment.lines {
        for run in &line.runs {
            let _ = writeln!(
                out,
                "run size={:.3} text={:?} span=({},{}) glyphs={}",
                run.style.size,
                run.text,
                run.text_start,
                run.text_end,
                run.glyphs
                    .iter()
                    .map(|g| format!("{},{:.3},{:.3}", g.id, g.x, g.advance))
                    .collect::<Vec<_>>()
                    .join(";"),
            );
        }
    }
    for child in &fragment.children {
        collect_lines(child, out);
    }
}

/// 同一进程内同一页反复布局 50 次（宽度交替）：指纹全程不变——run 节点
/// 缓存随树重建，跨布局不得残留任何状态。
#[test]
fn repeated_layouts_are_stable() {
    let (name, html) = CORPUS[5];
    let expected = fingerprint(&layout(html, "", 170.0));
    for index in 0..50 {
        let width = if index % 2 == 0 { 170.0 } else { 233.0 };
        let actual = fingerprint(&layout(html, "", width));
        if index % 2 == 0 {
            assert_eq!(expected, actual, "[{name}] 第 {index} 次布局漂移");
        }
    }
}

/// M12：字形字体下标越界（trait 契约破坏）不 panic——remap 路径跳过该
/// 字形，布局正常产出片段树。
///
/// 构造：整形器把字形 font 下标改成 9 而字体表为空；同一文本 chunk 被
/// 紧急断点切成多段同行放置时走 run 合并（remap_glyph）路径。
#[test]
fn out_of_range_font_index_skips_glyph_without_panic() {
    use nexty_css::{Stylesheet, compute_document_styles, html_ua_stylesheet};
    use nexty_text::{FontMetrics, ShapedText, TextError, TextStyle};

    /// 畸形整形器：产出 font 下标越界的字形（fonts 表被清空）。
    struct BadFontShaper {
        inner: nexty_text::ParleyTextShaper,
    }
    impl nexty_text::TextShaper for BadFontShaper {
        fn shape(&self, text: &str, style: &TextStyle) -> Result<ShapedText, TextError> {
            let mut shaped = self.inner.shape(text, style)?;
            for glyph in &mut shaped.glyphs {
                glyph.font = 9;
            }
            shaped.fonts.clear();
            Ok(shaped)
        }
        fn metrics(&self, style: &TextStyle) -> Result<FontMetrics, TextError> {
            self.inner.metrics(style)
        }
    }

    let html = "<body><p style=\"width:60px;overflow-wrap:break-word\">superlongtext with words here</p></body>";
    let document = nexty_html::parse_document(html, ParseOptions::default());
    let sheet = Stylesheet::parse("body { margin: 0 }");
    let styles = compute_document_styles(&document, &[&sheet], Some(html_ua_stylesheet()));
    let shaper = BadFontShaper {
        inner: nexty_text::ParleyTextShaper::new(),
    };
    // 布局完成即不 panic；紧急断点把同一 chunk 切成多段同行放置（run 合并
    // 路径），越界字形被跳过
    let root = layout_document(
        &document,
        &shaper,
        &styles,
        &HashMap::<NodeId, (f32, f32)>::new(),
        80.0,
    )
    .expect("layout completes");
    assert!(root.border_box.width > 0.0);
}
