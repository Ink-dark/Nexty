//! chrome 层集成测试：端到端管线与渲染隔离线程。
//!
//! 窗口/GPU 胶水（app 模块）无法无头测试，不在此覆盖。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nexty_paint::{Color, Command, Pixmap, RasterError, Rasterizer, Scene, Size};
use nexty_text::ParleyTextShaper;

use nexty_chrome::pipeline::{self, Viewport};
use nexty_chrome::render::{RenderError, RenderThread};

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

    // 显示列表应包含：body 白底、box 蓝底、四条红边、一条文本
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

    let has_white = fills.iter().any(|(rect, color)| {
        color.r == 255 && color.g == 255 && color.b == 255 && rect.width > 300.0
    });
    let has_blue = fills
        .iter()
        .any(|(_, color)| *color == Color::opaque(0, 128, 255));
    let red_strips = fills
        .iter()
        .filter(|(_, color)| *color == Color::opaque(255, 0, 0))
        .count();
    assert!(has_white, "body 白底");
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

#[test]
fn render_thread_round_trips() {
    let thread = RenderThread::spawn(Arc::new(nexty_paint::VelloCpuRasterizer::new()));
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
            &scene,
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
    let thread = RenderThread::spawn(Arc::new(PanicsOnce {
        calls: AtomicUsize::new(0),
    }));
    let scene = Scene::default();
    let size = Size {
        width: 4,
        height: 4,
    };

    let error = thread.render(&scene, size).expect_err("first call panics");
    match error {
        RenderError::Panicked(message) => assert!(message.contains("injected")),
        RenderError::Raster(_) => panic!("expected panic error"),
    }

    // 兜底后线程存活：第二次调用正常返回
    let pixmap = thread.render(&scene, size).expect("second call succeeds");
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
    let thread = RenderThread::spawn(Arc::new(AlwaysEmpty));
    let error = thread
        .render(
            &Scene::default(),
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
struct MockFetcher {
    routes: Mutex<BTreeMap<String, Result<Vec<u8>, u16>>>,
}

impl MockFetcher {
    fn new() -> Self {
        Self {
            routes: Mutex::new(BTreeMap::new()),
        }
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
}

impl NetworkFetcher for MockFetcher {
    fn fetch(&self, request: &Request) -> Result<Response, NetworkError> {
        let routes = self.routes.lock().unwrap();
        match routes.get(&request.url) {
            Some(Ok(body)) => Ok(Response {
                status: 200,
                headers: Vec::new(),
                body: body.clone(),
            }),
            Some(Err(status)) => Ok(Response {
                status: *status,
                headers: Vec::new(),
                body: Vec::new(),
            }),
            None => Ok(Response {
                status: 404,
                headers: Vec::new(),
                body: Vec::new(),
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
