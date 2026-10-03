//! 子资源抓取与解码：样式表/图片清单 → URL 解析 → 并发抓取 → 级联输入。
//!
//! 职责边界：解析归 nexty-html（清单收集）、传输归 nexty-network（字节）、
//! 解码归 nexty-paint（RGBA8）、级联归 nexty-css（多表合并）。本模块只做
//! 编排：把清单里的相对 URL 解析为绝对地址，并发抓取后按清单顺序交回。
//!
//! 降级语义（T5 退出条件）：单个子资源失败（URL 非法 / 传输错误 / 非 2xx /
//! 解码失败）只跳过该资源，不影响其余渲染——外链 CSS 失败 = 该表不参与级联；
//! 图片失败 = 该节点不产生像素（布局回退默认对象尺寸 300×150）。

use std::collections::HashMap;

use nexty_css::Stylesheet;
use nexty_dom::{Document, NodeId};
use nexty_html::StyleResource;
use nexty_network::{Method, NetworkError, NetworkFetcher, Request, resolve};
use nexty_paint::{Image, decode};

/// 并发抓取的线程上限：清单可能很大（真实页面几十张图），逐资源开线程
/// 会撑爆调度器；分块处理后每块内并发、块间串行。
const MAX_CONCURRENT_FETCHES: usize = 8;

/// 抓取完成的子资源。
pub struct Subresources {
    /// 作者样式表，按级联优先级排列（inline 与外链按文档出现顺序交错）。
    pub stylesheets: Vec<Stylesheet>,
    /// 已解码图片（`<img>` 节点 → RGBA 图像）。
    pub images: HashMap<NodeId, Image>,
}

/// 抓取文档的子资源（CSS + 图片）。
///
/// `base_url` 是文档 URL（相对 URL 的解析基准）；`about:blank` 等无网络
/// 内部页直接传空文档清单即可（本函数对空清单返回空结果）。
#[must_use]
pub fn fetch_subresources(
    fetcher: &dyn NetworkFetcher,
    base_url: &str,
    document: &Document,
) -> Subresources {
    let style_resources = nexty_html::collect_style_resources(document);
    let image_resources = nexty_html::collect_image_resources(document);

    // ---- 阶段一：解析 URL，编出抓取任务 ----
    /// 清单顺序的样式表槽位：inline 有文本，外链等抓取，解析失败的丢弃。
    enum SheetSlot {
        Inline(String),
        External { url: String },
        Unresolved,
    }
    let mut css_urls: HashMap<String, usize> = HashMap::new(); // 绝对 URL → 任务序号
    let mut css_tasks: Vec<String> = Vec::new();
    let mut sheet_slots: Vec<SheetSlot> = Vec::new();
    let mut image_urls: HashMap<String, Vec<NodeId>> = HashMap::new(); // 绝对 URL → 节点
    let mut image_tasks: Vec<String> = Vec::new();

    for resource in &style_resources {
        match resource {
            StyleResource::Inline { css } => {
                sheet_slots.push(SheetSlot::Inline(css.clone()));
            }
            StyleResource::External { href } => match resolve(base_url, href) {
                Ok(url) => {
                    // 同一 URL 只抓一次，级联里按首现位置参与
                    if css_urls.insert(url.clone(), css_tasks.len()).is_none() {
                        css_tasks.push(url.clone());
                    }
                    sheet_slots.push(SheetSlot::External { url });
                }
                // URL 解析失败 = 该表不参与级联（降级）
                Err(_) => sheet_slots.push(SheetSlot::Unresolved),
            },
        }
    }

    for resource in &image_resources {
        if let Ok(url) = resolve(base_url, &resource.src) {
            image_urls
                .entry(url.clone())
                .or_insert_with(|| {
                    image_tasks.push(url.clone());
                    Vec::new()
                })
                .push(resource.node);
        }
    }

    // ---- 阶段二：并发抓取（分块限流）----
    let css_bytes = fetch_many(fetcher, &css_tasks);
    let image_bytes = fetch_many(fetcher, &image_tasks);

    // ---- 阶段三：按清单顺序重组 ----
    // inline 与抓到的外链按文档出现顺序交错产出（顺序即级联优先级）；
    // 抓取失败的外链槽位跳过
    let mut stylesheets = Vec::new();
    for slot in &sheet_slots {
        match slot {
            SheetSlot::Inline(css) => stylesheets.push(Stylesheet::parse(css)),
            SheetSlot::External { url } => {
                let task = css_urls[url];
                if let Some(bytes) = &css_bytes[task] {
                    let css = String::from_utf8_lossy(bytes);
                    stylesheets.push(Stylesheet::parse(&css));
                }
            }
            SheetSlot::Unresolved => {}
        }
    }

    let mut images: HashMap<NodeId, Image> = HashMap::new();
    for (task, url) in image_tasks.iter().enumerate() {
        let Some(bytes) = &image_bytes[task] else {
            continue; // 抓取失败：该图所有节点都无像素（降级）
        };
        match decode(bytes) {
            Ok(image) => {
                if let Some(nodes) = image_urls.get(url) {
                    for node in nodes {
                        images.insert(*node, image.clone());
                    }
                }
            }
            Err(_) => continue, // 解码失败：同样降级
        }
    }

    Subresources {
        stylesheets,
        images,
    }
}

/// 并发抓取一组 URL，返回与 `urls` 等长的结果槽（失败为 `None`）。
fn fetch_many(fetcher: &dyn NetworkFetcher, urls: &[String]) -> Vec<Option<Vec<u8>>> {
    let mut results: Vec<Option<Vec<u8>>> = Vec::with_capacity(urls.len());
    for chunk in urls.chunks(MAX_CONCURRENT_FETCHES) {
        let chunk_results: Vec<Option<Vec<u8>>> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|url| {
                    let request = Request {
                        url: url.clone(),
                        method: Method::Get,
                        headers: Vec::new(),
                    };
                    scope.spawn(move || fetch_ok(fetcher, &request))
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().ok().flatten())
                .collect()
        });
        results.extend(chunk_results);
    }
    results
}

/// 抓取并把非 2xx / 错误归为 `None`（降级语义）。
fn fetch_ok(fetcher: &dyn NetworkFetcher, request: &Request) -> Option<Vec<u8>> {
    match fetcher.fetch(request) {
        Ok(response) if (200..300).contains(&response.status) => Some(response.body),
        Ok(_) => None,
        Err(NetworkError::InvalidUrl | NetworkError::Transport | NetworkError::Timeout) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexty_html::ParseOptions;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// 内存 mock fetcher：URL → 响应（body 或状态码）。未登记的 URL 返回 404。
    struct MockFetcher {
        routes: Mutex<BTreeMap<String, Result<Vec<u8>, u16>>>,
        /// 抓取计数（验证去重）。
        hits: Mutex<Vec<String>>,
    }

    impl MockFetcher {
        fn new() -> Self {
            Self {
                routes: Mutex::new(BTreeMap::new()),
                hits: Mutex::new(Vec::new()),
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
        fn fetch(&self, request: &Request) -> Result<nexty_network::Response, NetworkError> {
            self.hits.lock().unwrap().push(request.url.clone());
            let routes = self.routes.lock().unwrap();
            match routes.get(&request.url) {
                Some(Ok(body)) => Ok(nexty_network::Response {
                    status: 200,
                    headers: Vec::new(),
                    body: body.clone(),
                }),
                Some(Err(status)) => Ok(nexty_network::Response {
                    status: *status,
                    headers: Vec::new(),
                    body: Vec::new(),
                }),
                None => Ok(nexty_network::Response {
                    status: 404,
                    headers: Vec::new(),
                    body: Vec::new(),
                }),
            }
        }
    }

    fn document(html: &str) -> Document {
        nexty_html::parse_document(html, ParseOptions::default())
    }

    /// 最小合法 PNG：1×1 红色像素。
    fn tiny_png() -> Vec<u8> {
        let mut buffer = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([255, 0, 0, 255]),
        ))
        .write_to(
            &mut std::io::Cursor::new(&mut buffer),
            image::ImageFormat::Png,
        )
        .expect("encode png");
        buffer
    }

    const BASE: &str = "https://page.test/index.html";

    #[test]
    fn inline_and_external_stylesheets_cascade_in_tree_order() {
        let fetcher = MockFetcher::new();
        fetcher.serve(
            "https://page.test/ext.css",
            b"p { color: blue; border: 1px solid black }",
        );
        let document = document(
            "<head><style>p { color: red }</style>\
             <link rel=stylesheet href=ext.css></head><body><p>x</p></body>",
        );

        let resources = fetch_subresources(&fetcher, BASE, &document);
        assert_eq!(resources.stylesheets.len(), 2, "inline + 外链");

        // 后者覆盖前者：border 来自外链表，color 也应是 blue
        let sheet = resources.stylesheets.last().unwrap();
        let colors: Vec<_> = sheet
            .rules()
            .iter()
            .flat_map(|rule| rule.declarations())
            .filter(|declaration| declaration.property == nexty_css::PropertyId::Color)
            .collect();
        assert!(!colors.is_empty(), "外链表的颜色声明应保留");
    }

    #[test]
    fn failing_stylesheet_is_skipped_without_blocking_others() {
        let fetcher = MockFetcher::new();
        fetcher.serve("https://page.test/ok.css", b"div { margin: 0 }");
        fetcher.fail("https://page.test/broken.css", 500);
        let document = document(
            "<head>\
             <link rel=stylesheet href=broken.css>\
             <style>p { color: red }</style>\
             <link rel=stylesheet href=ok.css>\
             </head><body><p>x</p></body>",
        );

        let resources = fetch_subresources(&fetcher, BASE, &document);
        assert_eq!(
            resources.stylesheets.len(),
            2,
            "失败的表跳过，其余两张按序保留"
        );
    }

    #[test]
    fn images_decode_and_map_to_nodes_deduped_by_url() {
        let fetcher = MockFetcher::new();
        fetcher.serve("https://page.test/logo.png", &tiny_png());
        let document = document("<body><img src=logo.png><p>x</p><img src=logo.png></body>");

        let resources = fetch_subresources(&fetcher, BASE, &document);
        assert_eq!(resources.images.len(), 2, "两个节点都有像素");
        // URL 只抓一次（去重）
        assert_eq!(
            fetcher
                .hits
                .lock()
                .unwrap()
                .iter()
                .filter(|url| **url == "https://page.test/logo.png")
                .count(),
            1
        );
    }

    #[test]
    fn failing_or_undecodable_image_degrades_to_missing_pixels() {
        let fetcher = MockFetcher::new();
        fetcher.fail("https://page.test/lost.png", 404);
        fetcher.serve("https://page.test/junk.png", b"not an image");
        let document = document("<body><img src=lost.png><img src=junk.png></body>");

        let resources = fetch_subresources(&fetcher, BASE, &document);
        assert!(resources.images.is_empty(), "失败图片不产生像素");
    }

    #[test]
    fn relative_urls_resolve_against_document_base() {
        let fetcher = MockFetcher::new();
        // /dir/page.html 的 ../css/a.css → https://page.test/css/a.css
        fetcher.serve("https://page.test/css/a.css", b"body { margin: 0 }");
        let document =
            document("<head><link rel=stylesheet href=\"../css/a.css\"></head><body></body>");
        let resources = fetch_subresources(&fetcher, "https://page.test/dir/page.html", &document);
        assert_eq!(resources.stylesheets.len(), 1);
        assert!(
            fetcher
                .hits
                .lock()
                .unwrap()
                .contains(&"https://page.test/css/a.css".to_owned()),
            "相对 URL 按 base 解析"
        );
    }
}
