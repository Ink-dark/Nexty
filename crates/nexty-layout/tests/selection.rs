//! T1/T2：文本源映射、行首断行种类与选区几何的集成测试。
//!
//! 选区端点按「文本节点 + 源字节偏移」定位，复制文本按行的起点来源拼接；
//! 命中与矩形走真实 `layout_document` 管线。

use nexty_css::{Stylesheet, compute_document_styles, html_ua_stylesheet};
use nexty_dom::{Document, NodeId, NodeKind};
use nexty_html::ParseOptions;
use nexty_layout::{Fragment, LineStart, TextMap, TextPosition, layout_document};
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

// ---------------------------------------------------------------------------
// T2：选区几何与复制文本（TextMap）
// ---------------------------------------------------------------------------

/// 元素内容盒在根片段坐标系中的原点（与 `hit_test_ex` 同一坐标约定）。
fn content_origin(document: &Document, root: &Fragment, name: &str) -> Option<(f32, f32)> {
    fn walk(
        document: &Document,
        fragment: &Fragment,
        name: &str,
        ox: f32,
        oy: f32,
    ) -> Option<(f32, f32)> {
        let border_x = ox + fragment.border_box.x;
        let border_y = oy + fragment.border_box.y;
        let content_x = border_x + fragment.border.left + fragment.padding.left;
        let content_y = border_y + fragment.border.top + fragment.padding.top;
        if matches!(
            document.node(fragment.node),
            Some(NodeKind::Element(data)) if data.name == name
        ) {
            return Some((content_x, content_y));
        }
        let mut line_offset = 0.0;
        for line in &fragment.lines {
            for atomic in &line.boxes {
                if let Some(found) =
                    walk(document, atomic, name, content_x, content_y + line_offset)
                {
                    return Some(found);
                }
            }
            line_offset += line.height;
        }
        for child in &fragment.children {
            if let Some(found) = walk(document, child, name, content_x, content_y) {
                return Some(found);
            }
        }
        None
    }
    walk(document, root, name, 0.0, 0.0)
}

/// 某行第 `glyph_index` 个字形的点（`right_half` 取右半区）。
fn point_on_glyph(
    document: &Document,
    root: &Fragment,
    name: &str,
    line_index: usize,
    glyph_index: usize,
    right_half: bool,
) -> (f32, f32) {
    let (content_x, content_y) = content_origin(document, root, name).expect("element origin");
    let fragment = find(document, root, name).expect("element fragment");
    let top: f32 = content_y
        + fragment.lines[..line_index]
            .iter()
            .map(|line| line.height)
            .sum::<f32>();
    let glyphs: Vec<_> = fragment.lines[line_index]
        .runs
        .iter()
        .flat_map(|run| run.glyphs.iter())
        .collect();
    let glyph = glyphs[glyph_index];
    let x = content_x
        + glyph.x
        + if right_half {
            glyph.advance * 0.75
        } else {
            glyph.advance * 0.25
        };
    (x, top + fragment.lines[line_index].height / 2.0)
}

fn text_position(node: NodeId, offset: usize) -> TextPosition {
    TextPosition { node, offset }
}

#[test]
fn hit_snaps_to_glyph_halves_and_line_ends() {
    let (document, root) = layout("<p>ab</p>", "", 400.0);
    let map = TextMap::new(&root);
    let node = direct_text_node(&document, find(&document, &root, "p").unwrap().node);

    let (x, y) = point_on_glyph(&document, &root, "p", 0, 0, false);
    assert_eq!(map.hit(x, y), Some(text_position(node, 0)));
    let (x, y) = point_on_glyph(&document, &root, "p", 0, 0, true);
    assert_eq!(map.hit(x, y), Some(text_position(node, 1)));
    let (x, y) = point_on_glyph(&document, &root, "p", 0, 1, true);
    assert_eq!(map.hit(x, y), Some(text_position(node, 2)));
}

#[test]
fn hit_never_splits_grapheme_clusters() {
    let (document, root) = layout("<p>e\u{0301}x</p>", "", 400.0);
    let map = TextMap::new(&root);
    let node = direct_text_node(&document, find(&document, &root, "p").unwrap().node);

    let (x, y) = point_on_glyph(&document, &root, "p", 0, 0, false);
    assert_eq!(map.hit(x, y), Some(text_position(node, 0)));
    let (x, y) = point_on_glyph(&document, &root, "p", 0, 0, true);
    assert_eq!(
        map.hit(x, y),
        Some(text_position(node, 3)),
        "组合序列内部不得产生位置"
    );
}

#[test]
fn rects_cover_selected_glyphs() {
    let (document, root) = layout("<p>abc</p>", "", 400.0);
    let map = TextMap::new(&root);
    let p = find(&document, &root, "p").unwrap();
    let node = direct_text_node(&document, p.node);
    let (content_x, _) = content_origin(&document, &root, "p").unwrap();

    let rects = map.rects(text_position(node, 0), text_position(node, 2));
    assert_eq!(rects.len(), 1);
    let first = p.lines[0].runs[0].glyphs[0];
    let second = p.lines[0].runs[0].glyphs[1];
    assert!((rects[0].x - (content_x + first.x)).abs() < 0.5);
    let expected_right = content_x + second.x + second.advance;
    assert!((rects[0].x + rects[0].width - expected_right).abs() < 0.5);

    // 反向端点得到相同矩形；空选区不产出矩形。
    assert_eq!(
        rects,
        map.rects(text_position(node, 2), text_position(node, 0))
    );
    assert!(
        map.rects(text_position(node, 1), text_position(node, 1))
            .is_empty()
    );
}

#[test]
fn rects_are_per_visual_line() {
    let (_document, root) = layout("<p>abc def</p>", "", 40.0);
    let map = TextMap::new(&root);
    let (from, to) = map.all().unwrap();
    let rects = map.rects(from, to);
    assert!(rects.len() >= 2, "跨行选区必须逐行产出矩形");
    for pair in rects.windows(2) {
        assert!(pair[1].y >= pair[0].y + pair[0].height - 0.5);
    }
}

#[test]
fn resolve_clamps_and_rejects_unknown_nodes() {
    let (document, root) = layout("<p>abc</p>", "", 400.0);
    let map = TextMap::new(&root);
    let p = find(&document, &root, "p").unwrap();
    let node = direct_text_node(&document, p.node);

    assert!(
        map.resolve(text_position(p.node, 0)).is_none(),
        "元素节点不是文本端点"
    );
    let clamped = map.resolve(text_position(node, 999)).unwrap();
    assert_eq!(clamped.node, node);
    assert_eq!(clamped.offset, 3);
    assert_eq!(
        map.resolve(text_position(node, 1)),
        Some(text_position(node, 1))
    );
}

#[test]
fn word_range_selects_latin_word_and_single_cjk_char() {
    let (document, root) = layout("<p>hello world</p>", "", 400.0);
    let map = TextMap::new(&root);
    let (x, y) = point_on_glyph(&document, &root, "p", 0, 7, false);
    let (from, to) = map.word_range(map.hit(x, y).unwrap()).unwrap();
    assert_eq!(map.text(from, to), "world");

    // 空白上双击不选词。
    let (x, y) = point_on_glyph(&document, &root, "p", 0, 5, false);
    assert!(map.word_range(map.hit(x, y).unwrap()).is_none());

    let (document, root) = layout("<p>汉字</p>", "", 400.0);
    let map = TextMap::new(&root);
    let (x, y) = point_on_glyph(&document, &root, "p", 0, 0, false);
    let (from, to) = map.word_range(map.hit(x, y).unwrap()).unwrap();
    assert_eq!(map.text(from, to), "汉");
}

#[test]
fn text_joins_soft_wrap_with_space() {
    let (_document, root) = layout("<p>abc def</p>", "", 40.0);
    let map = TextMap::new(&root);
    let (from, to) = map.all().unwrap();
    assert_eq!(map.text(from, to), "abc def");
}

#[test]
fn text_inserts_newlines_for_hard_breaks_and_blocks() {
    let (_document, root) = layout("<p>a<br>b</p>", "", 400.0);
    let map = TextMap::new(&root);
    let (from, to) = map.all().unwrap();
    assert_eq!(map.text(from, to), "a\nb");

    let (_document, root) = layout("<p>a<br><br>b</p>", "", 400.0);
    let map = TextMap::new(&root);
    let (from, to) = map.all().unwrap();
    assert_eq!(map.text(from, to), "a\n\nb");

    let (_document, root) = layout("<p>a</p><p>b</p>", "", 400.0);
    let map = TextMap::new(&root);
    let (from, to) = map.all().unwrap();
    assert_eq!(map.text(from, to), "a\nb");

    let (_document, root) = layout("<pre>a\nb</pre>", "", 400.0);
    let map = TextMap::new(&root);
    let (from, to) = map.all().unwrap();
    assert_eq!(map.text(from, to), "a\nb");
}

#[test]
fn text_keeps_inline_content_and_preserved_whitespace() {
    let (_document, root) = layout("<p>a <b>b</b></p>", "", 400.0);
    let map = TextMap::new(&root);
    let (from, to) = map.all().unwrap();
    assert_eq!(map.text(from, to), "a b");

    // 行内原子盒内的文本按行内拼接，不插换行。
    let (_document, root) = layout(
        "<p>a<span style=\"display:inline-block\">b</span></p>",
        "",
        400.0,
    );
    let map = TextMap::new(&root);
    let (from, to) = map.all().unwrap();
    assert_eq!(map.text(from, to), "ab");

    // pre 保留首尾空格；折叠模式裁掉选区边缘空格。
    let (_document, root) = layout("<pre> a </pre>", "", 400.0);
    let map = TextMap::new(&root);
    let (from, to) = map.all().unwrap();
    assert_eq!(map.text(from, to), " a ");

    let (_document, root) = layout("<p>  abc  </p>", "", 400.0);
    let map = TextMap::new(&root);
    let (from, to) = map.all().unwrap();
    assert_eq!(map.text(from, to), "abc");
}

#[test]
fn all_is_none_without_text() {
    let (_document, root) = layout("<p></p>", "", 400.0);
    let map = TextMap::new(&root);
    assert!(map.all().is_none());
}

#[test]
fn spacing_keeps_hit_and_rect_geometry_per_glyph() {
    // letter-spacing 下的命中与矩形：cluster → 源偏移映射不受间距影响
    let (document, root) = layout("<p style=\"letter-spacing: 4px\">abcd</p>", "", 400.0);
    let map = TextMap::new(&root);
    let node = direct_text_node(&document, find(&document, &root, "p").unwrap().node);

    // 逐簇命中：每簇左半 → 本簇起点，右半 → 下一簇起点
    for index in 0..4 {
        let (x, y) = point_on_glyph(&document, &root, "p", 0, index, false);
        assert_eq!(
            map.hit(x, y),
            Some(text_position(node, index)),
            "簇 {index} 左半"
        );
        let (x, y) = point_on_glyph(&document, &root, "p", 0, index, true);
        let expected = if index == 3 { 4 } else { index + 1 };
        assert_eq!(
            map.hit(x, y),
            Some(text_position(node, expected)),
            "簇 {index} 右半"
        );
    }

    // 全选矩形宽：3 个簇间隙各 +4px
    let spaced = map.rects(text_position(node, 0), text_position(node, 4));
    assert_eq!(spaced.len(), 1);
    let plain = {
        let (document, root) = layout("<p>abcd</p>", "", 400.0);
        let map = TextMap::new(&root);
        let node = direct_text_node(&document, find(&document, &root, "p").unwrap().node);
        map.rects(text_position(node, 0), text_position(node, 4))
    };
    assert_eq!(plain.len(), 1);
    assert!(
        (spaced[0].width - plain[0].width - 3.0 * 4.0).abs() < 0.01,
        "选区矩形宽差 = 3×4px：{} - {}",
        spaced[0].width,
        plain[0].width
    );

    // 复制文本完整（间距不改源映射）
    let (from, to) = map.all().unwrap();
    assert_eq!(map.text(from, to), "abcd");
}

#[test]
fn selection_works_across_justified_lines() {
    // justify 分配发生在放置期，glyph 坐标即最终坐标：TextMap 命中与
    // 逐行矩形照常工作
    let (document, root) = layout(
        "<body><p style=\"width: 100px; text-align: justify\">\
         aaa bbb ccc ddd eee fff ggg hhh</p></body>",
        "",
        400.0,
    );
    let p = find(&document, &root, "p").expect("p fragment");
    let map = TextMap::new(&root);
    assert!(p.lines.len() >= 2, "折成多行");

    // 跨行选区：每行一个矩形；矩形 y 逐行排布
    let (from, to) = map.all().unwrap();
    let rects = map.rects(from, to);
    assert_eq!(rects.len(), p.lines.len(), "逐行矩形");
    for (index, rect) in rects.iter().enumerate() {
        assert!(rect.width.is_finite() && rect.width > 0.0);
        if index + 1 < rects.len() {
            assert!(rect.y < rects[index + 1].y, "矩形按行下移");
        }
    }
    // 末行之前的矩形右缘贴容器（justify 行右缘 = 可用宽）
    for rect in &rects[..rects.len() - 1] {
        assert!(
            (rect.x + rect.width - 100.0).abs() < 0.5,
            "justify 行选区矩形右缘 = 容器宽：{}",
            rect.x + rect.width
        );
    }
    // 命中落在行内文本上
    let mid_x = rects[0].x + rects[0].width / 2.0;
    let mid_y = rects[0].y + rects[0].height / 2.0;
    assert!(map.hit(mid_x, mid_y).is_some(), "justify 行内命中");
    // 复制文本完整（软折行补空格）
    assert_eq!(map.text(from, to), "aaa bbb ccc ddd eee fff ggg hhh");
}

#[test]
fn copy_keeps_hanging_spaces_across_pre_wrap_lines() {
    // pre-wrap 软折行处行尾空格悬挂（不占行宽）但源映射保留：跨行复制
    // 文本完整、不额外补空格
    let (_document, root) = layout(
        "<body><p style=\"width: 40px; white-space: pre-wrap\">abc   def</p></body>",
        "",
        400.0,
    );
    let map = TextMap::new(&root);
    assert!(map_all_lines(&root) >= 2, "折成两行");
    let (from, to) = map.all().unwrap();
    assert_eq!(map.text(from, to), "abc   def", "悬挂空格保留在复制文本中");
}

/// 片段树的行数（辅助断言）。
fn map_all_lines(root: &nexty_layout::Fragment) -> usize {
    fn walk(fragment: &nexty_layout::Fragment) -> usize {
        fragment.lines.len() + fragment.children.iter().map(walk).sum::<usize>()
    }
    walk(root)
}
