//! 盒级布局集成测试：普通流块级布局、margin 折叠、行内行盒。
//!
//! 行为 ground truth：CSS 2.1 §8.3.1 / §9.2.1 / §10.3.3 / §10.5 / §10.8。

use nexty_css::{Stylesheet, compute_document_styles, html_ua_stylesheet};
use nexty_dom::{Document, NodeId, NodeKind};
use nexty_html::ParseOptions;
use nexty_layout::{Fragment, LineFragment, layout_document};
use nexty_text::ParleyTextShaper;
use std::collections::HashMap;

/// 解析 + 级联（含 UA 默认样式）+ 布局的测试入口。
///
/// 测试统一把 body margin 归零，几何期望值不受 UA 默认 8px 干扰。
fn layout(html: &str, css: &str, viewport_width: f32) -> (Document, Fragment) {
    layout_with_images(html, css, HashMap::new(), viewport_width)
}

/// 带 `image_sizes`（node → 自然尺寸）的布局入口。
fn layout_with_images(
    html: &str,
    css: &str,
    image_sizes: HashMap<NodeId, (f32, f32)>,
    viewport_width: f32,
) -> (Document, Fragment) {
    let document = nexty_html::parse_document(html, ParseOptions::default());
    let sheet = Stylesheet::parse(&format!("body {{ margin: 0 }} {css}"));
    let styles = compute_document_styles(&document, &[&sheet], Some(html_ua_stylesheet()));
    let shaper = ParleyTextShaper::new();
    let fragment = layout_document(&document, &shaper, &styles, &image_sizes, viewport_width)
        .expect("document has a root element");
    (document, fragment)
}

/// 找到 `fragment` 子树中第一个局部名为 `name` 的元素的片段。
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

fn body<'a>(document: &Document, fragment: &'a Fragment) -> &'a Fragment {
    find(document, fragment, "body").expect("body fragment")
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

#[test]
fn inline_image_participates_in_line_box() {
    let (document, root) = layout(
        "<body>hi <img width=40 height=30 src=a.png> bye</body>",
        "",
        400.0,
    );
    let body_fragment = body(&document, &root);
    assert_eq!(body_fragment.lines.len(), 1, "短内容单行");
    let line = &body_fragment.lines[0];
    assert_eq!(line.images.len(), 1, "一张图片 run");
    let image = &line.images[0];
    assert!(approx(image.width, 40.0));
    assert!(approx(image.height, 30.0));
    // 替换元素底边对齐基线：顶缘 = baseline - height
    assert!(approx(image.y + image.height, line.baseline));
    // 文本 run 仍在（hi 与 bye）
    assert_eq!(line.runs.len(), 2, "图片两侧的文本各自成 run");
    // 图片在文本之后开始
    assert!(image.x > 0.0);
}

/// 未带尺寸属性且不在 `image_sizes` 里的图片按默认对象尺寸 300×150。
#[test]
fn image_without_size_falls_back_to_default_object_size() {
    let (document, root) = layout("<body><img src=a.png></body>", "", 800.0);
    let line = &body(&document, &root).lines[0];
    assert_eq!(line.images.len(), 1);
    let image = &line.images[0];
    assert!(approx(image.width, 300.0));
    assert!(approx(image.height, 150.0));
}

/// `image_sizes` 提供自然尺寸：无属性时按自然尺寸显示。
#[test]
fn image_natural_size_used_when_no_attributes() {
    let html = "<body><img src=a.png></body>";
    let document = nexty_html::parse_document(html, ParseOptions::default());
    let sheet = nexty_css::Stylesheet::parse("body { margin: 0 }");
    let styles = compute_document_styles(&document, &[&sheet], Some(html_ua_stylesheet()));
    // 模拟 chrome 层：解码后把自然尺寸按节点登记
    let mut image_sizes = HashMap::new();
    for resource in nexty_html::collect_image_resources(&document) {
        image_sizes.insert(resource.node, (320.0, 240.0));
    }
    let shaper = ParleyTextShaper::new();
    let root = layout_document(&document, &shaper, &styles, &image_sizes, 800.0).expect("root");
    let line = &root.children[0].lines[0];
    assert_eq!(line.images.len(), 1);
    assert!(approx(line.images[0].width, 320.0));
    assert!(approx(line.images[0].height, 240.0));
}

/// 放不下的图片整体换行，不拆分。
#[test]
fn image_breaks_to_next_line_when_it_does_not_fit() {
    let (document, root) = layout(
        "<body>text <img width=200 height=20 src=a.png> tail</body>",
        "",
        240.0,
    );
    let lines = &body(&document, &root).lines;
    // 240 宽下 "text" 后放不下 200 的图片 → 图片换行；tail 再换行
    assert!(
        lines.len() >= 2,
        "图片放不下必须换行，实际行数 {}",
        lines.len()
    );
    assert_eq!(
        lines.iter().map(|line| line.images.len()).sum::<usize>(),
        1,
        "图片只有一个 run，且不被拆分"
    );
}

/// 块级 `<img>`（display: block）按替换元素盒布局：尺寸取 HTML 属性。
#[test]
fn block_level_image_lays_out_as_replaced_box() {
    let (document, root) = layout(
        "<body><img style=\"display: block\" width=100 height=20 src=a.png></body>",
        "",
        400.0,
    );
    let img = find(&document, &root, "img").expect("img fragment");
    assert!(approx(img.border_box.width, 100.0));
    assert!(approx(img.border_box.height, 20.0));
    assert!(img.lines.is_empty() && img.children.is_empty());
}

// ---- T6：box-sizing / min-max（CSS Sizing §5.3、CSS 2.1 §10.4/§10.7） ----

/// box-sizing: border-box 下 width 含 padding + border（边框盒 = 指定宽）。
#[test]
fn border_box_width_includes_padding_and_border() {
    let (document, root) = layout(
        "<body><div style=\"width: 100px; padding: 10px; border: 5px solid red; height: 40px\"></div></body>",
        "div { box-sizing: content-box }",
        400.0,
    );
    // content-box：边框盒 = 100 + 2*10 + 2*5 = 130
    let div = find(&document, &root, "div").expect("div fragment");
    assert!(approx(div.border_box.width, 130.0));

    let (document, root) = layout(
        "<body><div style=\"width: 100px; padding: 10px; border: 5px solid red; height: 40px\"></div></body>",
        "div { box-sizing: border-box }",
        400.0,
    );
    // border-box：边框盒 = 100，内容盒 = 100 - 20 - 10 = 70
    let div = find(&document, &root, "div").expect("div fragment");
    assert!(approx(div.border_box.width, 100.0));
    assert!(approx(
        div.border_box.width - div.border.left - div.padding.left - div.border.right - div.padding.right,
        70.0
    ));
    // height 同理：边框盒 = 40，内容高 = 40 - 10 = 30
    assert!(approx(div.border_box.height, 40.0));
}

/// max-width 收窄盒宽；min-width 抬宽；min 优先于 max。
#[test]
fn min_max_width_clamp_used_width() {
    // 指定宽 300 → max-width 100 收窄
    let (document, root) = layout(
        "<body><div style=\"width: 300px; max-width: 100px\"></div></body>",
        "",
        400.0,
    );
    let div = find(&document, &root, "div").expect("div fragment");
    assert!(approx(div.border_box.width, 100.0));

    // auto 宽度同样受 max 约束
    let (document, root) = layout(
        "<body><div style=\"max-width: 120px\"></div></body>",
        "",
        400.0,
    );
    let div = find(&document, &root, "div").expect("div fragment");
    assert!(approx(div.border_box.width, 120.0));

    // 指定宽 50 → min-width 150 抬宽
    let (document, root) = layout(
        "<body><div style=\"width: 50px; min-width: 150px\"></div></body>",
        "",
        400.0,
    );
    let div = find(&document, &root, "div").expect("div fragment");
    assert!(approx(div.border_box.width, 150.0));

    // min > max：min 优先
    let (document, root) = layout(
        "<body><div style=\"width: 80px; min-width: 160px; max-width: 100px\"></div></body>",
        "",
        400.0,
    );
    let div = find(&document, &root, "div").expect("div fragment");
    assert!(approx(div.border_box.width, 160.0));
}

/// max-height 收窄、min-height 抬高内容高。
#[test]
fn min_max_height_clamp_used_height() {
    let (document, root) = layout(
        "<body><div style=\"height: 200px; max-height: 80px\"></div></body>",
        "",
        400.0,
    );
    let div = find(&document, &root, "div").expect("div fragment");
    assert!(approx(div.border_box.height, 80.0));

    let (document, root) = layout(
        "<body><div style=\"height: 10px; min-height: 60px\"></div></body>",
        "",
        400.0,
    );
    let div = find(&document, &root, "div").expect("div fragment");
    assert!(approx(div.border_box.height, 60.0));
}

#[test]
fn blocks_stack_and_fill_containing_width() {
    let (document, root) = layout(
        "<div style=\"height: 10px\"></div><div style=\"height: 20px\"></div>",
        "",
        200.0,
    );
    // 根 html：宽度占满视口，高度 = body 高 = 30
    assert!(approx(root.border_box.width, 200.0));
    assert!(approx(root.border_box.height, 30.0));

    let body = body(&document, &root);
    let children = &body.children;
    assert_eq!(children.len(), 2);
    // 块级依次纵向堆叠，宽度 = 包含块内容宽
    assert!(approx(children[0].border_box.y, 0.0));
    assert!(approx(children[0].border_box.height, 10.0));
    assert!(approx(children[1].border_box.y, 10.0));
    assert!(approx(children[1].border_box.height, 20.0));
    assert!(approx(children[0].border_box.width, 200.0));
    assert!(approx(children[1].border_box.width, 200.0));
}

#[test]
fn specified_width_resolves_auto_margins_and_centering() {
    let (document, root) = layout(
        "<div style=\"width: 100px; height: 5px; margin: auto\"></div>",
        "",
        200.0,
    );
    let body_fragment = body(&document, &root);
    let div = &body_fragment.children[0];
    // 两侧 auto margin 等分剩余 → 居中
    assert!(approx(div.border_box.x, 50.0));
    assert!(approx(div.border_box.width, 100.0));

    // 仅 margin-left: auto → 元素贴右
    let (document, root) = layout(
        "<div style=\"width: 100px; height: 5px; margin-left: auto\"></div>",
        "",
        200.0,
    );
    let div = &body(&document, &root).children[0];
    assert!(approx(div.border_box.x, 100.0));
}

#[test]
fn over_constrained_ignores_margin_right() {
    let (document, root) = layout(
        "<div style=\"width: 100px; height: 5px; margin-left: 10px; margin-right: 30px\"></div>",
        "",
        200.0,
    );
    let div = &body(&document, &root).children[0];
    // ltr 过约束：忽略 margin-right，盒子停在 margin-left 位置
    assert!(approx(div.border_box.x, 10.0));
    assert!(approx(div.border_box.width, 100.0));
}

#[test]
fn padding_and_border_shape_the_box() {
    let (document, root) = layout(
        "<div style=\"padding: 10px; border: 2px solid; height: 5px\"><div style=\"height: 3px\"></div></div>",
        "",
        200.0,
    );
    let body = body(&document, &root);
    let outer = &body.children[0];
    // 边框盒宽 = 内容宽 + 2*(border+padding) = 200
    assert!(approx(outer.border_box.width, 200.0));
    assert!(approx(outer.border_box.height, 5.0 + 2.0 * (2.0 + 10.0)));
    assert!(approx(outer.border.top, 2.0));
    assert!(approx(outer.padding.top, 10.0));
    // 子片段相对**外盒内容盒**定位：内容顶 = 边框盒顶 + border + padding
    let inner = &outer.children[0];
    assert!(approx(inner.border_box.width, 176.0));
    assert!(approx(inner.border_box.y, 0.0));
    assert!(approx(inner.border_box.x, 0.0));
    // 内容顶的绝对偏移（相对 body 内容）= outer.y + 2 + 10
    let inner_absolute =
        outer.border_box.y + outer.border.top + outer.padding.top + inner.border_box.y;
    assert!(approx(inner_absolute, 12.0));
}

#[test]
fn percentages_resolve_against_containing_block_width() {
    let (document, root) = layout(
        "<div style=\"width: 50%; height: 5px; margin: 10% \"></div>",
        "",
        200.0,
    );
    let div = &body(&document, &root).children[0];
    assert!(approx(div.border_box.width, 100.0));
    // margin 10% 相对包含块宽度 200 → 20（水平方向直接生效）
    assert!(approx(div.border_box.x, 20.0));
    // 垂直方向：div 的上边距与 body 上边距相邻折叠 → div 相对 body 内容顶为 0，
    // 折叠值呈现在 body 的位置上
    assert!(approx(div.border_box.y, 0.0));
    assert!(approx(body(&document, &root).border_box.y, 20.0));
}

#[test]
fn adjacent_sibling_margins_collapse() {
    let (document, root) = layout(
        "<div style=\"height: 10px; margin-bottom: 30px\"></div>\
         <div style=\"height: 20px; margin-top: 10px\"></div>",
        "",
        200.0,
    );
    let children = &body(&document, &root).children;
    // 折叠值 = max(30, 10) = 30 → 第二块 y = 10 + 30
    assert!(approx(children[1].border_box.y, 40.0));
}

#[test]
fn mixed_sign_margins_collapse_to_sum_of_extremes() {
    let (document, root) = layout(
        "<div style=\"height: 10px; margin-bottom: -5px\"></div>\
         <div style=\"height: 20px; margin-top: 20px\"></div>",
        "",
        200.0,
    );
    let children = &body(&document, &root).children;
    // c(-5, 20) = 20 - 5 = 15
    assert!(approx(children[1].border_box.y, 25.0));
}

#[test]
fn parent_and_first_child_top_margins_collapse() {
    let (document, root) = layout(
        "<body style=\"margin-top: 40px\"><div style=\"height: 10px; margin-top: 25px\"></div></body>",
        "",
        200.0,
    );
    // html（根）不与子盒折叠：body 边框边 = c(0, 40) = 40
    // body 与 div 的上边距相邻：c(40, 25) = 40 → body 边框边与 div 重合
    let div = find(&document, &root, "div").expect("div fragment");
    assert!(
        approx(div.border_box.y, 0.0),
        "div 相对 body 内容顶为 0（已折叠重合）"
    );
    let body = body(&document, &root);
    assert!(approx(body.border_box.y, 40.0));
}

#[test]
fn empty_block_collapses_through() {
    let (document, root) = layout(
        "<div style=\"height: 10px; margin-bottom: 4px\"></div>\
         <div style=\"margin-top: 6px; margin-bottom: 8px\"></div>\
         <div style=\"height: 20px; margin-top: 2px\"></div>",
        "",
        200.0,
    );
    let children = &body(&document, &root).children;
    // 空盒上下边距相邻 → 四个边距全部 adjoining：c(4, 6, 8, 2) = 8。
    // 空盒自身定位「如同有非零下 border」：位于 10 + c(4, 6) = 16；
    // 第三块位于 10 + 8 = 18（CSS 2.1 §8.3.1 collapse through）
    assert!(approx(children[1].border_box.y, 16.0));
    assert!(approx(children[2].border_box.y, 18.0));
}

#[test]
fn display_none_removes_subtree() {
    let (document, root) = layout(
        "<div style=\"display: none\"><p style=\"height: 5px\"></p></div>\
         <div style=\"height: 8px\"></div>",
        "",
        200.0,
    );
    let body = body(&document, &root);
    assert_eq!(body.children.len(), 1);
    assert!(approx(body.children[0].border_box.height, 8.0));
    assert!(find(&document, &root, "p").is_none());
}

#[test]
fn inline_siblings_group_into_anonymous_block() {
    let (document, root) = layout(
        "<div style=\"height: 5px\"></div>hello <b>world</b>",
        "",
        200.0,
    );
    let body = body(&document, &root);
    // 块级 div 之后连续行内内容归入匿名块
    assert_eq!(body.children.len(), 2);
    let anonymous = &body.children[1];
    assert!(anonymous.anonymous);
    assert!(!anonymous.lines.is_empty());
    // 行内元素 <b> 的文本与其前文在同一行内流
    let run_nodes: Vec<NodeId> = anonymous.lines[0].runs.iter().map(|run| run.node).collect();
    assert_eq!(run_nodes.len(), 2, "hello 与 world 分属两个 run");
}

#[test]
fn text_only_block_carries_lines_directly() {
    let (document, root) = layout("<p>hello</p>", "", 200.0);
    let p = find(&document, &root, "p").expect("p fragment");
    assert!(!p.anonymous);
    assert_eq!(p.children.len(), 0);
    assert_eq!(p.lines.len(), 1);
    let line = &p.lines[0];
    assert_eq!(line.runs.len(), 1);
    assert!(!line.runs[0].glyphs.is_empty());
}

#[test]
fn long_text_wraps_at_viewport_width() {
    let text = "aaa bbb ccc ddd eee fff ggg hhh iii jjj kkk lll mmm nnn ooo ppp";
    let (document, root) = layout(&format!("<p>{text}</p>"), "", 200.0);
    let p = find(&document, &root, "p").expect("p fragment");
    assert!(p.lines.len() >= 2, "文本应折成多行");
    // 每行不超宽（行首超长词的溢出场景除外）
    for line in &p.lines {
        let line_width = line
            .runs
            .iter()
            .map(|run| run.glyphs.iter().map(|glyph| glyph.advance).sum::<f32>())
            .sum::<f32>();
        assert!(line_width <= 200.0 + 1.0, "行宽 {line_width} 超出视口");
    }
    // 行首无前导空白：第二行首字形 x 非负且小于行宽
    let second = &p.lines[1];
    let first_x = second.runs[0].glyphs[0].x;
    assert!(first_x >= 0.0);
}

#[test]
fn single_unbreakable_word_overflows() {
    let word = "a".repeat(80);
    let (document, root) = layout(&format!("<p>{word}</p>"), "", 100.0);
    let p = find(&document, &root, "p").expect("p fragment");
    assert_eq!(p.lines.len(), 1, "无断点的单词不折行");
    let width: f32 = p.lines[0].runs[0]
        .glyphs
        .iter()
        .map(|glyph| glyph.advance)
        .sum();
    assert!(width > 100.0, "单词溢出视口");
}

#[test]
fn strut_sets_minimum_line_height() {
    // 行高 40px 的块内 16px 文本：行盒高度由 strut 决定
    let (document, root) = layout("<p style=\"line-height: 40px\">hi</p>", "", 200.0);
    let p = find(&document, &root, "p").expect("p fragment");
    assert!(approx(p.lines[0].height, 40.0));

    // 行内元素自身更大的行高抬升行盒
    let (document, root) = layout(
        "<p style=\"line-height: 20px\">x <b style=\"line-height: 60px\">big</b></p>",
        "",
        200.0,
    );
    let p = find(&document, &root, "p").expect("p fragment");
    assert!(approx(p.lines[0].height, 60.0), "行盒高度取最大行高");
}

#[test]
fn inline_element_style_applies_to_its_run() {
    let (document, root) = layout(
        "<p style=\"color: black\">plain <b style=\"color: red\">red</b></p>",
        "",
        200.0,
    );
    let p = find(&document, &root, "p").expect("p fragment");
    let runs = &p.lines[0].runs;
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].color, nexty_css::Rgba::opaque(0, 0, 0));
    assert_eq!(runs[1].color, nexty_css::Rgba::opaque(255, 0, 0));
    // b 的 run 位置在 p 文本之后
    let last_of_first = runs[0].glyphs.last().expect("glyphs");
    let first_of_second = runs[1].glyphs.first().expect("glyphs");
    assert!(first_of_second.x > last_of_first.x);
}

#[test]
fn blank_document_yields_none() {
    let shaper = ParleyTextShaper::new();
    // 真正的 None 场景：没有任何元素的文档
    let empty = Document::new();
    let styles = compute_document_styles(&empty, &[], None);
    let image_sizes = HashMap::new();
    assert!(layout_document(&empty, &shaper, &styles, &image_sizes, 100.0).is_none());
}

#[test]
fn line_fragment_baseline_inside_height() {
    let (document, root) = layout("<p>x</p>", "", 200.0);
    let p = find(&document, &root, "p").expect("p fragment");
    let line: &LineFragment = &p.lines[0];
    assert!(line.baseline > 0.0);
    assert!(line.baseline <= line.height);
}
