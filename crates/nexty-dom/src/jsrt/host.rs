//! runner 接缝 trait：脚本编译执行入口与引擎微任务源。
//!
//! [`ScriptHost`] 由引擎适配 crate 实现（当前：`nexty-jsrt-rquickjs`）；
//! [`JsrtRuntime`](super::JsrtRuntime) 的 `eval` / `pump` 以它驱动引擎。
//! pub API 不出现任何引擎类型（Hard Rules：依赖类型不入 pub API）——脚本求值
//! 结果 v0 不透传（inline script 的求值结果由宿主丢弃），JS 回调经
//! [`JsrtListener`](super::JsrtListener) / [`JsrtTimerCallback`](super::JsrtTimerCallback)
//! 回调面进入，引擎侧值由适配 crate 在桥接层封装。
//!
//! `gate` 参数：适配 crate 的绑定层在首次 `eval` 时装配（闭包捕获 `gate` 克隆，
//! 引擎函数对象经绑定注册表访问 DOM）；检查点执行引擎作业时同样经 `gate`
//! 短借用访问文档与泵（泵不持借用，见模块文档的借用纪律）。

use super::gate::JsrtGate;

/// 脚本执行错误：错误类别 + 消息（上抛，不 panic）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptError {
    /// 错误类别。
    pub kind: ScriptErrorKind,
    /// 引擎侧错误消息（适配 crate 负责把引擎异常转为可读文本）。
    pub message: String,
}

impl ScriptError {
    /// 构造脚本错误。
    pub fn new(kind: ScriptErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ScriptError {}

/// 脚本错误类别（HTML [script processing
/// model](https://html.spec.whatwg.org/multipage/webappapis.html#script-processing-model)
/// 的错误面简化档）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptErrorKind {
    /// 脚本编译失败（语法错误）。
    Syntax,
    /// 脚本执行中抛出异常（顶层 / 微任务 / 回调，含引擎中断）。
    Runtime,
}

/// runner 接缝 trait（引擎无关）。
///
/// 实现方约束：
/// - `eval` 同步编译并执行脚本（inline script 语义）；异常以
///   [`ScriptError`] 类别上抛，不 panic、不跨接缝携带引擎类型。
/// - `run_microtasks` 清空引擎侧微任务源（promise jobs 等）到尽；引擎回调
///   在此期间经 `gate` 绑定面访问 DOM（宿主对其不持借用）。
/// - 实现体允许持有引擎句柄（`!Send`），随宿主单线程使用。
pub trait ScriptHost {
    /// 编译并同步执行一段脚本；`name` 为脚本标识（诊断/错误消息用）。
    ///
    /// # Errors
    ///
    /// 编译失败返回 [`ScriptErrorKind::Syntax`]，执行异常返回
    /// [`ScriptErrorKind::Runtime`]。
    fn eval(&mut self, gate: &JsrtGate, source: &str, name: &str) -> Result<(), ScriptError>;

    /// 清空引擎微任务源到尽。
    ///
    /// # Errors
    ///
    /// 微任务执行中抛出异常时以 [`ScriptErrorKind::Runtime`] 上抛。
    fn run_microtasks(&mut self, gate: &JsrtGate) -> Result<(), ScriptError>;
}
