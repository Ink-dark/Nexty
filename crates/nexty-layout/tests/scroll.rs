//! 滚动容器元数据集成测试（K6 T2）。
//!
//! 行为 ground truth：CSS Overflow 3 §2–§3 简化档（偏差见 `scroll` 模块
//! 文档）。覆盖：容器识别（UA + 作者级联）、scrollable overflow rect
//! （块级后代 / 行内行带）、嵌套滚动容器截断传播、布局结果与滚动偏移无关。

#![cfg(feature = "scroll")]

use nexty_css::{Stylesheet, compute_document_styles, html_ua_stylesheet};
use nexty_dom::{Document, NodeId, NodeKind};
use nexty_html::ParseOptions;
use nexty_layout::{Fragment, layout_document};
use nexty_text::ParleyTextShaper;
use std::collections::HashMap;

/// 解析 + 级联（含 UA 默认样式）+ 布局的测试入口（同 `layout.rs` 夹具）。
fn layout(html: &str, css: &str) -> (Document, Fragment) {
    let document = nexty_html::parse_document(html, ParseOptions::default());
    let sheet = Stylesheet::parse(&format!("body {{ margin: 0 }} {css}"));
    let styles = compute_document_styles(&document, &[&sheet], Some(html_ua_stylesheet()));
    let shaper = ParleyTextShaper::new();
    let fragment = layout_document(&document, &shaper, &styles, &HashMap::new(), 400.0)
        .expect("document has a root element");
    (document, fragment)
}

/// 找到片段子树中第一个局部名为 `name` 的元素片段。
fn find<'a>(document: &Document, fragment: &'a Fragment, name: &str) -> Option<&'a Fragment> {
    fn element_name(document: &Document, node: NodeId) -> Option<String> {
        match document.node(node) {
            Some(NodeKind::Element(data)) => Some(data.name.clone()),
            _ => None,
        }
    }
    if element_name(document, fragment.node).as_deref() == Some(name) {
        return Some(fragment);
    }
    fragment
        .children
        .iter()
        .find_map(|child| find(document, child, name))
}

fn metadata<'a>(document: &Document, fragment: &'a Fragment, name: &str) -> &'a Fragment {
    let found = find(document, fragment, name).expect("fragment exists");
    assert!(
        found.scrollable_overflow.is_some() || !found.scrollable,
        "{name} 应携带滚动元数据"
    );
    found
}

#[test]
fn scroll_container_is_flagged_via_author_cascade() {
    // 作者样式表路径（非内联）：overflow: auto 容器 + 超出内容
    let (document, root) = layout(
        "<body><div class=scroller><div style=\"width:300px;height:150px\"></div></div></body>",
        ".scroller { overflow: auto; width: 100px; height: 50px }",
    );
    let scroller = metadata(&document, &root, "div");
    assert!(scroller.scrollable, "auto 容器可滚动");
    let rect = scroller.scrollable_overflow.expect("rect");
    // 内容盒 (0,0,100,50) ∪ 子盒 (0,0,300,150) = (0,0,300,150)
    assert!((rect.width - 300.0).abs() < 0.01, "width = {}", rect.width);
    assert!(
        (rect.height - 150.0).abs() < 0.01,
        "height = {}",
        rect.height
    );
    assert!((rect.x).abs() < 0.01 && rect.y.abs() < 0.01);
}

#[test]
fn hidden_container_clips_without_scrollable_flag() {
    let (document, root) = layout(
        "<body><div class=clipper><div style=\"width:300px;height:150px\"></div></div></body>",
        ".clipper { overflow: hidden; width: 100px; height: 50px }",
    );
    let clipper = metadata(&document, &root, "div");
    assert!(!clipper.scrollable, "hidden 只裁剪不可滚动");
    // 裁剪容器同样携带溢出矩形（消费方按 scrollable 决定是否滚动）
    let rect = clipper.scrollable_overflow.expect("rect");
    assert!((rect.width - 300.0).abs() < 0.01);
}

#[test]
fn visible_container_has_no_scroll_metadata() {
    let (document, root) = layout(
        "<body><div><div style=\"width:300px;height:150px\"></div></div></body>",
        "",
    );
    let plain = find(&document, &root, "div").expect("outer div");
    assert!(!plain.scrollable);
    assert!(plain.scrollable_overflow.is_none(), "visible 容器无元数据");
}

#[test]
fn nested_scroll_containers_truncate_propagation() {
    // 内层 auto 截断传播：外层溢出矩形只含内层边框盒，不含内层内部溢出
    let (document, root) = layout(
        "<body><div class=outer><section class=inner>\
         <article style=\"width:400px;height:400px\"></article>\
         </section></div></body>",
        ".outer { overflow: auto; width: 200px; height: 100px } \
         .inner { overflow: auto; width: 50px; height: 50px }",
    );
    let outer = metadata(&document, &root, "div");
    let inner = metadata(&document, &root, "section");
    assert!(outer.scrollable && inner.scrollable);

    let outer_rect = outer.scrollable_overflow.expect("outer rect");
    assert!(
        (outer_rect.width - 200.0).abs() < 0.01 && (outer_rect.height - 100.0).abs() < 0.01,
        "外层矩形 = 自身内容盒 ∪ 内层边框盒，不含内层内部溢出：{:?}",
        outer_rect
    );

    let inner_rect = inner.scrollable_overflow.expect("inner rect");
    assert!(
        (inner_rect.width - 400.0).abs() < 0.01 && (inner_rect.height - 400.0).abs() < 0.01,
        "内层矩形包含其自身内容溢出：{:?}",
        inner_rect
    );
}

#[test]
fn inline_line_band_contributes_to_overflow_rect() {
    // 行内图片超出定高容器：行盒带（横向右缘 + 纵向 [top, top+height]）并入
    let (document, root) = layout(
        "<body><div class=scroller><img width=300 height=150 src=a.png></div></body>",
        ".scroller { overflow: auto; width: 100px; height: 50px }",
    );
    let scroller = metadata(&document, &root, "div");
    let rect = scroller.scrollable_overflow.expect("rect");
    assert!(
        (rect.width - 300.0).abs() < 0.5,
        "行内图片右缘计入横向溢出：{}",
        rect.width
    );
    assert!(
        rect.height >= 150.0 - 0.5,
        "行盒带纵向并入（图片行高 ≥ 图片高）：{}",
        rect.height
    );
}

#[test]
fn layout_output_is_independent_of_scroll_offset() {
    // 滚动偏移不参与布局解算：同一文档两次布局产出逐字段相等的结果
    //（布局无滚动状态输入，也无跨调用可变状态）
    let html = "<body><div class=scroller>\
                <div style=\"width:300px;height:150px\"></div></div></body>";
    let css = ".scroller { overflow: auto; width: 100px; height: 50px }";
    let (document, first) = layout(html, css);
    let styles = {
        let sheet = Stylesheet::parse(&format!("body {{ margin: 0 }} {css}"));
        compute_document_styles(&document, &[&sheet], Some(html_ua_stylesheet()))
    };
    let shaper = ParleyTextShaper::new();
    let second = layout_document(&document, &shaper, &styles, &HashMap::new(), 400.0)
        .expect("root fragment");
    assert_eq!(first, second, "两次布局结果逐字段相等");
    let scroller = find(&document, &second, "div").expect("scroller");
    assert!(scroller.scrollable);
    assert!(scroller.scrollable_overflow.is_some());
}
