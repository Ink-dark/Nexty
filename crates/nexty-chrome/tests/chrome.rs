//! chrome 层集成测试：端到端管线与渲染隔离线程。
//!
//! 窗口/GPU 胶水（app 模块）无法无头测试，不在此覆盖。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nexty_paint::{Color, Command, Pixmap, RasterError, Rasterizer, Scene, Size};
use nexty_text::ParleyTextShaper;

use nexty_chrome::pipeline::{self, Viewport};
use nexty_chrome::render::{RenderError, RenderThread};
use nexty_chrome::resources::{fetch_document, normalize_address_bar_input};
use nexty_network::CachedFetcher;

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.01
}

#[test]
fn end_to_end_page_renders_background_and_border() {
    let page = pipeline::load_page(
        "<body><div id=\"box\">hello</div></body>",
        "body { background-color: #ffffff } #box { background-color: rgb(0 128 255); border: 3px solid red; height: 50px }",
    );
    let shaper = ParleyTextShaper::new();
    let root = pipeline::layout_page(&page, &shaper, 400.0).expect("root");
    let scene = pipeline::build_scene(&page, &root);

    // body 白底按 CSS 2.2 §14.2 传播到画布（搬移语义：场景中不再发 body
    // 白底，由呈现层把 canvas_background_commands 插到场景底部）
    let canvas = pipeline::canvas_background_commands(&page, 400.0, 300.0);
    assert!(
        matches!(
            &canvas[..],
            [Command::FillRect { color, .. }] if *color == Color::opaque(255, 255, 255)
        ),
        "body 白底传播为画布白色底指令"
    );

    // 显示列表应包含：box 蓝底、四条红边、一条文本
    let fills: Vec<_> = scene
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::FillRect { rect, color } => Some((*rect, *color)),
            _ => None,
        })
        .collect();
    let texts: Vec<_> = scene
        .commands
        .iter()
        .filter(|command| matches!(command, Command::DrawText { .. }))
        .collect();

    let has_blue = fills
        .iter()
        .any(|(_, color)| *color == Color::opaque(0, 128, 255));
    let red_strips = fills
        .iter()
        .filter(|(_, color)| *color == Color::opaque(255, 0, 0))
        .count();
    assert!(has_blue, "box 蓝底");
    assert_eq!(red_strips, 4, "四条红边");
    assert_eq!(texts.len(), 1, "hello 一个文本 run");

    // 光栅化后像素验证
    let rasterizer = nexty_paint::VelloCpuRasterizer::new();
    let pixmap = rasterizer
        .rasterize(
            &scene,
            Viewport {
                width: 400,
                height: 300,
            },
        )
        .expect("rasterize");
    // box 的位置由布局决定：找蓝底填充带的 y，在其中心取像素
    let (blue_rect, _) = fills
        .iter()
        .find(|(_, color)| *color == Color::opaque(0, 128, 255))
        .expect("blue rect");
    let sample_x = (blue_rect.x + blue_rect.width / 2.0) as u32;
    let sample_y = (blue_rect.y + blue_rect.height / 2.0) as u32;
    assert_eq!(
        pixmap.pixel(sample_x, sample_y),
        Some([0, 128, 255, 255]),
        "box 中心是蓝底（边框内）"
    );
    // 边框线上是红色
    let border_y = (blue_rect.y + 1.0) as u32;
    assert_eq!(pixmap.pixel(sample_x, border_y), Some([255, 0, 0, 255]));
    // 文字有非透明像素
    let text_pixels = (0..400u32)
        .flat_map(|x| (0..300u32).map(move |y| (x, y)))
        .filter(|(x, y)| {
            pixmap.pixel(*x, *y).is_some_and(|p| {
                p[3] > 0
                    && !(p == [255, 255, 255, 255]
                        || p == [0, 128, 255, 255]
                        || p == [255, 0, 0, 255])
            })
        })
        .count();
    assert!(text_pixels > 0, "文本产生可见像素");
}

#[test]
fn anonymous_fragments_draw_no_background_or_border() {
    let page = pipeline::load_page(
        "<body>text <b>bold</b><div style=\"height: 4px\"></div>more text</body>",
        "",
    );
    let shaper = ParleyTextShaper::new();
    let root = pipeline::layout_page(&page, &shaper, 400.0).expect("root");
    let scene = pipeline::build_scene(&page, &root);

    // 除 body 背景（若 UA 给了）外，不允许出现匿名背景/边框；
    // 匿名片段的指令只可能是 DrawText
    let text_count = scene
        .commands
        .iter()
        .filter(|command| matches!(command, Command::DrawText { .. }))
        .count();
    assert!(text_count >= 2, "两段文本各自成行");
}

/// T0 回归：折行段落的每行 baseline 必须按行高递增（此前所有行画在同一
/// baseline 上，第二行起与首行重叠）。
#[test]
fn multiline_baselines_advance_by_line_height() {
    let text = "aaa bbb ccc ddd eee fff ggg hhh iii jjj kkk lll mmm nnn ooo ppp qqq rrr sss ttt";
    let page = pipeline::load_page(&format!("<p>{text}</p>"), "");
    let shaper = ParleyTextShaper::new();
    let root = pipeline::layout_page(&page, &shaper, 300.0).expect("root");
    let scene = pipeline::build_scene(&page, &root);

    let mut baselines: Vec<f32> = scene
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::DrawText { baseline, .. } => Some(*baseline),
            _ => None,
        })
        .collect();
    baselines.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    assert!(baselines.len() >= 2, "该文本在 300px 视口下应折成多行");
    for pair in baselines.windows(2) {
        assert!(
            pair[1] > pair[0],
            "相邻两行 baseline 相同（{pair:?}）→ 行重叠"
        );
    }
}

/// T8：行内原子盒（inline-block）的内容进入显示列表——背景与内部文本。
#[test]
fn inline_block_content_is_emitted_into_scene() {
    let page = pipeline::load_page(
        "<body>out <span style=\"display: inline-block; width: 80px; height: 30px; background-color: rgb(0 200 0)\">in</span></body>",
        "",
    );
    let shaper = ParleyTextShaper::new();
    let root = pipeline::layout_page(&page, &shaper, 400.0).expect("root");
    let scene = pipeline::build_scene(&page, &root);

    let green: Vec<_> = scene
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::FillRect { rect, color } => Some((*rect, *color)),
            _ => None,
        })
        .filter(|(_, color)| *color == Color::opaque(0, 200, 0))
        .collect();
    assert_eq!(green.len(), 1, "inline-block 背景下发");
    let (rect, _) = green[0];
    assert!(approx(rect.width, 80.0));
    assert!(approx(rect.height, 30.0));
    // 原子盒内部文本也在场景里：外层 out 与盒内 in 各一段 run
    let text_count = scene
        .commands
        .iter()
        .filter(|command| matches!(command, Command::DrawText { .. }))
        .count();
    assert_eq!(text_count, 2, "out 与 in 分别下发");
}

/// T4：块级链接命中 → href 原文 → 相对地址归一；空白区不产生链接。
#[test]
fn hit_test_finds_block_link_and_resolves_href() {
    let page = pipeline::load_page(
        "<body><a href=\"/target\" style=\"display: block; height: 20px\">block link</a></body>",
        "body { margin: 0; height: 50px }",
    );
    let shaper = ParleyTextShaper::new();
    let root = pipeline::layout_page(&page, &shaper, 200.0).expect("root");

    let node = pipeline::hit_test(&root, 10.0, 5.0).expect("命中链接");
    assert_eq!(
        pipeline::link_target(&page, node).as_deref(),
        Some("/target")
    );
    let resolved = nexty_network::resolve("https://origin.test/dir/page", "/target");
    assert_eq!(resolved.as_deref(), Ok("https://origin.test/target"));

    // a（高 20）之外、body（高 50）之内：命中 body，无链接
    let node = pipeline::hit_test(&root, 10.0, 30.0).expect("命中 body");
    assert!(pipeline::link_target(&page, node).is_none());
}

/// T4：行内链接文本 run 命中（坐标取自 run 字形范围）。
#[test]
fn hit_test_finds_inline_link_text_run() {
    let page = pipeline::load_page(
        "<body><p style=\"margin: 0\"><a href=\"https://a.test/x\">linktext</a></p></body>",
        "body { margin: 0 }",
    );
    let shaper = ParleyTextShaper::new();
    let root = pipeline::layout_page(&page, &shaper, 200.0).expect("root");
    let body = &root.children[0];
    let p = &body.children[0];
    let run = &p.lines[0].runs[0];
    let first = run.glyphs[0];
    let last = run.glyphs.last().expect("glyphs");
    let mid = (first.x + last.x + last.advance) / 2.0;

    let node = pipeline::hit_test(p, mid, 2.0).expect("命中行内链接");
    assert_eq!(
        pipeline::link_target(&page, node).as_deref(),
        Some("https://a.test/x")
    );
    // p 盒内、run 之外（右侧空白）：命中 p 本身，无链接
    let node = pipeline::hit_test(p, p.border_box.width - 1.0, 2.0).expect("命中 p");
    assert!(pipeline::link_target(&page, node).is_none());
}

/// T4：链接内的图片命中后沿祖先链找到 `<a href>`。
#[test]
fn hit_test_finds_image_inside_anchor() {
    let page = pipeline::load_page(
        "<body><a href=\"logo\" style=\"display: block\"><img width=40 height=30 src=a.png></a></body>",
        "body { margin: 0 }",
    );
    let shaper = ParleyTextShaper::new();
    let root = pipeline::layout_page(&page, &shaper, 200.0).expect("root");
    let body = &root.children[0];
    let anchor = &body.children[0];

    let node = pipeline::hit_test(anchor, 10.0, 10.0).expect("命中图片");
    assert_eq!(pipeline::link_target(&page, node).as_deref(), Some("logo"));
    // 图片矩形之外（a 盒内、图片下方）→ a 自身，仍有链接
    let node = pipeline::hit_test(anchor, 10.0, anchor.border_box.height - 1.0).expect("命中 a");
    assert_eq!(pipeline::link_target(&page, node).as_deref(), Some("logo"));
}

/// T4：`<a>` 缺 href 不产生链接目标；盒外命中测试返回 None。
#[test]
fn link_target_requires_href_and_miss_returns_none() {
    let page = pipeline::load_page(
        "<body><a style=\"display: block; height: 20px\">no href</a></body>",
        "body { margin: 0 }",
    );
    let shaper = ParleyTextShaper::new();
    let root = pipeline::layout_page(&page, &shaper, 200.0).expect("root");
    let node = pipeline::hit_test(&root, 5.0, 5.0).expect("命中 a");
    assert!(pipeline::link_target(&page, node).is_none());
    // body 之外（视口下方远处）
    assert!(pipeline::hit_test(&root, 5.0, 300.0).is_none());
}

/// T4：悬停光标语义——链接（文本/块级/图片）→ 手型，正文文本 → I 形，空白 → 默认。
#[test]
fn hover_kind_follows_links_text_and_blank() {
    let page = pipeline::load_page(
        "<body style=\"margin: 0\">\
         <p style=\"margin: 0\"><a href=\"/x\">linktext</a></p>\
         <p style=\"margin: 0\">plaintext</p>\
         </body>",
        "",
    );
    let shaper = ParleyTextShaper::new();
    let root = pipeline::layout_page(&page, &shaper, 300.0).expect("root");
    let body = &root.children[0];
    let link_p = &body.children[0];
    let plain_p = &body.children[1];

    // 链接文本 run 中点 → 手型
    let run = &link_p.lines[0].runs[0];
    let first = run.glyphs[0];
    let last = run.glyphs.last().expect("glyphs");
    let mid = (first.x + last.x + last.advance) / 2.0;
    let y = link_p.border_box.y + 2.0;
    assert_eq!(
        pipeline::hover_kind(&page, body, mid, y),
        pipeline::HoverKind::Pointer,
        "链接文本 → 手型"
    );

    // 正文文本 run 中点 → I 形
    let run = &plain_p.lines[0].runs[0];
    let first = run.glyphs[0];
    let last = run.glyphs.last().expect("glyphs");
    let mid = (first.x + last.x + last.advance) / 2.0;
    let y = plain_p.border_box.y + 2.0;
    assert_eq!(
        pipeline::hover_kind(&page, body, mid, y),
        pipeline::HoverKind::Text,
        "正文文本 → I 形"
    );

    // 盒内空白（文本右侧、行纵向之外）→ 默认；盒外 → 默认
    assert_eq!(
        pipeline::hover_kind(&page, body, plain_p.border_box.width - 1.0, y),
        pipeline::HoverKind::Default,
        "盒内非文本区 → 默认"
    );
    assert_eq!(
        pipeline::hover_kind(&page, body, 10.0, -50.0),
        pipeline::HoverKind::Default,
        "盒外 → 默认"
    );
}

/// T4：块级链接整个盒 → 手型；链接内图片 → 手型，且命中类型区分为图片。
#[test]
fn hover_kind_covers_block_link_and_image() {
    let page = pipeline::load_page(
        "<body><a href=\"t\" style=\"display: block; height: 20px\">block</a>\
         <a href=\"logo\" style=\"display: block\"><img width=40 height=30 src=a.png></a></body>",
        "body { margin: 0 }",
    );
    let shaper = ParleyTextShaper::new();
    let root = pipeline::layout_page(&page, &shaper, 200.0).expect("root");
    let body = &root.children[0];
    let block_link = &body.children[0];
    let image_link = &body.children[1];

    // 块级链接盒内（文本之下）仍是链接
    assert_eq!(
        pipeline::hover_kind(
            &page,
            body,
            10.0,
            block_link.border_box.y + block_link.border_box.height - 2.0
        ),
        pipeline::HoverKind::Pointer,
        "块级链接 → 手型"
    );

    // 链接内图片：命中类型为 Image，光标仍是手型
    let hit = pipeline::hit_test_ex(body, 10.0, image_link.border_box.y + 10.0).expect("命中图片");
    assert_eq!(hit.kind, pipeline::HitKind::Image);
    assert_eq!(
        pipeline::hover_kind(&page, body, 10.0, image_link.border_box.y + 10.0),
        pipeline::HoverKind::Pointer
    );
}

/// T4：`hit_test` 仍是 `hit_test_ex` 的薄封装（节点一致）。
#[test]
fn hit_test_wrapper_matches_hit_test_ex_node() {
    let page = pipeline::load_page(
        "<body><p style=\"margin: 0\">hello</p></body>",
        "body { margin: 0 }",
    );
    let shaper = ParleyTextShaper::new();
    let root = pipeline::layout_page(&page, &shaper, 200.0).expect("root");
    let node = pipeline::hit_test(&root, 5.0, 5.0).expect("命中");
    let hit = pipeline::hit_test_ex(&root, 5.0, 5.0).expect("命中");
    assert_eq!(Some(node), Some(hit.node));
    assert_eq!(hit.kind, pipeline::HitKind::TextRun, "命中的是文本 run");
}

/// T5：`<title>` 文本提取；空白/缺失/非 HTML 命名空间返回 None。
#[test]
fn document_title_extraction() {
    let page = pipeline::load_page(
        "<html><head><title>文档标题</title></head><body></body></html>",
        "",
    );
    assert_eq!(
        pipeline::document_title(&page.document).as_deref(),
        Some("文档标题")
    );

    // 空白 title 视为缺失
    let page = pipeline::load_page("<head><title>   </title></head><body></body>", "");
    assert!(pipeline::document_title(&page.document).is_none());

    // 无 title
    let page = pipeline::load_page("<body></body>", "");
    assert!(pipeline::document_title(&page.document).is_none());
}

#[test]
fn render_thread_round_trips() {
    let thread = RenderThread::spawn(
        Arc::new(nexty_paint::VelloCpuRasterizer::new()),
        Arc::new(|| {}),
    );
    let scene = Scene {
        commands: vec![Command::FillRect {
            rect: nexty_paint::Rect {
                x: 0.0,
                y: 0.0,
                width: 4.0,
                height: 4.0,
            },
            color: Color::opaque(255, 0, 0),
        }],
        images: Vec::new(),
    };
    let pixmap = thread
        .render(
            scene,
            Size {
                width: 8,
                height: 8,
            },
        )
        .expect("render");
    assert_eq!(pixmap.pixel(0, 0), Some([255, 0, 0, 255]));
    assert_eq!(pixmap.pixel(7, 7), Some([0, 0, 0, 0]));
}

/// 首次调用 panic 的 Rasterizer（验证兜底后线程仍可服务）。
struct PanicsOnce {
    calls: AtomicUsize,
}

impl Rasterizer for PanicsOnce {
    fn rasterize(&self, _scene: &Scene, _size: Size) -> Result<Pixmap, RasterError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            panic!("injected rasterizer failure");
        }
        Ok(Pixmap {
            size: Size {
                width: 1,
                height: 1,
            },
            data: vec![1, 2, 3, 4],
        })
    }
}

#[test]
fn render_thread_survives_panic_and_keeps_serving() {
    let thread = RenderThread::spawn(
        Arc::new(PanicsOnce {
            calls: AtomicUsize::new(0),
        }),
        Arc::new(|| {}),
    );
    let scene = Scene::default();
    let size = Size {
        width: 4,
        height: 4,
    };

    let error = thread
        .render(scene.clone(), size)
        .expect_err("first call panics");
    match error {
        RenderError::Panicked(message) => assert!(message.contains("injected")),
        RenderError::Raster(_) => panic!("expected panic error"),
    }

    // 兜底后线程存活：第二次调用正常返回
    let pixmap = thread.render(scene, size).expect("second call succeeds");
    assert_eq!(pixmap.data, vec![1, 2, 3, 4]);
}

#[test]
fn render_thread_reports_raster_errors() {
    struct AlwaysEmpty;
    impl Rasterizer for AlwaysEmpty {
        fn rasterize(&self, _scene: &Scene, _size: Size) -> Result<Pixmap, RasterError> {
            Err(RasterError::EmptySize)
        }
    }
    let thread = RenderThread::spawn(Arc::new(AlwaysEmpty), Arc::new(|| {}));
    let error = thread
        .render(
            Scene::default(),
            Size {
                width: 0,
                height: 0,
            },
        )
        .expect_err("raster error");
    assert_eq!(error, RenderError::Raster(RasterError::EmptySize));
}

#[test]
fn viewport_alias_and_helpers() {
    // Viewport 是 paint::Size 的别名，尺寸直通
    let viewport = Viewport {
        width: 10,
        height: 20,
    };
    assert!(approx(viewport.width as f32, 10.0));
}

// ---- T5：子资源抓取与页面重组（无头端到端） ----

use nexty_chrome::resources::{self, Subresources};
use nexty_dom::Document;
use nexty_html::ParseOptions;
use nexty_network::{NetworkError, NetworkFetcher, Request, Response};
use std::collections::BTreeMap;
use std::sync::Mutex;

/// 内存 mock fetcher：URL → body / 状态码；未登记的 URL 返回 404。
/// K3 T5 起支持按 URL 的响应头与 final_url 覆盖（charset / 缓存头 /
/// 重定向落点模拟），并记录抓取计数供缓存命中断言。
struct MockFetcher {
    routes: Mutex<BTreeMap<String, Result<Vec<u8>, u16>>>,
    /// 按 URL 的响应头（未登记 = 空头表）。
    headers: Mutex<BTreeMap<String, Vec<(String, String)>>>,
    /// 按 URL 的 final_url 覆盖（重定向落点；未登记 = 请求 URL）。
    final_urls: Mutex<BTreeMap<String, String>>,
    /// 抓取计数（共享句柄：CachedFetcher 包住 mock 后仍可读内部计数）。
    hits: Arc<Mutex<Vec<String>>>,
}

impl MockFetcher {
    fn new() -> Self {
        Self {
            routes: Mutex::new(BTreeMap::new()),
            headers: Mutex::new(BTreeMap::new()),
            final_urls: Mutex::new(BTreeMap::new()),
            hits: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 抓取计数句柄：mock 被搬进 CachedFetcher 后从外部读计数用。
    fn hits(&self) -> Arc<Mutex<Vec<String>>> {
        self.hits.clone()
    }

    fn serve(&self, url: &str, body: &[u8]) {
        self.routes
            .lock()
            .unwrap()
            .insert(url.to_owned(), Ok(body.to_vec()));
    }

    fn fail(&self, url: &str, status: u16) {
        self.routes
            .lock()
            .unwrap()
            .insert(url.to_owned(), Err(status));
    }

    /// 带响应头的登记（charset / cache-control 等用例）。
    fn serve_with_headers(&self, url: &str, headers: Vec<(String, String)>, body: &[u8]) {
        self.serve(url, body);
        self.headers.lock().unwrap().insert(url.to_owned(), headers);
    }

    /// 模拟重定向落点：响应的 final_url 与请求 URL 不同。
    fn serve_final_url(&self, url: &str, final_url: &str, body: &[u8]) {
        self.serve(url, body);
        self.final_urls
            .lock()
            .unwrap()
            .insert(url.to_owned(), final_url.to_owned());
    }
}

impl NetworkFetcher for MockFetcher {
    fn fetch(&self, request: &Request) -> Result<Response, NetworkError> {
        self.hits.lock().unwrap().push(request.url.clone());
        let routes = self.routes.lock().unwrap();
        let final_url = self
            .final_urls
            .lock()
            .unwrap()
            .get(&request.url)
            .cloned()
            .unwrap_or_else(|| request.url.clone());
        match routes.get(&request.url) {
            Some(Ok(body)) => Ok(Response {
                status: 200,
                headers: self
                    .headers
                    .lock()
                    .unwrap()
                    .get(&request.url)
                    .cloned()
                    .unwrap_or_default(),
                body: body.clone(),
                final_url,
            }),
            Some(Err(status)) => Ok(Response {
                status: *status,
                headers: Vec::new(),
                body: Vec::new(),
                final_url,
            }),
            None => Ok(Response {
                status: 404,
                headers: Vec::new(),
                body: Vec::new(),
                final_url,
            }),
        }
    }
}

/// 1×1 蓝色 PNG。
fn tiny_png() -> Vec<u8> {
    let mut buffer = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        1,
        1,
        image::Rgba([0, 0, 255, 255]),
    ))
    .write_to(
        &mut std::io::Cursor::new(&mut buffer),
        image::ImageFormat::Png,
    )
    .expect("encode png");
    buffer
}

/// 加载入口（与 app.rs 后台线程相同的编排序列）。
fn load_page_with_resources(
    fetcher: &MockFetcher,
    base_url: &str,
    html: &str,
) -> (Document, Subresources) {
    let document = nexty_html::parse_document(html, ParseOptions::default());
    let resources = resources::fetch_subresources(fetcher, base_url, &document);
    (document, resources)
}

/// M1 零拷贝：同一 URL 的多个 `<img>` 节点共享同一份像素（解码一次，
/// `Arc` 引用计数），页面池内两两 `Arc::ptr_eq`。
#[test]
fn same_url_images_share_one_pixel_buffer() {
    let fetcher = MockFetcher::new();
    fetcher.serve("https://page.test/dot.png", &tiny_png());
    let html = "<body><img src=dot.png><p>x</p><img src=dot.png></body>";
    let (_, sub) = load_page_with_resources(&fetcher, "https://page.test/index.html", html);
    assert_eq!(sub.images.len(), 2, "两个节点都有像素");
    let [a, b] = sub
        .images
        .values()
        .collect::<Vec<_>>()
        .try_into()
        .expect("two entries");
    assert!(Arc::ptr_eq(a, b), "同 URL 的两个节点共享一份像素");
}

/// 含外链 CSS + 图片的页面：级联按源顺序，图片按 DrawImage 下发。
#[test]
fn subresources_load_and_scene_carries_external_css_and_image() {
    let fetcher = MockFetcher::new();
    // 外链表覆盖 inline 表的同名属性：后者应胜出
    fetcher.serve(
        "https://page.test/ext.css",
        b"p { color: blue; border: 2px solid rgb(0 128 0) }",
    );
    fetcher.serve("https://page.test/logo.png", &tiny_png());
    let html = "<head><style>p { color: red }</style>\
                <link rel=stylesheet href=ext.css></head>\
                <body><p>hi</p><img src=logo.png width=10 height=10></body>";

    let (_, sub) = load_page_with_resources(&fetcher, "https://page.test/index.html", html);
    assert_eq!(sub.stylesheets.len(), 2, "inline + 外链按源顺序");
    assert_eq!(sub.images.len(), 1, "图片已解码");

    // 组装页面 → 布局 → Scene
    let document = nexty_html::parse_document(html, ParseOptions::default());
    let page = pipeline::build_page(document, sub.stylesheets, sub.images);
    let shaper = ParleyTextShaper::new();
    let root = pipeline::layout_page(&page, &shaper, 400.0).expect("root");
    let scene = pipeline::build_scene(&page, &root);

    // Scene 指令序列：图片指令 + 图片池 + 文本
    let images_drawn: Vec<_> = scene
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::DrawImage { rect, image } => Some((*rect, *image)),
            _ => None,
        })
        .collect();
    assert_eq!(images_drawn.len(), 1, "一张图片一条 DrawImage");
    let (rect, index) = images_drawn[0];
    assert_eq!(scene.images.len(), 1, "图片池一份像素");
    assert_eq!(index, 0);
    assert!(approx(rect.width, 10.0) && approx(rect.height, 10.0));
    // M1 零拷贝：场景图片池与页面图片池是同一份像素（Arc 引用计数共享，
    // 构建路径无深拷贝）
    let page_image = page.images.values().next().expect("page has image");
    assert!(
        Arc::ptr_eq(page_image, &scene.images[0]),
        "scene 与 page 的图片必须是同一份像素"
    );
    // 图片落点在地址栏下方内容区（y > 0）且在文本之后
    assert!(rect.y > 0.0);
    assert!(
        scene
            .commands
            .iter()
            .any(|command| matches!(command, Command::DrawText { .. })),
        "文本指令仍在"
    );

    // 级联顺序：外链表的 blue 压过 inline 的 red —— 光栅化像素验证
    let rasterizer = nexty_paint::VelloCpuRasterizer::new();
    let pixmap = rasterizer
        .rasterize(
            &scene,
            Viewport {
                width: 400,
                height: 300,
            },
        )
        .expect("rasterize");
    let pixel = pixmap
        .pixel(rect.x as u32 + 5, rect.y as u32 + 5)
        .expect("像素在画布内");
    // vello_cpu 预乘/反预乘往返有 ±1 量化误差
    assert!(
        pixel[0] <= 1 && pixel[1] <= 1 && pixel[2] >= 254 && pixel[3] == 255,
        "1×1 PNG 的蓝色落在目标矩形内，实际 {pixel:?}"
    );
}

/// 单个子资源失败不影响其余渲染（降级而非白屏）。
#[test]
fn failing_subresources_degrade_without_blocking_render() {
    let fetcher = MockFetcher::new();
    fetcher.fail("https://page.test/broken.css", 500);
    fetcher.fail("https://page.test/lost.png", 404);
    fetcher.serve("https://page.test/junk.png", b"not an image");
    let html = "<head><link rel=stylesheet href=broken.css></head>\
                <body><p>still here</p><img src=lost.png><img src=junk.png></body>";

    let (_, sub) = load_page_with_resources(&fetcher, "https://page.test/index.html", html);
    assert!(sub.stylesheets.is_empty(), "失败的外链表不参与级联");
    assert!(sub.images.is_empty(), "失败图片无像素");

    let document = nexty_html::parse_document(html, ParseOptions::default());
    let page = pipeline::build_page(document, sub.stylesheets, sub.images);
    let shaper = ParleyTextShaper::new();
    let root = pipeline::layout_page(&page, &shaper, 400.0).expect("root");
    let scene = pipeline::build_scene(&page, &root);

    // 文本照常渲染；无图片指令（失败图片不产生 DrawImage）
    assert!(
        scene
            .commands
            .iter()
            .any(|command| matches!(command, Command::DrawText { .. })),
        "文本内容不受子资源失败影响"
    );
    assert!(
        !scene
            .commands
            .iter()
            .any(|command| matches!(command, Command::DrawImage { .. })),
        "无像素的图片不下发指令"
    );
}

/// 抓取不到自然尺寸的图片：布局回退默认对象尺寸，失败图片无指令。
#[test]
fn image_natural_size_flows_from_decoding_into_layout() {
    // 3×2 蓝色 PNG：无 width/height 属性时按自然尺寸
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        3,
        2,
        image::Rgba([10, 20, 30, 255]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .expect("encode");
    let fetcher = MockFetcher::new();
    fetcher.serve("https://page.test/n.png", &png);
    let html = "<body><img src=n.png></body>";

    let (_, sub) = load_page_with_resources(&fetcher, "https://page.test/index.html", html);
    let document = nexty_html::parse_document(html, ParseOptions::default());
    let page = pipeline::build_page(document, sub.stylesheets, sub.images);
    let shaper = ParleyTextShaper::new();
    let root = pipeline::layout_page(&page, &shaper, 400.0).expect("root");
    let scene = pipeline::build_scene(&page, &root);

    let drawn: Vec<_> = scene
        .commands
        .iter()
        .filter_map(|command| match command {
            Command::DrawImage { rect, .. } => Some(*rect),
            _ => None,
        })
        .collect();
    assert_eq!(drawn.len(), 1);
    assert!(
        approx(drawn[0].width, 3.0) && approx(drawn[0].height, 2.0),
        "自然尺寸 3×2 从解码流入布局，实际 {:?}",
        (drawn[0].width, drawn[0].height)
    );
}

// ---- 不可信输入边界加固：全链回归（本轮验收旗舰用例） ----

use nexty_dom::MAX_TREE_DEPTH;
use nexty_dom::NodeKind;
use nexty_layout::Fragment;

/// CRC-32（IEEE，PNG chunk 校验用）。
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// 真实 1×1 PNG，IHDR 尺寸改写为 65535×65535（重算 CRC）——头部声明巨图、
/// 像素数据只有 1×1 的攻击样本。
fn declared_huge_png() -> Vec<u8> {
    let mut png = tiny_png();
    png[16..20].copy_from_slice(&65535u32.to_be_bytes());
    png[20..24].copy_from_slice(&65535u32.to_be_bytes());
    let crc = crc32(&png[12..29]);
    png[29..33].copy_from_slice(&crc.to_be_bytes());
    png
}

/// 片段树所有几何值必须有限（污染即在此可见）。
fn assert_geometry_finite(fragment: &Fragment) {
    let border_box = fragment.border_box;
    assert!(
        border_box.x.is_finite()
            && border_box.y.is_finite()
            && border_box.width.is_finite()
            && border_box.height.is_finite(),
        "片段几何有限，实际 {border_box:?}"
    );
    for line in &fragment.lines {
        assert!(line.baseline.is_finite() && line.height.is_finite());
        for run in &line.runs {
            assert!(
                run.glyphs
                    .iter()
                    .all(|glyph| glyph.x.is_finite() && glyph.advance.is_finite())
            );
        }
        for image in &line.images {
            assert!(
                image.x.is_finite()
                    && image.y.is_finite()
                    && image.width.is_finite()
                    && image.height.is_finite()
            );
        }
    }
    for child in &fragment.children {
        assert_geometry_finite(child);
    }
}

/// 组合攻击面（深嵌套 + 巨数属性 + 巨图声明 + 巨数 CSS）走完整管线：
/// parse → 子资源抓取（含解码预检）→ 级联 → 布局 → TextMap → 标题/资源清单
/// ——不崩、有限时间、几何有限。
///
/// 嵌套规模取上限 2 倍（1000 层）：html5ever 对每个起始标签都在 open
/// elements 栈上线性扫描（O(深度²)），更大语料在测试里不可行（实测
/// debug 1000 层 ≈ 0.8s、2000 层 ≈ 3s）；截断语义已由 nexty-html 单测钉住。
///
/// 栈预算（实测）：512 层布局递归（taffy 解算 + 片段装配的栈帧合计约
/// 10 KB/层）需 5–6 MB 栈。生产路径中布局只跑在主线程，Windows PE 主线程
/// 栈默认仅 1 MiB——曾直接栈溢出拖垮进程（实机验收抓到），已由
/// `nexty-chrome/build.rs` 提到 16 MiB。渲染隔离线程只光栅化、抓取线程
/// 只解析（均为迭代或浅递归）。测试线程默认栈只有 2 MB，故显式放到
/// 32 MB 线程执行——顺带把「布局递归的栈需求 ≤ 主线程预算」这一资源
/// 约束钉在本用例里。
#[test]
fn hardening_full_chain_survives_adversarial_page() {
    // panic 通过 join 传回测试线程，失败仍计入本用例。
    let result = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(hardening_full_chain_body)
        .expect("spawn test thread")
        .join();
    assert!(result.is_ok(), "全链用例在线程内失败：{result:?}");
}

fn hardening_full_chain_body() {
    let huge = "9".repeat(45);
    let mut html = String::from(
        "<html><head><title>加固回归</title>\
         <style>p { margin: 1e40px; font-size: 1e30em } img { width: 1e40px }</style></head>\
         <body>",
    );
    // 深嵌套部分：超上限，解析入口截断
    html.push_str(&"<div>".repeat(1_000));
    html.push_str(&"</div>".repeat(1_000));
    // 浅层部分：巨数尺寸属性 + 指向巨图声明的 <img>（在截断预算内存活）
    html.push_str(&format!(
        "<p>before <img width=\"{huge}\" height=\"{huge}\" src=huge.png> after</p></body></html>"
    ));

    let fetcher = MockFetcher::new();
    fetcher.serve("https://page.test/huge.png", &declared_huge_png());

    // parse → 子资源抓取（解码预检在 fetch 阶段拒绝巨图声明）
    let (document, sub) = load_page_with_resources(&fetcher, "https://page.test/index.html", &html);
    assert!(sub.images.is_empty(), "巨图声明预检被拒，无像素");

    // 深度截断：树中任意节点 ≤ MAX_TREE_DEPTH，深嵌套部分只保留预算内的层
    let mut max_depth = 0u32;
    let mut div_count = 0usize;
    let mut stack = vec![document.root()];
    while let Some(id) = stack.pop() {
        max_depth = max_depth.max(document.depth(id));
        if let Some(NodeKind::Element(data)) = document.node(id)
            && data.name == "div"
        {
            div_count += 1;
        }
        for child in document.children(id) {
            stack.push(child);
        }
    }
    assert!((max_depth as usize) <= MAX_TREE_DEPTH, "深度有界");
    assert_eq!(
        div_count,
        MAX_TREE_DEPTH - 2,
        "html/body 之外只保留预算内 div"
    );

    // 标题与资源清单在全链末端仍可用
    assert_eq!(
        pipeline::document_title(&document).as_deref(),
        Some("加固回归")
    );
    assert_eq!(nexty_html::collect_image_resources(&document).len(), 1);

    // 级联 → 布局 → TextMap：页面可布局且几何全部有限
    let page = pipeline::build_page(document, sub.stylesheets, sub.images);
    let shaper = ParleyTextShaper::new();
    let root = pipeline::layout_page(&page, &shaper, 400.0).expect("root");
    let text_map = nexty_layout::TextMap::new(&root);
    let _ = &text_map;
    assert_geometry_finite(&root);

    // 显示列表可构建：文本指令在（浅层段落的 before/after）
    let scene = pipeline::build_scene(&page, &root);
    assert!(
        scene
            .commands
            .iter()
            .any(|command| matches!(command, Command::DrawText { .. })),
        "文本照常下发"
    );
    assert!(
        !scene
            .commands
            .iter()
            .any(|command| matches!(command, Command::DrawImage { .. })),
        "巨图声明无像素、无 DrawImage"
    );
}

// ---- K1 T3：事件模型接线——生命周期派发（先派发后默认、各恰一次） ----

use nexty_chrome::pipeline::PendingLifecycle;
use nexty_dom::{DOM_CONTENT_LOADED, Event, EventListener, EventLoop, LOAD};
use std::cell::RefCell;
use std::rc::Rc;

type LifecycleLog = Rc<RefCell<Vec<&'static str>>>;

/// 生命周期计数钩子：把派发的事件类型记入日志。
fn lifecycle_recorder(log: &LifecycleLog, label: &'static str) -> Box<dyn EventListener> {
    let log = Rc::clone(log);
    Box::new(move |_event: &mut Event| log.borrow_mut().push(label))
}

/// ③ DOMContentLoaded 在解析后、load 在子资源 settle 后各恰一次：走与
/// app.rs 后台线程相同的编排序列（真实 parse → settle 回调信号 →
/// build_page → 信号 post → 排空），计数钩子挂在文档根上。
#[test]
fn lifecycle_events_fire_once_each_in_order_after_signals() {
    let fetcher = MockFetcher::new();
    fetcher.serve("https://page.test/ext.css", b"p { color: blue }");
    let html = "<head><link rel=stylesheet href=ext.css></head><body><p>hi</p></body>";

    // 信号捕获点与 app.rs load 的后台线程一致
    let document = nexty_html::parse_document(html, ParseOptions::default());
    let parsed = true; // parse_document 返回
    let mut settled = false;
    let sub = resources::fetch_subresources_with_settle(
        &fetcher,
        "https://page.test/index.html",
        &document,
        &mut || settled = true,
    );
    assert!(settled, "首轮编排 settle（成功路径）恰触发一次回调");

    let mut page = pipeline::build_page(document, sub.stylesheets, sub.images);
    let log: LifecycleLog = Rc::new(RefCell::new(Vec::new()));
    let root = page.document.root();
    page.document.add_listener(
        root,
        DOM_CONTENT_LOADED,
        false,
        lifecycle_recorder(&log, "dcl"),
    );
    page.document
        .add_listener(root, LOAD, false, lifecycle_recorder(&log, "load"));

    // 信号齐 → pipeline 单处钩子 post → EventLoop 排空派发
    let mut event_loop = EventLoop::new();
    page.post_lifecycle_tasks(
        &mut event_loop,
        PendingLifecycle {
            dom_content_loaded: parsed,
            load: settled,
        },
    );
    event_loop.run_pending_tasks(&mut page.document);
    assert_eq!(
        *log.borrow(),
        ["dcl", "load"],
        "DOMContentLoaded 先于 load，各恰一次"
    );

    // 队列已空：重复排空不重复派发（每页各恰一次）
    event_loop.run_pending_tasks(&mut page.document);
    assert_eq!(log.borrow().len(), 2, "排空后无重复派发");
}

/// ③（降级路径）：子资源全部失败/超时/超预算时 settle 回调仍恰触发一次，
/// load 照常派发（含失败/超时/预算降级的语义）。
#[test]
fn lifecycle_load_fires_after_settled_degraded_subresources() {
    let fetcher = MockFetcher::new();
    fetcher.fail("https://page.test/broken.css", 500);
    fetcher.fail("https://page.test/lost.png", 404);
    let html = "<head><link rel=stylesheet href=broken.css></head>\
                <body><p>hi</p><img src=lost.png></body>";

    let document = nexty_html::parse_document(html, ParseOptions::default());
    let mut settled = false;
    let sub = resources::fetch_subresources_with_settle(
        &fetcher,
        "https://page.test/index.html",
        &document,
        &mut || settled = true,
    );
    assert!(settled, "全失败路径 settle 回调仍触发");
    assert!(sub.stylesheets.is_empty() && sub.images.is_empty());

    let mut page = pipeline::build_page(document, sub.stylesheets, sub.images);
    let log: LifecycleLog = Rc::new(RefCell::new(Vec::new()));
    let root = page.document.root();
    page.document
        .add_listener(root, LOAD, false, lifecycle_recorder(&log, "load"));

    let mut event_loop = EventLoop::new();
    page.post_lifecycle_tasks(
        &mut event_loop,
        PendingLifecycle {
            dom_content_loaded: true,
            load: settled,
        },
    );
    event_loop.run_pending_tasks(&mut page.document);
    assert_eq!(*log.borrow(), ["load"], "降级 settle 后 load 恰一次");
}

/// ③（门控）：信号未到不派发；只到解析信号只派发 DOMContentLoaded——
/// K3/K4 异步 settle 的接缝。
#[test]
fn lifecycle_events_wait_for_their_signals() {
    let mut page = pipeline::load_page("<body>hi</body>", "");
    let log: LifecycleLog = Rc::new(RefCell::new(Vec::new()));
    let root = page.document.root();
    page.document.add_listener(
        root,
        DOM_CONTENT_LOADED,
        false,
        lifecycle_recorder(&log, "dcl"),
    );
    page.document
        .add_listener(root, LOAD, false, lifecycle_recorder(&log, "load"));

    let mut event_loop = EventLoop::new();
    page.post_lifecycle_tasks(&mut event_loop, PendingLifecycle::default());
    event_loop.run_pending_tasks(&mut page.document);
    assert!(log.borrow().is_empty(), "无信号不派发");

    page.post_lifecycle_tasks(
        &mut event_loop,
        PendingLifecycle {
            dom_content_loaded: true,
            load: false,
        },
    );
    event_loop.run_pending_tasks(&mut page.document);
    assert_eq!(*log.borrow(), ["dcl"], "只到解析信号 → 仅 DOMContentLoaded");
}

// ---- K3 T5：导航编排接线（主文档解码 / 缓存接入 / 重定向落点） ----

/// 主文档解码：Content-Type charset=gbk → 按 GBK 解码，标题正确——
/// 非 UTF-8 页面不再进错误页（错误页 = fetch_document 返回 Err）。
#[test]
fn gbk_document_with_content_type_charset_decodes_title() {
    let fetcher = MockFetcher::new();
    // 「中文」的 GBK 编码字节（0xD6 0xD0 0xCE 0xC4）
    let mut body = b"<html><head><title>".to_vec();
    body.extend_from_slice(&[0xD6, 0xD0, 0xCE, 0xC4]);
    body.extend_from_slice(b"</title></head><body></body></html>");
    fetcher.serve_with_headers(
        "https://page.test/gbk.html",
        vec![(
            "content-type".to_owned(),
            "text/html; charset=gbk".to_owned(),
        )],
        &body,
    );

    let fetched = fetch_document(&fetcher, "https://page.test/gbk.html")
        .expect("GBK 页面应成功返回而非错误页");
    let document = nexty_html::parse_document(&fetched.data.html, ParseOptions::default());
    assert_eq!(
        pipeline::document_title(&document).as_deref(),
        Some("中文"),
        "GBK 字节按 charset 解码为可读标题"
    );
}

/// 无 charset 提示的 GBK 字节：按默认 UTF-8 替换语义兑底，不 panic、
/// 不进错误页（Err）。非法序列退化为 U+FFFD 可见。
#[test]
fn gbk_bytes_without_charset_hint_degrade_without_panic() {
    let fetcher = MockFetcher::new();
    fetcher.serve("https://page.test/raw.html", &[0xD6, 0xD0, 0xCE, 0xC4]);

    let fetched =
        fetch_document(&fetcher, "https://page.test/raw.html").expect("解码兑底路径不失败");
    assert!(
        fetched.data.html.contains('\u{FFFD}'),
        "无提示的非法 UTF-8 序列替换为 U+FFFD"
    );
}

/// 缓存接入：CachedFetcher 包裹后同 URL 二次导航命中内存缓存——
/// mock 计数恰 1 次，两次产物一致。
#[test]
fn cached_fetcher_serves_second_navigation_from_cache() {
    let fetcher = MockFetcher::new();
    fetcher.serve_with_headers(
        "https://page.test/doc.html",
        vec![("cache-control".to_owned(), "max-age=60".to_owned())],
        b"<title>cached</title><body>x</body>",
    );
    let hits = fetcher.hits();
    let cached = CachedFetcher::new(fetcher);

    let first = fetch_document(&cached, "https://page.test/doc.html").expect("first");
    let second = fetch_document(&cached, "https://page.test/doc.html").expect("second");
    assert_eq!(first.data.html, second.data.html, "缓存体与首抓一致");
    assert_eq!(
        hits.lock().unwrap().len(),
        1,
        "同 URL 二次导航命中缓存，网络只打一次"
    );
}

/// 重定向落点：响应 final_url 与请求 URL 不同时，子资源相对地址按落点
/// （base_url）解析，不按原始请求 URL（WHATWG fetch final URL 语义）。
#[test]
fn subresources_resolve_against_final_url_after_redirect() {
    let fetcher = MockFetcher::new();
    fetcher.serve_final_url(
        "https://page.test/r",
        "https://page.test/final/page.html",
        b"<head><link rel=stylesheet href=style.css></head><body>x</body>",
    );
    fetcher.serve("https://page.test/final/style.css", b"p { color: red }");
    let hits = fetcher.hits();

    let fetched = fetch_document(&fetcher, "https://page.test/r").expect("ok");
    assert_eq!(
        fetched.base_url, "https://page.test/final/page.html",
        "文档 base = 重定向落点"
    );
    assert!(
        hits.lock()
            .unwrap()
            .contains(&"https://page.test/final/style.css".to_owned()),
        "相对子资源按落点解析"
    );
}

/// 地址栏输入规范化接进导航：无 scheme 补 http:// 后可达；非法输入走
/// 抓取路径返回收敛错误串（错误页兑底），全程不 panic。
#[test]
fn address_bar_navigation_normalizes_and_fails_safe() {
    let fetcher = MockFetcher::new();
    fetcher.serve("http://plain.test/x", b"<title>ok</title>");

    // 无 scheme 输入补全 http:// 后命中
    let normalized = normalize_address_bar_input("plain.test/x");
    assert_eq!(normalized, "http://plain.test/x");
    let fetched = fetch_document(&fetcher, &normalized).expect("补全后可导航");
    assert!(fetched.data.html.contains("<title>ok</title>"));

    // 非法输入（含空格）补全后仍非法：mock 按未登记 → 404 → 错误串收敛，
    // 不 panic（真实后端 reqwest 同路径判 InvalidUrl →「无效的地址」）
    let invalid = normalize_address_bar_input("hello world");
    assert_eq!(
        fetch_document(&fetcher, &invalid).err().as_deref(),
        Some("HTTP 404")
    );
}

// ---- K4 T3：@font-face 字体子资源编排（无头 MockFetcher，与 app.rs 同序） ----

use nexty_chrome::resources::{
    FetchedFont, FontOrchestrator, FontRegistration, MAX_FONT_BODY_BYTES, fetch_fonts,
};
use nexty_layout::TextRun;

/// 独特测试族名：避开系统字体碰撞（nexty-text T2 偏差——同名系统字体合并
/// 不保证 @font-face 优先）。
const TEST_FAMILY: &str = "NextyChromeTestMono";

/// 测试字体字节：nexty-text T2 入库的 OFL 字体资产，复用同一份不另入库副本。
fn test_font_bytes() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../nexty-text/tests/data/SpaceMono-Regular.ttf"
    ))
    .expect("test font asset (nexty-text tests/data)")
}

/// 树中首个非空文本 run（字体归属断言用）；找不到时 panic（测试页必有文本）。
fn first_run(fragment: &Fragment) -> &TextRun {
    fn find(fragment: &Fragment) -> Option<&TextRun> {
        for line in &fragment.lines {
            for run in &line.runs {
                if !run.glyphs.is_empty() {
                    return Some(run);
                }
            }
        }
        for child in &fragment.children {
            if let Some(run) = find(child) {
                return Some(run);
            }
        }
        None
    }
    find(fragment).expect("no text run in fragment tree")
}

/// ①③④：@font-face 页字体字节被抓取且注册（shape 归属）；就绪后恰一轮
/// 重排；同 URL 重复导航不重复抓取。编排序列与 app.rs 的 page_loaded /
/// spawn_font_fetch / fonts_ready 一致（无头模拟，整形器接线同构造点）。
#[test]
fn font_face_page_fetches_registers_and_relayouts_exactly_once() {
    let font_bytes = test_font_bytes();
    let fetcher = MockFetcher::new();
    fetcher.serve("https://page.test/mono.ttf", &font_bytes);
    let html = format!(
        "<style>@font-face {{ font-family: {TEST_FAMILY}; src: url(mono.ttf); }}</style>\
         <body><p style=\"font-family: {TEST_FAMILY}\">Hello</p></body>"
    );
    let base = "https://page.test/index.html";

    // 会话与整形器接线：与 app.rs 构造点同序（registry 先于 shaper）
    let mut fonts = FontOrchestrator::new();
    let shaper = ParleyTextShaper::with_registry(fonts.registry());
    let mut layouts = 0usize;

    // 第一轮导航：抓取 → 装配 → 收集（build_page 之前，同 page_loaded）
    let document = nexty_html::parse_document(&html, ParseOptions::default());
    let resources = resources::fetch_subresources(&fetcher, base, &document);
    let requests = fonts.collect(&resources.stylesheets, base);
    assert_eq!(requests.len(), 1, "收集到 @font-face 的 url() 源");
    let page = pipeline::build_page(document, resources.stylesheets, resources.images);

    // 首帧布局：注册表为空 → 系统回退字体（不 panic；字节不是注册字体）
    let root1 = {
        layouts += 1;
        pipeline::layout_page(&page, &shaper, 400.0).expect("root")
    };
    assert_eq!(layouts, 1);
    assert!(
        first_run(&root1)
            .fonts
            .iter()
            .all(|font| font.data().as_ref() != font_bytes.as_slice()),
        "字体未就绪首帧用系统字体渲染"
    );

    // SceneCache 缓存键（页面代次）：基线一次 emit
    let mut cache = pipeline::SceneCache::new();
    let _ = cache.scene_for(1, &page, &root1, 400.0, &[]);
    assert_eq!(cache.emits(), 1);

    // 字节就绪（后台 fetch_fonts）→ 注册 → registered > 0 → 恰一轮重排
    let fetched = fetch_fonts(&fetcher, &requests);
    assert_eq!(fetched.len(), 1);
    let registration = fonts.register(fetched);
    assert_eq!(
        registration,
        FontRegistration {
            registered: 1,
            rejected: 0
        }
    );
    let root2 = {
        layouts += 1;
        pipeline::layout_page(&page, &shaper, 400.0).expect("root")
    };
    assert_eq!(layouts, 2, "就绪后恰一轮重排（布局调用恰 +1）");

    // shape 归属：run 字体表含注册字节（同一 shaper 实例自动生效，未重建）
    assert!(
        first_run(&root2)
            .fonts
            .iter()
            .any(|font| font.data().as_ref() == font_bytes.as_slice()),
        "注册字体进入 shape 归属"
    );

    // 代次推进 → 场景缓存失效重建（重排接既有缓存失效机制）
    let _ = cache.scene_for(2, &page, &root2, 400.0, &[]);
    assert_eq!(cache.emits(), 2, "重排推进代次使 SceneCache 重建");

    // 重复/迟到载荷：registered == 0 → 不再重排（layouts 保持 2）
    let duplicate = fonts.register(vec![FetchedFont {
        url: "https://page.test/mono.ttf".to_owned(),
        descriptor: requests[0].descriptor.clone(),
        bytes: font_bytes.clone(),
    }]);
    assert_eq!(duplicate.registered, 0, "重复载荷不重复注册");

    // 第二轮导航（同 URL）：会话去重 → 不再收集、不再抓取、不再重排
    let document = nexty_html::parse_document(&html, ParseOptions::default());
    let resources = resources::fetch_subresources(&fetcher, base, &document);
    assert!(
        fonts.collect(&resources.stylesheets, base).is_empty(),
        "会话去重：二次导航不再收集"
    );
    assert_eq!(layouts, 2, "二次导航零重排");
    assert_eq!(
        fetcher
            .hits()
            .lock()
            .unwrap()
            .iter()
            .filter(|url| **url == "https://page.test/mono.ttf")
            .count(),
        1,
        "mock 计数恰 1"
    );
}

/// ②：404 / 非字体数据 / 超预算字体 → 不 panic，页面回退系统字体照常
/// 渲染；注册失败不触发重排。
#[test]
fn font_failures_degrade_to_system_fonts_without_panicking() {
    let fetcher = MockFetcher::new();
    fetcher.fail("https://page.test/lost.ttf", 404);
    fetcher.serve("https://page.test/junk.ttf", b"not a font");
    fetcher.serve(
        "https://page.test/huge.ttf",
        &vec![0u8; MAX_FONT_BODY_BYTES + 1],
    );
    let html = "<style>\
                @font-face { font-family: Lost; src: url(lost.ttf); }\
                @font-face { font-family: Junk; src: url(junk.ttf); }\
                @font-face { font-family: Huge; src: url(huge.ttf); }\
                </style><body><p style=\"font-family: Lost, Junk, Huge\">Hello</p></body>";
    let base = "https://page.test/index.html";

    let mut fonts = FontOrchestrator::new();
    let shaper = ParleyTextShaper::with_registry(fonts.registry());
    let mut layouts = 0usize;

    let document = nexty_html::parse_document(html, ParseOptions::default());
    let resources = resources::fetch_subresources(&fetcher, base, &document);
    let requests = fonts.collect(&resources.stylesheets, base);
    assert_eq!(
        requests.len(),
        3,
        "url() 源全部收集（格式/预算过滤发生在抓取、注册层）"
    );
    let page = pipeline::build_page(document, resources.stylesheets, resources.images);
    let root = {
        layouts += 1;
        pipeline::layout_page(&page, &shaper, 400.0).expect("root")
    };

    // 抓取：404 / 超预算缺席；junk 字节到达但注册期拒收（记报告）
    let fetched = fetch_fonts(&fetcher, &requests);
    assert_eq!(fetched.len(), 1, "404 与超预算缺席");
    let registration = fonts.register(fetched);
    assert_eq!(
        registration,
        FontRegistration {
            registered: 0,
            rejected: 1
        },
        "非字体数据拒收记数"
    );
    assert_eq!(layouts, 1, "注册失败不触发重排");

    // 页面照常渲染：系统回退文本进显示列表（不 panic、不空白）
    let scene = pipeline::build_scene(&page, &root);
    assert!(
        scene
            .commands
            .iter()
            .any(|command| matches!(command, Command::DrawText { .. })),
        "回退路径照常渲染文本"
    );
}
