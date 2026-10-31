//! K6 T4 集成测试：容器滚动/裁剪（chrome 层）。
//!
//! 覆盖泳道退出条件：容器滚动（clip + 偏移绘制期施加）、hidden 裁剪不滚动、
//! 偏移保留与 clamp（scrollTop 语义）、双容器独立滚动、容器滚动条几何与拖拽、
//! 滚轮由内向外链式消费与边界冒泡。窗口交互（winit 输入路由）无头环境不可
//! 自动化，实机判据见 `tools/acceptance/pages/batch2/k6/`。

use std::collections::HashMap;

use nexty_chrome::pipeline;
use nexty_chrome::scrollbar::{self, ScrollGeometry, Scrollbar};
use nexty_dom::NodeId;
use nexty_layout::Fragment;
use nexty_paint::{Command, Rect as PaintRect, Scene};
use nexty_text::ParleyTextShaper;

/// 按 id 找元素节点。
fn find_by_id(page: &pipeline::Page, id: &str) -> NodeId {
    fn walk(page: &pipeline::Page, node: NodeId, id: &str) -> Option<NodeId> {
        if let Some(nexty_dom::NodeKind::Element(data)) = page.document.node(node)
            && data
                .attributes
                .iter()
                .any(|attr| attr.name == "id" && attr.value == id)
        {
            return Some(node);
        }
        page.document
            .children(node)
            .find_map(|child| walk(page, child, id))
    }
    walk(page, page.document.root(), id).expect("element with id")
}

/// 布局一页（默认视口宽 400）。
fn lay(page: &pipeline::Page, width: f32) -> Fragment {
    let shaper = ParleyTextShaper::new();
    pipeline::layout_page(page, &shaper, width).expect("root fragment")
}

/// 场景中全部 PushClip 矩形。
fn clip_rects(scene: &Scene) -> Vec<PaintRect> {
    scene
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::PushClip { rect } => Some(*rect),
            _ => None,
        })
        .collect()
}

/// 场景中全部文本基线（DrawText 顺序 = 绘制顺序 = 文档序）。
fn baselines(scene: &Scene) -> Vec<f32> {
    scene
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::DrawText { baseline, .. } => Some(*baseline),
            _ => None,
        })
        .collect()
}

/// 容器滚动：PushClip 裁到 padding box、内容随 scrollTop 整体上移、
/// 裁剪矩形本身不平移、Push/Pop 配对。
#[test]
fn scrollable_container_emits_clip_and_applies_scroll_offset() {
    let mut page = pipeline::load_page(
        "<body><div id=\"box\" style=\"overflow: auto; height: 100px; border: 3px solid black\">\
         <p style=\"margin: 0\">first</p><p style=\"margin: 0\">second</p>\
         <p style=\"margin: 0\">third</p><p style=\"margin: 0\">fourth</p>\
         <p style=\"margin: 0\">fifth</p><p style=\"margin: 0\">sixth</p></div></body>",
        "body { margin: 0 }",
    );
    let root = lay(&page, 400.0);

    let scene0 = pipeline::build_scene(&page, &root);
    let clips = clip_rects(&scene0);
    assert_eq!(clips.len(), 1, "一个滚动容器恰一条 PushClip");
    // padding box = 边框盒收缩 border：body margin 0，div 在 (0,0)；content-box
    // 下 height:100 = 内容高，border 3 → padding box (3,3,394,100)
    assert!(
        (clips[0].x - 3.0).abs() < 0.01,
        "clip x = 盒 x + border.left"
    );
    assert!((clips[0].y - 3.0).abs() < 0.01);
    assert!((clips[0].width - 394.0).abs() < 0.01);
    assert!((clips[0].height - 100.0).abs() < 0.01, "裁剪 = padding box");
    let pops = scene0
        .commands
        .iter()
        .filter(|command| matches!(command, Command::PopClip))
        .count();
    assert_eq!(pops, 1, "PushClip/PopClip 配对");

    // 偏移 30：裁剪矩形不变，文本基线整体上移 30
    let box_node = find_by_id(&page, "box");
    page.scroll_offsets.insert(box_node, 30.0);
    let scene1 = pipeline::build_scene(&page, &root);
    assert_eq!(clip_rects(&scene1), clips, "裁剪矩形不随内容滚动平移");
    let b0 = baselines(&scene0);
    let b1 = baselines(&scene1);
    assert_eq!(b0.len(), b1.len());
    assert!(!b0.is_empty(), "场景含文本");
    for (before, after) in b0.iter().zip(&b1) {
        assert!(
            (before - after - 30.0).abs() < 0.01,
            "内容随 scrollTop 上移 30：{before} → {after}"
        );
    }
}

/// hidden 裁剪：PushClip 存在但不可滚动——不进滚动条区域、不进滚轮链、
/// 手工塞入的偏移被绘制忽略并被 clamp 清除。
#[test]
fn hidden_container_clips_without_scrolling() {
    let mut page = pipeline::load_page(
        "<body><div id=\"box\" style=\"overflow: hidden; height: 100px\">\
         <p style=\"margin: 0\">first</p><p style=\"margin: 0\">second</p>\
         <p style=\"margin: 0\">third</p><p style=\"margin: 0\">fourth</p>\
         <p style=\"margin: 0\">fifth</p><p style=\"margin: 0\">sixth</p></div></body>",
        "body { margin: 0 }",
    );
    let root = lay(&page, 400.0);

    let scene0 = pipeline::build_scene(&page, &root);
    assert_eq!(clip_rects(&scene0).len(), 1, "hidden 容器裁剪内容");
    // hidden 不是滚动容器：滚动条区域与滚轮链都不含它
    let areas = pipeline::collect_container_scroll_areas(&root, &page.scroll_offsets);
    assert!(areas.is_empty(), "hidden 不产生滚动条区域");
    let chain = pipeline::scroll_chain_at(&root, 10.0, 50.0, &page.scroll_offsets);
    assert!(chain.is_empty(), "hidden 容器不参与滚轮链");

    // 手工塞入偏移：绘制期忽略（scrollable=false），clamp 清除条目
    let box_node = find_by_id(&page, "box");
    page.scroll_offsets.insert(box_node, 40.0);
    let scene1 = pipeline::build_scene(&page, &root);
    assert_eq!(
        baselines(&scene0),
        baselines(&scene1),
        "hidden 容器的偏移不影响绘制"
    );
    pipeline::clamp_scroll_offsets(&root, &mut page.scroll_offsets);
    assert!(
        !page.scroll_offsets.contains_key(&box_node),
        "clamp 清除非滚动容器的条目"
    );
}

/// 偏移保留与 clamp（scrollTop 语义）：重排（同 Page 再布局）保留界内偏移、
/// 超界偏移夹到新 max、无效条目清除。
#[test]
fn scroll_offsets_survive_relayout_and_clamp() {
    let mut page = pipeline::load_page(
        "<body id=\"page\"><div id=\"box\" style=\"overflow: auto; height: 100px\">\
         <p style=\"margin: 0\">first</p><p style=\"margin: 0\">second</p>\
         <p style=\"margin: 0\">third</p><p style=\"margin: 0\">fourth</p>\
         <p style=\"margin: 0\">fifth</p><p style=\"margin: 0\">sixth</p></div></body>",
        "body { margin: 0 }",
    );
    let root = lay(&page, 400.0);
    let areas = pipeline::collect_container_scroll_areas(&root, &page.scroll_offsets);
    assert_eq!(areas.len(), 1);
    let (node, max) = (areas[0].node, areas[0].max_scroll);
    assert!(max > 0.0, "内容超出容器：max_scroll = {max}");

    // 界内偏移：clamp 保留
    let mut offsets = HashMap::from([(node, max / 2.0)]);
    pipeline::clamp_scroll_offsets(&root, &mut offsets);
    assert!((offsets[&node] - max / 2.0).abs() < 1e-4, "界内偏移不变");

    // 超界偏移：夹到 max
    offsets.insert(node, max * 10.0);
    pipeline::clamp_scroll_offsets(&root, &mut offsets);
    assert!((offsets[&node] - max).abs() < 1e-4, "超界偏移夹到 max");

    // 重排（同 Page 再布局，scrollTop 语义）：偏移保留、clamp 仍成立；
    // 非滚动容器（body）的条目被清除
    let body = find_by_id(&page, "page");
    page.scroll_offsets.insert(node, max / 2.0);
    page.scroll_offsets.insert(body, 12.0);
    let root2 = lay(&page, 400.0);
    pipeline::clamp_scroll_offsets(&root2, &mut page.scroll_offsets);
    assert_eq!(page.scroll_offsets.len(), 1, "非滚动容器条目清除");
    assert!(
        (page.scroll_offsets[&node] - max / 2.0).abs() < 1e-4,
        "重排后偏移保留"
    );
}

/// 双容器独立滚动：各自的 PushClip 与各自的偏移互不干扰。
#[test]
fn two_containers_scroll_independently() {
    let mut page = pipeline::load_page(
        "<body>\
         <div id=\"a\" style=\"overflow: auto; height: 80px\">\
         <p style=\"margin: 0\">a1</p><p style=\"margin: 0\">a2</p><p style=\"margin: 0\">a3</p>\
         <p style=\"margin: 0\">a4</p><p style=\"margin: 0\">a5</p><p style=\"margin: 0\">a6</p></div>\
         <div id=\"b\" style=\"overflow: auto; height: 80px\">\
         <p style=\"margin: 0\">b1</p><p style=\"margin: 0\">b2</p><p style=\"margin: 0\">b3</p>\
         <p style=\"margin: 0\">b4</p><p style=\"margin: 0\">b5</p><p style=\"margin: 0\">b6</p></div>\
         </body>",
        "body { margin: 0 }",
    );
    let root = lay(&page, 400.0);
    let areas = pipeline::collect_container_scroll_areas(&root, &page.scroll_offsets);
    assert_eq!(areas.len(), 2, "两个独立滚动容器");
    let (node_a, node_b) = (find_by_id(&page, "a"), find_by_id(&page, "b"));
    assert_eq!(areas[0].node, node_a, "外→内排列 = 文档序");
    assert_eq!(areas[1].node, node_b);

    // 偏移空 = 基线场景；随后双容器各自设偏移
    let scene0 = pipeline::build_scene(&page, &root);
    page.scroll_offsets.insert(node_a, 20.0);
    page.scroll_offsets.insert(node_b, 5.0);
    let scene1 = pipeline::build_scene(&page, &root);

    let b0 = baselines(&scene0);
    let b1 = baselines(&scene1);
    assert_eq!(b0.len(), b1.len());
    // 绘制顺序 = 文档序：前一半是 a 的文本行（含被裁剪出界的行——指令照发、
    // 由 PushClip 裁掉），后一半是 b 的；各自按自己的偏移平移
    assert_eq!(b0.len() % 2, 0);
    let half = b0.len() / 2;
    for (index, (before, after)) in b0.iter().zip(&b1).enumerate() {
        let expect = if index < half { 20.0 } else { 5.0 };
        let delta = before - after;
        assert!(
            (delta - expect).abs() < 0.01,
            "容器 {} 文本平移 {}: {} → {}（期望 {}）",
            if index < half { "a" } else { "b" },
            expect,
            before,
            after,
            expect
        );
    }
}

/// 滚轮链：由内向外排列、最内未到边界先消费、到边界冒泡祖先、全链到边界
/// 回落视口（None）；容器外的点链为空。
#[test]
fn wheel_chain_walks_innermost_first_and_bubbles_at_boundary() {
    let mut page = pipeline::load_page(
        "<body id=\"page\"><div id=\"outer\" style=\"overflow: auto; height: 120px\">\
         <p style=\"margin: 0\">o1</p>\
         <div id=\"inner\" style=\"overflow: auto; height: 60px\">\
         <p style=\"margin: 0\">i1</p><p style=\"margin: 0\">i2</p>\
         <p style=\"margin: 0\">i3</p><p style=\"margin: 0\">i4</p></div>\
         <p style=\"margin: 0\">o2</p><p style=\"margin: 0\">o3</p><p style=\"margin: 0\">o4</p>\
         <p style=\"margin: 0\">o5</p><p style=\"margin: 0\">o6</p><p style=\"margin: 0\">o7</p>\
         <p style=\"margin: 0\">o8</p></div></body>",
        "body { margin: 0 }",
    );
    let root = lay(&page, 400.0);
    let inner_node = find_by_id(&page, "inner");
    let outer_node = find_by_id(&page, "outer");

    let areas = pipeline::collect_container_scroll_areas(&root, &page.scroll_offsets);
    assert_eq!(areas.len(), 2);
    assert_eq!(areas[0].node, outer_node, "外层先收集");
    assert_eq!(areas[1].node, inner_node, "内层后收集");
    let inner = areas[1];
    assert!(inner.max_scroll > 0.0 && areas[0].max_scroll > 0.0);

    // 内层 padding box 中心：链 = [inner, outer]（由内向外）
    let cx = inner.padding_box.x + inner.padding_box.width / 2.0;
    let cy = inner.padding_box.y + inner.padding_box.height / 2.0;
    let chain = pipeline::scroll_chain_at(&root, cx, cy, &page.scroll_offsets);
    assert_eq!(
        chain.iter().map(|entry| entry.0).collect::<Vec<_>>(),
        vec![inner_node, outer_node],
        "链由内向外"
    );

    // 下滚：最内未到边界 → 消费并停（整步消费、夹到边界：dy 超过内层
    // 剩余空间时停在内层 max，剩余量不传祖先——滚动锁存）
    let (node, offset) =
        scrollbar::wheel_chain_target(chain.clone(), 40.0).expect("inner consumes");
    assert_eq!(node, inner_node);
    assert!(
        (offset - inner.max_scroll).abs() < 1e-4,
        "消费整步夹到内层边界：{offset} vs max {}",
        inner.max_scroll
    );

    // 上滚（两容器都在顶）：全链到边界 → None → 视口滚动
    assert_eq!(
        scrollbar::wheel_chain_target(chain, -5.0),
        None,
        "全链到顶回落视口"
    );

    // 内层滚到底后再下滚：冒泡外层
    page.scroll_offsets.insert(inner_node, inner.max_scroll);
    let chain = pipeline::scroll_chain_at(&root, cx, cy, &page.scroll_offsets);
    let (node, offset) = scrollbar::wheel_chain_target(chain, 10.0).expect("outer consumes");
    assert_eq!(node, outer_node, "内层到底冒泡外层");
    assert!((offset - 10.0).abs() < 1e-4);

    // 容器外（页面区域）：链为空 → 视口消费
    let chain = pipeline::scroll_chain_at(&root, 5.0, 400.0, &page.scroll_offsets);
    assert!(chain.is_empty(), "容器外无链");
}

/// 容器滚动条几何与拖拽（K6）：轨道贴 padding box 右缘、thumb 拖拽端点
/// 对齐、轨道点击翻页；Scrollbar 状态机原样复用。
#[test]
fn container_scrollbar_geometry_and_drag() {
    let page = pipeline::load_page(
        "<body><div id=\"box\" style=\"overflow: auto; height: 200px\">\
         <p style=\"margin: 0\">1</p><p style=\"margin: 0\">2</p><p style=\"margin: 0\">3</p>\
         <p style=\"margin: 0\">4</p><p style=\"margin: 0\">5</p><p style=\"margin: 0\">6</p>\
         <p style=\"margin: 0\">7</p><p style=\"margin: 0\">8</p><p style=\"margin: 0\">9</p>\
         <p style=\"margin: 0\">10</p><p style=\"margin: 0\">11</p><p style=\"margin: 0\">12</p>\
         </div></body>",
        "body { margin: 0 }",
    );
    let root = lay(&page, 400.0);
    let areas = pipeline::collect_container_scroll_areas(&root, &page.scroll_offsets);
    assert_eq!(areas.len(), 1);
    let area = areas[0];
    assert!((area.padding_box.width - 400.0).abs() < 0.01);
    assert!((area.padding_box.height - 200.0).abs() < 0.01);

    let geometry = ScrollGeometry::for_container(
        area.padding_box.width,
        area.padding_box.height,
        area.max_scroll,
    );
    let track = geometry.track().expect("溢出容器有轨道");
    assert_eq!(track.width, scrollbar::TRACK_WIDTH);
    assert!((track.x - (area.padding_box.width - scrollbar::TRACK_WIDTH)).abs() < 0.01);
    let thumb = geometry.thumb(0.0).expect("thumb");
    assert!((thumb.x - track.x).abs() < 0.01);
    assert!(thumb.height >= scrollbar::MIN_THUMB - 1e-4);

    // 拖拽：thumb 中部按下不跳变，拖到轨道底 = max，拖出顶部 = 0
    let tx = track.x + track.width / 2.0;
    let mut sb = Scrollbar::new();
    let grab = thumb.y + thumb.height / 2.0;
    assert_eq!(sb.on_press(&geometry, 0.0, tx, grab), None, "按下不改滚动");
    assert!(sb.is_dragging());
    let bottom = sb
        .on_drag(&geometry, tx, track.y + track.height)
        .expect("drag");
    assert!(
        (bottom - area.max_scroll).abs() < 0.01,
        "拖到底 = max_scroll"
    );
    let top = sb.on_drag(&geometry, tx, 0.0).expect("drag");
    assert!(top.abs() < 0.01, "拖出顶 = 0");
    sb.on_release();
    assert_eq!(sb.on_drag(&geometry, tx, 100.0), None, "释放后失效");

    // 轨道点击翻页 = client 高，但 scroll=0 时步长受 max 夹取
    assert_eq!(
        Scrollbar::new().on_press(&geometry, 0.0, tx, track.y + track.height - 1.0),
        Some(area.max_scroll),
        "向下翻页 = min(page_step, max_scroll)"
    );
}
