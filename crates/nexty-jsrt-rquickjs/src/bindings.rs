//! 原生绑定注册：把 T1 [`DomRegistry`](nexty_dom::DomRegistry) 全清单以
//! `__dom_*` 全局函数暴露给引擎，并实现 [`JsrtListener`](nexty_dom::JsrtListener) /
//! [`JsrtTimerCallback`](nexty_dom::JsrtTimerCallback) 的引擎回调桥。
//!
//! 引擎值不跨接缝：JS 回调函数存进 shim 的 `__fns` 数组（引擎堆自管 GC），
//! Rust 侧只持函数下标；回调经 `__invoke_listener` / `__invoke_timer` 拉起。
//! 绑定错误经 [`Exception::throw_message`] 转 JS 异常（消息 = 错误串）。

use rquickjs::function::Rest;
use rquickjs::{Context, Ctx, Exception, Function, Object, Result as JsResult};

use nexty_dom::{
    DomRegistry, Event, JsrtError, JsrtGate, JsrtListener, JsrtScope, JsrtTimerCallback,
    ListenerHandle, ListenerInvocation, ListenerOutcome, NodeId, ScriptError, ScriptErrorKind,
};

use crate::WriteSlot;

/// 绑定层单例（[`DomRegistry`] 无状态）。
const REGISTRY: DomRegistry = DomRegistry;

/// JS 数值 nodeId → [`NodeId`]（负数 / 溢出饱和为非法索引，由注册表报 NotFound）。
fn node_id(value: i64) -> NodeId {
    NodeId::from_index(usize::try_from(value).unwrap_or(usize::MAX))
}

/// 监听器句柄 ↔ JS 数值（[`ListenerHandle::as_u32`] / [`ListenerHandle::from_u32`]）。
fn handle_to_js(handle: ListenerHandle) -> i64 {
    i64::from(handle.as_u32())
}

fn handle_from_js(value: i64) -> ListenerHandle {
    ListenerHandle::from_u32(value.clamp(0, i64::from(u32::MAX)) as u32)
}

/// 绑定错误的统一出口：转带消息的 JS 异常。
fn throw(ctx: &Ctx<'_>, error: &JsrtError) -> rquickjs::Error {
    Exception::throw_message(ctx, &error.to_string())
}

/// 在全局对象上注册全部 `__dom_*` 原生绑定（`gate` 克隆进各闭包；
/// `write_slot` 供 `document.write` 捕获，T4）。
pub fn register(context: &Context, gate: &JsrtGate, write_slot: &WriteSlot) -> JsResult<()> {
    context.with(|ctx| register_all(ctx, context, gate, write_slot))
}

/// 注册体：`'js` 为注册点上下文生命周期——绑定闭包与其 JS 值返回同域。
fn register_all<'js>(
    ctx: Ctx<'js>,
    context: &Context,
    gate: &JsrtGate,
    write_slot: &WriteSlot,
) -> JsResult<()> {
    let globals = ctx.globals();

    // ---- 树变更 ----
    {
        let gate = gate.clone();
        globals.set(
            "__dom_create_element",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, name: String| -> JsResult<i64> {
                    match REGISTRY.create_element(&gate, &name) {
                        Ok(id) => Ok(id.index() as i64),
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }
    {
        let gate = gate.clone();
        globals.set(
            "__dom_append_child",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, parent: i64, child: i64| -> JsResult<()> {
                    match REGISTRY.append_child(&gate, node_id(parent), node_id(child)) {
                        Ok(()) => Ok(()),
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }
    {
        let gate = gate.clone();
        globals.set(
            "__dom_remove",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, node: i64| -> JsResult<()> {
                    match REGISTRY.remove(&gate, node_id(node)) {
                        Ok(()) => Ok(()),
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }

    // ---- textContent ----
    {
        let gate = gate.clone();
        globals.set(
            "__dom_text_content",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, node: i64| -> JsResult<Option<String>> {
                    match REGISTRY.text_content(&gate, node_id(node)) {
                        Ok(text) => Ok(text),
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }
    {
        let gate = gate.clone();
        globals.set(
            "__dom_set_text_content",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, node: i64, value: String| -> JsResult<()> {
                    match REGISTRY.set_text_content(&gate, node_id(node), &value) {
                        Ok(()) => Ok(()),
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }

    // ---- attributes ----
    {
        let gate = gate.clone();
        globals.set(
            "__dom_get_attribute",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, node: i64, name: String| -> JsResult<Option<String>> {
                    match REGISTRY.get_attribute(&gate, node_id(node), &name) {
                        Ok(value) => Ok(value),
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }
    {
        let gate = gate.clone();
        globals.set(
            "__dom_set_attribute",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, node: i64, name: String, value: String| -> JsResult<()> {
                    match REGISTRY.set_attribute(&gate, node_id(node), &name, &value) {
                        Ok(()) => Ok(()),
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }

    // ---- classList ----
    {
        let gate = gate.clone();
        globals.set(
            "__dom_class_list_add",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, node: i64, tokens: Rest<String>| -> JsResult<()> {
                    let refs: Vec<&str> = tokens.iter().map(String::as_str).collect();
                    match REGISTRY.class_list_add(&gate, node_id(node), &refs) {
                        Ok(()) => Ok(()),
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }
    {
        let gate = gate.clone();
        globals.set(
            "__dom_class_list_remove",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, node: i64, tokens: Rest<String>| -> JsResult<()> {
                    let refs: Vec<&str> = tokens.iter().map(String::as_str).collect();
                    match REGISTRY.class_list_remove(&gate, node_id(node), &refs) {
                        Ok(()) => Ok(()),
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }
    {
        let gate = gate.clone();
        globals.set(
            "__dom_class_list_contains",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, node: i64, token: String| -> JsResult<bool> {
                    match REGISTRY.class_list_contains(&gate, node_id(node), &token) {
                        Ok(contains) => Ok(contains),
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }
    {
        let gate = gate.clone();
        globals.set(
            "__dom_class_list_toggle",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, node: i64, token: String| -> JsResult<bool> {
                    match REGISTRY.class_list_toggle(&gate, node_id(node), &token) {
                        Ok(toggled) => Ok(toggled),
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }

    // ---- 查找 / 事件 / 钩子 ----
    {
        let gate = gate.clone();
        globals.set(
            "__dom_get_elements_by_tag_name",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, tag: String| -> JsResult<Vec<i64>> {
                    match REGISTRY.get_elements_by_tag_name(&gate, &tag) {
                        Ok(nodes) => Ok(nodes.iter().map(|id| id.index() as i64).collect()),
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }
    {
        let gate = gate.clone();
        globals.set(
            "__dom_document_element",
            Function::new(ctx.clone(), move |ctx: Ctx<'js>| -> JsResult<Option<i64>> {
                match REGISTRY.document_element(&gate) {
                    Ok(found) => Ok(found.map(|id| id.index() as i64)),
                    Err(error) => Err(throw(&ctx, &error)),
                }
            })?,
        )?;
    }
    {
        let gate = gate.clone();
        globals.set(
            "__dom_get_element_by_id",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, id: String| -> JsResult<Option<i64>> {
                    match REGISTRY.get_element_by_id(&gate, &id) {
                        Ok(found) => Ok(found.map(|id| id.index() as i64)),
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }
    {
        let gate = gate.clone();
        let bridge_ctx = context.clone();
        globals.set(
            "__dom_add_event_listener",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>,
                      node: i64,
                      type_: String,
                      fn_idx: i64,
                      capture: bool|
                      -> JsResult<i64> {
                    let callback = Box::new(ListenerBridge {
                        ctx: Some(bridge_ctx.clone()),
                        fn_index: fn_idx.clamp(0, i64::from(u32::MAX)) as u32,
                    });
                    match REGISTRY.add_event_listener(
                        &gate,
                        node_id(node),
                        &type_,
                        capture,
                        callback,
                    ) {
                        Ok(handle) => Ok(handle_to_js(handle)),
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }
    {
        let gate = gate.clone();
        globals.set(
            "__dom_remove_event_listener",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, node: i64, handle: i64| -> JsResult<bool> {
                    match REGISTRY.remove_event_listener(
                        &gate,
                        node_id(node),
                        handle_from_js(handle),
                    ) {
                        Ok(removed) => Ok(removed),
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }
    {
        let gate = gate.clone();
        globals.set(
            "__dom_dispatch",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>,
                      node: i64,
                      type_: String,
                      bubbles: bool,
                      cancelable: bool|
                      -> JsResult<bool> {
                    let mut event = Event::new(type_, bubbles, cancelable);
                    // JS 侧派发发生在引擎执行期间：构造环境作用域，回调桥经它
                    // 拉起 JS 监听器（不重入引擎入口锁）。
                    let ambient = Ambient { ctx: ctx.clone() };
                    match REGISTRY.dispatch_event(&gate, Some(&ambient), node_id(node), &mut event)
                    {
                        Ok(dispatched) => Ok(dispatched),
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }
    {
        let gate = gate.clone();
        globals.set(
            "__dom_query_selector",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, scope: i64, selector: String| -> JsResult<Option<i64>> {
                    match REGISTRY.query_selector(&gate, node_id(scope), &selector) {
                        Ok(found) => Ok(found.map(|id| id.index() as i64)),
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }
    {
        let gate = gate.clone();
        globals.set(
            "__dom_get_computed_style",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, node: i64| -> JsResult<Object<'js>> {
                    match REGISTRY.get_computed_style(&gate, node_id(node)) {
                        Ok(styles) => {
                            let object = Object::new(ctx.clone())?;
                            for (name, value) in styles {
                                object.set(name, value)?;
                            }
                            Ok(object)
                        }
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }

    // ---- document.write 捕获（T4 scripted 解析）----
    {
        let slot = write_slot.clone();
        globals.set(
            "__dom_write",
            Function::new(
                ctx.clone(),
                move |_ctx: Ctx<'js>, text: String| -> JsResult<()> {
                    // 先取出克隆再调用：回调内可能再入本绑定，不持槽借用。
                    let sink = slot.borrow().clone();
                    if let Some(sink) = sink {
                        (sink.borrow_mut())(&text);
                    }
                    Ok(())
                },
            )?,
        )?;
    }

    // ---- timers ----
    {
        let gate = gate.clone();
        let bridge_ctx = context.clone();
        globals.set(
            "__dom_set_timeout",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, fn_idx: i64, delay: i64| -> JsResult<i64> {
                    let callback = Box::new(TimerBridge {
                        ctx: bridge_ctx.clone(),
                        fn_index: fn_idx.clamp(0, i64::from(u32::MAX)) as u32,
                    });
                    match gate.set_timeout(callback, delay.max(0) as u64) {
                        Ok(id) => Ok(i64::from(id)),
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }
    {
        let gate = gate.clone();
        let bridge_ctx = context.clone();
        globals.set(
            "__dom_set_interval",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, fn_idx: i64, delay: i64| -> JsResult<i64> {
                    let callback = Box::new(TimerBridge {
                        ctx: bridge_ctx.clone(),
                        fn_index: fn_idx.clamp(0, i64::from(u32::MAX)) as u32,
                    });
                    match gate.set_interval(callback, delay.max(0) as u64) {
                        Ok(id) => Ok(i64::from(id)),
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }
    {
        let gate = gate.clone();
        globals.set(
            "__dom_clear_timeout",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, id: i64| -> JsResult<bool> {
                    match gate.clear_timeout(handle_from_js(id).as_u32()) {
                        Ok(removed) => Ok(removed),
                        Err(error) => Err(throw(&ctx, &error)),
                    }
                },
            )?,
        )?;
    }

    Ok(())
}

/// JS 引擎作业异常的消息提取（[`rquickjs::Runtime::execute_pending_job`]
/// 错误路径：异常仍挂在引擎上，经 `Ctx::catch` 取出读 `message`）。
pub fn job_exception_message(context: &Context) -> String {
    context.with(|ctx| {
        let value = ctx.catch();
        let message = value
            .into_object()
            .and_then(|object| object.get::<_, Option<String>>("message").ok().flatten());
        message.unwrap_or_else(|| String::from("uncaught exception in engine job"))
    })
}

/// 事件监听器桥：按函数下标拉起 JS 监听器，回读干预标志写入派发中的事件。
///
/// 双路径：JS 侧派发（引擎执行期间）经 [`JsrtScope`] 作用域拉起（不重入引擎
/// 锁）；泵驱动的派发（`scope = None`）自行经 `ctx.with` 进入引擎。
pub(crate) struct ListenerBridge {
    /// 引擎上下文句柄（泵驱动路径进入引擎用）。
    pub(crate) ctx: Option<Context>,
    /// shim `__fns` 数组中的函数下标。
    pub(crate) fn_index: u32,
}

impl ListenerBridge {
    /// 把监听器结果写入派发中的事件。
    fn apply(outcome: ListenerOutcome, event: &mut Event) -> Result<(), JsrtError> {
        if outcome.prevented {
            event.prevent_default();
        }
        if outcome.immediate {
            event.stop_immediate_propagation();
        } else if outcome.stopped {
            event.stop_propagation();
        }
        match outcome.error {
            Some(message) => Err(JsrtError::Script(ScriptError::new(
                ScriptErrorKind::Runtime,
                message,
            ))),
            None => Ok(()),
        }
    }
}

impl JsrtListener for ListenerBridge {
    fn call(
        &mut self,
        _gate: &JsrtGate,
        scope: Option<&dyn JsrtScope>,
        invocation: &ListenerInvocation,
        event: &mut Event,
    ) -> Result<(), JsrtError> {
        // 派发算法已构造载荷（阶段 / 类型 / target / currentTarget），补上
        // 自身函数标识后透传。
        let mut invocation = invocation.clone();
        invocation.fn_index = self.fn_index;
        if let Some(scope) = scope {
            // JS 侧派发：已在引擎执行环境内，经作用域拉起（不重入锁）。
            return Self::apply(scope.invoke_listener(invocation)?, event);
        }
        // 泵驱动派发：进入引擎拉起。
        let context = self.ctx.as_ref().ok_or(JsrtError::Reentrant(
            "listener bridge without engine context",
        ))?;
        let outcome = invoke_listener_in(context, &invocation).map_err(|err| {
            JsrtError::Script(ScriptError::new(ScriptErrorKind::Runtime, err.to_string()))
        })?;
        Self::apply(outcome, event)
    }
}

/// 在引擎环境内拉起一个监听器（读取 `__invoke_listener` 的结果对象）。
fn invoke_listener_in(
    context: &Context,
    invocation: &ListenerInvocation,
) -> JsResult<ListenerOutcome> {
    context.with(|ctx| invoke_listener_outcome(&ctx, invocation))
}

/// 同上，直接以当前 [`Ctx`] 执行（已在引擎环境内时不重入 `with`）。
fn invoke_listener_outcome(
    ctx: &Ctx<'_>,
    invocation: &ListenerInvocation,
) -> JsResult<ListenerOutcome> {
    let invoke: Function = ctx.globals().get("__invoke_listener")?;
    let result: Object = invoke.call((
        i64::from(invocation.fn_index),
        i64::from(invocation.phase),
        invocation.type_.as_str(),
        invocation.bubbles,
        invocation.cancelable,
        invocation.target_index as i64,
        invocation.current_target_index as i64,
    ))?;
    Ok(ListenerOutcome {
        error: result.get("error")?,
        stopped: result.get("stopped")?,
        immediate: result.get("immediate")?,
        prevented: result.get("prevented")?,
    })
}

/// JS 侧派发的引擎作用域：持有当前 `Ctx`，监听器拉起不重入引擎入口锁。
struct Ambient<'js> {
    ctx: Ctx<'js>,
}

impl<'js> JsrtScope for Ambient<'js> {
    fn invoke_listener(
        &self,
        invocation: ListenerInvocation,
    ) -> Result<ListenerOutcome, JsrtError> {
        invoke_listener_outcome(&self.ctx, &invocation).map_err(|err| {
            JsrtError::Script(ScriptError::new(ScriptErrorKind::Runtime, err.to_string()))
        })
    }
}

/// 定时器回调桥：按函数下标拉起 JS 回调；异常消息以 `Some` 返回。
pub(crate) struct TimerBridge {
    /// 引擎上下文句柄。
    pub(crate) ctx: Context,
    /// shim `__fns` 数组中的函数下标。
    pub(crate) fn_index: u32,
}

impl JsrtTimerCallback for TimerBridge {
    fn call(&mut self, _gate: &JsrtGate) -> Result<(), JsrtError> {
        let fn_index = self.fn_index;
        let outcome: JsResult<Option<String>> = self.ctx.with(|ctx| {
            let invoke: Function = ctx.globals().get("__invoke_timer")?;
            invoke.call((i64::from(fn_index),))
        });
        match outcome {
            Ok(None) => Ok(()),
            Ok(Some(message)) => Err(JsrtError::Script(ScriptError::new(
                ScriptErrorKind::Runtime,
                message,
            ))),
            Err(err) => Err(JsrtError::Script(ScriptError::new(
                ScriptErrorKind::Runtime,
                err.to_string(),
            ))),
        }
    }
}
