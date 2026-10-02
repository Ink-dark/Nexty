//! Nexty 网络 facade。
//!
//! 上游：`reqwest`（MIT/Apache-2.0），提供 TLS、重定向与连接池。
//! 决策见 `docs/decisions/2026-10-01-crate-selection.md`。
//!
//! 接口当前是同步的：导航与子资源抓取由 chrome 层串行驱动，异步入口等到确有
//! 并发需求时再加。
//!
//! 上游类型一律不得出现在本 crate 的 pub 导出中（AGENTS.md 硬规则）；
//! [`ReqwestFetcher`] 持有上游 client，对外只暴露 [`NetworkFetcher`] trait 与
//! 自有请求 / 响应类型。

#![forbid(unsafe_code)]

use std::time::Duration;

/// HTTP 方法。当前只覆盖导航与子资源抓取所需的最小集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// GET。
    Get,
    /// HEAD。
    Head,
}

/// 一次请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// 绝对 URL。
    pub url: String,
    /// 请求方法。
    pub method: Method,
}

/// 一次响应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// HTTP 状态码。
    pub status: u16,
    /// 响应头（reqwest 的头表不保序，条目完整但顺序与源报文无关）。
    pub headers: Vec<(String, String)>,
    /// 响应体字节。
    pub body: Vec<u8>,
}

/// 抓取后端。
///
/// 实现方负责 TLS、重定向与连接池等细节，只对外暴露本 crate 的类型。
pub trait NetworkFetcher {
    /// 执行一次请求。
    ///
    /// # Errors
    ///
    /// URL 非法、连接失败或超时时返回 [`NetworkError`]。
    fn fetch(&self, request: &Request) -> Result<Response, NetworkError>;
}

/// 抓取错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkError {
    /// URL 无法解析，或协议不受支持。
    InvalidUrl,
    /// 连接或传输失败。
    Transport,
    /// 超过超时限制。
    Timeout,
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
pub fn resolve(base: &str, target: &str) -> Result<String, NetworkError> {
    let base = reqwest::Url::parse(base).map_err(|_| NetworkError::InvalidUrl)?;
    // 空 target 解析为 base 自身。opaque scheme（about: / data:）的
    // `join("")` 会失败（无 hierarchical path 可继承），这里显式短路。
    if target.is_empty() {
        return Ok(base.into());
    }
    let resolved = base.join(target).map_err(|_| NetworkError::InvalidUrl)?;
    Ok(resolved.into())
}

/// [`NetworkFetcher`] 的 reqwest 实现。
///
/// 重定向跟随使用 reqwest 默认策略（最多 10 次）；TLS 使用 reqwest 默认的
/// rustls 配置。[`ReqwestFetcher::with_timeout`] 可设置整请求超时，超时映射为
/// [`NetworkError::Timeout`]。
#[derive(Debug)]
pub struct ReqwestFetcher {
    client: reqwest::blocking::Client,
}

impl ReqwestFetcher {
    /// 创建使用 reqwest 默认配置（无超时限制）的抓取器。
    ///
    /// # Errors
    ///
    /// reqwest client 初始化失败（TLS 后端不可用等）时返回 [`NetworkError::Transport`]。
    pub fn new() -> Result<Self, NetworkError> {
        Ok(Self {
            client: reqwest::blocking::Client::builder()
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
                .timeout(timeout)
                .build()
                .map_err(transport_from_builder)?,
        })
    }
}

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
        let response = self
            .client
            .request(method, url)
            .send()
            .map_err(map_request_error)?;

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
        let body = response.bytes().map_err(map_request_error)?.to_vec();

        Ok(Response {
            status,
            headers,
            body,
        })
    }
}

/// client 构造失败归入传输错误。
fn transport_from_builder(_error: reqwest::Error) -> NetworkError {
    NetworkError::Transport
}

/// reqwest 请求错误 → 自有错误。
fn map_request_error(error: reqwest::Error) -> NetworkError {
    if error.is_timeout() {
        NetworkError::Timeout
    } else {
        NetworkError::Transport
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread::JoinHandle;

    /// 起一个只应答一次的本地 HTTP 服务，返回其地址。
    fn serve_once(response: &'static [u8]) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer);
                let _ = stream.write_all(response);
                let _ = stream.flush();
            }
        });
        (format!("http://127.0.0.1:{port}/"), handle)
    }

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
        let (url, server) = serve_once(response_bytes);

        let response = fetcher()
            .fetch(&Request {
                url,
                method: Method::Get,
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
        server.join().expect("server thread");
    }

    #[test]
    fn head_request_sends_head_method() {
        // 校验请求行确实是 HEAD：服务端回读请求首行
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buffer = [0u8; 4096];
            let read = stream.read(&mut buffer).expect("read");
            let request_line = String::from_utf8_lossy(&buffer[..read]).into_owned();
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            request_line
        });

        let response = fetcher()
            .fetch(&Request {
                url: format!("http://127.0.0.1:{port}/"),
                method: Method::Head,
            })
            .expect("fetch");

        assert_eq!(response.status, 200);
        assert!(response.body.is_empty());
        let request_line = server.join().expect("server thread");
        assert!(
            request_line.starts_with("HEAD / "),
            "request line: {request_line}"
        );
    }

    #[test]
    fn invalid_url_maps_to_invalid_url() {
        let error = fetcher()
            .fetch(&Request {
                url: "not a url".to_owned(),
                method: Method::Get,
            })
            .expect_err("invalid url");
        assert_eq!(error, NetworkError::InvalidUrl);

        // 协议不受支持同样归入 InvalidUrl
        let error = fetcher()
            .fetch(&Request {
                url: "ftp://example.test/file".to_owned(),
                method: Method::Get,
            })
            .expect_err("unsupported scheme");
        assert_eq!(error, NetworkError::InvalidUrl);
    }

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
                })
                .is_err()
        );
    }
}
