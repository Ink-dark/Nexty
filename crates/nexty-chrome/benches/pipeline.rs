//! 管线性能基准（性能轮 T1）。
//!
//! 执行：`BENCH_MODE=1 cargo bench -p nexty-chrome --bench pipeline`。
//! 无 `BENCH_MODE=1` 时打印 skip 立即退出——`cargo test` / `cargo clippy
//! --all-targets` 只会编译本文件，不会执行基准正文（门控语义有单测钉住）。
//!
//! 语料三页确定性生成（[`nexty_chrome::bench`]），场景：
//! 1. **首屏全管线**——parse / cascade / layout / scene 各阶段分别计时，
//!    取 N 次中位数（std::time 计时 + 预热，不引入 criterion）；
//! 2. **resize 风暴**——宽度 ±1 连续 20 次重排（recascade + layout），
//!    记录管线执行次数与整形调用数（H4 放大倍数）；
//! 3. **滚动风暴**——同一布局下 50 个滚动偏移的 emit + rasterize，
//!    分别计时（M6 的目标场景）。
//!
//! 基线结论入 `docs/2026-10-25-perf-baseline.md`。

use std::collections::HashMap;
use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nexty_chrome::bench::{
    CountingShaper, count_text_nodes, fill_images, image_heavy_html, rules_heavy_corpus,
    text_heavy_html,
};
use nexty_chrome::pipeline;
use nexty_css::{MediaViewport, Stylesheet};
use nexty_dom::NodeId;
use nexty_html::ParseOptions;
use nexty_paint::{Image, Rasterizer, Size, VelloCpuRasterizer};
use nexty_text::ParleyTextShaper;

/// 基准视口（与 app 默认窗口 1000×640 一致）。
const VIEWPORT: MediaViewport = MediaViewport {
    width: 1000.0,
    height: 640.0,
};

/// 每个场景的采样次数（预热 1 次不计入）。中位数取自这些样本。
const RUNS: usize = 5;
const WARMUPS: usize = 1;
const RESIZE_COUNT: usize = 20;
/// resize 合帧的事件/帧比（拖动边缘的典型突发：一帧内到多个事件）。
const RESIZE_EVENTS_PER_FRAME: usize = 4;
const SCROLL_FRAMES: usize = 50;

/// 首屏各阶段耗时（ms）。
#[derive(Debug, Default, Clone, Copy)]
struct StageMs {
    parse: f64,
    cascade: f64,
    layout: f64,
    scene: f64,
}

fn median(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).expect("finite durations"));
    samples[samples.len() / 2]
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// 一份语料：HTML、作者 CSS 与预生成图片池（NodeId 对确定性解析稳定）。
struct Corpus {
    name: &'static str,
    html: String,
    css: String,
    images: HashMap<NodeId, Arc<Image>>,
}

impl Corpus {
    fn new(name: &'static str, html: String, css: &str) -> Self {
        let document = nexty_html::parse_document(&html, ParseOptions::default());
        let images = fill_images(&document);
        let text_nodes = count_text_nodes(&document);
        println!(
            "[{name}] corpus: {} bytes html, {} bytes css, {} images, {text_nodes} text nodes",
            html.len(),
            css.len(),
            images.len(),
        );
        Self {
            name,
            html,
            css: css.to_owned(),
            images,
        }
    }

    /// 首屏全管线一次（不计时）：预热或整形计数用。
    fn once(&self, shaper: &CountingShaper) {
        let document = nexty_html::parse_document(&self.html, ParseOptions::default());
        let sheet = Stylesheet::parse(&self.css);
        let mut page = pipeline::build_page(document, vec![sheet], self.images.clone());
        pipeline::recascade(&mut page, VIEWPORT);
        let root = pipeline::layout_page(&page, shaper, VIEWPORT.width).expect("layout");
        black_box(pipeline::build_scene_at_with_highlights(
            &page,
            &root,
            0.0,
            0.0,
            &[],
        ));
    }

    /// 首屏全管线计时：四阶段分别取中位数。
    fn first_load(&self, shaper: &CountingShaper) -> StageMs {
        let mut parse_samples = Vec::new();
        let mut cascade_samples = Vec::new();
        let mut layout_samples = Vec::new();
        let mut scene_samples = Vec::new();
        for _ in 0..WARMUPS {
            self.once(shaper);
        }
        for _ in 0..RUNS {
            let owned_images = black_box(self.images.clone());
            let start = Instant::now();
            let document = nexty_html::parse_document(&self.html, ParseOptions::default());
            parse_samples.push(ms(start.elapsed()));

            let start = Instant::now();
            let sheet = Stylesheet::parse(&self.css);
            // build_page 的级联 + recascade 的视口级联：首屏共两次全量级联
            let mut page = pipeline::build_page(document, vec![sheet], owned_images);
            pipeline::recascade(&mut page, VIEWPORT);
            cascade_samples.push(ms(start.elapsed()));

            let start = Instant::now();
            let root = pipeline::layout_page(&page, shaper, VIEWPORT.width).expect("layout");
            layout_samples.push(ms(start.elapsed()));

            let start = Instant::now();
            let scene = pipeline::build_scene_at_with_highlights(&page, &root, 0.0, 0.0, &[]);
            scene_samples.push(ms(start.elapsed()));
            black_box(scene);
        }
        StageMs {
            parse: median(parse_samples),
            cascade: median(cascade_samples),
            layout: median(layout_samples),
            scene: median(scene_samples),
        }
    }

    /// resize 风暴（M6 合帧后语义）：`RESIZE_COUNT` 个 Resized 事件按
    /// 每 `RESIZE_EVENTS_PER_FRAME` 个一帧连发（拖动边缘的典型突发），
    /// 经 [`pipeline::ResizeCoalescer`] 合帧——管线每帧至多一次。返回
    /// （每帧中位数 ms，帧数=管线执行次数，事件数，shape 调用数，metrics 调用数）。
    fn resize_storm(&self, shaper: &CountingShaper) -> (f64, usize, usize, usize, usize, usize) {
        let document = nexty_html::parse_document(&self.html, ParseOptions::default());
        let sheet = Stylesheet::parse(&self.css);
        let mut page = pipeline::build_page(document, vec![sheet], self.images.clone());
        pipeline::recascade(&mut page, VIEWPORT);
        pipeline::layout_page(&page, shaper, VIEWPORT.width).expect("layout");

        let mut coalescer = pipeline::ResizeCoalescer::default();
        shaper.reset();
        let storm_start = Instant::now();
        let mut per_frame = Vec::new();
        let mut pipeline_runs = 0;
        let mut events = 0;
        let mut next_width_step = 0;
        while events < RESIZE_COUNT {
            // 一帧内连发若干 Resized 事件（合帧器只置脏）
            let burst = RESIZE_EVENTS_PER_FRAME.min(RESIZE_COUNT - events);
            for _ in 0..burst {
                coalescer.on_resized();
                events += 1;
            }
            if coalescer.take() {
                let width = VIEWPORT.width + f32::from((next_width_step % 2) as i16);
                next_width_step += 1;
                let start = Instant::now();
                pipeline::recascade(
                    &mut page,
                    MediaViewport {
                        width,
                        height: VIEWPORT.height,
                    },
                );
                pipeline::layout_page(&page, shaper, width).expect("layout");
                per_frame.push(ms(start.elapsed()));
                pipeline_runs += 1;
            }
        }
        let _ = ms(storm_start.elapsed());
        (
            median(per_frame),
            pipeline_runs,
            events,
            shaper.shape_calls(),
            shaper.metrics_calls(),
            RESIZE_COUNT,
        )
    }

    /// 滚动风暴（M6 场景缓存后语义）：同一布局下 `SCROLL_FRAMES` 个滚动
    /// 偏移经 [`pipeline::SceneCache`] 取页面坐标系场景（首帧后零 emit），
    /// emit 阶段 = 缓存命中 + [`nexty_paint::Scene::translated`] origin
    /// 平移。返回（平移中位数 ms，rasterize 中位数 ms，emit 次数）。
    fn scroll_storm(&self, shaper: &CountingShaper) -> (f64, f64, usize) {
        let document = nexty_html::parse_document(&self.html, ParseOptions::default());
        let sheet = Stylesheet::parse(&self.css);
        let mut page = pipeline::build_page(document, vec![sheet], self.images.clone());
        pipeline::recascade(&mut page, VIEWPORT);
        let root = pipeline::layout_page(&page, shaper, VIEWPORT.width).expect("layout");
        let content_height = root.border_box.height;
        let max_scroll = (content_height - VIEWPORT.height).max(0.0);

        let mut cache = pipeline::SceneCache::new();
        // 首帧构建（导航语义），其后统计窗口内的 emit 应为 0
        let _ = cache.scene_for(0, &page, &root, VIEWPORT.width, &[]);
        cache.reset_emits();

        let rasterizer = VelloCpuRasterizer::new();
        let mut emit_samples = Vec::new();
        let mut raster_samples = Vec::new();
        for frame in 0..SCROLL_FRAMES {
            // 确定性偏移：137 步长在典型页高上打满全程
            let scroll = if max_scroll > 0.0 {
                ((frame as u64) * 137) % (max_scroll as u64) + 1
            } else {
                0
            } as f32;
            let start = Instant::now();
            let page_scene = cache.scene_for(0, &page, &root, VIEWPORT.width, &[]);
            let scene = page_scene.translated(0.0, -scroll);
            emit_samples.push(ms(start.elapsed()));
            let start = Instant::now();
            let pixmap = rasterizer
                .rasterize(
                    &scene,
                    Size {
                        width: VIEWPORT.width as u32,
                        height: VIEWPORT.height as u32,
                    },
                )
                .expect("rasterize");
            raster_samples.push(ms(start.elapsed()));
            black_box(pixmap);
        }
        (median(emit_samples), median(raster_samples), cache.emits())
    }
}

fn run() {
    println!(
        "== pipeline benchmark: viewport {}x{}, {RUNS} runs (+{WARMUPS} warmup), median ==",
        VIEWPORT.width, VIEWPORT.height
    );
    let shaper = CountingShaper::new(Box::new(ParleyTextShaper::new()));

    let (rules_html, rules_css) = rules_heavy_corpus();
    let corpora = [
        Corpus::new("text-heavy", text_heavy_html(), ""),
        Corpus::new("rules-heavy", rules_html, &rules_css),
        Corpus::new("image-heavy", image_heavy_html(), ""),
    ];

    for corpus in &corpora {
        // 首屏：阶段计时 + 单次布局的整形调用数（独立计数窗口）
        let stages = corpus.first_load(&shaper);
        println!(
            "[{}] first-load median ms: parse {:.1} | cascade {:.1} | layout {:.1} | scene {:.1}",
            corpus.name, stages.parse, stages.cascade, stages.layout, stages.scene,
        );

        let document = nexty_html::parse_document(&corpus.html, ParseOptions::default());
        let chunks = count_text_nodes(&document);
        let sheet = Stylesheet::parse(&corpus.css);
        let mut page = pipeline::build_page(document, vec![sheet], corpus.images.clone());
        pipeline::recascade(&mut page, VIEWPORT);
        shaper.reset();
        let _ = pipeline::layout_page(&page, &shaper, VIEWPORT.width).expect("layout");
        println!(
            "[{}] single layout shaping: shape {} calls / {chunks} chunks = {:.2}x, metrics {} calls",
            corpus.name,
            shaper.shape_calls(),
            shaper.shape_calls() as f64 / chunks.max(1) as f64,
            shaper.metrics_calls(),
        );

        let (per_frame, runs, _events, shapes, metricses, total_events) =
            corpus.resize_storm(&shaper);
        println!(
            "[{}] resize storm: {total_events} events @ {} events/frame -> pipeline runs {runs} (was {total_events} un-coalesced), median/frame {per_frame:.1} ms, shape {shapes} calls ({:.1}x chunks), metrics {metricses} calls",
            corpus.name,
            RESIZE_EVENTS_PER_FRAME,
            shapes as f64 / chunks.max(1) as f64,
        );

        let (emit, raster, emits) = corpus.scroll_storm(&shaper);
        println!(
            "[{}] scroll storm: {SCROLL_FRAMES} frames, emit count {emits} (must be 0 after initial build), translate median {emit:.3} ms, rasterize median {raster:.1} ms",
            corpus.name,
        );
    }
}

fn main() {
    if !nexty_chrome::bench::bench_enabled() {
        println!("skip: pipeline benchmark needs BENCH_MODE=1 (cargo test/clippy only compile it)");
        return;
    }
    // 布局递归（taffy 解算 + 断行 + 整形）实测需数 MB 栈；bench 是独立
    // PE，build.rs 的 `/STACK`（rustc-link-arg-bins）只作用于 bin 目标，
    // 主线程仍是 1 MiB 默认预留——正文跑在显式大栈线程里，与 chrome
    // 全链加固测试同一约束。
    let result = std::thread::Builder::new()
        .stack_size(32 * 1024 * 1024)
        .spawn(run)
        .expect("spawn bench thread")
        .join();
    if result.is_err() {
        std::process::exit(101);
    }
}
