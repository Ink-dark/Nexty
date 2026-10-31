//! 内存级 HTTP 缓存（feature `cache`，隐含 `fetch`）。
//!
//! [`CachedFetcher`] 装饰任意 [`NetworkFetcher`]，对 GET / HEAD 的 200 响应做
//! 内存缓存：
//! - `Cache-Control: max-age=N`——新鲜度内直接复用；`no-store`——不存；
//!   `no-cache`——强制再验证（即便仍新鲜）；
//! - `ETag` → 条件请求 `If-None-Match`，`Last-Modified` → `If-Modified-Since`
//!   （两者都在则都发）；服务器 304 时复用缓存体、刷新新鲜度与 validator；
//! - `Vary` 含 `User-Agent` 以外字段（或 `*`）→ 整条不缓存；仅 `User-Agent`
//!   时记录填充请求的 UA，复用要求 UA 一致；
//! - 无新鲜度且无 validator（且非 no-cache）的 200 不缓存（简化档不认
//!   `Expires`）；
//! - 单条响应体超过 [`MAX_CACHE_ENTRY_BODY_BYTES`] 不缓存；条目数达
//!   [`MAX_CACHE_ENTRIES`] 时按 LRU（最近使用时间戳）逐出。
//!
//! 与 RFC 9111 的偏差（简化档）：只认 `max-age`，不认 `Expires` / `Age` /
//! `s-maxage` / `must-revalidate`；字段限定的 `no-cache="field"` 指令不识别；
//! 再验证请求失败（网络错误）直接上抛，不回退过期响应；并发未命中不做请求
//! 合并（后写覆盖先写）；缓存体按值克隆返回，无共享引用计数。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::{Method, NetworkError, NetworkFetcher, Request, Response};

/// 缓存条目数上限；超出时逐出最久未使用的条目（LRU）。
pub const MAX_CACHE_ENTRIES: usize = 64;

/// 单条响应体缓存上限：超过此大小的响应不缓存（巨响应不进内存；
/// 与抓取层的字节预算语义一致，分配失败是 abort，panic 兜底兜不住）。
pub const MAX_CACHE_ENTRY_BODY_BYTES: usize = 20 * 1024 * 1024;

/// 内存级 HTTP 缓存装饰器：包住任意 [`NetworkFetcher`]，自身也是
/// [`NetworkFetcher`]（`Send + Sync`，可直接 `Arc` 后给并发抓取共享）。
///
/// 缓存键为（method, url）；语义清单见模块文档。
///
/// ```
/// use nexty_network::{CachedFetcher, NetworkError, NetworkFetcher, Request, Response};
///
/// struct NullFetcher;
/// impl NetworkFetcher for NullFetcher {
///     fn fetch(&self, _request: &Request) -> Result<Response, NetworkError> {
///         Ok(Response {
///             status: 200,
///             headers: Vec::new(),
///             body: Vec::new(),
///             final_url: String::new(),
///         })
///     }
/// }
///
/// let fetcher = CachedFetcher::new(NullFetcher);
/// ```
pub struct CachedFetcher<F: NetworkFetcher> {
    inner: F,
    state: Mutex<CacheState>,
}

struct CacheState {
    entries: HashMap<(Method, String), CacheEntry>,
    /// 逻辑时钟：每次访问 / 写入递增，用作 LRU 时间戳。
    clock: u64,
}

struct CacheEntry {
    /// 缓存的完整表示（status 200 的响应，含头、体与 final_url）。
    response: Response,
    /// 新鲜度起点（写入或最近一次 304 刷新时刻）。
    stored_at: Instant,
    /// `Cache-Control: max-age` 的新鲜度寿命。
    max_age: Option<Duration>,
    /// `Cache-Control: no-cache`：复用前强制再验证。
    must_revalidate: bool,
    etag: Option<String>,
    last_modified: Option<String>,
    /// `Vary: User-Agent` 时的 UA 约束：外层 `Some` 表示设了约束，内层
    /// `None` 表示填充请求本就无 UA 头；复用要求 UA 一致。
    vary_ua: Option<Option<String>>,
    /// 最近一次被使用（命中 / 再验证 / 写入）的逻辑时钟值。
    last_used: u64,
}

impl<F: NetworkFetcher> CachedFetcher<F> {
    /// 创建缓存装饰器。
    pub fn new(inner: F) -> Self {
        Self {
            inner,
            state: Mutex::new(CacheState {
                entries: HashMap::new(),
                clock: 0,
            }),
        }
    }

    /// 锁住缓存状态；锁中毒（他线程持锁时 panic）归入传输错误，不让缓存
    /// 层 panic 上抛。
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, CacheState>, NetworkError> {
        self.state.lock().map_err(|_| NetworkError::Transport)
    }

    /// 按缓存策略决定是否存下一次响应；可存则写入并做 LRU 逐出。
    fn store(&self, key: &(Method, String), request: &Request, response: &Response) {
        let Some(mut entry) = build_entry(request, response) else {
            return;
        };
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.clock += 1;
        entry.last_used = state.clock;
        while state.entries.len() >= MAX_CACHE_ENTRIES {
            let Some(victim) = state
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            state.entries.remove(&victim);
        }
        state.entries.insert(key.clone(), entry);
    }
}

/// 一次查询的处置：直接命中 / 带条件头再验证 / 未命中重抓。
enum Action {
    Serve(Response),
    Revalidate {
        if_none_match: Option<String>,
        if_modified_since: Option<String>,
    },
    Miss,
}

impl<F: NetworkFetcher> NetworkFetcher for CachedFetcher<F> {
    fn fetch(&self, request: &Request) -> Result<Response, NetworkError> {
        let key = (request.method, request.url.clone());
        // 决策阶段持锁、网络阶段不持锁：抓取可能阻塞整请求时长，
        // 持锁跨网络会串行化所有并发抓取。
        let action = {
            let mut state = self.lock()?;
            state.clock += 1;
            let stamp = state.clock;
            match state.entries.get_mut(&key) {
                Some(entry) => {
                    if !vary_matches(entry, request) {
                        Action::Miss
                    } else if !entry.must_revalidate
                        && entry
                            .max_age
                            .is_some_and(|age| entry.stored_at.elapsed() < age)
                    {
                        entry.last_used = stamp;
                        Action::Serve(entry.response.clone())
                    } else if entry.etag.is_some() || entry.last_modified.is_some() {
                        // 过期（或 no-cache）且可再验证
                        entry.last_used = stamp;
                        Action::Revalidate {
                            if_none_match: entry.etag.clone(),
                            if_modified_since: entry.last_modified.clone(),
                        }
                    } else {
                        // 过期且无 validator 可发：无法再验证，重新抓取
                        Action::Miss
                    }
                }
                None => Action::Miss,
            }
        };
        match action {
            Action::Serve(response) => Ok(response),
            Action::Miss => {
                let response = self.inner.fetch(request)?;
                self.store(&key, request, &response);
                Ok(response)
            }
            Action::Revalidate {
                if_none_match,
                if_modified_since,
            } => {
                let mut conditional = request.clone();
                if let Some(tag) = if_none_match {
                    conditional.headers.push(("If-None-Match".to_owned(), tag));
                }
                if let Some(since) = if_modified_since {
                    conditional
                        .headers
                        .push(("If-Modified-Since".to_owned(), since));
                }
                let response = self.inner.fetch(&conditional)?;
                if response.status == 304 {
                    let mut state = self.lock()?;
                    state.clock += 1;
                    let stamp = state.clock;
                    match state.entries.get_mut(&key) {
                        Some(entry) => {
                            refresh_entry(entry, &response, stamp);
                            Ok(entry.response.clone())
                        }
                        // 条目在再验证期间被并发逐出：304 无体可用，重新整抓
                        None => {
                            drop(state);
                            let response = self.inner.fetch(request)?;
                            self.store(&key, request, &response);
                            Ok(response)
                        }
                    }
                } else {
                    self.store(&key, request, &response);
                    Ok(response)
                }
            }
        }
    }
}

/// 304 刷新：重置新鲜度起点；304 头里出现的新 `Cache-Control` / `ETag` /
/// `Last-Modified` 覆盖旧值（RFC 9111 §4.3.4 更新存储头的简化档）。
fn refresh_entry(entry: &mut CacheEntry, response_304: &Response, stamp: u64) {
    entry.stored_at = Instant::now();
    entry.last_used = stamp;
    let has_cache_control = response_304
        .headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("cache-control"));
    if has_cache_control {
        let cache_control = parse_cache_control(response_304);
        entry.max_age = cache_control.max_age;
        entry.must_revalidate = cache_control.no_cache;
    }
    for (name, value) in &response_304.headers {
        if name.eq_ignore_ascii_case("etag") {
            entry.etag = Some(value.clone());
        } else if name.eq_ignore_ascii_case("last-modified") {
            entry.last_modified = Some(value.clone());
        }
    }
}

/// 按缓存策略判定一次响应是否可存；可存则构造条目。
fn build_entry(request: &Request, response: &Response) -> Option<CacheEntry> {
    if response.status != 200 {
        // 只缓存 200（错误页与重定向终态不进缓存）
        return None;
    }
    if response.body.len() > MAX_CACHE_ENTRY_BODY_BYTES {
        return None;
    }
    let cache_control = parse_cache_control(response);
    if cache_control.no_store {
        return None;
    }
    let etag = header(response, "etag");
    let last_modified = header(response, "last-modified");
    if cache_control.no_cache && etag.is_none() && last_modified.is_none() {
        // 强制再验证但无 validator 可发，缓存无意义
        return None;
    }
    if cache_control.max_age.is_none() && etag.is_none() && last_modified.is_none() {
        // 无新鲜度且无 validator：不缓存（简化档不认 Expires）
        return None;
    }
    let vary_ua = match vary_policy(response, request) {
        VaryPolicy::NotCacheable => return None,
        VaryPolicy::Unconstrained => None,
        VaryPolicy::UserAgent(ua) => Some(ua),
    };
    Some(CacheEntry {
        response: response.clone(),
        stored_at: Instant::now(),
        max_age: cache_control.max_age,
        must_revalidate: cache_control.no_cache,
        etag,
        last_modified,
        vary_ua,
        last_used: 0,
    })
}

/// `Vary` 处置：`Unconstrained`——无 Vary（或空值）→ 自由缓存；`UserAgent`——
/// 仅 `User-Agent` → 携带填充请求的 UA（可能为无 UA 头）；`NotCacheable`——
/// 含 `User-Agent` 以外字段（或 `*`）→ 整条不缓存。
enum VaryPolicy {
    Unconstrained,
    UserAgent(Option<String>),
    NotCacheable,
}

fn vary_policy(response: &Response, request: &Request) -> VaryPolicy {
    let Some(vary) = header(response, "vary") else {
        return VaryPolicy::Unconstrained;
    };
    let mut saw_user_agent = false;
    for field in vary.split(',') {
        let field = field.trim();
        if field.is_empty() {
            continue;
        }
        if field.eq_ignore_ascii_case("user-agent") {
            saw_user_agent = true;
        } else {
            return VaryPolicy::NotCacheable;
        }
    }
    if saw_user_agent {
        VaryPolicy::UserAgent(
            request
                .headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("user-agent"))
                .map(|(_, value)| value.clone()),
        )
    } else {
        VaryPolicy::Unconstrained
    }
}

/// `Vary: User-Agent` 一致性：条目设了 UA 约束时，请求 UA 必须与填充时一致。
fn vary_matches(entry: &CacheEntry, request: &Request) -> bool {
    let Some(fill_ua) = &entry.vary_ua else {
        return true;
    };
    let request_ua = request
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("user-agent"))
        .map(|(_, value)| value.as_str());
    request_ua == fill_ua.as_deref()
}

struct CacheControl {
    no_store: bool,
    no_cache: bool,
    max_age: Option<Duration>,
}

/// 解析 `Cache-Control`（可多条，指令逗号分隔；非法值忽略）。
fn parse_cache_control(response: &Response) -> CacheControl {
    let mut parsed = CacheControl {
        no_store: false,
        no_cache: false,
        max_age: None,
    };
    for (name, value) in &response.headers {
        if !name.eq_ignore_ascii_case("cache-control") {
            continue;
        }
        for directive in value.split(',') {
            let directive = directive.trim();
            if directive.eq_ignore_ascii_case("no-store") {
                parsed.no_store = true;
            } else if directive.eq_ignore_ascii_case("no-cache") {
                parsed.no_cache = true;
            } else if let Some(seconds) = directive
                .split_once('=')
                .filter(|(name, _)| name.trim().eq_ignore_ascii_case("max-age"))
                .and_then(|(_, value)| value.trim().parse::<u64>().ok())
            {
                parsed.max_age = Some(Duration::from_secs(seconds));
            }
        }
    }
    parsed
}

/// 取第一个同名响应头（名字大小写不敏感）。
fn header(response: &Response, name: &str) -> Option<String> {
    response
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ReqwestFetcher;
    use crate::test_loopback::{Server, request_path, respond};

    fn fetcher() -> CachedFetcher<ReqwestFetcher> {
        CachedFetcher::new(ReqwestFetcher::new().expect("fetcher"))
    }

    fn get(fetcher: &CachedFetcher<ReqwestFetcher>, url: &str) -> Response {
        fetcher
            .fetch(&Request {
                url: url.to_owned(),
                method: Method::Get,
                headers: Vec::new(),
            })
            .expect("fetch")
    }

    fn get_with_ua(fetcher: &CachedFetcher<ReqwestFetcher>, url: &str, ua: &str) -> Response {
        fetcher
            .fetch(&Request {
                url: url.to_owned(),
                method: Method::Get,
                headers: vec![("User-Agent".to_owned(), ua.to_owned())],
            })
            .expect("fetch")
    }

    /// 首次未命中走网络；新鲜度内二次请求命中缓存（服务器只收到一次请求）。
    #[test]
    fn fresh_entry_is_served_from_cache_after_first_miss() {
        let server = Server::start(1, |_, _| {
            respond(200, "OK", &[("Cache-Control", "max-age=60")], b"v1")
        });
        let url = format!("{}/doc", server.base());
        let fetcher = fetcher();

        let first = get(&fetcher, &url);
        assert_eq!(first.body, b"v1");
        assert_eq!(server.requests().len(), 1); // 首次未命中：走网络

        let second = get(&fetcher, &url);
        assert_eq!(second.status, 200);
        assert_eq!(second.body, b"v1"); // 命中：内容一致
        assert_eq!(second.final_url, first.final_url);
        assert_eq!(server.requests().len(), 1); // 服务器未再收到请求
        server.join();
    }

    /// 过期条目经 If-None-Match 再验证：304 后复用缓存体，对外仍是 200。
    #[test]
    fn stale_entry_revalidates_with_304_and_reuses_body() {
        let server = Server::start(2, |index, _| {
            if index == 0 {
                respond(
                    200,
                    "OK",
                    &[("Cache-Control", "max-age=0"), ("ETag", "\"v1\"")],
                    b"stale-body",
                )
            } else {
                respond(304, "Not Modified", &[("ETag", "\"v1\"")], b"")
            }
        });
        let url = format!("{}/doc", server.base());
        let fetcher = fetcher();

        let first = get(&fetcher, &url);
        assert_eq!(first.body, b"stale-body");

        // max-age=0：立即过期 → 必须再验证
        let second = get(&fetcher, &url);
        assert_eq!(second.status, 200);
        assert_eq!(second.body, b"stale-body"); // 304 → 缓存体复用

        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert!(
            requests[1]
                .to_ascii_lowercase()
                .contains("if-none-match: \"v1\""),
            "二次请求应带条件头: {}",
            requests[1]
        );
        server.join();
    }

    /// 同时有 ETag 与 Last-Modified：再验证请求两个条件头都带。
    #[test]
    fn revalidation_sends_both_validators() {
        let server = Server::start(2, |index, _| {
            if index == 0 {
                respond(
                    200,
                    "OK",
                    &[
                        ("Cache-Control", "max-age=0"),
                        ("ETag", "\"v1\""),
                        ("Last-Modified", "Wed, 21 Oct 2026 07:28:00 GMT"),
                    ],
                    b"body",
                )
            } else {
                respond(304, "Not Modified", &[("ETag", "\"v1\"")], b"")
            }
        });
        let url = format!("{}/doc", server.base());
        let fetcher = fetcher();

        get(&fetcher, &url);
        let second = get(&fetcher, &url);
        assert_eq!(second.body, b"body");

        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        let head = requests[1].to_ascii_lowercase();
        assert!(head.contains("if-none-match"), "应带 If-None-Match: {head}");
        assert!(
            head.contains("if-modified-since"),
            "应带 If-Modified-Since: {head}"
        );
        server.join();
    }

    /// no-cache 强制再验证：即使 max-age 未过也要发条件请求。
    #[test]
    fn no_cache_forces_revalidation_even_when_fresh() {
        let server = Server::start(2, |index, _| {
            if index == 0 {
                respond(
                    200,
                    "OK",
                    &[
                        ("Cache-Control", "no-cache, max-age=9999"),
                        ("ETag", "\"v1\""),
                    ],
                    b"A",
                )
            } else {
                respond(304, "Not Modified", &[("ETag", "\"v1\"")], b"")
            }
        });
        let url = format!("{}/doc", server.base());
        let fetcher = fetcher();

        let first = get(&fetcher, &url);
        assert_eq!(first.body, b"A");
        let second = get(&fetcher, &url);
        assert_eq!(second.status, 200);
        assert_eq!(second.body, b"A"); // 304 复用缓存体

        assert_eq!(server.requests().len(), 2); // 尽管仍新鲜，仍发起了再验证
        server.join();
    }

    /// no-store 不缓存：两次请求都落到服务器。
    #[test]
    fn no_store_is_never_cached() {
        let server = Server::start(2, |_, _| {
            respond(200, "OK", &[("Cache-Control", "no-store")], b"n")
        });
        let url = format!("{}/doc", server.base());
        let fetcher = fetcher();

        assert_eq!(get(&fetcher, &url).body, b"n");
        assert_eq!(get(&fetcher, &url).body, b"n");
        assert_eq!(server.requests().len(), 2);
        server.join();
    }

    /// Vary 含 User-Agent 以外字段：不缓存。
    #[test]
    fn vary_other_than_user_agent_is_not_cached() {
        let server = Server::start(2, |_, _| {
            respond(
                200,
                "OK",
                &[("Cache-Control", "max-age=60"), ("Vary", "Accept-Language")],
                b"v",
            )
        });
        let url = format!("{}/doc", server.base());
        let fetcher = fetcher();

        assert_eq!(get(&fetcher, &url).body, b"v");
        assert_eq!(get(&fetcher, &url).body, b"v");
        assert_eq!(server.requests().len(), 2);
        server.join();
    }

    /// Vary: User-Agent 可缓存；UA 一致命中，不同 UA 未命中。
    #[test]
    fn vary_user_agent_matches_on_ua_equality() {
        let server = Server::start(2, |_, _| {
            respond(
                200,
                "OK",
                &[("Cache-Control", "max-age=60"), ("Vary", "User-Agent")],
                b"ua-body",
            )
        });
        let url = format!("{}/doc", server.base());
        let fetcher = fetcher();

        get_with_ua(&fetcher, &url, "agent-a");
        let hit = get_with_ua(&fetcher, &url, "agent-a");
        assert_eq!(hit.body, b"ua-body");
        assert_eq!(server.requests().len(), 1); // 同 UA：命中

        let miss = get_with_ua(&fetcher, &url, "agent-b");
        assert_eq!(miss.body, b"ua-body");
        assert_eq!(server.requests().len(), 2); // 不同 UA：未命中
        server.join();
    }

    /// 缓存键含 method：GET 已缓存不影响 HEAD 的独立键。
    #[test]
    fn cache_key_includes_method() {
        let server = Server::start(2, |index, head| {
            if index == 0 {
                assert!(head.starts_with("GET "), "首请求应为 GET: {head}");
                respond(200, "OK", &[("Cache-Control", "max-age=60")], b"get-body")
            } else {
                assert!(head.starts_with("HEAD "), "次请求应为 HEAD: {head}");
                respond(200, "OK", &[("Cache-Control", "max-age=60")], b"")
            }
        });
        let url = format!("{}/k", server.base());
        let fetcher = fetcher();

        let got = get(&fetcher, &url);
        assert_eq!(got.body, b"get-body");
        let headed = fetcher
            .fetch(&Request {
                url: url.clone(),
                method: Method::Head,
                headers: Vec::new(),
            })
            .expect("head");
        assert_eq!(headed.body, b""); // HEAD 独立键：走到服务器（体空是 HEAD 语义）
        assert_eq!(server.requests().len(), 2);
        server.join();
    }

    /// 超容量 LRU 逐出：最久未使用的条目被逐出后再次抓取重新走网络，
    /// 新条目仍命中。
    #[test]
    fn lru_evicts_least_recently_used_beyond_capacity() {
        let server = Server::start(MAX_CACHE_ENTRIES + 2, |_, head| {
            respond(
                200,
                "OK",
                &[("Cache-Control", "max-age=60")],
                request_path(head).as_bytes(),
            )
        });
        let url = |name: &str| format!("{}/e{name}", server.base());
        let fetcher = fetcher();

        // 填满容量再多放一个：最早的 /e00 被逐出
        get(&fetcher, &url("00"));
        for i in 1..=MAX_CACHE_ENTRIES {
            get(&fetcher, &url(&format!("{i:02}")));
        }
        assert_eq!(server.requests().len(), MAX_CACHE_ENTRIES + 1);

        // 新条目仍命中：不产生请求
        let recent_name = format!("{MAX_CACHE_ENTRIES:02}");
        let recent = get(&fetcher, &url(&recent_name));
        assert_eq!(recent.body, format!("/e{recent_name}").into_bytes());
        assert_eq!(server.requests().len(), MAX_CACHE_ENTRIES + 1);

        // 被逐出的最旧条目：重新走网络
        let evicted = get(&fetcher, &url("00"));
        assert_eq!(evicted.body, b"/e00");
        assert_eq!(server.requests().len(), MAX_CACHE_ENTRIES + 2);
        server.join();
    }

    /// 非 200 不缓存：404 两次都走网络。
    #[test]
    fn non_200_is_not_cached() {
        let server = Server::start(2, |_, _| respond(404, "Not Found", &[], b"missing"));
        let url = format!("{}/missing", server.base());
        let fetcher = fetcher();

        assert_eq!(get(&fetcher, &url).status, 404);
        assert_eq!(get(&fetcher, &url).status, 404);
        assert_eq!(server.requests().len(), 2);
        server.join();
    }

    /// 超过单条体积上限的响应不缓存。
    #[test]
    fn oversized_body_is_not_cached() {
        let big = vec![b'x'; MAX_CACHE_ENTRY_BODY_BYTES + 1];
        let server = Server::start(2, move |_, _| {
            respond(200, "OK", &[("Cache-Control", "max-age=60")], &big)
        });
        let url = format!("{}/big", server.base());
        let fetcher = fetcher();

        assert_eq!(
            get(&fetcher, &url).body.len(),
            MAX_CACHE_ENTRY_BODY_BYTES + 1
        );
        assert_eq!(
            get(&fetcher, &url).body.len(),
            MAX_CACHE_ENTRY_BODY_BYTES + 1
        );
        assert_eq!(server.requests().len(), 2); // 未被缓存
        server.join();
    }
}
