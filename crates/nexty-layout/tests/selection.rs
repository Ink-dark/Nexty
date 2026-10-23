//! T1：文本项 → 源文本节点映射与行首断行种类的集成测试。
//!
//! 选区端点按「文本节点 + 源字节偏移」定位，复制文本按行的起点来源拼接；
//! 这些测试全部走真实 `layout_document` 管线。

use nexty_css::{Stylesheet, compute_document_styles, html_ua_stylesheet};
use nexty_dom::{Document, NodeId, NodeKind};
use nexty_html::ParseOptions;
use nexty_layout::{Fragment, LineStart, layout_document};
use nexty_text::ParleyTextShaper;
use std::collections::HashMap;

fn layout(html: &str, css: &str, viewport_width: f32) -> (Document, Fragment) {
    let document = nexty_html::parse_document(html, ParseOptions::default());
    let sheet = Stylesheet::parse(&format!("body {{ margin: 0 }} {css}"));
    let styles = compute_document_styles(&document, &[&sheet], Some(html_ua_stylesheet()));
    let shaper = ParleyTextShaper::new();
    let fragment = layout_document(&document, &shaper, &styles, &HashMap::new(), viewport_width)
        .expect("document has a root element");
    (document, fragment)
}

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

fn direct_text_node(document: &Document, node: NodeId) -> NodeId {
    document
        .children(node)
        .find(|child| matches!(document.node(*child), Some(NodeKind::Text(_))))
        .expect("element has a direct text child")
}

#[test]
fn runs_carry_text_node_and_source_map() {
    let (document, root) = layout("<p>ab</p>", "", 400.0);
    let p = find(&document, &root, "p").expect("p");
    let run = &p.lines[0].runs[0];

    assert_eq!(&*run.text, "ab");
    assert_eq!(run.text_node, direct_text_node(&document, p.node));
    assert_eq!(run.source_map.len(), 2);
    assert_eq!(
        (
            run.source_map[0].chunk,
            run.source_map[0].source,
            run.source_map[0].source_end
        ),
        (0, 0, 1)
    );
    assert_eq!(
        (
            run.source_map[1].chunk,
            run.source_map[1].source,
            run.source_map[1].source_end
        ),
        (1, 1, 2)
    );

    // 每个 glyph 的 cluster 都能在映射中找到对应字符。
    for glyph in &run.glyphs {
        assert!(
            run.source_map
                .iter()
                .any(|entry| entry.chunk == glyph.cluster),
            "glyph cluster {} 没有源映射",
            glyph.cluster
        );
    }
}

#[test]
fn collapsed_whitespace_maps_to_consumed_run() {
    let (document, root) = layout("<p>a   b</p>", "", 400.0);
    let p = find(&document, &root, "p").expect("p");
    let run = &p.lines[0].runs[0];

    assert_eq!(&*run.text, "a b");
    // 折叠后的空格映射到源文本中整段空白 [1, 4)。
    let space = run
        .source_map
        .iter()
        .find(|entry| entry.chunk == 1)
        .expect("space entry");
    assert_eq!((space.source, space.source_end), (1, 4));
}

#[test]
fn pre_tab_maps_to_tab_character() {
    let (document, root) = layout("<pre>a\tb</pre>", "", 400.0);
    let pre = find(&document, &root, "pre").expect("pre");
    let run = &pre.lines[0].runs[0];

    assert_eq!(&*run.text, "a b");
    let space = run
        .source_map
        .iter()
        .find(|entry| entry.chunk == 1)
        .expect("tab entry");
    assert_eq!((space.source, space.source_end), (1, 2), "制表按单字符映射");
}

#[test]
fn br_and_preserved_newline_mark_hard_line_start() {
    let (document, root) = layout("<p>a<br>b</p>", "", 400.0);
    let p = find(&document, &root, "p").expect("p");
    assert_eq!(p.lines.len(), 2);
    assert_eq!(p.lines[0].start, LineStart::Soft { space: false });
    assert_eq!(p.lines[1].start, LineStart::Hard);

    let (document, root) = layout("<pre>a\nb</pre>", "", 400.0);
    let pre = find(&document, &root, "pre").expect("pre");
    assert_eq!(pre.lines.len(), 2);
    assert_eq!(pre.lines[1].start, LineStart::Hard);
}

#[test]
fn consecutive_breaks_mark_each_following_line_hard() {
    let (document, root) = layout("<p>a<br><br>b</p>", "", 400.0);
    let p = find(&document, &root, "p").expect("p");
    assert_eq!(p.lines.len(), 3);
    assert_eq!(p.lines[0].start, LineStart::Soft { space: false });
    assert_eq!(p.lines[1].start, LineStart::Hard);
    assert_eq!(p.lines[2].start, LineStart::Hard);
}

#[test]
fn soft_wrap_at_space_marks_space() {
    let (document, root) = layout("<p>abc def</p>", "", 40.0);
    let p = find(&document, &root, "p").expect("p");
    assert!(p.lines.len() >= 2, "必须在空格处折行");
    assert_eq!(p.lines[0].start, LineStart::Soft { space: false });
    assert_eq!(p.lines[1].start, LineStart::Soft { space: true });

    // 行尾被裁掉的空格仍留在源映射里。
    let run = &p.lines[0].runs[0];
    assert_eq!(run.text.chars().nth(3), Some(' '));
    assert!(
        run.source_map
            .iter()
            .any(|entry| entry.chunk == 3 && entry.source == 3)
    );
}

#[test]
fn soft_wrap_between_cjk_does_not_mark_space() {
    let (document, root) = layout("<p>汉汉汉汉汉汉</p>", "", 40.0);
    let p = find(&document, &root, "p").expect("p");
    assert!(p.lines.len() >= 2, "CJK 必须折行");
    assert_eq!(p.lines[1].start, LineStart::Soft { space: false });
}

#[test]
fn sibling_text_nodes_keep_distinct_identity() {
    let (document, root) = layout("<p>a<!--x-->b</p>", "", 400.0);
    let p = find(&document, &root, "p").expect("p");
    let text_nodes: Vec<NodeId> = document
        .children(p.node)
        .filter(|child| matches!(document.node(*child), Some(NodeKind::Text(_))))
        .collect();
    assert_eq!(text_nodes.len(), 2);

    let runs = &p.lines[0].runs;
    assert_eq!(runs.len(), 2, "不同文本节点不得合并为同一 run");
    assert_eq!(&*runs[0].text, "a");
    assert_eq!(&*runs[1].text, "b");
    assert_eq!(runs[0].text_node, text_nodes[0]);
    assert_eq!(runs[1].text_node, text_nodes[1]);
}
