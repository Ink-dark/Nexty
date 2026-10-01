//! Nexty 网络 facade。
//!
//! 上游：`reqwest`（MIT/Apache-2.0），提供 TLS、重定向与连接池。
//! 决策见 `docs/decisions/2026-10-01-crate-selection.md`。
//!
//! 接口当前是同步的：导航与子资源抓取由 chrome 层串行驱动，异步入口等到确有
//! 并发需求时再加。
//!
//! 上游类型一律不得出现在本 crate 的 pub 导出中（AGENTS.md 硬规则）。

#![forbid(unsafe_code)]

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
    /// 响应头，保持原始顺序。
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkError {
    /// URL 无法解析，或协议不受支持。
    InvalidUrl,
    /// 连接或传输失败。
    Transport,
    /// 超过超时限制。
    Timeout,
}
