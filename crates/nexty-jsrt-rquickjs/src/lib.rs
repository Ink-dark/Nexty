//! `nexty-jsrt-rquickjs`：Nexty JSRT 的 rquickjs（QuickJS-NG）引擎适配层。
//!
//! 实现 [`nexty_dom::ScriptHost`] 接缝（K5 T2）：脚本编译执行（inline script
//! 同步语义，`src` defer 档由 chrome T3 编排）、引擎微任务源
//! （`execute_pending_job` 清到尽）、T1 [`DomRegistry`](nexty_dom::DomRegistry)
//! 全清单绑定（`document` / 元素包装 / 事件门面 / setTimeout·setInterval，
//! 见 [`shim`] 与 [`bindings`] 模块文档）。
//!
//! 用法：[`Jsrt::new`] 持有 `Document` 与引擎实例；`eval` 同步执行脚本；
//! [`nexty_dom::JsrtRuntime::pump`]（经 [`Jsrt::pump`]）按「微任务检查点 →
//! 宏任务 → 到期定时器」节奏驱动事件循环。
//!
//! ## unsafe 边界
//!
//! 本 crate 允许 unsafe（ADR `docs/decisions/2026-10-31-jsrt.md`），但当前
//! **零 unsafe**：rquickjs-core 提供完整 safe wrapper，FFI 边界全部在第三方
//! 内部（rquickjs-sys）；无 SAFETY 注释即无 unsafe 块。
//!
//! ## 偏差清单（v0）
//!
//! - 元素 / 事件为 shim 门面对象（见 [`shim`] 模块偏差清单）。
//! - 引擎作业（promise）异常经 `execute_pending_job` 错误路径上抛为
//!   [`ScriptErrorKind::Runtime`]，消息经 `Ctx::catch` 提取。
//! - eval 的 Syntax / Runtime 分类由 shim 的 `instanceof SyntaxError` 判定。
//! - 未绑定：queueMicrotask / Event 构造器 / fetch / localStorage / worker /
//!   ESM（T4 登记偏差）。
//!
//! # Errors
//!
//! 全部错误以 [`JsrtError`](nexty_dom::JsrtError) /
//! [`ScriptError`](nexty_dom::ScriptError) 类别上抛，不 panic。

mod bindings;
mod shim;

use std::cell::RefCell;
use std::rc::Rc;

use rquickjs::{Context, Runtime};

use nexty_dom::{
    Event, JsrtError, JsrtGate, JsrtRuntime, NodeId, PumpStatus, ScriptError, ScriptErrorKind,
    ScriptHost,
};

/// `document.write` 内容的宿主回调（T4 scripted 解析用）：Rc 共享 + 内部
/// 可变，绑定闭包与宿主两侧各持一份句柄。
pub type WriteSink = Rc<RefCell<dyn FnMut(&str)>>;

/// 捕获槽：装入 [`WriteSink`] 时 `document.write` 内容转发给宿主，未装入时
/// 为 no-op（解析器外的 write 记偏差忽略）。
pub(crate) type WriteSlot = Rc<RefCell<Option<WriteSink>>>;

/// rquickjs（QuickJS-NG）引擎的 [`ScriptHost`] 实现。
struct EngineHost {
    /// 引擎运行时（微任务作业源）。
    rt: Runtime,
    /// 引擎上下文（脚本与绑定所在全局）。
    ctx: Context,
    /// 绑定是否已装配（首次 eval时装配，闭包捕获 gate）。
    installed: bool,
    /// `document.write` 捕获槽（装配绑定时装入绑定闭包，T4）。
    write_slot: WriteSlot,
}

impl EngineHost {
    /// 装配原生绑定与 DOM shim（首次 eval 时执行一次）。
    fn install(&mut self, gate: &JsrtGate) -> Result<(), JsrtError> {
        bindings::register(&self.ctx, gate, &self.write_slot)
            .and_then(|()| self.ctx.with(|ctx| ctx.eval::<(), _>(shim::SHIM)))
            .map_err(|error| {
                JsrtError::Script(ScriptError::new(
                    ScriptErrorKind::Runtime,
                    format!("jsrt install failed: {error}"),
                ))
            })
    }

    /// 执行一段脚本（shim 的 `__dom_eval`：同步、异常捕获、类别化）。
    fn eval_inner(&mut self, source: &str, name: &str) -> Result<(), ScriptError> {
        let payload: Option<String> = self
            .ctx
            .with(|ctx| {
                let f: rquickjs::Function = ctx.globals().get("__dom_eval")?;
                f.call((source, name))
            })
            .map_err(|error| ScriptError::new(ScriptErrorKind::Runtime, error.to_string()))?;
        let Some(payload) = payload else {
            return Ok(());
        };
        let (kind, message) = payload
            .split_once('\u{1f}')
            .unwrap_or(("runtime", payload.as_str()));
        let kind = if kind == "syntax" {
            ScriptErrorKind::Syntax
        } else {
            ScriptErrorKind::Runtime
        };
        Err(ScriptError::new(kind, message))
    }
}

impl ScriptHost for EngineHost {
    fn eval(&mut self, gate: &JsrtGate, source: &str, name: &str) -> Result<(), ScriptError> {
        if !self.installed {
            self.install(gate)
                .map_err(|error| ScriptError::new(ScriptErrorKind::Runtime, error.to_string()))?;
            self.installed = true;
        }
        self.eval_inner(source, name)
    }

    fn run_microtasks(&mut self, _gate: &JsrtGate) -> Result<(), ScriptError> {
        loop {
            match self.rt.execute_pending_job() {
                Ok(true) => continue,
                Ok(false) => return Ok(()),
                Err(job_exception) => {
                    let message = bindings::job_exception_message(&job_exception.0);
                    return Err(ScriptError::new(ScriptErrorKind::Runtime, message));
                }
            }
        }
    }
}

/// 引擎初始化错误的类别化封装。
fn engine_error(where_: &str, error: rquickjs::Error) -> JsrtError {
    JsrtError::Script(ScriptError::new(
        ScriptErrorKind::Runtime,
        format!("{where_}: {error}"),
    ))
}

/// rquickjs 引擎的 JSRT 运行时门面：装配 [`JsrtRuntime`] + [`EngineHost`]。
///
/// 单线程使用（`Document` !Send 且引擎 Context !Send，见 ADR
/// `docs/decisions/2026-10-31-jsrt.md`）。
pub struct Jsrt {
    runtime: JsrtRuntime,
    /// `document.write` 捕获槽（T4）：与绑定闭包共享。
    write_slot: WriteSlot,
}

impl Jsrt {
    /// 以文档所有权构造（引擎实例 + 绑定装配随首次 eval 完成）。
    ///
    /// # Errors
    ///
    /// 引擎初始化失败返回 [`JsrtError::Script`]（类别 [`ScriptErrorKind::Runtime`]）。
    pub fn new(document: nexty_dom::Document) -> Result<Self, JsrtError> {
        let (rt, ctx) = engine_init()?;
        let write_slot = Rc::new(RefCell::new(None));
        Ok(Self {
            runtime: JsrtRuntime::new(
                document,
                Box::new(EngineHost {
                    rt,
                    ctx,
                    installed: false,
                    write_slot: Rc::clone(&write_slot),
                }),
            ),
            write_slot,
        })
    }

    /// 以既有共享句柄构造（chrome 侧 `Document` 已在 `Rc<RefCell<_>>` 中时用，
    /// T3 装配路径）。
    ///
    /// # Errors
    ///
    /// 引擎初始化失败返回 [`JsrtError::Script`]。
    pub fn from_shared(document: Rc<RefCell<nexty_dom::Document>>) -> Result<Self, JsrtError> {
        let (rt, ctx) = engine_init()?;
        let write_slot = Rc::new(RefCell::new(None));
        Ok(Self {
            runtime: JsrtRuntime::from_shared(
                document,
                Box::new(EngineHost {
                    rt,
                    ctx,
                    installed: false,
                    write_slot: Rc::clone(&write_slot),
                }),
            ),
            write_slot,
        })
    }

    /// 以注入时钟构造（测试用：手动推进时间，定时器行为完全确定）。
    ///
    /// # Errors
    ///
    /// 引擎初始化失败返回 [`JsrtError::Script`]。
    pub fn with_timer_clock(
        document: nexty_dom::Document,
        clock: Box<dyn Fn() -> u64>,
    ) -> Result<Self, JsrtError> {
        let (rt, ctx) = engine_init()?;
        let write_slot = Rc::new(RefCell::new(None));
        Ok(Self {
            runtime: JsrtRuntime::with_timer_clock(
                document,
                Box::new(EngineHost {
                    rt,
                    ctx,
                    installed: false,
                    write_slot: Rc::clone(&write_slot),
                }),
                clock,
            ),
            write_slot,
        })
    }

    /// 装入/卸下 `document.write` 捕获槽（T4 scripted 解析驱动用）：装入后
    /// 脚本内 `document.write` 的内容逐次转发给回调；卸下后 write 为 no-op。
    pub fn set_document_write_sink(&mut self, sink: Option<WriteSink>) {
        *self.write_slot.borrow_mut() = sink;
    }

    /// 内层 [`JsrtRuntime`]（钩子注入 / dispatch / post_task 等完整接缝面）。
    #[must_use]
    pub fn runtime(&self) -> &JsrtRuntime {
        &self.runtime
    }

    /// 内层 [`JsrtRuntime`] 的可变访问（钩子注入等装配期操作）。
    pub fn runtime_mut(&mut self) -> &mut JsrtRuntime {
        &mut self.runtime
    }

    /// 共享句柄（绑定层 / chrome 回调面）。
    #[must_use]
    pub fn gate(&self) -> &JsrtGate {
        self.runtime.gate()
    }

    /// 编译并同步执行脚本（inline script 语义），随后做微任务检查点。
    ///
    /// # Errors
    ///
    /// 编译失败 [`ScriptErrorKind::Syntax`]，执行异常 [`ScriptErrorKind::Runtime`]。
    pub fn eval(&mut self, source: &str, name: &str) -> Result<(), JsrtError> {
        self.runtime.eval(source, name)
    }

    /// 任务泵（节奏见 [`JsrtRuntime::pump`]）。
    ///
    /// # Errors
    ///
    /// 引擎回调异常上抛（不 panic）。
    pub fn pump(&mut self) -> Result<PumpStatus, JsrtError> {
        self.runtime.pump()
    }

    /// 宿主侧事件派发（生命周期事件等）进引擎回调面；返回 `!defaultPrevented`。
    ///
    /// # Errors
    ///
    /// 目标不存在 / 回调异常上抛。
    pub fn dispatch(&self, target: NodeId, event: &mut Event) -> Result<bool, JsrtError> {
        self.runtime.dispatch(target, event)
    }

    /// 取走重排脏位：脚本经绑定面改过文档即置位（chrome 重排触发用）。
    #[must_use]
    pub fn take_mutated(&self) -> bool {
        self.runtime.take_mutated()
    }
}

/// 引擎初始化：`Runtime` + 默认上下文。
fn engine_init() -> Result<(Runtime, Context), JsrtError> {
    let rt = Runtime::new().map_err(|error| engine_error("runtime init failed", error))?;
    let ctx = Context::full(&rt).map_err(|error| engine_error("context init failed", error))?;
    Ok((rt, ctx))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 引擎初始化与 eval 冒烟。
    #[test]
    fn engine_boots_and_evals() {
        let mut jsrt = Jsrt::new(nexty_dom::Document::new()).unwrap();
        jsrt.eval("globalThis.__smoke = 40 + 2;", "smoke").unwrap();
        jsrt.eval("if (__smoke !== 42) throw new Error('bad');", "check")
            .unwrap();
    }

    /// eval 错误上抛为错误类别（语法 / 运行时），不 panic。
    #[test]
    fn eval_errors_are_classified() {
        let mut jsrt = Jsrt::new(nexty_dom::Document::new()).unwrap();

        let error = jsrt.eval("this is not javascript(", "syntax").unwrap_err();
        let JsrtError::Script(script) = error else {
            panic!("expected script error");
        };
        assert_eq!(script.kind, ScriptErrorKind::Syntax);

        let error = jsrt
            .eval("throw new Error('boom');", "runtime")
            .unwrap_err();
        let JsrtError::Script(script) = error else {
            panic!("expected script error");
        };
        assert_eq!(script.kind, ScriptErrorKind::Runtime);
        assert!(
            script.message.contains("boom"),
            "message: {}",
            script.message
        );
    }
}
