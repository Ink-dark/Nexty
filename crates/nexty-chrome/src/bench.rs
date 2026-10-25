//! 性能基准的共享支撑：确定性语料生成、整形调用计数与执行门控。
//!
//! 基准正文在 [`crate`] 的 `benches/pipeline.rs`（`harness = false`）：
//! 只有 `BENCH_MODE=1 cargo bench -p nexty-chrome --bench pipeline` 执行，
//! 其余入口（`cargo test` / `cargo clippy --all-targets`）只编译不执行。
//! 本模块提供语料与计数器，供基准正文与单测使用：
//!
//! - 语料三页全部确定性生成（xorshift64* 固定种子），不依赖网络与本机
//!   状态，两次生成逐字节一致（单测钉住）；
//! - [`CountingShaper`] 把 H4 的放大倍数表达为 shape / metrics 调用次数
//!   （机器无关），是 T2 优化前后的对照量。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nexty_dom::{Document, Namespace, NodeId, NodeKind};
use nexty_paint::Image;
use nexty_text::{FontMetrics, ShapedText, TextError, TextShaper, TextStyle};

/// 基准执行的环境变量名。
pub const BENCH_ENV_VAR: &str = "BENCH_MODE";

/// 门控判定（纯函数，供单测钉住 skip 语义）：只有值恰为 `"1"` 才执行。
#[must_use]
pub fn bench_enabled_from(value: Option<&str>) -> bool {
    value == Some("1")
}

/// 当前进程是否应执行基准正文（[`BENCH_ENV_VAR`] 恰为 `"1"`）。
#[must_use]
pub fn bench_enabled() -> bool {
    bench_enabled_from(std::env::var(BENCH_ENV_VAR).ok().as_deref())
}

/// 包装另一个 [`TextShaper`] 并统计 shape / metrics 调用次数。
///
/// 基准以调用次数表达放大倍数：单次全量布局的 `shape` 调用数 ÷ 文本
/// chunk 数、`metrics` 调用数 ÷ 不同 [`TextStyle`] 数。H4 落地前前者
/// 远大于 1（每次 measure 重整形），落地后应 ≤ 1。
pub struct CountingShaper {
    inner: Box<dyn TextShaper>,
    shape_calls: AtomicUsize,
    metrics_calls: AtomicUsize,
}

impl CountingShaper {
    /// 包装一个底层整形器。
    #[must_use]
    pub fn new(inner: Box<dyn TextShaper>) -> Self {
        Self {
            inner,
            shape_calls: AtomicUsize::new(0),
            metrics_calls: AtomicUsize::new(0),
        }
    }

    /// 累计的 `shape` 调用次数。
    #[must_use]
    pub fn shape_calls(&self) -> usize {
        self.shape_calls.load(Ordering::Relaxed)
    }

    /// 累计的 `metrics` 调用次数。
    #[must_use]
    pub fn metrics_calls(&self) -> usize {
        self.metrics_calls.load(Ordering::Relaxed)
    }

    /// 两个计数器清零（场景之间隔离统计窗口）。
    pub fn reset(&self) {
        self.shape_calls.store(0, Ordering::Relaxed);
        self.metrics_calls.store(0, Ordering::Relaxed);
    }
}

impl TextShaper for CountingShaper {
    fn shape(&self, text: &str, style: &TextStyle) -> Result<ShapedText, TextError> {
        self.shape_calls.fetch_add(1, Ordering::Relaxed);
        self.inner.shape(text, style)
    }

    fn metrics(&self, style: &TextStyle) -> Result<FontMetrics, TextError> {
        self.metrics_calls.fetch_add(1, Ordering::Relaxed);
        self.inner.metrics(style)
    }
}

/// 确定性伪随机源（xorshift64*），语料生成的唯一随机源，种子固定。
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }
}

/// 语料用的常用汉字集（混排文本的中文来源）。
const HAN_CHARS: &str = "的一是不了在人有我他这个上们来到时大地为子中你说生国年着就那和要她出也得里后自以会家可下而过天去能对小多然于心学么之都好看起发当没成只如事把还用第样道想作种开美总从无情己面最女但现前些所同日手又行意动方期它头经长儿回位分爱老因很给名法间斯知世什两次使身者被高已亲其进此话常与活正感";

/// 语料用的英文词表（混排文本的拉丁来源）。
const LATIN_WORDS: &[&str] = &[
    "layout", "shaping", "browser", "engine", "text", "measure", "cache", "render", "pipeline",
    "style", "cascade", "cluster", "glyph", "width", "storm", "scroll", "bench", "median",
    "warmup", "fragment",
];

/// 从混排语料取一段中英文混合文本（`len` 为近似字符数）。
fn mixed_text(rng: &mut Rng, len: usize) -> String {
    let han = HAN_CHARS.as_bytes();
    let mut text = String::with_capacity(len * 3);
    while text.chars().count() < len {
        match rng.below(3) {
            0 => {
                let start = rng.below(han.len() - 6);
                // 落在字符边界：常用字集全部是 BMP 内 3 字节 UTF-8
                let chunk = &han[start - start % 3..start - start % 3 + 3];
                text.push_str(std::str::from_utf8(chunk).expect("han chars"));
            }
            1 => {
                text.push_str(LATIN_WORDS[rng.below(LATIN_WORDS.len())]);
                text.push(' ');
            }
            _ => {
                text.push('中');
                text.push(' ');
            }
        }
    }
    text
}

/// 文本密集页：约 2,000 段中英混排 + 行内样式嵌套（每段 3 个文本 chunk）。
#[must_use]
pub fn text_heavy_html() -> String {
    let mut rng = Rng::new(0x5EED_0001);
    let mut html = String::from(
        "<!DOCTYPE html><html><head><title>text-heavy</title></head>\
         <body><main>",
    );
    for i in 0..2000 {
        let size = 11 + i % 10;
        let r = 40 + i % 200;
        let g = 60 + (i * 3) % 180;
        let b = 80 + (i * 7) % 160;
        html.push_str(&format!(
            "<p style=\"font-size:{size}px;color:rgb({r} {g} {b})\">"
        ));
        html.push_str(&mixed_text(&mut rng, 40));
        html.push_str("<span style=\"font-weight:700\">");
        html.push_str(&mixed_text(&mut rng, 16));
        html.push_str("</span>");
        html.push_str(&mixed_text(&mut rng, 24));
        html.push_str("<span style=\"letter-spacing:1.5px\">");
        html.push_str(&mixed_text(&mut rng, 16));
        html.push_str("</span>");
        html.push_str("</p>");
    }
    html.push_str("</main></body></html>");
    html
}

/// 规则密集页：约 5,000 条类/元素规则 × 约 20,000 个元素。
///
/// 返回 `(html, author_css)`。类选择器按 5,000 取模循环命中，无分桶时
/// 每个元素都要线性扫过全部规则——M5 的目标场景。
#[must_use]
pub fn rules_heavy_corpus() -> (String, String) {
    let mut rng = Rng::new(0x5EED_0002);
    let mut html =
        String::from("<!DOCTYPE html><html><head><title>rules-heavy</title></head><body>");
    const CLASSES: usize = 5_000;
    const ELEMENTS: usize = 10_000;
    for i in 0..ELEMENTS {
        let class = i % CLASSES;
        let inner = (i * 7 + 3) % CLASSES;
        html.push_str(&format!(
            "<div class=\"c{class}\"><span class=\"c{inner}\">"
        ));
        html.push_str(&mixed_text(&mut rng, 12));
        html.push_str("</span></div>");
    }
    html.push_str("</body></html>");

    let mut css = String::from("body { font-size: 14px } p { margin: 2px }\n");
    for i in 0..CLASSES {
        let margin = i % 17;
        let padding = i % 23;
        let r = i % 256;
        let g = (i * 3) % 256;
        let b = (i * 7) % 256;
        css.push_str(&format!(
            ".c{i} {{ margin: {margin}px; padding-left: {padding}px; color: rgb({r} {g} {b}); }}\n"
        ));
    }
    (html, css)
}

/// 图片页：24 张 `<img>`（块级与行内上下文各半），像素由 [`fill_images`]
/// 按节点注入——基准不走网络抓取与解码，直接构造 RGBA 池。
#[must_use]
pub fn image_heavy_html() -> String {
    let mut html =
        String::from("<!DOCTYPE html><html><head><title>image-heavy</title></head><body>");
    for i in 0..24 {
        if i % 2 == 0 {
            html.push_str("<div><img></div>");
        } else {
            html.push_str("<p>图前文本 <img> 图后文本</p>");
        }
    }
    html.push_str("</body></html>");
    html
}

/// 按文档序收集 `<img>` 元素节点（HTML 命名空间）。
fn image_nodes(document: &Document) -> Vec<NodeId> {
    fn walk(document: &Document, node: NodeId, out: &mut Vec<NodeId>) {
        if let Some(NodeKind::Element(data)) = document.node(node)
            && data.namespace == Namespace::Html
            && data.name == "img"
        {
            out.push(node);
        }
        for child in document.children(node) {
            walk(document, child, out);
        }
    }
    let mut out = Vec::new();
    walk(document, document.root(), &mut out);
    out
}

/// 为文档里的每个 `<img>` 生成一张确定性 RGBA 图（320×200，按节点序
/// 填充渐变噪声图案），返回页面图片池（`Arc` 共享，与 `Page.images` 同型）。
#[must_use]
pub fn fill_images(document: &Document) -> std::collections::HashMap<NodeId, Arc<Image>> {
    const WIDTH: u32 = 320;
    const HEIGHT: u32 = 200;
    let nodes = image_nodes(document);
    let mut images = std::collections::HashMap::new();
    for (index, node) in nodes.iter().enumerate() {
        let mut rng = Rng::new(0x5EED_1000 + index as u64);
        let mut data = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let noise = rng.next_u64() as u8;
                data.extend_from_slice(&[
                    (x * 255 / WIDTH) as u8,
                    (y * 255 / HEIGHT) as u8,
                    noise,
                    255,
                ]);
            }
        }
        images.insert(*node, Arc::new(Image::new(WIDTH, HEIGHT, data)));
    }
    images
}

/// 文档中的文本节点数（H4 放大倍数的分母：文本 chunk 数的代理——
/// 语料中一个文本节点恰为一个文本 chunk）。
#[must_use]
pub fn count_text_nodes(document: &Document) -> usize {
    fn walk(document: &Document, node: NodeId) -> usize {
        let own = usize::from(matches!(document.node(node), Some(NodeKind::Text(_))));
        own + document
            .children(node)
            .map(|child| walk(document, child))
            .sum::<usize>()
    }
    walk(document, document.root())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexty_html::ParseOptions;

    /// 语料确定性：两次生成逐字节一致（基准可重复的前提）。
    #[test]
    fn corpora_are_deterministic() {
        assert_eq!(text_heavy_html(), text_heavy_html());
        assert_eq!(rules_heavy_corpus(), rules_heavy_corpus());
        assert_eq!(image_heavy_html(), image_heavy_html());
    }

    /// 图片池确定性：确定性解析给出相同 NodeId 序列，像素逐字节一致。
    #[test]
    fn images_are_deterministic() {
        let html = image_heavy_html();
        let first = nexty_html::parse_document(&html, ParseOptions::default());
        let second = nexty_html::parse_document(&html, ParseOptions::default());
        let a = fill_images(&first);
        let b = fill_images(&second);
        assert_eq!(a.len(), 24);
        assert_eq!(a, b);
    }

    /// 文本节点计数覆盖混排语料：2,000 段 × 4 个文本 chunk + `<title>`。
    #[test]
    fn text_heavy_page_has_expected_chunks() {
        let document = nexty_html::parse_document(&text_heavy_html(), ParseOptions::default());
        assert_eq!(count_text_nodes(&document), 8001);
    }

    /// 门控纯函数：只有恰为 "1" 才执行（skip 路径的钉子）。
    #[test]
    fn bench_gate_accepts_only_one() {
        assert!(!bench_enabled_from(None));
        assert!(!bench_enabled_from(Some("")));
        assert!(!bench_enabled_from(Some("0")));
        assert!(!bench_enabled_from(Some("true")));
        assert!(bench_enabled_from(Some("1")));
    }
}
