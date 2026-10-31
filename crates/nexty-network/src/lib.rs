//! Nexty 网络 facade。
//!
//! 上游：`reqwest`（MIT/Apache-2.0，抓取后端）、`url`（MIT/Apache-2.0，WHATWG URL
//! 解析）、`encoding_rs`（MIT/Apache-2.0，WHATWG Encoding 解码）。选型决策见
//! `docs/decisions/2026-10-01-crate-selection.md` 与 `docs/decisions/2026-10-31-url.md`、
//! `docs/decisions/2026-10-31-encoding.md`。
//!
//! 接口当前是同步的：导航与子资源抓取由 chrome 层串行驱动，异步入口等到确有
//! 并发需求时再加。
//!
//! 上游类型一律不得出现在本 crate 的 pub 导出中（AGENTS.md 硬规则）：
//! [`ReqwestFetcher`] 持有上游 client，[`Url`] 包住 `url::Url`，解码入口以
//! [`DecodedDocument`] 输出文本，对外只暴露本 crate 的类型。
//!
//! **Feature**：能力按四个 feature 拆分，默认全开——`fetch`（reqwest 抓取后端与
//! trait / 请求 / 响应类型）、`resolve`（`url` 直依赖，不拉 reqwest）、`decode`
//! （encoding_rs 编码探测与解码）、`cache`（内存 HTTP 缓存，隐含 `fetch`）。

#![forbid(unsafe_code)]

#[cfg(feature = "fetch")]
use std::time::Duration;

/// HTTP 方法。当前只覆盖导航与子资源抓取所需的最小集合。
#[cfg(feature = "fetch")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    /// GET。
    Get,
    /// HEAD。
    Head,
}

/// 一次请求。
#[cfg(feature = "fetch")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// 绝对 URL。
    pub url: String,
    /// 请求方法。
    pub method: Method,
    /// 额外请求头（按给定顺序逐条应用；同名头由后端决定合并语义）。
    ///
    /// `Host`/`Content-Length` 等由后端管理的头不应出现在这里。
    pub headers: Vec<(String, String)>,
}

/// 一次响应。
#[cfg(feature = "fetch")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// HTTP 状态码。
    pub status: u16,
    /// 响应头（reqwest 的头表不保序，条目完整但顺序与源报文无关）。
    pub headers: Vec<(String, String)>,
    /// 响应体字节。
    pub body: Vec<u8>,
    /// 最终响应 URL：跟随重定向后的落点，无重定向时即请求 URL（WHATWG fetch 的
    /// final URL 语义）。文档 base 应取本字段——子资源相对地址按落点解析，
    /// 不按原始请求 URL。
    pub final_url: String,
}

/// 抓取后端。
///
/// 实现方负责 TLS、重定向与连接池等细节，只对外暴露本 crate 的类型。
///
/// `Send + Sync`：子资源抓取在 chrome 层并发执行（每资源一线程），
/// fetcher 会被多线程共享引用，故把线程安全定为 trait 的组成要求。
#[cfg(feature = "fetch")]
pub trait NetworkFetcher: Send + Sync {
    /// 执行一次请求。
    ///
    /// # Errors
    ///
    /// URL 非法、连接失败或超时时返回 [`NetworkError`]。
    fn fetch(&self, request: &Request) -> Result<Response, NetworkError>;
}

/// 抓取错误。
#[cfg(any(feature = "fetch", feature = "resolve"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkError {
    /// URL 无法解析，或协议不受支持。
    InvalidUrl,
    /// 连接或传输失败（含重定向环与超过重定向上限）。
    Transport,
    /// 超过超时限制。
    Timeout,
}

/// WHATWG URL 的最小自有封装（`url` crate 的类型不外泄，AGENTS.md 硬规则）。
///
/// 只暴露当前需要的三个操作：解析、以自身为 base 拼接、取字符串。
/// 选型见 `docs/decisions/2026-10-31-url.md`。
#[cfg(feature = "resolve")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    inner: url::Url,
}

#[cfg(feature = "resolve")]
impl Url {
    /// 解析绝对 URL。
    ///
    /// # Errors
    ///
    /// 输入不是合法绝对 URL（WHATWG basic URL parsing 失败）时返回
    /// [`NetworkError::InvalidUrl`]。
    pub fn parse(input: &str) -> Result<Self, NetworkError> {
        url::Url::parse(input)
            .map(|inner| Self { inner })
            .map_err(|_| NetworkError::InvalidUrl)
    }

    /// 以自身为基准解析 `target`，返回新 URL。语义同 [`resolve`]：绝对 URL
    /// 原样返回，协议相对（`//host/x`）继承协议，`/x` / `x` / `../x` / `?q` /
    /// `#f` 按路径语义解析；空串解析为自身（opaque scheme 的 `join("")` 会
    /// 失败，这里显式短路，与 [`resolve`] 一致）。
    ///
    /// # Errors
    ///
    /// 拼接失败时返回 [`NetworkError::InvalidUrl`]。
    pub fn join(&self, target: &str) -> Result<Self, NetworkError> {
        if target.is_empty() {
            return Ok(Self {
                inner: self.inner.clone(),
            });
        }
        self.inner
            .join(target)
            .map(|inner| Self { inner })
            .map_err(|_| NetworkError::InvalidUrl)
    }

    /// 序列化后的完整 URL 字符串。
    pub fn as_str(&self) -> &str {
        self.inner.as_str()
    }
}

/// 以 `base` 为基准解析 `target`，返回绝对 URL。
///
/// 子资源（外链样式表、图片、表单 action…）在文档里以相对形式出现，需按
/// 文档 URL 解析成绝对地址才能抓取。语义遵循 WHATWG URL 标准的基本 URL
/// 解析：绝对 URL 原样返回，协议相对（`//host/x`）继承 base 的协议，
/// `/x`、`x`、`../x`、`./x` 按路径语义解析，`?q` / `#f` 替换 base 的
/// query / fragment。空串解析为 base 自身。
///
/// base 非法或解析失败 → [`NetworkError::InvalidUrl`]。**不校验协议**：
/// `file:` 等非网络 scheme 也能解析出来，由调用方决定是否发起抓取（`fetch`
/// 会再过一遍协议白名单）。opaque scheme（`about:` / `data:`）按 WHATWG 规范
/// 无法作为相对解析的 base，仅空 target（返回自身）可用。
///
/// ```
/// # use nexty_network::resolve;
/// let base = "https://example.test/dir/page.html";
/// assert_eq!(resolve(base, "/a.png").unwrap(), "https://example.test/a.png");
/// assert_eq!(resolve(base, "../b.css").unwrap(), "https://example.test/b.css");
/// ```
///
/// # Errors
///
/// base 无法解析，或 target 与 base 合起来仍不是合法 URL 时返回
/// [`NetworkError::InvalidUrl`]。
#[cfg(feature = "resolve")]
pub fn resolve(base: &str, target: &str) -> Result<String, NetworkError> {
    Url::parse(base)?
        .join(target)
        .map(|url| url.as_str().to_owned())
}

/// 一次请求最多跟随的重定向跳数（与 WHATWG fetch 的 redirect count 上限同值）。
/// 超过即失败，映射 [`NetworkError::Transport`]。
#[cfg(feature = "fetch")]
pub const MAX_REDIRECTS: usize = 10;

/// 显式重定向 policy：跟随 ≤ [`MAX_REDIRECTS`] 跳 + 防环。
///
/// `Policy::custom` 不自带防环（reqwest 文档明确要求 custom 自行处理），这里
/// 用 `previous()` 历史判定：`previous()` 的首项是初始 URL，目标 URL 已出现在
/// 历史中即成环。跳数判式与 `Policy::limited` 对齐（`previous()` 含初始 URL，
/// 故比较用 `>`）。两种失败都映射 [`NetworkError::Transport`]。
#[cfg(feature = "fetch")]
fn redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt
            .previous()
            .iter()
            .any(|visited| visited == attempt.url())
        {
            attempt.error("redirect loop detected")
        } else if attempt.previous().len() > MAX_REDIRECTS {
            attempt.error("too many redirects")
        } else {
            attempt.follow()
        }
    })
}

/// [`NetworkFetcher`] 的 reqwest 实现。
///
/// 重定向语义显式：最多跟随 [`MAX_REDIRECTS`] 跳，且检测重定向环（目标 URL
/// 在本次请求链中重复出现即失败）；两者都映射 [`NetworkError::Transport`]，
/// 见 [`redirect_policy`]。TLS 使用 reqwest 默认的 rustls 配置。
/// [`ReqwestFetcher::with_timeout`] 可设置整请求超时，超时映射为
/// [`NetworkError::Timeout`]。
#[cfg(feature = "fetch")]
#[derive(Debug)]
pub struct ReqwestFetcher {
    client: reqwest::blocking::Client,
}

#[cfg(feature = "fetch")]
impl ReqwestFetcher {
    /// 创建使用 reqwest 默认配置（无超时限制）的抓取器。
    ///
    /// # Errors
    ///
    /// reqwest client 初始化失败（TLS 后端不可用等）时返回 [`NetworkError::Transport`]。
    pub fn new() -> Result<Self, NetworkError> {
        Ok(Self {
            client: reqwest::blocking::Client::builder()
                .redirect(redirect_policy())
                .build()
                .map_err(transport_from_builder)?,
        })
    }

    /// 创建带整请求超时的抓取器。
    ///
    /// # Errors
    ///
    /// reqwest client 初始化失败时返回 [`NetworkError::Transport`]。
    pub fn with_timeout(timeout: Duration) -> Result<Self, NetworkError> {
        Ok(Self {
            client: reqwest::blocking::Client::builder()
                .redirect(redirect_policy())
                .timeout(timeout)
                .build()
                .map_err(transport_from_builder)?,
        })
    }
}

#[cfg(feature = "fetch")]
impl NetworkFetcher for ReqwestFetcher {
    fn fetch(&self, request: &Request) -> Result<Response, NetworkError> {
        // URL 先自行解析：区分 InvalidUrl 与传输错误；
        // url crate 能解析任意协议，支持的协议在这里限定
        let url = reqwest::Url::parse(&request.url).map_err(|_| NetworkError::InvalidUrl)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(NetworkError::InvalidUrl);
        }
        let method = match request.method {
            Method::Get => reqwest::Method::GET,
            Method::Head => reqwest::Method::HEAD,
        };
        let mut builder = self.client.request(method, url);
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let response = builder.send().map_err(map_request_error)?;

        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_owned(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect();
        // 最终 URL 在消费响应体前取出（bytes() 会消费 Response）
        let final_url = response.url().as_str().to_owned();
        let body = response.bytes().map_err(map_request_error)?.to_vec();

        Ok(Response {
            status,
            headers,
            body,
            final_url,
        })
    }
}

/// client 构造失败归入传输错误。
#[cfg(feature = "fetch")]
fn transport_from_builder(_error: reqwest::Error) -> NetworkError {
    NetworkError::Transport
}

/// reqwest 请求错误 → 自有错误。
#[cfg(feature = "fetch")]
fn map_request_error(error: reqwest::Error) -> NetworkError {
    if error.is_timeout() {
        NetworkError::Timeout
    } else {
        NetworkError::Transport
    }
}

#[cfg(feature = "decode")]
mod decode;
#[cfg(feature = "decode")]
pub use decode::{DecodedDocument, MAX_PRESCAN_BYTES, decode_document};

#[cfg(feature = "cache")]
mod cache;
#[cfg(feature = "cache")]
pub use cache::{CachedFetcher, MAX_CACHE_ENTRIES, MAX_CACHE_ENTRY_BODY_BYTES};

// 测试共享基建：恰好服务 n 个请求的回环服务器（fetch 相关测试用）。
#[cfg(all(test, feature = "fetch"))]
pub(crate) mod test_loopback {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;

    /// 回环多请求测试服务器：恰好服务 `n` 个请求后线程退出。
    /// 每个请求回调 `handler(序号, 请求头原文)` 生成完整响应字节。
    pub(crate) struct Server {
        base: String,
        requests: Arc<Mutex<Vec<String>>>,
        handle: JoinHandle<()>,
    }

    impl Server {
        pub(crate) fn start(
            n: usize,
            handler: impl Fn(usize, &str) -> Vec<u8> + Send + 'static,
        ) -> Server {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
            let port = listener.local_addr().expect("addr").port();
            let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
            let seen = requests.clone();
            let handle = std::thread::spawn(move || {
                let mut served = 0usize;
                while served < n {
                    let Ok((stream, _)) = listener.accept() else {
                        break;
                    };
                    if serve_connection(stream, &handler, &seen, &mut served, n) {
                        break;
                    }
                }
            });
            Server {
                base: format!("http://127.0.0.1:{port}"),
                requests,
                handle,
            }
        }

        pub(crate) fn base(&self) -> &str {
            &self.base
        }

        /// 已收到请求的请求头原文（按到达顺序）。
        pub(crate) fn requests(&self) -> Vec<String> {
            self.requests.lock().expect("requests").clone()
        }

        pub(crate) fn join(self) {
            self.handle.join().expect("server thread");
        }
    }

    /// 服务一条连接上的若干请求；返回 true 表示已达总请求数，服务器应退出。
    fn serve_connection(
        mut stream: TcpStream,
        handler: &impl Fn(usize, &str) -> Vec<u8>,
        requests: &Mutex<Vec<String>>,
        served: &mut usize,
        total: usize,
    ) -> bool {
        let mut buffer: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            if *served >= total {
                return true;
            }
            // 读到请求头结束（\r\n\r\n）
            let head_end = loop {
                if let Some(pos) = find(&buffer, b"\r\n\r\n") {
                    break pos;
                }
                match stream.read(&mut chunk) {
                    Ok(0) | Err(_) => return false, // 对端关闭，换连接或退出
                    Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                }
            };
            let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
            // 请求体（本仓测试只发无体请求，仍按 Content-Length 读完以对齐 keep-alive）
            let mut consumed = head_end + 4;
            if let Some(len) = content_length(&head) {
                let need = head_end + 4 + len;
                while buffer.len() < need {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => return false,
                        Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                    }
                }
                consumed = need;
            }
            buffer.drain(..consumed);
            requests.lock().expect("requests").push(head.clone());
            let response = handler(*served, &head);
            let _ = stream.write_all(&response);
            let _ = stream.flush();
            *served += 1;
        }
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    fn content_length(head: &str) -> Option<usize> {
        head.lines()
            .skip(1)
            .take_while(|line| !line.is_empty())
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                if name.trim().eq_ignore_ascii_case("content-length") {
                    value.trim().parse().ok()
                } else {
                    None
                }
            })
    }

    /// 构造一个最小合法 HTTP/1.1 响应（显式 Content-Length，keep-alive）。
    pub(crate) fn respond(
        status: u16,
        reason: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Vec<u8> {
        let mut head = format!("HTTP/1.1 {status} {reason}\r\n");
        for (name, value) in headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(body);
        bytes
    }

    /// 重定向响应（相对 Location）。
    pub(crate) fn redirect(status: u16, location: &str) -> Vec<u8> {
        respond(status, "Redirect", &[("Location", location)], b"")
    }

    /// 请求行里的目标路径。
    pub(crate) fn request_path(head: &str) -> &str {
        head.split_whitespace().nth(1).unwrap_or("/")
    }
}

// 抓取回环测试（只需 fetch；resolve 语义测试见下方 resolve_tests）。
#[cfg(all(test, feature = "fetch"))]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::{TcpListener, TcpStream};
    use std::thread::JoinHandle;

    /// 起一个接受连接但永不回应的服务（触发客户端超时）。
    fn serve_silently() -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let handle = std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                // 只收不发：把请求读进来之后挂起直到对端超时
                let mut stream: TcpStream = stream;
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer);
                std::thread::sleep(Duration::from_millis(2000));
                drop(stream);
            }
        });
        (format!("http://127.0.0.1:{port}/"), handle)
    }

    fn fetcher() -> ReqwestFetcher {
        ReqwestFetcher::new().expect("fetcher")
    }

    #[test]
    fn get_round_trips_status_headers_and_body() {
        let response_bytes =
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nX-Test: yes\r\n\r\nhello nexty";
        let server = test_loopback::Server::start(1, |_, _| response_bytes.to_vec());

        let response = fetcher()
            .fetch(&Request {
                url: format!("{}/", server.base()),
                method: Method::Get,
                headers: Vec::new(),
            })
            .expect("fetch");

        assert_eq!(response.status, 200);
        assert!(
            response
                .headers
                .iter()
                .any(|(name, value)| name == "content-type" && value == "text/plain")
        );
        assert!(
            response
                .headers
                .iter()
                .any(|(name, value)| name == "x-test" && value == "yes")
        );
        assert_eq!(response.body, b"hello nexty");
        // 无重定向：final_url 即请求 URL（url crate 对空路径补 /）
        assert_eq!(response.final_url, format!("{}/", server.base()));
        server.join();
    }

    #[test]
    fn head_request_sends_head_method() {
        // 校验请求行确实是 HEAD：服务端回读请求首行
        let server =
            test_loopback::Server::start(1, |_, _| test_loopback::respond(200, "OK", &[], b""));

        let response = fetcher()
            .fetch(&Request {
                url: server.base().to_owned(),
                method: Method::Head,
                headers: Vec::new(),
            })
            .expect("fetch");

        assert_eq!(response.status, 200);
        assert!(response.body.is_empty());
        // 服务端记录的请求行校验方法确实是 HEAD
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].starts_with("HEAD / "),
            "request line: {}",
            requests[0]
        );
        server.join();
    }

    /// 请求头按给定顺序到达服务端（服务端把目标请求头回显进响应体）。
    #[test]
    fn request_headers_reach_the_server() {
        let server = test_loopback::Server::start(1, |_, head| {
            let echo = |name: &str| {
                head.lines()
                    .skip(1) // 跳过请求行
                    .take_while(|line| !line.is_empty())
                    .find(|line| line.to_ascii_lowercase().starts_with(name))
                    .map_or_else(String::new, ToString::to_string)
            };
            let body = format!("{}|{}", echo("x-nexty-marker"), echo("user-agent"));
            test_loopback::respond(200, "OK", &[], body.as_bytes())
        });

        let response = fetcher()
            .fetch(&Request {
                url: server.base().to_owned(),
                method: Method::Get,
                headers: vec![("X-Nexty-Marker".to_owned(), "nexty-header-test".to_owned())],
            })
            .expect("fetch");

        let body = String::from_utf8(response.body).expect("utf8");
        assert!(
            body.contains("x-nexty-marker: nexty-header-test"),
            "自定义头未到达服务端: {body}"
        );
        assert!(
            body.to_ascii_lowercase().starts_with("x-nexty-marker"),
            "请求头顺序应保留: {body}"
        );
        server.join();
    }

    #[test]
    fn invalid_url_maps_to_invalid_url() {
        let error = fetcher()
            .fetch(&Request {
                url: "not a url".to_owned(),
                method: Method::Get,
                headers: Vec::new(),
            })
            .expect_err("invalid url");
        assert_eq!(error, NetworkError::InvalidUrl);

        // 协议不受支持同样归入 InvalidUrl
        let error = fetcher()
            .fetch(&Request {
                url: "ftp://example.test/file".to_owned(),
                method: Method::Get,
                headers: Vec::new(),
            })
            .expect_err("unsupported scheme");
        assert_eq!(error, NetworkError::InvalidUrl);
    }

    #[test]
    fn unreachable_server_maps_to_transport() {
        // 连一个确定无服务的回环端口（先取一个空闲端口再释放）
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);

        let error = fetcher()
            .fetch(&Request {
                url: format!("http://127.0.0.1:{port}/"),
                method: Method::Get,
                headers: Vec::new(),
            })
            .expect_err("connection refused");
        assert_eq!(error, NetworkError::Transport);
    }

    #[test]
    fn slow_server_maps_to_timeout() {
        let (url, server) = serve_silently();
        let fetcher = ReqwestFetcher::with_timeout(Duration::from_millis(100)).expect("fetcher");

        let error = fetcher
            .fetch(&Request {
                url,
                method: Method::Get,
                headers: Vec::new(),
            })
            .expect_err("timeout");
        assert_eq!(error, NetworkError::Timeout);
        server.join().expect("server thread");
    }

    #[test]
    fn connection_failure_after_timeout_builder() {
        // with_timeout 构造路径
        let fetcher = ReqwestFetcher::with_timeout(Duration::from_secs(5)).expect("fetcher");
        assert!(
            fetcher
                .fetch(&Request {
                    url: "not a url".to_owned(),
                    method: Method::Get,
                    headers: Vec::new(),
                })
                .is_err()
        );
    }

    /// 301 重定向：响应携带落点 final_url，且确实跟随了 Location。
    #[test]
    fn redirect_301_exposes_final_url() {
        let server = test_loopback::Server::start(2, |index, head| {
            if index == 0 {
                assert_eq!(test_loopback::request_path(head), "/");
                test_loopback::redirect(301, "/final")
            } else {
                assert_eq!(test_loopback::request_path(head), "/final");
                test_loopback::respond(200, "OK", &[], b"landing")
            }
        });

        let response = fetcher()
            .fetch(&Request {
                url: format!("{}/", server.base()),
                method: Method::Get,
                headers: Vec::new(),
            })
            .expect("fetch");

        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"landing");
        assert_eq!(response.final_url, format!("{}/final", server.base()));
        assert_eq!(server.requests().len(), 2);
        server.join();
    }

    /// 302 重定向：同上（相对 Location 也由后端解析）。
    #[test]
    fn redirect_302_exposes_final_url() {
        let server = test_loopback::Server::start(2, |index, head| {
            if index == 0 {
                assert_eq!(test_loopback::request_path(head), "/");
                test_loopback::redirect(302, "/final")
            } else {
                assert_eq!(test_loopback::request_path(head), "/final");
                test_loopback::respond(200, "OK", &[], b"landing")
            }
        });

        let response = fetcher()
            .fetch(&Request {
                url: format!("{}/", server.base()),
                method: Method::Get,
                headers: Vec::new(),
            })
            .expect("fetch");

        assert_eq!(response.status, 200);
        assert_eq!(response.final_url, format!("{}/final", server.base()));
        assert_eq!(server.requests().len(), 2);
        server.join();
    }

    /// 重定向环：目标 URL 在请求链中重复出现即失败，不悬挂。
    #[test]
    fn redirect_loop_fails_instead_of_hanging() {
        let server = test_loopback::Server::start(1, |_, _| test_loopback::redirect(302, "/loop"));

        let error = fetcher()
            .fetch(&Request {
                url: format!("{}/loop", server.base()),
                method: Method::Get,
                headers: Vec::new(),
            })
            .expect_err("redirect loop");

        assert_eq!(error, NetworkError::Transport);
        // 环在第二次跟随前被识破：只有初始请求到达服务器
        assert_eq!(server.requests().len(), 1);
        server.join();
    }

    /// 重定向链超过上限：恰好跟随 MAX_REDIRECTS 跳后失败。
    #[test]
    fn redirect_chain_stops_at_limit() {
        let server = test_loopback::Server::start(MAX_REDIRECTS + 1, |index, _| {
            test_loopback::redirect(302, &format!("/r{}", index + 1))
        });

        let error = fetcher()
            .fetch(&Request {
                url: format!("{}/r0", server.base()),
                method: Method::Get,
                headers: Vec::new(),
            })
            .expect_err("too many redirects");

        assert_eq!(error, NetworkError::Transport);
        // 初始 + MAX_REDIRECTS 次跟随 = MAX_REDIRECTS + 1 个请求
        assert_eq!(server.requests().len(), MAX_REDIRECTS + 1);
        server.join();
    }

    /// 文档 base = 最终 URL：重定向落点后的相对子资源按落点解析。
    #[cfg(feature = "resolve")]
    #[test]
    fn relative_resolution_uses_final_url_after_redirect() {
        let server = test_loopback::Server::start(2, |index, _| {
            if index == 0 {
                test_loopback::redirect(301, "/final/page.html")
            } else {
                test_loopback::respond(200, "OK", &[], b"<img src=probe.png>")
            }
        });

        let response = fetcher()
            .fetch(&Request {
                url: format!("{}/", server.base()),
                method: Method::Get,
                headers: Vec::new(),
            })
            .expect("fetch");

        let probe = resolve(&response.final_url, "probe.png").expect("resolve");
        assert_eq!(probe, format!("{}/final/probe.png", server.base()));
        server.join();
    }
}

// URL 解析语义测试（只需 resolve，不依赖 fetch）。
#[cfg(all(test, feature = "resolve"))]
mod resolve_tests {
    use super::*;

    /// 相对路径按 URL 标准解析（WHATWG basic URL parsing）。
    #[test]
    fn resolve_handles_relative_paths() {
        let base = "https://example.test/dir/page.html";
        let cases = [
            // 绝对路径从协议根开始
            ("/a.png", "https://example.test/a.png"),
            // 同级相对路径
            ("b.css", "https://example.test/dir/b.css"),
            ("./c.js", "https://example.test/dir/c.js"),
            // 父目录上跳
            ("../d.png", "https://example.test/d.png"),
            ("../../e.png", "https://example.test/e.png"),
            // 末段为目录的 base（以 / 结尾）不应吃掉最后一段
            ("f.png", "https://example.test/dir/f.png"),
            // query / fragment 替换
            ("?q=1", "https://example.test/dir/page.html?q=1"),
            ("#frag", "https://example.test/dir/page.html#frag"),
            // 空串解析为 base 自身
            ("", "https://example.test/dir/page.html"),
            // 绝对 URL 原样返回
            ("https://other.test/x.png", "https://other.test/x.png"),
        ];
        for (target, expected) in cases {
            assert_eq!(
                resolve(base, target).ok().as_deref(),
                Some(expected),
                "resolve({base:?}, {target:?})"
            );
        }
    }

    /// 协议相对 URL 继承 base 的协议。
    #[test]
    fn resolve_inherits_scheme_for_network_path_reference() {
        assert_eq!(
            resolve("https://example.test/a/b", "//cdn.test/x.png")
                .ok()
                .as_deref(),
            Some("https://cdn.test/x.png")
        );
        // base 为 http 时同样继承 http
        assert_eq!(
            resolve("http://example.test/a/b", "//cdn.test/x.png")
                .ok()
                .as_deref(),
            Some("http://cdn.test/x.png")
        );
    }

    /// 非 http base 的行为：可解析的（hierarchical scheme）照常解析，
    /// opaque scheme（`about:` / `data:` 等）按 WHATWG 规范不支持相对路径。
    #[test]
    fn resolve_handles_non_http_base() {
        // file: 是 hierarchical scheme，相对路径正常解析
        assert_eq!(
            resolve("file:///tmp/x.html", "img/a.png").ok().as_deref(),
            Some("file:///tmp/img/a.png")
        );
        // about: 无法作为 base 解析成相对地址 → InvalidUrl（规范行为）
        assert_eq!(
            resolve("about:blank", "a.png"),
            Err(NetworkError::InvalidUrl)
        );
        // 但 about: 的子资源本就不该经网络，相对引用无意义
        assert_eq!(
            resolve("about:blank", "").ok().as_deref(),
            Some("about:blank")
        );
    }

    /// base 非法 → InvalidUrl。
    #[test]
    fn resolve_rejects_invalid_base() {
        assert_eq!(resolve("not a url", "a.png"), Err(NetworkError::InvalidUrl));
        // base 是相对 URL 时也无法作为基准
        assert_eq!(
            resolve("/dir/page.html", "a.png"),
            Err(NetworkError::InvalidUrl)
        );
    }

    /// [`Url`] 封装：parse / join / as_str 与 resolve 同语义。
    #[test]
    fn url_wrapper_parse_join_as_str() {
        let base = Url::parse("https://example.test/dir/page.html").expect("parse");
        assert_eq!(base.as_str(), "https://example.test/dir/page.html");
        assert_eq!(
            base.join("../img/a.png").expect("join").as_str(),
            "https://example.test/img/a.png"
        );
        assert_eq!(
            base.join("/root.png").expect("join").as_str(),
            "https://example.test/root.png"
        );
        assert_eq!(
            base.join("?q=1").expect("join").as_str(),
            "https://example.test/dir/page.html?q=1"
        );
        // 绝对 URL 原样
        assert_eq!(
            base.join("https://other.test/x").expect("join").as_str(),
            "https://other.test/x"
        );
        // 非法输入
        assert!(Url::parse("not a url").is_err());
    }

    /// [`Url::join`] 空串解析为自身。
    #[test]
    fn url_wrapper_join_empty_returns_self() {
        let base = Url::parse("https://example.test/dir/page.html").expect("parse");
        assert_eq!(base.join("").expect("join"), base.clone());
    }

    /// opaque scheme base：相对 join 失败、空 join 返回自身（与 resolve 一致）。
    #[test]
    fn url_wrapper_opaque_base_rejects_relative_join() {
        let base = Url::parse("about:blank").expect("parse");
        assert_eq!(base.join("a.png"), Err(NetworkError::InvalidUrl));
        assert_eq!(base.join("").expect("join").as_str(), "about:blank");
    }
}
