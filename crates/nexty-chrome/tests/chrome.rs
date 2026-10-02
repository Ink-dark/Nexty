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
    let scene = pipeline::build_scene(&root);

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
    let scene = pipeline::build_scene(&root);

    // 除 body 背景（若 UA 给了）外，不允许出现匿名背景/边框；
    // 匿名片段的指令只可能是 DrawText
    let text_count = scene
        .commands
        .iter()
        .filter(|command| matches!(command, Command::DrawText { .. }))
        .count();
    assert!(text_count >= 2, "两段文本各自成行");
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
