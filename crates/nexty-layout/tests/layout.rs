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
        div.border_box.width
            - div.border.left
            - div.padding.left
            - div.border.right
            - div.padding.right,
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

// ---- T8：inline-block 与单行 flex（CSS 2.1 §10.3.7、CSS Flexbox 子集） ----

/// inline-block 作为原子盒参与行盒：整体不拆行，内部行盒独立。
#[test]
fn inline_block_is_atomic_and_not_split() {
    let (document, root) = layout(
        "<body><i style=\"display: inline-block; width: 100px; height: 30px\">inner</i> tail</body>",
        "",
        400.0,
    );
    let body_fragment = body(&document, &root);
    assert_eq!(body_fragment.lines.len(), 1, "原子盒放得下，单行");
    let line = &body_fragment.lines[0];
    assert_eq!(line.boxes.len(), 1, "一个行内原子盒");
    assert!(line.images.is_empty());
    let atomic = &line.boxes[0];
    assert!(approx(atomic.border_box.width, 100.0));
    assert!(approx(atomic.border_box.height, 30.0));
    assert!(approx(atomic.border_box.x, 0.0), "行首第一项无前导空白");
    // 原子盒内部内容独立成行盒，不并入父行
    assert_eq!(atomic.lines.len(), 1, "内部内容单行");
    // 父行文本只剩 tail，且从原子盒右侧开始
    assert_eq!(line.runs.len(), 1);
    assert!(line.runs[0].glyphs[0].x >= 100.0, "tail 在原子盒之后");
}

/// 原子盒在剩余空间放不下时整体换行，不被拆分。
#[test]
fn inline_block_breaks_to_next_line_without_splitting() {
    let (document, root) = layout(
        "<body>text <i style=\"display: inline-block; width: 250px; height: 10px\"></i></body>",
        "",
        260.0,
    );
    let lines = &body(&document, &root).lines;
    assert_eq!(lines.len(), 2, "text 在第一行，原子盒整体换到第二行");
    assert!(lines[0].boxes.is_empty(), "第一行无原子盒");
    assert_eq!(lines[1].boxes.len(), 1);
    let atomic = &lines[1].boxes[0];
    assert!(approx(atomic.border_box.width, 250.0), "原子盒不被拆分");
    assert!(approx(atomic.border_box.x, 0.0), "行首原子盒无前导空白");
}

/// flex 单行等分：三项 grow 1、basis 0 → 平分容器主轴。
#[test]
fn flex_row_distributes_free_space_equally() {
    let (document, root) = layout(
        "<body><div style=\"display: flex; width: 300px\">\
         <div style=\"flex-grow: 1; flex-basis: 0px\">a</div>\
         <div style=\"flex-grow: 1; flex-basis: 0px\">b</div>\
         <div style=\"flex-grow: 1; flex-basis: 0px\">c</div></div></body>",
        "",
        400.0,
    );
    let container = &body(&document, &root).children[0];
    assert_eq!(container.children.len(), 3);
    for (index, item) in container.children.iter().enumerate() {
        assert!(
            approx(item.border_box.width, 100.0),
            "第 {index} 项等分 100"
        );
        assert!(
            approx(item.border_box.x, 100.0 * index as f32),
            "第 {index} 项主轴位置"
        );
        assert!(approx(item.border_box.y, 0.0), "单行：所有项同一交叉位置");
    }
    assert!(approx(container.border_box.width, 300.0));
}

/// flex 按 grow 比例分配：basis 100×3、grow 1:1:2、容器 400 → 125/125/150。
#[test]
fn flex_row_distributes_free_space_by_grow_ratio() {
    let (document, root) = layout(
        "<body><div style=\"display: flex; width: 400px\">\
         <div style=\"flex-basis: 100px; flex-grow: 1\"></div>\
         <div style=\"flex-basis: 100px; flex-grow: 1\"></div>\
         <div style=\"flex-basis: 100px; flex-grow: 2\"></div></div></body>",
        "",
        400.0,
    );
    let items = &body(&document, &root).children[0].children;
    let widths: Vec<f32> = items.iter().map(|item| item.border_box.width).collect();
    assert!(approx(widths[0], 125.0), "grow 1 → 100 + 25");
    assert!(approx(widths[1], 125.0));
    assert!(approx(widths[2], 150.0), "grow 2 → 100 + 50");
    assert!(approx(items[2].border_box.x, 250.0));
}

/// flex 不换行（nowrap 默认）：基和超宽的项按 flex-shrink（初始值 1）等比收缩，
/// 容器高度不增高。（T3 差异：旧自研不实现收缩、任由项溢出，taffy 按规范收缩，
/// 见 block.rs 模块文档差异 2。）
#[test]
fn flex_row_shrinks_overflowing_items() {
    let (document, root) = layout(
        "<body><div style=\"display: flex; width: 200px\">\
         <div style=\"flex-basis: 100px; height: 10px\"></div>\
         <div style=\"flex-basis: 100px; height: 10px\"></div>\
         <div style=\"flex-basis: 100px; height: 10px\"></div></div></body>",
        "",
        200.0,
    );
    let container = &body(&document, &root).children[0];
    assert_eq!(container.children.len(), 3);
    // 收缩：赤字 100 按基权 100:100:100 分摊 → 每项 100 - 33.33 ≈ 66.67
    for (index, item) in container.children.iter().enumerate() {
        assert!(approx(item.border_box.y, 0.0), "不换行：所有项同处一行");
        assert!(
            approx(item.border_box.width, 200.0 / 3.0),
            "第 {index} 项按 flex-shrink 收缩"
        );
        assert!(
            approx(item.border_box.x, 200.0 / 3.0 * index as f32),
            "第 {index} 项主轴位置"
        );
    }
    assert!(
        approx(container.border_box.height, 10.0),
        "容器高度 = 行交叉尺寸，不因溢出增高"
    );
}

/// 交叉轴 stretch：容器指定高度时，高度 auto 的项拉伸到行交叉尺寸。
#[test]
fn flex_row_stretches_auto_height_items_to_container_height() {
    let (document, root) = layout(
        "<body><div style=\"display: flex; width: 300px; height: 50px\">\
         <div style=\"flex-grow: 1\"></div><div style=\"flex-grow: 1\"></div></div></body>",
        "",
        400.0,
    );
    let container = &body(&document, &root).children[0];
    assert!(approx(container.border_box.height, 50.0));
    for item in &container.children {
        assert!(approx(item.border_box.width, 150.0));
        assert!(
            approx(item.border_box.height, 50.0),
            "auto 高项拉伸到容器高"
        );
        assert!(approx(item.border_box.y, 0.0));
    }
}

/// inline-flex：行内级 flex 容器按 shrink-to-fit 参与行内行盒（原子盒）。
#[test]
fn inline_flex_participates_in_line_box_as_atomic_box() {
    let (document, root) = layout(
        "<body>x <i style=\"display: inline-flex\">\
         <b style=\"flex-grow: 1; flex-basis: 0px\"></b>\
         <b style=\"flex-grow: 1; flex-basis: 0px\"></b></i></body>",
        "i { width: 100px }",
        400.0,
    );
    let lines = &body(&document, &root).lines;
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].boxes.len(), 1, "inline-flex 作为原子盒进父行");
    let flex = &lines[0].boxes[0];
    assert!(
        approx(flex.border_box.width, 100.0),
        "shrink-to-fit 不超过指定宽"
    );
    assert_eq!(flex.children.len(), 2, "子项单行 flex 布局");
    assert!(
        approx(flex.children[0].border_box.width, 50.0),
        "两项等分主轴"
    );
    assert!(approx(flex.children[1].border_box.x, 50.0));
}

// ---------------------------------------------------------------------------
// T6 专项：grid / 多行 flex / 绝对定位 / contents 提升 / 匿名块混排
// ---------------------------------------------------------------------------

/// grid 双列模板：固定列 + `fr` 列（CSS Grid §7.1 轨道尺寸）。
#[test]
fn grid_two_column_template_places_items() {
    let (document, root) = layout(
        "<body><div style=\"display: grid; grid-template-columns: 100px 1fr\">\
         <div>a</div><div>b</div></div></body>",
        "",
        300.0,
    );
    let grid = &body(&document, &root).children[0];
    assert_eq!(grid.children.len(), 2, "块级子元素成为 grid item");
    assert!(approx(grid.children[0].border_box.x, 0.0));
    assert!(approx(grid.children[0].border_box.width, 100.0), "固定列宽");
    assert!(
        approx(grid.children[1].border_box.x, 100.0),
        "1fr 列从固定列之后开始"
    );
    assert!(
        approx(grid.children[1].border_box.width, 200.0),
        "1fr 分得剩余宽度（视口 300 - 100）"
    );
    assert!(
        approx(grid.children[1].border_box.y, 0.0),
        "单行 grid：两项同一行"
    );
}

/// 多行 flex wrap：放不下的项换到第二行，容器高度随行数增长（CSS Flexbox §6）。
#[test]
fn flex_wrap_moves_overflow_items_to_second_line() {
    let (document, root) = layout(
        "<body><div style=\"display: flex; flex-wrap: wrap; width: 250px\">\
         <div style=\"width: 100px; height: 20px\"></div>\
         <div style=\"width: 100px; height: 20px\"></div>\
         <div style=\"width: 100px; height: 20px\"></div></div></body>",
        "",
        400.0,
    );
    let container = &body(&document, &root).children[0];
    assert_eq!(container.children.len(), 3);
    assert!(approx(container.children[0].border_box.y, 0.0));
    assert!(
        approx(container.children[1].border_box.y, 0.0),
        "前两项同处第一行"
    );
    assert!(approx(container.children[2].border_box.x, 0.0));
    assert!(
        approx(container.children[2].border_box.y, 20.0),
        "放不下的项换到第二行"
    );
    assert!(
        approx(container.border_box.height, 40.0),
        "容器高度 = 两行交叉尺寸之和"
    );
}

/// 绝对定位 inset：left/top 从包含块（此处即树内父盒）padding 盒起算；
/// right/bottom 负向定位；绝对定位子盒不占流空间。
///
/// 坐标约定：Fragment border_box 相对父内容盒，无 padding 时与 padding 盒重合。
#[test]
fn absolute_inset_positions_box_relative_to_containing_block() {
    let (document, root) = layout(
        "<body><div style=\"position: relative; width: 200px; height: 100px\">\
         <div style=\"position: absolute; left: 30px; top: 10px; width: 50px; height: 20px\"></div>\
         <div style=\"position: absolute; right: 0px; bottom: 0px; width: 40px; height: 20px\">\
         </div></div></body>",
        "",
        400.0,
    );
    let wrap = &body(&document, &root).children[0];
    assert_eq!(wrap.children.len(), 2);
    let top_left = &wrap.children[0];
    let bottom_right = &wrap.children[1];
    assert!(
        approx(top_left.border_box.x, 30.0),
        "left:30 从包含块左缘起算"
    );
    assert!(approx(top_left.border_box.y, 10.0));
    assert!(
        approx(bottom_right.border_box.x, 160.0),
        "right:0 贴包含块右缘"
    );
    assert!(
        approx(bottom_right.border_box.y, 80.0),
        "bottom:0 贴包含块底缘"
    );
    assert!(
        approx(wrap.border_box.height, 100.0),
        "绝对定位子盒不把父盒撑高（不占流空间）"
    );
}

/// `display: contents`：元素自身无片段，子盒提升为父盒的直接子盒（CSS Display §3.2）。
#[test]
fn display_contents_children_appear_in_parent_fragment() {
    let (document, root) = layout(
        "<body><div style=\"height: 30px\"></div>\
         <section style=\"display: contents\">\
         <div style=\"height: 40px\"></div></section></body>",
        "",
        400.0,
    );
    let body_fragment = body(&document, &root);
    assert!(
        find(&document, &root, "section").is_none(),
        "contents 元素自身不产片段"
    );
    assert_eq!(
        body_fragment.children.len(),
        2,
        "contents 的子盒提升为 body 的直接子盒"
    );
    let lifted = &body_fragment.children[1];
    assert!(
        approx(lifted.border_box.y, 30.0),
        "提升的子盒接在首个子盒下方"
    );
    assert!(approx(lifted.border_box.height, 40.0));
}

/// 行内流与块级子盒混排：两段行内流各成匿名块（CSS 2.1 §9.2.1.1），三者按
/// DOM 序垂直排列、互不重叠；行盒由匿名块片段承接而非并入容器。
#[test]
fn mixed_inline_and_block_content_does_not_overlap() {
    let (document, root) = layout(
        "<body>intro line<div style=\"height: 25px\"></div>tail line</body>",
        "",
        400.0,
    );
    let body_fragment = body(&document, &root);
    assert!(
        body_fragment.lines.is_empty(),
        "混排容器：行盒不并入容器，由匿名块承接"
    );
    assert_eq!(
        body_fragment.children.len(),
        3,
        "匿名块 + 块级子盒 + 匿名块按 DOM 序排列"
    );
    let intro = &body_fragment.children[0];
    let mid = &body_fragment.children[1];
    let tail = &body_fragment.children[2];
    assert!(intro.anonymous && tail.anonymous, "行内流片段为匿名块");
    assert!(!mid.anonymous, "真实元素片段非匿名");
    assert!(
        approx(mid.border_box.y, intro.border_box.height),
        "块级子盒接在首段行内流（匿名块）下方"
    );
    assert!(
        approx(tail.border_box.y, intro.border_box.height + 25.0),
        "末段行内流接在块级子盒下方"
    );
    assert!(
        intro.border_box.y + intro.border_box.height <= mid.border_box.y + 0.01
            && mid.border_box.y + mid.border_box.height <= tail.border_box.y + 0.01,
        "三者垂直堆叠、互不重叠"
    );
}

// ---- text-align 行盒对齐（CSS Text 4 §5.1） ----

/// 取片段首行首个字形 x 与末字形右缘（行宽参照）。
fn line_extent(line: &LineFragment) -> (f32, f32) {
    let first = line
        .runs
        .first()
        .and_then(|run| run.glyphs.first())
        .expect("line has glyphs")
        .x;
    let mut end = first;
    for run in &line.runs {
        if let Some(glyph) = run.glyphs.last() {
            end = end.max(glyph.x + glyph.advance);
        }
    }
    (first, end)
}

#[test]
fn text_align_center_shifts_line() {
    let (document, root) = layout(
        "<div style=\"width: 400px; text-align: center\">hello world</div>",
        "",
        800.0,
    );
    let div = find(&document, &root, "div").expect("div fragment");
    let line = div.lines.first().expect("one line");
    let (first, end) = line_extent(line);
    // 整行平移：first = (400 - 行宽) / 2，行宽 = end - first
    assert!(
        approx(first, (400.0 - (end - first)) / 2.0),
        "居中：first={first}, end={end}"
    );
}

#[test]
fn text_align_right_shifts_line_to_edge() {
    let (document, root) = layout(
        "<div style=\"width: 400px; text-align: right\">hello world</div>",
        "",
        800.0,
    );
    let div = find(&document, &root, "div").expect("div fragment");
    let line = div.lines.first().expect("one line");
    let (first, end) = line_extent(line);
    assert!(
        approx(first, 400.0 - (end - first)),
        "右对齐：行尾贴内容盒右缘"
    );
}

#[test]
fn text_align_overfull_line_stays_left() {
    // 不可拆长词超出容器：自由空间为 0，center 不产生偏移（防负偏移）
    let (document, root) = layout(
        "<div style=\"width: 50px; text-align: center\">wwwwwwwwwwwwwwwwwwwwww</div>",
        "",
        800.0,
    );
    let div = find(&document, &root, "div").expect("div fragment");
    let line = div.lines.first().expect("one line");
    let (first, _) = line_extent(line);
    assert!(approx(first, 0.0), "溢出行退化为左对齐");
}

#[test]
fn text_align_inherits_into_block_children() {
    let (document, root) = layout(
        "<div style=\"width: 400px; text-align: center\"><p>ab</p></div>",
        "",
        800.0,
    );
    let p = find(&document, &root, "p").expect("p fragment");
    let line = p.lines.first().expect("one line");
    let (first, end) = line_extent(line);
    assert!(
        approx(first, (400.0 - (end - first)) / 2.0),
        "继承属性：块级子元素的行也居中"
    );
}

#[test]
fn text_align_applies_to_anonymous_blocks() {
    // 混排：行内流由匿名块承接，text-align 相对匿名块宽（=容器内容宽）平移
    let (document, root) = layout(
        "<div style=\"width: 400px; text-align: center\">intro tail<div></div></div>",
        "",
        800.0,
    );
    let div = find(&document, &root, "div").expect("div fragment");
    let anonymous = div.children.first().expect("anonymous block");
    assert!(anonymous.anonymous, "行内流为匿名块");
    let line = anonymous.lines.first().expect("one line");
    let (first, end) = line_extent(line);
    assert!(
        approx(first, (400.0 - (end - first)) / 2.0),
        "匿名块内的行同样居中"
    );
}

// ---- flex baseline 对齐（taffy run 叶上报首行 baseline） ----

#[test]
fn flex_baseline_aligns_first_line_baselines() {
    // run 叶向 taffy 上报首行基线后，align-items: baseline 应让两个不同
    // 字号 item 的首行基线落在同一水平线（退化行为是顶对齐，基线错位）
    let (document, root) = layout(
        "<div style=\"display: flex; align-items: baseline\">\
         <span style=\"font-size: 24px\">big</span>\
         <span style=\"font-size: 14px\">small</span></div>",
        "",
        800.0,
    );
    let flex = find(&document, &root, "div").expect("flex container");
    assert_eq!(flex.children.len(), 2, "两个 span 各成一个 item");
    let baseline_of = |item: &Fragment| {
        let line = item.lines.first().expect("item has a line");
        item.border_box.y + line.baseline
    };
    let big = baseline_of(&flex.children[0]);
    let small = baseline_of(&flex.children[1]);
    assert!(approx(big, small), "首行基线对齐：big={big}, small={small}");
    // 且确实发生了基线对齐而非顶对齐：小字号 item 必须被下移
    assert!(
        flex.children[1].border_box.y > 0.5,
        "小字号 item 相对容器顶下移（baseline 对齐生效）"
    );
}

#[test]
fn flex_baseline_without_text_still_lays_out() {
    // 无文本 item（空 span + 有文本 span）：无 baseline 的一侧不 panic，
    // 退化为各自静态位置
    let (document, root) = layout(
        "<div style=\"display: flex; align-items: baseline\">\
         <span></span><span>text</span></div>",
        "",
        800.0,
    );
    let flex = find(&document, &root, "div").expect("flex container");
    assert_eq!(flex.children.len(), 2);
    assert!(flex.children[0].border_box.y >= 0.0);
}

// ---- 不可信输入边界：巨数属性注入（数值卫生） ----

#[test]
fn huge_img_attribute_falls_back_to_natural_size_on_both_paths() {
    // 45 位巨数：旧实现 parse::<f32>() 得 inf 且通过 > 0 过滤，沿布局扩散
    let huge = "9".repeat(45);

    // 行内路径：属性无效回落默认对象尺寸（CSS Images §5.1 的 300×150）
    let (document, root) = layout(
        &format!("<body><img width=\"{huge}\" height=\"{huge}\" src=a.png></body>"),
        "",
        400.0,
    );
    let body_fragment = body(&document, &root);
    let image = &body_fragment.lines[0].images[0];
    assert!(
        approx(image.width, 300.0) && approx(image.height, 150.0),
        "行内 <img> 巨数属性回落，实际 {:?}",
        (image.width, image.height)
    );

    // 块级路径（display: block 的替换盒）同样回落
    let (document, root) = layout(
        &format!(
            "<body><img style=\"display: block\" width=\"{huge}\" height=\"{huge}\" src=a.png></body>"
        ),
        "",
        400.0,
    );
    let img = find(&document, &root, "img").expect("img fragment");
    assert!(
        approx(img.border_box.width, 300.0) && approx(img.border_box.height, 150.0),
        "块级 <img> 巨数属性回落，实际 {:?}",
        (img.border_box.width, img.border_box.height)
    );
}

#[test]
fn img_attribute_boundary_respects_max_css_length() {
    // 恰好 1e7 通过（有效，按属性值布局）
    let (document, root) = layout(
        "<body><img width=\"10000000\" height=\"10000000\" src=a.png></body>",
        "",
        400.0,
    );
    let image = &body(&document, &root).lines[0].images[0];
    assert!(approx(image.width, 1.0e7) && approx(image.height, 1.0e7));

    // 超限 1px 即无效，回落自然尺寸
    let (document, root) = layout(
        "<body><img width=\"10000001\" height=\"10000001\" src=a.png></body>",
        "",
        400.0,
    );
    let image = &body(&document, &root).lines[0].images[0];
    assert!(
        approx(image.width, 300.0) && approx(image.height, 150.0),
        "超限属性回落，实际 {:?}",
        (image.width, image.height)
    );
}

#[test]
fn huge_attribute_does_not_pollute_sibling_coordinates() {
    // 巨数图片夹在两段文本之间：同行其余元素的坐标必须保持有限、不受污染
    let huge = "9".repeat(45);
    let (document, root) = layout(
        &format!("<body>a <img width=\"{huge}\" height=\"{huge}\" src=a.png> b</body>"),
        "",
        400.0,
    );
    let line = &body(&document, &root).lines[0];
    assert!(line.height.is_finite(), "行高有限");
    assert!(line.baseline.is_finite(), "baseline 有限");
    for run in &line.runs {
        assert!(
            run.glyphs
                .iter()
                .all(|glyph| glyph.x.is_finite() && glyph.advance.is_finite()),
            "文本 run 字形坐标有限（绝对坐标 = 字形 x，污染即在此可见）"
        );
    }
    for image in &line.images {
        assert!(image.x.is_finite() && image.y.is_finite());
        assert!(image.width.is_finite() && image.height.is_finite());
    }
}

#[test]
fn huge_css_values_still_layout_finite() {
    // font-size 巨数 em：计算出口钳到上限，页面可布局
    let (document, root) = layout(
        "<body><p style=\"font-size: 1e30em\">x</p></body>",
        "",
        400.0,
    );
    let p = find(&document, &root, "p").expect("p fragment");
    assert!(p.border_box.width.is_finite() && p.border_box.height.is_finite());
    assert!(
        p.border_box.height <= 1.0e7 * 2.0,
        "字号钳在 1e7 量级内，实际 {}",
        p.border_box.height
    );

    // width: 1e40px → 声明无效回落 auto；巨数 margin 同理
    let (document, root) = layout(
        "<body><div style=\"width: 1e40px; margin-top: 1e40px; height: 20px\"></div></body>",
        "",
        400.0,
    );
    let div = find(&document, &root, "div").expect("div fragment");
    assert!(
        approx(div.border_box.width, 400.0),
        "width 无效回落 auto=包含块宽"
    );
    assert!(approx(div.border_box.y, 0.0), "margin-top 无效回落 0");
}

// ---- CSS Text 间距：letter-spacing / word-spacing（CSS Text 3 §8） ----

#[test]
fn letter_spacing_widens_line_by_internal_boundaries() {
    // N 个字形簇的文本加字距 s：行净宽差 = (N−1)×s（行末簇的字距不计）
    let (document, root) = layout("<body>abcd</body>", "", 400.0);
    let plain = line_extent(&body(&document, &root).lines[0]).1;

    let (document, root) = layout("<body>abcd</body>", "body { letter-spacing: 4px }", 400.0);
    let spaced = line_extent(&body(&document, &root).lines[0]).1;
    assert!(
        approx(spaced - plain, 3.0 * 4.0),
        "4 簇文本行净宽差 = 3×4px：{spaced} - {plain}"
    );

    // 负字距收窄且不 panic（字形可重叠）
    let (document, root) = layout("<body>abcd</body>", "body { letter-spacing: -2px }", 400.0);
    let narrow = line_extent(&body(&document, &root).lines[0]).1;
    assert!(
        approx(narrow - plain, -3.0 * 2.0),
        "负字距按 (N−1)×|s| 收窄：{narrow} - {plain}"
    );
}

#[test]
fn letter_spacing_trailing_excluded_from_right_alignment() {
    // 右对齐用净宽：行末字距不占位，行尾贴内容盒右缘
    let (document, root) = layout(
        "<body>abcd</body>",
        "body { text-align: right; letter-spacing: 4px }",
        400.0,
    );
    let (first, end) = line_extent(&body(&document, &root).lines[0]);
    assert!(approx(end, 400.0), "行尾贴右缘（end={end}）");
    let plain_width = {
        let (document, root) = layout("<body>abcd</body>", "", 400.0);
        line_extent(&body(&document, &root).lines[0]).1
    };
    assert!(
        approx(first, 400.0 - plain_width - 3.0 * 4.0),
        "行首 x = 可用宽 − 净宽（first={first}）"
    );
}

#[test]
fn word_spacing_widens_single_line_by_separator_count() {
    let (document, root) = layout("<body>a b c</body>", "", 400.0);
    let plain = line_extent(&body(&document, &root).lines[0]).1;

    let (document, root) = layout("<body>a b c</body>", "body { word-spacing: 8px }", 400.0);
    let spaced = line_extent(&body(&document, &root).lines[0]).1;
    assert!(
        approx(spaced - plain, 2.0 * 8.0),
        "两个分隔符各 +8px：{spaced} - {plain}"
    );
}

#[test]
fn word_spacing_separators_count_into_narrow_measurement() {
    // width: 0 逐 span 成行 = 最窄测量路径：每个分隔符落在行尾仍占宽
    //（break-spaces 行尾空白不裁剪）→ 行净宽差合计 = 8px × 分隔符数
    let (document, root) = layout(
        "<div style=\"width: 0; white-space: break-spaces\">a b c</div>",
        "",
        400.0,
    );
    let div = find(&document, &root, "div").expect("div fragment");
    let plain: Vec<f32> = div.lines.iter().map(|line| line_extent(line).1).collect();
    assert_eq!(plain.len(), 3, "逐词成行");

    let (document, root) = layout(
        "<div style=\"width: 0; white-space: break-spaces; word-spacing: 8px\">a b c</div>",
        "",
        400.0,
    );
    let div = find(&document, &root, "div").expect("div fragment");
    let spaced: Vec<f32> = div.lines.iter().map(|line| line_extent(line).1).collect();
    assert_eq!(spaced.len(), 3);
    let diff: f32 = spaced.iter().zip(&plain).map(|(s, p)| s - p).sum();
    assert!(approx(diff, 2.0 * 8.0), "行尾分隔符各 +8px：差合计 {diff}");
}

// ---- text-align: justify（CSS Text 3 §7，auto 方法的可执行近似） ----

#[test]
fn justify_flushes_non_last_lines_to_container_width() {
    // 非末行右缘贴满容器（机会点均匀分配自由空间），末行左 ragged。
    // 断言不依赖字体度量：不断言具体像素宽（各平台字体 advance 不同），
    // 而是断言不变量——所有非末行 justify 后右缘彼此相等（同为可用宽），
    // 且都大于末行右缘（末行不分配）。
    let (document, root) = layout(
        "<body><p style=\"width: 100px; text-align: justify\">\
         aaa bbb ccc ddd eee fff ggg hhh</p></body>",
        "",
        400.0,
    );
    let p = find(&document, &root, "p").expect("p fragment");
    assert!(p.lines.len() >= 2, "文本折成多行");
    let non_last: Vec<f32> = p.lines[..p.lines.len() - 1]
        .iter()
        .map(|line| line_extent(line).1)
        .collect();
    let last_end = line_extent(p.lines.last().expect("last line")).1;
    let flushed = non_last[0];
    for (index, end) in non_last.iter().enumerate() {
        assert!(
            approx(*end, flushed),
            "第 {index} 行 justify 后右缘与首行一致：{end} vs {flushed}"
        );
        assert!(
            *end > last_end,
            "第 {index} 行右缘大于末行（末行不分配）：{end} vs {last_end}"
        );
    }
}

#[test]
fn justify_line_without_opportunities_stays_left_and_finite() {
    // 断点恰在唯一空格处：首行只剩 "aaaaaa"——无空格字形、无同块簇边界
    //（单 span），机会点为 0 → 退化左对齐，与 left 布局逐字形相同
    let lines_of = |align: &str| {
        let (document, root) = layout(
            &format!("<body><p style=\"width: 60px; {align}\">aaaaaa bbbbbb</p></body>"),
            "",
            400.0,
        );
        let p = find(&document, &root, "p").expect("p fragment");
        p.lines
            .iter()
            .map(|line| {
                line.runs
                    .iter()
                    .flat_map(|run| {
                        run.glyphs
                            .iter()
                            .map(|glyph| (glyph.x, glyph.advance))
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };
    let justified = lines_of("text-align: justify;");
    let left = lines_of("");
    assert_eq!(justified.len(), 2, "两词各自成行");
    assert_eq!(justified, left, "无机会行退化左对齐（逐字形同构）");
}

#[test]
fn justify_does_not_apply_to_pre_lines() {
    // pre 无折叠空格无软折行：各行以硬断行结尾或为末行，不分配间距
    let (document, root) = layout(
        "<body><pre style=\"width: 400px; text-align: justify\">aaaa bbbb cccc\
         \ndddd</pre></body>",
        "",
        400.0,
    );
    let pre = find(&document, &root, "pre").expect("pre fragment");
    assert_eq!(pre.lines.len(), 2);
    let (first, end) = line_extent(&pre.lines[0]);
    assert!(approx(first, 0.0));
    assert!(end < 300.0, "pre 行保持自然宽度，不拉伸：{end}");
}

#[test]
fn justify_with_zero_free_space_matches_left_exactly() {
    // 自由空间为 0（溢出行）：justify 与 left 的字形坐标逐个相同
    let (document, root) = layout(
        "<body><p style=\"width: 60px; text-align: justify\">aaaaaaaaaa bbbbbbbbbb</p></body>",
        "",
        400.0,
    );
    let justified: Vec<Vec<f32>> = find(&document, &root, "p")
        .expect("p")
        .lines
        .iter()
        .map(|line| {
            line.runs
                .iter()
                .flat_map(|run| run.glyphs.iter().map(|glyph| glyph.x).collect::<Vec<_>>())
                .collect()
        })
        .collect();
    let (document, root) = layout(
        "<body><p style=\"width: 60px\">aaaaaaaaaa bbbbbbbbbb</p></body>",
        "",
        400.0,
    );
    let left: Vec<Vec<f32>> = find(&document, &root, "p")
        .expect("p")
        .lines
        .iter()
        .map(|line| {
            line.runs
                .iter()
                .flat_map(|run| run.glyphs.iter().map(|glyph| glyph.x).collect::<Vec<_>>())
                .collect()
        })
        .collect();
    assert_eq!(justified, left, "自由空间为 0 时与 left 逐字形同构");
}

#[test]
fn justify_keeps_intrinsic_measurements() {
    // measure 路径不分配间距：justify 与 left 的 min/max-content 相等
    //（max-content 经 shrink-to-fit inline-block 宽度观察）
    let widths = |align: &str| {
        let (document, root) = layout(
            &format!(
                "<body><i style=\"display: inline-block; {align}\">\
                 aaa bbb ccc ddd eee fff ggg hhh iii jjj</i></body>"
            ),
            "",
            400.0,
        );
        let atomic = &body(&document, &root).lines[0].boxes[0];
        atomic.border_box.width
    };
    let max_content_left = widths("");
    let max_content_justify = widths("text-align: justify;");
    assert!(
        approx(max_content_left, max_content_justify),
        "max-content 不受 justify 影响：{max_content_left} vs {max_content_justify}"
    );

    // min-content（width: 0 逐 span 成行）同样不受影响
    let min_widths = |align: &str| {
        let (document, root) = layout(
            &format!("<body><div style=\"width: 0; {align}\">aaa bbb ccc</div></body>"),
            "",
            400.0,
        );
        find(&document, &root, "div")
            .expect("div")
            .lines
            .iter()
            .map(|line| line_extent(line).1)
            .collect::<Vec<_>>()
    };
    assert_eq!(min_widths(""), min_widths("text-align: justify;"));
}

// ---- text-indent（CSS Text 3 §4） ----

#[test]
fn text_indent_shifts_first_line_only() {
    let (document, root) = layout(
        "<body><p style=\"width: 80px; text-indent: 40px\">aaa bbb ccc ddd</p></body>",
        "",
        400.0,
    );
    let p = find(&document, &root, "p").expect("p fragment");
    assert!(p.lines.len() >= 2, "折成多行");
    let first_x = p.lines[0].runs[0].glyphs[0].x;
    let second_x = p.lines[1].runs[0].glyphs[0].x;
    assert!(approx(first_x, 40.0), "首行缩进 40px：{first_x}");
    assert!(approx(second_x, 0.0), "第二行不缩进：{second_x}");
}

#[test]
fn text_indent_percentage_resolves_against_containing_block() {
    // auto 宽 p 填满 body（内容宽 400）→ 10% = 40px
    let (document, root) = layout(
        "<body><p style=\"text-indent: 10%\">abc</p></body>",
        "",
        400.0,
    );
    let p = find(&document, &root, "p").expect("p fragment");
    let first_x = p.lines[0].runs[0].glyphs[0].x;
    assert!(approx(first_x, 40.0), "10% 按可用宽折算：{first_x}");

    // 行盒的包含块 = 块容器自身内容盒：显式宽度元素按自身内容宽折算
    //（与 Chromium 一致，Edge 实机对照 420px 宽 × 10% ≈ 42px）——200 × 10% = 20
    let (document, root) = layout(
        "<body><p style=\"width: 200px; text-indent: 10%\">abc</p></body>",
        "",
        400.0,
    );
    let p = find(&document, &root, "p").expect("p fragment");
    let first_x = p.lines[0].runs[0].glyphs[0].x;
    assert!(
        approx(first_x, 20.0),
        "显式宽度下按自身内容宽折算：{first_x}"
    );
}

#[test]
fn text_indent_applies_before_alignment() {
    // center：先缩进后居中——文本在剩余空间 [40, 200] 内居中
    let (document, root) = layout(
        "<body><p style=\"width: 200px; text-indent: 40px; text-align: center\">abc</p></body>",
        "",
        400.0,
    );
    let p = find(&document, &root, "p").expect("p fragment");
    let (first, end) = line_extent(&p.lines[0]);
    let content = end - first;
    assert!(
        approx(first, 40.0 + (160.0 - content) / 2.0),
        "缩进后在剩余空间居中：{first}"
    );
}

#[test]
fn text_indent_participates_in_justified_first_line() {
    // justify：首行带缩进且右缘仍贴满容器（首行须有 2+ 个放置项才有机会点）。
    // 语料用单字母词，保证任何平台字体下缩进后的首行都能放下两个词——
    // 否则（如 ubuntu 的宽 serif 上首行只剩一个词）无机会点、正确行为是
    // 不拉伸，断言就会把字体差异误判成实现缺陷。
    let (document, root) = layout(
        "<body><p style=\"width: 100px; text-indent: 40px; text-align: justify\">\
         a b c d e f g h i j k l m n o p</p></body>",
        "",
        400.0,
    );
    let p = find(&document, &root, "p").expect("p fragment");
    assert!(p.lines.len() >= 2);
    let first_x = p.lines[0].runs[0].glyphs[0].x;
    assert!(approx(first_x, 40.0), "缩进保留：{first_x}");
    // 不变量（不依赖字体度量）：非末行（含首行）右缘彼此相等，且都大于
    // 末行右缘——缩进只占首行行首，不改变 justify 的右缘。
    let non_last: Vec<f32> = p.lines[..p.lines.len() - 1]
        .iter()
        .map(|line| line_extent(line).1)
        .collect();
    let last_end = line_extent(p.lines.last().expect("last line")).1;
    let flushed = non_last[0];
    for (index, end) in non_last.iter().enumerate() {
        assert!(
            approx(*end, flushed),
            "第 {index} 行 justify 后右缘与首行一致：{end} vs {flushed}"
        );
        assert!(
            *end > last_end,
            "第 {index} 行右缘大于末行（末行不分配）：{end} vs {last_end}"
        );
    }
}

#[test]
fn text_indent_excluded_from_intrinsic_sizes() {
    // css-sizing：min/max-content 测量不含缩进（与 Chromium 一致）
    let width_of = |indent: &str| {
        let (document, root) = layout(
            &format!(
                "<body><i style=\"display: inline-block; {indent}\">\
                 aaa bbb ccc ddd eee fff ggg hhh</i></body>"
            ),
            "",
            400.0,
        );
        body(&document, &root).lines[0].boxes[0].border_box.width
    };
    let plain = width_of("");
    let indented = width_of("text-indent: 40px;");
    assert!(
        approx(plain, indented),
        "max-content 不含缩进：{plain} vs {indented}"
    );

    // min-content：容器压到 1px，inline-block 宽被 min-content 托底
    //（AvailableSpace::MinContent 测量路径不含缩进）
    let min_content_of = |indent: &str| {
        let (document, root) = layout(
            &format!(
                "<body><div style=\"width: 1px\">\
                 <i style=\"display: inline-block; {indent}\">aaa bbb ccc</i></div></body>"
            ),
            "",
            400.0,
        );
        let div = find(&document, &root, "div").expect("div");
        div.lines[0].boxes[0].border_box.width
    };
    let plain_min = min_content_of("");
    let indented_min = min_content_of("text-indent: 40px;");
    assert!(
        approx(plain_min, indented_min),
        "min-content 不含缩进：{plain_min} vs {indented_min}"
    );
}

// ---- tab-size 与 pre-wrap 行尾悬挂（CSS Text 3 §5） ----

/// pre 单行内全部字形的 (x, advance)。
fn pre_glyphs(document: &Document, root: &Fragment) -> Vec<(f32, f32)> {
    let pre = find(document, root, "pre").expect("pre fragment");
    pre.lines[0]
        .runs
        .iter()
        .flat_map(|run| {
            run.glyphs
                .iter()
                .map(|glyph| (glyph.x, glyph.advance))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn tab_size_advances_to_next_stop() {
    // a\tb（tab-size 4）：制表推进到 4 × 空格宽的下一整数倍 → b 在第 4 列
    let (document, root) = layout("<pre>a\tb</pre>", "pre { tab-size: 4 }", 400.0);
    let glyphs = pre_glyphs(&document, &root);
    assert_eq!(glyphs.len(), 3, "a、制表、b 三个字形");

    let (document, root) = layout("<pre>a b</pre>", "", 400.0);
    let reference = pre_glyphs(&document, &root);
    let space_advance = reference[1].1;
    let (a_x, a_advance) = glyphs[0];
    assert!(approx(a_x, 0.0));
    assert!(
        approx(glyphs[2].0, 4.0 * space_advance),
        "b 推进到第 4 列：{} vs 4×{space_advance}",
        glyphs[2].0
    );
    assert!(approx(a_advance, reference[0].1), "a 宽不受影响");
}

#[test]
fn tab_size_zero_collapses_tab() {
    let (document, root) = layout("<pre>a\tb</pre>", "pre { tab-size: 0 }", 400.0);
    let glyphs = pre_glyphs(&document, &root);
    assert_eq!(glyphs.len(), 3);
    let (a_x, a_advance) = glyphs[0];
    assert!(approx(glyphs[2].0, a_x + a_advance), "tab-size 0：制表零宽");
}

#[test]
fn tab_size_length_form_uses_pixel_stops() {
    // tab-size: 40px：制表推进到 40px 的下一整数倍
    let (document, root) = layout("<pre>a\tb</pre>", "pre { tab-size: 40px }", 400.0);
    let glyphs = pre_glyphs(&document, &root);
    let (a_x, _) = glyphs[0];
    assert!(
        approx(glyphs[2].0, 40.0),
        "b 推进到 40px 整数倍：{}",
        glyphs[2].0
    );
    assert!(approx(a_x, 0.0));
}

#[test]
fn tab_stops_accumulate_across_multiple_tabs() {
    // a\t\tb：第一个制表到第 4 列，第二个从第 4 列到第 8 列
    let (document, root) = layout("<pre>a\t\tb</pre>", "pre { tab-size: 4 }", 400.0);
    let glyphs = pre_glyphs(&document, &root);
    let (document, root) = layout("<pre>a b</pre>", "", 400.0);
    let space_advance = pre_glyphs(&document, &root)[1].1;
    let b_x = glyphs.last().expect("b glyph").0;
    assert!(approx(b_x, 8.0 * space_advance), "b 推进到第 8 列：{b_x}");
}

#[test]
fn pre_wrap_trailing_spaces_hang_at_line_end() {
    // pre-wrap 行尾保留空格悬挂：不占对齐宽度（右对齐时可见文本贴右缘），
    // break-spaces 不悬挂（空格把可见文本往左推）
    let visible_end = |white_space: &str| {
        let (document, root) = layout(
            &format!(
                "<body><div style=\"width: 300px; text-align: right;\
                 white-space: {white_space}\">abc   </div></body>"
            ),
            "",
            400.0,
        );
        let div = find(&document, &root, "div").expect("div");
        // 第三个字形（c）的右缘 = 可见文本末尾
        let glyph = &div.lines[0].runs[0].glyphs[2];
        glyph.x + glyph.advance
    };
    let pre_wrap_end = visible_end("pre-wrap");
    assert!(
        approx(pre_wrap_end, 300.0),
        "pre-wrap 悬挂：可见文本贴右缘 {pre_wrap_end}"
    );
    let break_spaces_end = visible_end("break-spaces");
    assert!(
        break_spaces_end < 300.0,
        "break-spaces 不悬挂，行尾空格占宽：{break_spaces_end}"
    );
}

#[test]
fn pre_wrap_hanging_spaces_do_not_force_wrap() {
    // 行尾空格不触发折行：可用宽恰容纳可见文本时，空格悬挂不产生第二行
    let (document, root) = layout(
        "<body><p style=\"width: 60px; white-space: pre-wrap\">aaaa   </p></body>",
        "",
        400.0,
    );
    let p = find(&document, &root, "p").expect("p fragment");
    assert_eq!(p.lines.len(), 1, "悬挂空格不触发折行");
}

// ---- M9：行内子树中的块级子元素不再丢弃（CSS 2.2 §9.2.1.1） ----

#[test]
fn block_in_inline_subtree_splits_into_anonymous_blocks() {
    // <div>a <span><div>X</div></span> b</div>：span 内的块级 X 终止当前
    // 行内流——「a 」与「 b」各成匿名块，X 按普通块盒布局，三者垂直堆叠
    let (document, root) = layout(
        "<body><div>a <span><div>X</div></span> b</div></body>",
        "",
        400.0,
    );
    let outer = find(&document, &root, "div").expect("outer div");
    assert!(outer.lines.is_empty(), "混排容器：行盒由匿名块承接");
    assert_eq!(outer.children.len(), 3, "匿名块(a) + X + 匿名块(b)");
    let before = &outer.children[0];
    let x = &outer.children[1];
    let after = &outer.children[2];
    assert!(before.anonymous && after.anonymous, "两段行内流为匿名块");
    assert!(!x.anonymous, "X 是真实元素盒");
    assert_eq!(before.lines.len(), 1, "前半 run 成行");
    assert_eq!(after.lines.len(), 1, "后半 run 成行");
    // 垂直堆叠不重叠
    assert!(
        before.border_box.y + before.border_box.height <= x.border_box.y + 0.01,
        "X 接在前半行内流下方"
    );
    assert!(
        x.border_box.y + x.border_box.height <= after.border_box.y + 0.01,
        "后半行内流接在 X 下方"
    );

    // X 的文本可被 TextMap 命中并复制；块边界插换行
    let map = nexty_layout::TextMap::new(&root);
    let (start, end) = map.all().expect("document has text");
    let text = map.text(start, end);
    assert_eq!(text, "a\nX\nb", "复制文本含 X（块边界换行）");
    // X 的行盒有内容（「X」字形存在）
    assert_eq!(x.lines.len(), 1);
    assert_eq!(x.lines[0].runs.len(), 1);
}

#[test]
fn deeply_nested_block_in_inline_subtree_survives() {
    // 两层行内元素包一层块级：块级子盒仍提升为容器的参与盒
    let (document, root) = layout("<body><span><em><div>Y</div></em></span></body>", "", 400.0);
    let body_fragment = body(&document, &root);
    assert_eq!(body_fragment.children.len(), 1, "Y 是 body 的参与盒");
    let y = &body_fragment.children[0];
    assert!(!y.anonymous);
    assert_eq!(y.lines.len(), 1, "Y 的内容成行");
    assert!(
        !y.lines[0].runs.is_empty(),
        "Y 的文本不丢（M9 缺陷下整棵子树消失）"
    );
}

#[test]
fn display_none_and_contents_in_inline_subtree_unchanged() {
    // none 子树不产内容；contents 透传文本；混合后行内流连续
    let (document, root) = layout(
        "<body><div>a<span>\
         <div style=\"display: none\">gone</div>\
         <section style=\"display: contents\">C</section>\
         b</span></div></body>",
        "",
        400.0,
    );
    let outer = find(&document, &root, "div").expect("outer div");
    assert_eq!(
        outer.children.len(),
        0,
        "无块级子盒（none 跳过、contents 穿透）"
    );
    assert_eq!(outer.lines.len(), 1, "全部行内内容在一个 run 叶");
    let line = &outer.lines[0];
    let text: String = line.runs.iter().map(|run| run.text.as_ref()).collect();
    assert_eq!(text, "aCb", "none 丢弃、contents 穿透、其余连续");
}
