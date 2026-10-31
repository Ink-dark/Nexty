//! 回调面与泵状态门面：[`JsrtGate`]（共享句柄）、监听器表与派发算法。
//!
//! 派发行为 ground truth：DOM Standard §2.9
//! [dispatching events](https://dom.spec.whatwg.org/#dispatching-events) 简化档。
//!
//! ## 派发偏差（相对 `Document::dispatch`）
//!
//! - JS 监听器登记在 jsrt 会话的监听器表（本模块），不经 `Document` 的
//!   `ListenerRegistry`（后者服务 chrome 输入事件的 Rust 监听器，K1 语义不变）。
//! - 事件路径在派发开始时快照（同规范）；回调中的树变更不影响本次传播。
//! - 监听器执行期间**不持有任何文档/泵借用**——回调经
//!   [`JsrtGate`] 短借用访问 DOM，重入派发与绑定调用安全。
//! - 监听器在自身回调内被移除：不再触发（取出口语义，同规范）；
//!   无 capture 默认值 / once / passive / signal 选项（v0）。

use std::cell::{Cell, Ref, RefCell, RefMut};
use std::rc::Rc;

use super::timers::{NESTING_CLAMP_MIN_DELAY_MS, NESTING_CLAMP_THRESHOLD};
use super::{BindingError, JsrtError, RuntimeState};
use crate::event::{Event, EventPhase, Task};
use crate::{Document, NodeId};

/// 事件监听器回调面：JS 监听器的引擎无关回调形态。
///
/// 由适配 crate 实现（把引擎侧函数值封装为回调），经
/// [`DomRegistry::add_event_listener`](super::DomRegistry::add_event_listener)
/// 登记。`gate` 提供文档与泵的短借用访问；`scope` 为引擎作用域——JS 侧派发
/// （引擎执行期间）由绑定传入，回调桥据此在引擎环境内拉起 JS；泵驱动的派发
/// 传 `None`，回调桥自行进入引擎。`event` 可读事件面并干预
/// （stopPropagation / preventDefault）。
///
/// # Errors
///
/// 引擎回调异常时上抛 [`JsrtError`]，派发方终止传播并上抛。
pub trait JsrtListener {
    /// 事件命中时被拉起。
    ///
    /// # Errors
    ///
    /// 引擎回调异常时上抛 [`JsrtError`]。
    fn call(
        &mut self,
        gate: &JsrtGate,
        scope: Option<&dyn super::super::JsrtScope>,
        event: &mut Event,
    ) -> Result<(), JsrtError>;
}

/// 监听器句柄：登记时返回，凭句柄移除。
///
/// 句柄以数值形态跨界 JS 绑定面（引擎侧无类型信息），提供 u32 往返。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ListenerHandle(u32);

impl ListenerHandle {
    /// 句柄的数值形态（跨界 JS 绑定面用）。
    #[must_use]
    pub fn as_u32(self) -> u32 {
        self.0
    }

    /// 由数值形态构造（[`as_u32`](Self::as_u32) 的逆变换；非法值在移除时返回 false）。
    #[must_use]
    pub fn from_u32(value: u32) -> Self {
        Self(value)
    }
}

/// 单条监听器登记（注册序全局表内）。执行期间 `callback` 被取出。
struct ListenerEntry {
    id: u32,
    target: NodeId,
    type_: String,
    capture: bool,
    callback: Option<Box<dyn JsrtListener>>,
}

/// 监听器表：注册序全局表（id 即注册序），派发时按（目标，类型，capture）过滤。
#[derive(Default)]
pub(crate) struct ListenerTable {
    entries: Vec<ListenerEntry>,
    next_id: u32,
}

impl ListenerTable {
    /// 登记监听器，返回移除用句柄。允许重复注册（同 DOM 语义：触发两次）。
    pub(crate) fn add(
        &mut self,
        target: NodeId,
        event_type: &str,
        capture: bool,
        callback: Box<dyn JsrtListener>,
    ) -> ListenerHandle {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.push(ListenerEntry {
            id,
            target,
            type_: event_type.to_string(),
            capture,
            callback: Some(callback),
        });
        ListenerHandle(id)
    }

    /// 凭（目标，句柄）移除；目标不符或句柄不存在返回 false。
    pub(crate) fn remove_owned(&mut self, target: NodeId, handle: ListenerHandle) -> bool {
        let Some(position) = self
            .entries
            .iter()
            .position(|entry| entry.id == handle.0 && entry.target == target)
        else {
            return false;
        };
        self.entries.remove(position);
        true
    }

    /// 按派发阶段收集命中条目 id（注册序）。
    pub(crate) fn ids_for(
        &self,
        target: NodeId,
        event_type: &str,
        filter: CaptureFilter,
    ) -> Vec<u32> {
        self.entries
            .iter()
            .filter(|entry| {
                entry.target == target
                    && entry.type_ == event_type
                    && entry.callback.is_some()
                    && match filter {
                        CaptureFilter::All => true,
                        CaptureFilter::CaptureOnly => entry.capture,
                        CaptureFilter::BubbleOnly => !entry.capture,
                    }
            })
            .map(|entry| entry.id)
            .collect()
    }

    /// 取出条目回调（执行期间不持借用）；条目已被移除时返回 `None`。
    pub(crate) fn take(&mut self, id: u32) -> Option<Box<dyn JsrtListener>> {
        let entry = self.entries.iter_mut().find(|entry| entry.id == id)?;
        entry.callback.take()
    }

    /// 放回取出的回调；条目已被移除（回调内自行移除）则丢弃。
    pub(crate) fn restore(&mut self, id: u32, callback: Box<dyn JsrtListener>) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
            entry.callback = Some(callback);
        }
    }
}

impl std::fmt::Debug for ListenerTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ListenerTable")
            .field("entries", &self.entries.len())
            .finish()
    }
}

/// 派发阶段对 capture 标志的过滤：capture 下行 / target 全部（注册序）/ bubble 上行。
#[derive(Debug, Clone, Copy)]
pub(crate) enum CaptureFilter {
    /// capture 阶段：只触发 capture 登记条目。
    CaptureOnly,
    /// target 阶段：capture 与非 capture 都触发、按注册序（DOM §2.9 invoke）。
    All,
    /// bubble 阶段：只触发非 capture 条目。
    BubbleOnly,
}

/// 泵状态与文档的共享句柄（Clone 传播给绑定闭包与回调桥）。
///
/// 单线程使用（`Document` !Send，见 ADR `docs/decisions/2026-10-31-jsrt`）。
/// 借用纪律：**一切访问都是短借用**——泵与派发不跨回调持借用，回调内经本门面
/// 访问文档与泵；冲突以 [`JsrtError::Reentrant`] 上报而非 panic。
///
/// 注意：泵执行 DOM 侧宏/微任务体期间持有文档与泵状态的借用——任务体内经
/// 本门面的访问（含派发与绑定调用）会拿到 [`JsrtError::Reentrant`]；任务体
/// 应改用 [`TaskContext`](crate::event::TaskContext) 直接操作文档，需要派发
/// 进 JS 回调面的工作在任务外由宿主调用。
#[derive(Clone)]
pub struct JsrtGate {
    pub(crate) document: Rc<RefCell<Document>>,
    pub(crate) state: Rc<RefCell<RuntimeState>>,
    /// 绑定面改动过文档的脏位（chrome 重排触发用；Cell 免借用冲突）。
    mutated: Rc<Cell<bool>>,
}

impl JsrtGate {
    /// 构造共享句柄（由 [`JsrtRuntime`](super::JsrtRuntime) 持有原件）。
    pub(crate) fn new(document: Rc<RefCell<Document>>, state: Rc<RefCell<RuntimeState>>) -> Self {
        Self {
            document,
            state,
            mutated: Rc::new(Cell::new(false)),
        }
    }

    /// 脏位句柄：[`JsrtRuntime`](super::JsrtRuntime) 凭此读取重排触发。
    pub(crate) fn mutated_cell(&self) -> Rc<Cell<bool>> {
        Rc::clone(&self.mutated)
    }

    /// 只读访问文档。
    ///
    /// # Errors
    ///
    /// 文档已被外层借用时返回 [`JsrtError::Reentrant`]。
    pub fn document(&self) -> Result<Ref<'_, Document>, JsrtError> {
        self.document
            .try_borrow()
            .map_err(|_| JsrtError::Reentrant("document"))
    }

    /// 可变访问文档。引擎回调内可用（泵/派发对其不持借用）；DOM 侧宏/微任务体
    /// 应改用 [`TaskContext::document`](crate::event::TaskContext::document)。
    ///
    /// # Errors
    ///
    /// 文档已被外层借用时返回 [`JsrtError::Reentrant`]。
    pub fn document_mut(&self) -> Result<RefMut<'_, Document>, JsrtError> {
        self.document
            .try_borrow_mut()
            .map_err(|_| JsrtError::Reentrant("document_mut"))
    }

    /// 投递宏任务（事件循环任务队列；任务体经 [`TaskContext`] 访问文档）。
    ///
    /// # Errors
    ///
    /// 泵状态已被外层借用时返回 [`JsrtError::Reentrant`]。
    pub fn post_task(&self, task: Task) -> Result<(), JsrtError> {
        let mut state = self.borrow_state_mut("post_task")?;
        state.event_loop.post_task(task);
        Ok(())
    }

    /// 投递 DOM 微任务：在下一个微任务检查点执行，先于下一个宏任务。
    ///
    /// # Errors
    ///
    /// 泵状态已被外层借用时返回 [`JsrtError::Reentrant`]。
    pub fn post_microtask(&self, task: Task) -> Result<(), JsrtError> {
        let mut state = self.borrow_state_mut("post_microtask")?;
        state.event_loop.post_microtask(task);
        Ok(())
    }

    /// `setTimeout` 简化档：登记一次性定时器，返回 ID。
    ///
    /// 嵌套层级：定时器回调内调度为运行层 + 1，链外调度为 0（HTML §8.1.4）。
    /// 新层级超过 [`NESTING_CLAMP_THRESHOLD`] 时延迟下限钳到
    /// [`NESTING_CLAMP_MIN_DELAY_MS`]（任务卡简化档 5ms，见 [`timers`] 偏差清单）。
    ///
    /// # Errors
    ///
    /// 泵状态已被外层借用时返回 [`JsrtError::Reentrant`]。
    pub fn set_timeout(
        &self,
        callback: Box<dyn super::JsrtTimerCallback>,
        delay_ms: u64,
    ) -> Result<u32, JsrtError> {
        let mut state = self.borrow_state_mut("set_timeout")?;
        let nesting = state.nesting.map_or(0, |level| level + 1);
        let delay = if nesting > NESTING_CLAMP_THRESHOLD {
            delay_ms.max(NESTING_CLAMP_MIN_DELAY_MS)
        } else {
            delay_ms
        };
        Ok(state.timers.schedule(delay, None, callback, nesting))
    }

    /// `setInterval` 简化档：登记重复定时器，返回 ID。首跑延迟按嵌套钳制
    /// （同 [`set_timeout`](Self::set_timeout)），重复间隔下限 1ms
    /// （防止 0 间隔在泵内 busy loop，偏差已记录）。
    ///
    /// # Errors
    ///
    /// 泵状态已被外层借用时返回 [`JsrtError::Reentrant`]。
    pub fn set_interval(
        &self,
        callback: Box<dyn super::JsrtTimerCallback>,
        interval_ms: u64,
    ) -> Result<u32, JsrtError> {
        let mut state = self.borrow_state_mut("set_interval")?;
        let nesting = state.nesting.map_or(0, |level| level + 1);
        let delay = if nesting > NESTING_CLAMP_THRESHOLD {
            interval_ms.max(NESTING_CLAMP_MIN_DELAY_MS)
        } else {
            interval_ms
        };
        Ok(state
            .timers
            .schedule(delay.max(1), Some(interval_ms.max(1)), callback, nesting))
    }

    /// `clearTimeout`（与 `clearInterval` 共用同一 ID 空间，可互换）。
    ///
    /// # Errors
    ///
    /// 泵状态已被外层借用时返回 [`JsrtError::Reentrant`]。
    pub fn clear_timeout(&self, id: u32) -> Result<bool, JsrtError> {
        let mut state = self.borrow_state_mut("clear_timeout")?;
        Ok(state.timers.cancel(id))
    }

    /// `clearInterval`（与 `clearTimeout` 共用同一 ID 空间，可互换）。
    ///
    /// # Errors
    ///
    /// 泵状态已被外层借用时返回 [`JsrtError::Reentrant`]。
    pub fn clear_interval(&self, id: u32) -> Result<bool, JsrtError> {
        let mut state = self.borrow_state_mut("clear_interval")?;
        Ok(state.timers.cancel(id))
    }

    /// 登记事件监听器（绑定面走
    /// [`DomRegistry::add_event_listener`](super::DomRegistry::add_event_listener)）。
    pub(crate) fn add_listener(
        &self,
        target: NodeId,
        event_type: &str,
        capture: bool,
        callback: Box<dyn JsrtListener>,
    ) -> Result<ListenerHandle, JsrtError> {
        let mut state = self.borrow_state_mut("add_listener")?;
        Ok(state.listeners.add(target, event_type, capture, callback))
    }

    /// 凭（目标，句柄）移除监听器：句柄不属于目标时不移除。
    pub(crate) fn remove_listener_of(
        &self,
        target: NodeId,
        handle: ListenerHandle,
    ) -> Result<bool, JsrtError> {
        let mut state = self.borrow_state_mut("remove_listener_of")?;
        Ok(state.listeners.remove_owned(target, handle))
    }

    /// 派发事件进回调面（DOM §2.9 简化档，见模块文档偏差清单）。
    ///
    /// `scope` 为引擎作用域：JS 侧派发（引擎执行期间）传入，回调桥在引擎环境
    /// 内拉起 JS；泵驱动的派发传 `None`（回调桥自行进入引擎）。
    pub(crate) fn dispatch(
        &self,
        target: NodeId,
        event: &mut Event,
        scope: Option<&dyn super::super::JsrtScope>,
    ) -> Result<bool, JsrtError> {
        {
            let doc = self.document()?;
            if doc.node(target).is_none() {
                return Err(JsrtError::Binding(BindingError::NotFound));
            }
        }
        // 事件路径快照：根 → target 直接父节点（DOM §2.9：路径在派发开始时计算，
        // 回调中的树变更不影响本次传播）。深度受 MAX_TREE_DEPTH 约束，迭代收集。
        let mut chain: Vec<NodeId> = Vec::new();
        {
            let doc = self.document()?;
            let mut current = doc.parent(target);
            while let Some(node) = current {
                chain.push(node);
                current = doc.parent(node);
            }
        }
        chain.reverse();

        for &node in &chain {
            if event.is_propagation_stopped() {
                break;
            }
            event.set_dispatch_phase(EventPhase::Capturing);
            self.invoke(node, event, scope, CaptureFilter::CaptureOnly)?;
        }
        if !event.is_propagation_stopped() {
            event.set_dispatch_phase(EventPhase::AtTarget);
            self.invoke(target, event, scope, CaptureFilter::All)?;
        }
        if event.bubbles() {
            for &node in chain.iter().rev() {
                if event.is_propagation_stopped() {
                    break;
                }
                event.set_dispatch_phase(EventPhase::Bubbling);
                self.invoke(node, event, scope, CaptureFilter::BubbleOnly)?;
            }
        }
        event.set_dispatch_phase(EventPhase::None);
        Ok(!event.default_prevented())
    }

    /// 在单个节点上按阶段过滤触发监听器（DOM §2.9 invoke 简化档）。
    ///
    /// 每个条目「取出回调 → 放下所有借用 → 执行 → 放回」：回调（引擎桥）内的
    /// 绑定调用、嵌套派发、定时器登记都经 `JsrtGate` 短借用完成，互不冲突；
    /// `scope` 透传给回调桥（JS 侧派发时为引擎作用域）。
    fn invoke(
        &self,
        node: NodeId,
        event: &mut Event,
        scope: Option<&dyn super::super::JsrtScope>,
        filter: CaptureFilter,
    ) -> Result<(), JsrtError> {
        let ids = {
            let state = self.borrow_state("invoke")?;
            state.listeners.ids_for(node, event.type_(), filter)
        };
        for id in ids {
            if event.is_immediate_propagation_stopped() {
                break;
            }
            let callback = {
                let mut state = self.borrow_state_mut("invoke/take")?;
                state.listeners.take(id)
            };
            let Some(mut callback) = callback else {
                continue;
            };
            let result = callback.call(self, scope, event);
            {
                let mut state = self.borrow_state_mut("invoke/restore")?;
                state.listeners.restore(id, callback);
            }
            result?;
        }
        Ok(())
    }

    /// 标记文档已被绑定面改动（chrome 重排触发的脏位）。
    ///
    /// Cell 免借用：任何上下文（含泵持借用期间的间接调用）都置位成功。
    pub(crate) fn mark_mutated(&self) {
        self.mutated.set(true);
    }

    /// 只读借用泵状态（`jsrt` 模块内的绑定与泵共用）。
    ///
    /// # Errors
    ///
    /// 状态已被外层借用时返回 [`JsrtError::Reentrant`]。
    pub(crate) fn borrow_state(
        &self,
        where_: &'static str,
    ) -> Result<Ref<'_, RuntimeState>, JsrtError> {
        self.state
            .try_borrow()
            .map_err(|_| JsrtError::Reentrant(where_))
    }

    fn borrow_state_mut(
        &self,
        where_: &'static str,
    ) -> Result<RefMut<'_, RuntimeState>, JsrtError> {
        self.state
            .try_borrow_mut()
            .map_err(|_| JsrtError::Reentrant(where_))
    }
}

impl std::fmt::Debug for JsrtGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsrtGate")
            .field("document", &"Rc<RefCell<Document>>")
            .finish_non_exhaustive()
    }
}
