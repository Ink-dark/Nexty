//! chrome 选区状态机集成测试（无头）：用真实布局产出的 `TextMap` 驱动。
//!
//! 窗口/GPU 胶水（app 模块）不在此覆盖；拖拽/Shift+点击/双击/全选的决策
//! 逻辑在 selection 模块内，链接点击与拖拽的分流用 `is_click` 覆盖。

use nexty_chrome::pipeline;
use nexty_chrome::selection::{Selection, is_click};
use nexty_layout::{TextMap, TextPosition};
use nexty_paint::Command;
use nexty_text::ParleyTextShaper;

fn text_map(html: &str, width: f32) -> TextMap {
    let page = pipeline::load_page(html, "");
    let shaper = ParleyTextShaper::new();
    let root = pipeline::layout_page(&page, &shaper, width).expect("root");
    TextMap::new(&root)
}

fn at(node: nexty_dom::NodeId, offset: usize) -> TextPosition {
    TextPosition { node, offset }
}

#[test]
fn drag_selection_normalizes_direction() {
    let map = text_map("<p>abc def</p>", 400.0);
    let (first, _) = map.all().unwrap();
    let node = first.node;

    let mut selection = Selection::new();
    selection.begin(at(node, 1), false);
    selection.extend_to(at(node, 5));
    selection.finish();
    assert_eq!(selection.text(&map), "bc d");
    assert!(!selection.rects(&map).is_empty());

    // 反向拖拽得到同一段文本。
    let mut reverse = Selection::new();
    reverse.begin(at(node, 5), false);
    reverse.extend_to(at(node, 1));
    reverse.finish();
    assert_eq!(reverse.text(&map), "bc d");
}

#[test]
fn shift_click_extends_existing_selection() {
    let map = text_map("<p>abc def</p>", 400.0);
    let node = map.all().unwrap().0.node;

    let mut selection = Selection::new();
    selection.begin(at(node, 2), false);
    selection.finish();
    selection.begin(at(node, 6), true);
    selection.finish();
    assert_eq!(selection.text(&map), "c de");
}

#[test]
fn single_click_collapses_selection() {
    let map = text_map("<p>abc def</p>", 400.0);
    let node = map.all().unwrap().0.node;

    let mut selection = Selection::new();
    selection.select(at(node, 0), at(node, 7));
    assert!(!selection.is_empty());

    selection.begin(at(node, 3), false);
    selection.finish();
    assert!(selection.is_empty());
    assert!(selection.rects(&map).is_empty());
    assert_eq!(selection.text(&map), "");
}

#[test]
fn word_selection_and_select_all() {
    let map = text_map("<p>hello world</p>", 400.0);
    let node = map.all().unwrap().0.node;

    let (from, to) = map.word_range(at(node, 7)).unwrap();
    let mut selection = Selection::new();
    selection.select(from, to);
    assert_eq!(selection.text(&map), "world");

    let (from, to) = map.all().unwrap();
    selection.select(from, to);
    assert_eq!(selection.text(&map), "hello world");
    assert!(!selection.rects(&map).is_empty());
}

#[test]
fn resolve_keeps_positions_after_relayout_and_clears_when_text_is_gone() {
    let narrow = text_map("<p>abc def</p>", 40.0);
    let node = narrow.all().unwrap().0.node;

    let mut selection = Selection::new();
    selection.begin(at(node, 2), false);
    selection.extend_to(at(node, 6));
    selection.finish();

    let wide = text_map("<p>abc def</p>", 400.0);
    assert!(selection.resolve(&wide), "重排后选区应保留");
    assert_eq!(selection.text(&wide), "c de");

    let empty = text_map("<p></p>", 400.0);
    assert!(!selection.resolve(&empty));
    assert!(selection.is_empty());
}

#[test]
fn click_and_drag_are_distinguished_for_link_navigation() {
    assert!(is_click((10.0, 10.0), (12.0, 11.0)));
    assert!(is_click((10.0, 10.0), (10.0, 10.0)));
    assert!(!is_click((10.0, 10.0), (30.0, 10.0)));
    assert!(!is_click((10.0, 10.0), (10.0, 30.0)));
}

#[test]
fn highlight_is_emitted_between_background_and_text() {
    let page = pipeline::load_page(
        "<p>abc</p>",
        "body { background-color: #ffffff; margin: 0 }",
    );
    let shaper = ParleyTextShaper::new();
    let root = pipeline::layout_page(&page, &shaper, 400.0).expect("root");
    let map = TextMap::new(&root);
    let (from, to) = map.all().unwrap();
    let rects = map.rects(from, to);
    assert!(!rects.is_empty());

    let scene = pipeline::build_scene_at_with_highlights(&page, &root, 0.0, 20.0, &rects);
    let highlight = scene
        .commands
        .iter()
        .position(|command| matches!(command, Command::FillRect { color, .. } if color.a == 64))
        .expect("highlight rect");
    let background = scene
        .commands
        .iter()
        .position(|command| {
            matches!(command, Command::FillRect { color, .. }
                if color.a == 255 && color.r == 255 && color.g == 255 && color.b == 255)
        })
        .expect("opaque white background");
    let text = scene
        .commands
        .iter()
        .position(|command| matches!(command, Command::DrawText { .. }))
        .expect("text");

    assert!(background < highlight, "高亮必须在页面背景之上");
    assert!(highlight < text, "高亮必须在文字之下");
    match &scene.commands[highlight] {
        Command::FillRect { rect, .. } => {
            assert!((rect.x - rects[0].x).abs() < 0.01);
            assert!((rect.y - (rects[0].y + 20.0)).abs() < 0.01);
            assert!((rect.width - rects[0].width).abs() < 0.01);
        }
        _ => unreachable!(),
    }
}

#[test]
fn empty_highlights_leave_scene_unchanged() {
    let page = pipeline::load_page("<p>abc</p>", "body { margin: 0 }");
    let shaper = ParleyTextShaper::new();
    let root = pipeline::layout_page(&page, &shaper, 400.0).expect("root");
    let plain = pipeline::build_scene(&page, &root);
    let empty = pipeline::build_scene_at_with_highlights(&page, &root, 0.0, 0.0, &[]);
    assert_eq!(plain.commands.len(), empty.commands.len());
}

#[test]
fn highlight_matches_nested_block_lines_with_origin() {
    let page = pipeline::load_page(
        "<h2>1. title</h2><div style=\"width:320px; border:1px solid #999; padding:4px\">中文段落测试文本一二三四五六七八九十</div>",
        "body { margin: 8px }",
    );
    let shaper = ParleyTextShaper::new();
    let root = pipeline::layout_page(&page, &shaper, 800.0).expect("root");
    let map = TextMap::new(&root);
    let (from, to) = map.all().unwrap();
    let rects = map.rects(from, to);
    assert!(!rects.is_empty());
    let scene = pipeline::build_scene_at_with_highlights(&page, &root, 0.0, 48.0, &rects);
    let emitted = scene
        .commands
        .iter()
        .filter(|command| matches!(command, Command::FillRect { color, .. } if color.a == 64))
        .count();
    assert_eq!(emitted, rects.len(), "高亮矩形应逐行发射");
}
