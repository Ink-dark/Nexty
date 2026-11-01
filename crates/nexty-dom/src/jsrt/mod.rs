//! JSRT 接缝（K5 T1）：runner 接缝 trait、回调面、绑定注册表与任务泵（引擎无关）。
//!
//! 结构：
//! - [`host`]：[`ScriptHost`] 接缝 trait——脚本编译执行入口 + 引擎微任务源，
//!   由引擎适配 crate 实现（`nexty-jsrt-rquickjs`）。
//! - [`gate`]：[`JsrtGate`] 共享句柄（文档与泵状态的短借用访问）、
//!   [`JsrtListener`] 回调面与 JS 侧事件派发算法（DOM §2.9 简化档）。
//! - [`registry`]：[`DomRegistry`] 绑定注册表（createElement / appendChild /
//!   remove / textContent / get·setAttribute / classList / getElementById /
//!   add·removeEventListener / dispatchEvent），querySelector 与
//!   getComputedStyle 经注入钩子（chrome T3 装配）——dom 不反向依赖 css。
//! - [`timers`]：setTimeout / setInterval 注册表（ID 管理、清除、嵌套钳制 5ms）。
//! - [`JsrtRuntime`]：任务泵——宏任务、DOM 微任务与引擎微任务在检查点合流，
//!   微任务先于下一个宏任务清空；到期定时器逐条执行。
//!
//! ## 引擎无关性
//!
//! 本模块不含任何引擎类型（Hard Rules：依赖类型不入 pub API；kernel facade
//! 维持 `forbid(unsafe_code)`）。引擎值经回调面进出：JS 函数封装为
//! [`JsrtListener`] / [`JsrtTimerCallback`]，引擎异常封装为 [`ScriptError`]
//! 类别上抛。
//!
//! ## 泵语义（HTML event loop 简化档）
//!
//! [`JsrtRuntime::pump`] 的单轮节奏：
//! 1. 微任务检查点：DOM 微任务清到尽 → 引擎微任务清到尽，交替往复直到双空
//!    （引擎任务投递的 DOM 微任务同轮补跑）；
//! 2. 取一个宏任务执行（任务队列 FIFO）；
//! 3. 取一条到期定时器执行（`(due, id)` 序；嵌套钳制见 [`timers`]）；
//! 4. 回到 1，直到无可运行：未到期定时器 → [`PumpStatus::Waiting`]（载荷为
//!    最近到期时刻，与定时器时钟同域），否则 [`PumpStatus::Idle`]。
//!
//! ## 偏差清单（简化档）
//!
//! - 同一时刻既有待跑宏任务又有到期定时器时先跑宏任务：定时器视为到期时
//!   入队，不维护与任务队列的合并入队序。
//! - 微任务检查点内 DOM 微任务先于引擎微任务清空（两源不合并为单一 FIFO）。
//! - 定时器回调异常后不再重排，错误经泵上抛（HTML 为报告 uncaught exception
//!   并继续事件循环）。
//! - 泵执行 DOM 侧宏/微任务体期间持有文档与泵状态借用：任务体内经 [`JsrtGate`]
//!   的访问（绑定 / 派发 / post）报 [`JsrtError::Reentrant`]——任务体应经
//!   [`TaskContext`](crate::event::TaskContext) 操作文档，需要派发进 JS 回调面
//!   的工作在任务外由宿主调用（如生命周期事件经 [`JsrtRuntime::dispatch`]）。
//! - 派发偏差见 [`gate`] 模块文档；绑定偏差见 [`registry`] 模块文档；
//!   定时器偏差见 [`timers`] 模块文档。

mod gate;
mod host;
mod registry;
mod timers;

pub use gate::{JsrtGate, JsrtListener, ListenerHandle};
pub use host::{ScriptError, ScriptErrorKind, ScriptHost};
pub use registry::{BindingError, DomRegistry, GetComputedStyleHook, QuerySelectorHook};
use std::cell::{Cell, RefCell};
use std::fmt;
use std::rc::Rc;
pub use timers::JsrtTimerCallback;

use crate::event::{Event, EventLoop, Task};
use crate::{Document, NodeId};

/// JSRT 运行时错误：重入冲突 / 绑定错误 / 脚本错误（引擎异常的类别化封装，
/// 上抛不 panic）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsrtError {
    /// 泵或派发执行期间的文档 / 状态借用冲突；载荷标注冲突发生点。
    Reentrant(&'static str),
    /// 绑定操作错误，见 [`BindingError`]。
    Binding(BindingError),
    /// 脚本编译或执行错误，见 [`ScriptError`]。
    Script(ScriptError),
}
impl fmt::Display for JsrtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reentrant(where_) => write!(f, "reentrant access at {where_}"),
            Self::Binding(error) => write!(f, "binding error: {error}"),
            Self::Script(error) => write!(f, "script error: {error}"),
        }
    }
}

impl std::error::Error for JsrtError {}

impl From<BindingError> for JsrtError {
    fn from(error: BindingError) -> Self {
        Self::Binding(error)
    }
}

/// 引擎回调执行环境（适配 crate 在持有引擎执行权时的作用域对象）。
///
/// dom 不见引擎类型：适配 crate 以具体类型（含引擎上下文句柄）实现本 trait，
/// 在其引擎绑定入口处构造**局部**对象并传给派发入口；回调桥经
/// [`JsrtScope::invoke_listener`] 在引擎环境内拉起 JS 监听器——避免在引擎
/// 执行期间重入引擎入口（rquickjs 的运行时锁非重入）。
///
/// 泵驱动的派发（宿主侧 / 定时器回调）无作用域，由回调桥自行进入引擎。
pub trait JsrtScope {
    /// 在引擎执行环境内拉起一个监听器回调。
    ///
    /// `invocation.fn_index` 是适配 crate 自定义的函数标识（dom 只透传）；
    /// 返回干预标志与异常消息（`None` = 正常完成）。
    ///
    /// # Errors
    ///
    /// 引擎侧异常时返回 [`JsrtError::Script`]。
    fn invoke_listener(&self, invocation: ListenerInvocation)
    -> Result<ListenerOutcome, JsrtError>;
}

/// 监听器拉起请求（引擎无关载荷；`fn_index` 由适配 crate 补充）。
#[derive(Debug, Clone)]
pub struct ListenerInvocation {
    /// 适配 crate 侧的函数标识（shim 回调表下标；派发算法构造时置 0，由
    /// 回调桥以自身标识覆盖）。
    pub fn_index: u32,
    /// 传播阶段（DOM `eventPhase` 序数：0 none / 1 capture / 2 target / 3 bubble）。
    pub phase: u8,
    /// 事件类型（自有字符串——载荷须在 `&mut Event` 借用之外存活）。
    pub type_: String,
    /// 是否冒泡。
    pub bubbles: bool,
    /// 是否可取消。
    pub cancelable: bool,
    /// 派发目标节点的索引值（DOM `event.target` 语义；NodeId::index 形态，
    /// 适配 crate 据此还原元素包装——T4 补面）。
    pub target_index: usize,
    /// 当前正在调用监听器的节点索引值（DOM `event.currentTarget` 语义）。
    pub current_target_index: usize,
}

/// 监听器拉起结果（引擎无关载荷）。
#[derive(Debug, Clone, Default)]
pub struct ListenerOutcome {
    /// 监听器异常消息（`None` = 正常完成）。
    pub error: Option<String>,
    /// `stopPropagation` 标志。
    pub stopped: bool,
    /// `stopImmediatePropagation` 标志（含 stopped 语义）。
    pub immediate: bool,
    /// `preventDefault` 标志。
    pub prevented: bool,
}

/// 泵运行状态（crate 内部件）：任务队列 / 定时器 / JS 监听器表 / 钩子 / 嵌套层级。
pub(crate) struct RuntimeState {
    /// 宏任务与 DOM 微任务队列（K1 `EventLoop`，微任务位已填充）。
    pub(crate) event_loop: EventLoop,
    /// 定时器注册表（`setTimeout` / `setInterval` 共用 ID 空间）。
    pub(crate) timers: timers::TimerRegistry,
    /// JS 监听器表（与 K1 的 Rust 监听器注册表分离）。
    pub(crate) listeners: gate::ListenerTable,
    /// 运行中定时器回调的嵌套层级；`None` = 不在定时器回调内。
    /// `set_timeout` / `set_interval` 据此计算新定时器的层级（HTML §8.1.4）。
    pub(crate) nesting: Option<u32>,
    /// querySelector 注入钩子（chrome T3 装配 / 测试注入）。
    pub(crate) query_selector: Option<QuerySelectorHook>,
    /// getComputedStyle 注入钩子（chrome T3 装配 / 测试注入）。
    pub(crate) get_computed_style: Option<GetComputedStyleHook>,
}

impl RuntimeState {
    fn with_system_clock() -> Self {
        Self::with_timer_clock(Box::new(std_time_ms))
    }

    fn with_timer_clock(clock: Box<dyn Fn() -> u64>) -> Self {
        Self {
            event_loop: EventLoop::new(),
            timers: timers::TimerRegistry::with_clock(clock),
            listeners: gate::ListenerTable::default(),
            nesting: None,
            query_selector: None,
            get_computed_style: None,
        }
    }
}

impl fmt::Debug for RuntimeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeState")
            .field("event_loop", &self.event_loop)
            .field("timers", &self.timers)
            .field("listeners", &self.listeners)
            .field("nesting", &self.nesting)
            .finish_non_exhaustive()
    }
}

/// 系统当前时刻（Unix 毫秒，饱和）。
fn std_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since_epoch| {
            u64::try_from(since_epoch.as_millis()).unwrap_or(u64::MAX)
        })
}

/// 泵一轮结束时的状态：宿主据此决定下次驱动时机。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpStatus {
    /// 任务、微任务与定时器全部清空。
    Idle,
    /// 还有未到期定时器；载荷为最近到期时刻（毫秒，与定时器时钟同域）。
    Waiting(u64),
}

/// JSRT 运行时：[`ScriptHost`] 装配 + 任务泵（K5 T1 接缝的宿主侧入口）。
///
/// 单线程使用（`Document` !Send，见 ADR `docs/decisions/2026-10-31-jsrt.md`）。
/// 适配 crate（`nexty-jsrt-rquickjs`）实现 [`ScriptHost`] 并把 [`JsrtGate`] /
/// [`DomRegistry`] 接到引擎绑定上；chrome（T3）经本类型的 eval / dispatch /
/// pump 驱动脚本与任务。
pub struct JsrtRuntime {
    document: Rc<RefCell<Document>>,
    state: Rc<RefCell<RuntimeState>>,
    gate: JsrtGate,
    host: Box<dyn ScriptHost>,
    registry: DomRegistry,
    mutated: Rc<Cell<bool>>,
}

impl JsrtRuntime {
    /// 以文档所有权构造（文档移入共享句柄）。
    pub fn new(document: Document, host: Box<dyn ScriptHost>) -> Self {
        Self::from_shared(Rc::new(RefCell::new(document)), host)
    }

    /// 以既有共享句柄构造（chrome 侧 Document 已在 `Rc<RefCell<_>>` 中时用，
    /// T3 装配路径）。
    pub fn from_shared(document: Rc<RefCell<Document>>, host: Box<dyn ScriptHost>) -> Self {
        let state = Rc::new(RefCell::new(RuntimeState::with_system_clock()));
        let gate = JsrtGate::new(Rc::clone(&document), Rc::clone(&state));
        let mutated = gate.mutated_cell();
        Self {
            document,
            state,
            gate,
            host,
            registry: DomRegistry,
            mutated,
        }
    }

    /// 以注入时钟构造（测试用：手动推进时间，定时器行为完全确定）。
    pub fn with_timer_clock(
        document: Document,
        host: Box<dyn ScriptHost>,
        clock: Box<dyn Fn() -> u64>,
    ) -> Self {
        let document = Rc::new(RefCell::new(document));
        let state = Rc::new(RefCell::new(RuntimeState::with_timer_clock(clock)));
        let gate = JsrtGate::new(Rc::clone(&document), Rc::clone(&state));
        let mutated = gate.mutated_cell();
        Self {
            document,
            state,
            gate,
            host,
            registry: DomRegistry,
            mutated,
        }
    }

    /// 注入 querySelector 钩子（测试注入点；chrome T3 装配 css 选择器匹配）。
    pub fn set_query_selector_hook(&mut self, hook: QuerySelectorHook) {
        self.state.borrow_mut().query_selector = Some(hook);
    }

    /// 注入 getComputedStyle 钩子（测试注入点；chrome T3 装配级联只读快照）。
    pub fn set_get_computed_style_hook(&mut self, hook: GetComputedStyleHook) {
        self.state.borrow_mut().get_computed_style = Some(hook);
    }

    /// 共享句柄：适配 crate 的绑定层与回调桥经它访问文档与泵。
    pub fn gate(&self) -> &JsrtGate {
        &self.gate
    }

    /// 绑定注册表：适配 crate 的绑定函数调用入口。
    pub fn registry(&self) -> &DomRegistry {
        &self.registry
    }

    /// 编译并同步执行脚本（inline script 语义），随后做微任务检查点。
    ///
    /// # Errors
    ///
    /// 编译失败返回 [`ScriptErrorKind::Syntax`]、执行异常返回
    /// [`ScriptErrorKind::Runtime`]（经 [`JsrtError::Script`] 上抛，不 panic）。
    pub fn eval(&mut self, source: &str, name: &str) -> Result<(), JsrtError> {
        self.host
            .eval(&self.gate, source, name)
            .map_err(JsrtError::Script)?;
        self.checkpoint()
    }

    /// 事件派发进引擎回调面的入口（生命周期事件等宿主侧派发，无引擎作用域
    /// ——回调桥自行进入引擎）。返回 `!defaultPrevented`。
    ///
    /// # Errors
    ///
    /// 目标不存在 / 回调异常见 [`JsrtError`]。
    pub fn dispatch(&self, target: NodeId, event: &mut Event) -> Result<bool, JsrtError> {
        self.gate.dispatch(target, event, None)
    }

    /// 投递宏任务（与 [`JsrtGate::post_task`] 等价；宿主便捷入口）。
    ///
    /// # Errors
    ///
    /// 泵状态被占用时返回 [`JsrtError::Reentrant`]。
    pub fn post_task(&self, task: Task) -> Result<(), JsrtError> {
        self.gate.post_task(task)
    }

    /// 投递 DOM 微任务（检查点清空，先于下一个宏任务）。
    ///
    /// # Errors
    ///
    /// 泵状态被占用时返回 [`JsrtError::Reentrant`]。
    pub fn post_microtask(&self, task: Task) -> Result<(), JsrtError> {
        self.gate.post_microtask(task)
    }

    /// 任务泵：清空当前可运行的一切（微任务检查点 → 宏任务 → 到期定时器，
    /// 节奏见[模块文档](self)），返回宿主下次驱动时机。
    ///
    /// # Errors
    ///
    /// 引擎回调异常（[`JsrtError::Script`]）原样上抛；重入冲突见
    /// [`JsrtError::Reentrant`]。
    pub fn pump(&mut self) -> Result<PumpStatus, JsrtError> {
        loop {
            self.checkpoint()?;
            if self.run_next_macrostep()? {
                continue;
            }
            let now = { self.state.borrow().timers.now() };
            if self.run_due_timer(now)? {
                continue;
            }
            break;
        }
        let state = self.state.borrow();
        Ok(state
            .timers
            .next_due()
            .map_or(PumpStatus::Idle, PumpStatus::Waiting))
    }

    /// 取走重排脏位：绑定面改动过文档即置位，读取即清零（chrome 重排触发用）。
    pub fn take_mutated(&self) -> bool {
        self.mutated.replace(false)
    }

    /// 微任务检查点：DOM 微任务与引擎微任务交替清空到尽（模块文档偏差：
    /// DOM 源先清）。
    fn checkpoint(&mut self) -> Result<(), JsrtError> {
        loop {
            {
                let mut doc = self.document.borrow_mut();
                let mut state = self.state.borrow_mut();
                state.event_loop.run_microtasks(&mut doc);
            }
            self.host
                .run_microtasks(&self.gate)
                .map_err(JsrtError::Script)?;
            if !self.state.borrow().event_loop.has_pending_microtasks() {
                return Ok(());
            }
        }
    }

    /// 取一个宏任务执行（任务队列 FIFO；执行期间持文档与状态借用——任务体
    /// 内经 [`JsrtGate`] 的访问会拿到 [`JsrtError::Reentrant`]，见模块文档）。
    fn run_next_macrostep(&mut self) -> Result<bool, JsrtError> {
        if !self.state.borrow().event_loop.has_pending_tasks() {
            return Ok(false);
        }
        let mut doc = self.document.borrow_mut();
        let mut state = self.state.borrow_mut();
        state.event_loop.run_next_task(&mut doc);
        Ok(true)
    }

    /// 取一条到期定时器执行：嵌套层级随条目写入运行状态（钳制判定用），
    /// 执行后恢复；重复定时器按 `due + interval` 重排，一次性清除条目；
    /// 回调异常不再重排并上抛（模块文档偏差）。
    fn run_due_timer(&mut self, now: u64) -> Result<bool, JsrtError> {
        let Some((id, due, interval, nesting, mut callback)) =
            self.state.borrow_mut().timers.pop_due(now)
        else {
            return Ok(false);
        };
        self.state.borrow_mut().nesting = Some(nesting);
        let result = callback.call(&self.gate);
        self.state.borrow_mut().nesting = None;
        let mut state = self.state.borrow_mut();
        match result {
            Ok(()) => match interval {
                Some(interval) => {
                    state
                        .timers
                        .reschedule(id, due.saturating_add(interval), callback);
                }
                None => {
                    state.timers.cancel(id);
                }
            },
            Err(error) => {
                state.timers.cancel(id);
                return Err(error);
            }
        }
        Ok(true)
    }
}

impl fmt::Debug for JsrtRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JsrtRuntime")
            .field("state", &self.state.borrow())
            .finish_non_exhaustive()
    }
}

#[cfg(all(test, feature = "jsrt"))]
mod tests {
    use super::*;
    use crate::node::{ElementData, Namespace, NodeKind};
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    /// 日志共享句柄。
    type Log = Rc<RefCell<Vec<String>>>;

    fn log_new() -> Log {
        Rc::new(RefCell::new(Vec::new()))
    }

    /// 引擎侧作业：`run_microtasks` 检查点期间逐个执行。
    type Job = Box<dyn FnOnce(&JsrtGate) -> Result<(), ScriptError>>;
    type Jobs = Rc<RefCell<VecDeque<Job>>>;

    /// `eval` 行为闭包（假引擎宿主的注入点）。
    type EvalFn = Box<dyn FnMut(&JsrtGate, &str, &str) -> Result<(), ScriptError>>;

    /// `eval` 行为可注入的假引擎宿主。
    struct FakeHost {
        on_eval: EvalFn,
        jobs: Jobs,
    }

    impl FakeHost {
        fn noop() -> Self {
            Self {
                on_eval: Box::new(|_, _, _| Ok(())),
                jobs: Rc::new(RefCell::new(VecDeque::new())),
            }
        }
    }

    impl ScriptHost for FakeHost {
        fn eval(&mut self, gate: &JsrtGate, source: &str, name: &str) -> Result<(), ScriptError> {
            (self.on_eval)(gate, source, name)
        }

        fn run_microtasks(&mut self, gate: &JsrtGate) -> Result<(), ScriptError> {
            while let Some(job) = self.jobs.borrow_mut().pop_front() {
                job(gate)?;
            }
            Ok(())
        }
    }

    /// 测试定时器回调：按注入行为执行。
    struct FakeTimer {
        on_call: Box<dyn FnMut(&JsrtGate)>,
    }

    impl FakeTimer {
        fn new(on_call: Box<dyn FnMut(&JsrtGate)>) -> Self {
            Self { on_call }
        }
    }

    impl JsrtTimerCallback for FakeTimer {
        fn call(&mut self, gate: &JsrtGate) -> Result<(), JsrtError> {
            (self.on_call)(gate);
            Ok(())
        }
    }

    /// 附加行为（测试注入的可变动作）。
    type ListenerAction = Box<dyn FnMut(&mut Event)>;

    /// 测试事件监听器：记录（标签，类型，阶段），可选附加行为。
    struct FakeListener {
        log: Log,
        tag: &'static str,
        action: Option<ListenerAction>,
    }

    impl JsrtListener for FakeListener {
        fn call(
            &mut self,
            _gate: &JsrtGate,
            _scope: Option<&dyn JsrtScope>,
            _invocation: &ListenerInvocation,
            event: &mut Event,
        ) -> Result<(), JsrtError> {
            self.log.borrow_mut().push(format!(
                "{}:{:?}:{}",
                self.tag,
                event.event_phase(),
                event.type_()
            ));
            if let Some(action) = &mut self.action {
                action(event);
            }
            Ok(())
        }
    }

    fn runtime() -> JsrtRuntime {
        JsrtRuntime::new(Document::new(), Box::new(FakeHost::noop()))
    }

    fn element(doc: &mut Document, name: &str) -> NodeId {
        doc.create_node(NodeKind::Element(ElementData {
            name: name.into(),
            namespace: Namespace::Html,
            attributes: Vec::new(),
        }))
    }

    // ---- 泵：微任务 / 宏任务 / 引擎源合流 ----

    /// 检查点在 eval 后立即清空微任务（DOM 源先于引擎源），宏任务留到泵。
    #[test]
    fn eval_runs_checkpoint_before_pump() {
        let log = log_new();
        let jobs: Jobs = Rc::new(RefCell::new(VecDeque::new()));

        let log_dom = Rc::clone(&log);
        let host = FakeHost {
            on_eval: Box::new(move |gate, _, _| {
                let log_m = Rc::clone(&log_dom);
                gate.post_microtask(Task::new(move |_| {
                    log_m.borrow_mut().push("dom-micro".into());
                }))
                .unwrap();
                let log_t = Rc::clone(&log_dom);
                gate.post_task(Task::new(move |_| {
                    log_t.borrow_mut().push("task".into());
                }))
                .unwrap();
                Ok(())
            }),
            jobs: Rc::clone(&jobs),
        };
        let mut runtime = JsrtRuntime::new(Document::new(), Box::new(host));

        let log_job = Rc::clone(&log);
        jobs.borrow_mut().push_back(Box::new(move |_gate| {
            log_job.borrow_mut().push("engine-micro".into());
            Ok(())
        }));

        runtime.eval("script", "inline").unwrap();
        assert_eq!(
            *log.borrow(),
            ["dom-micro", "engine-micro"],
            "eval 的检查点清空两类微任务，宏任务不动"
        );

        runtime.pump().unwrap();
        assert_eq!(*log.borrow(), ["dom-micro", "engine-micro", "task"]);
    }

    /// 引擎微任务投递的 DOM 微任务在同一个检查点内补跑。
    #[test]
    fn engine_job_posted_dom_microtask_runs_in_same_checkpoint() {
        let log = log_new();
        let jobs: Jobs = Rc::new(RefCell::new(VecDeque::new()));
        let mut runtime = JsrtRuntime::new(
            Document::new(),
            Box::new(FakeHost {
                on_eval: Box::new(|_, _, _| Ok(())),
                jobs: Rc::clone(&jobs),
            }),
        );

        let log_job = Rc::clone(&log);
        let log_dom = Rc::clone(&log);
        jobs.borrow_mut().push_back(Box::new(move |gate| {
            log_job.borrow_mut().push("engine-job".into());
            gate.post_microtask(Task::new(move |_| {
                log_dom.borrow_mut().push("dom-after-engine".into());
            }))
            .unwrap();
            Ok(())
        }));

        runtime.pump().unwrap();
        assert_eq!(*log.borrow(), ["engine-job", "dom-after-engine"]);
        assert_eq!(runtime.pump().unwrap(), PumpStatus::Idle);
    }

    /// 微任务先于下一个宏任务：任务 A 投微任务 M 与任务 B，泵节奏下 M 先于 B
    /// （引擎无关版回归，与 event_loop 的同名测试互补）。
    #[test]
    fn microtasks_before_next_macrotask_through_pump() {
        let log = log_new();
        let mut runtime = runtime();

        let log_a = Rc::clone(&log);
        runtime
            .post_task(Task::new(move |ctx| {
                log_a.borrow_mut().push("a".into());
                let log_m = Rc::clone(&log_a);
                ctx.post_microtask(Task::new(move |_| {
                    log_m.borrow_mut().push("m".into());
                }));
                let log_b = Rc::clone(&log_a);
                ctx.post_task(Task::new(move |_| {
                    log_b.borrow_mut().push("b".into());
                }));
            }))
            .unwrap();

        runtime.pump().unwrap();
        assert_eq!(*log.borrow(), ["a", "m", "b"]);
    }

    /// 泵空转返回 Idle；只有未来定时器时返回 Waiting（最近到期时刻）。
    #[test]
    fn pump_reports_idle_and_waiting() {
        let clock = Rc::new(Cell::new(1_000u64));
        let mut runtime = JsrtRuntime::with_timer_clock(
            Document::new(),
            Box::new(FakeHost::noop()),
            Box::new({
                let clock = Rc::clone(&clock);
                move || clock.get()
            }),
        );
        assert_eq!(runtime.pump().unwrap(), PumpStatus::Idle);

        let gate = runtime.gate().clone();
        gate.set_timeout(Box::new(FakeTimer::new(Box::new(|_| {}))), 250)
            .unwrap();
        assert_eq!(runtime.pump().unwrap(), PumpStatus::Waiting(1_250));

        clock.set(1_250);
        assert_eq!(runtime.pump().unwrap(), PumpStatus::Idle, "到期定时器已跑");
    }

    // ---- 定时器：顺序 / 清除 / 嵌套钳制 ----

    /// 到期定时器按 (due, id) 序执行：先到期先跑，同到期按登记序。
    #[test]
    fn timers_run_in_due_then_id_order() {
        let clock = Rc::new(Cell::new(1_000u64));
        let mut runtime = JsrtRuntime::with_timer_clock(
            Document::new(),
            Box::new(FakeHost::noop()),
            Box::new({
                let clock = Rc::clone(&clock);
                move || clock.get()
            }),
        );
        let log = log_new();
        let gate = runtime.gate().clone();

        for (tag, delay) in [("a", 50u64), ("b", 10u64), ("c", 50u64)] {
            let log_cb = Rc::clone(&log);
            gate.set_timeout(
                Box::new(FakeTimer::new(Box::new(move |_| {
                    log_cb.borrow_mut().push(tag.into());
                }))),
                delay,
            )
            .unwrap();
        }

        assert_eq!(runtime.pump().unwrap(), PumpStatus::Waiting(1_010));
        clock.set(1_010);
        runtime.pump().unwrap();
        assert_eq!(*log.borrow(), ["b"], "只有最早到期的 b 跑");

        clock.set(1_050);
        assert_eq!(runtime.pump().unwrap(), PumpStatus::Idle);
        assert_eq!(*log.borrow(), ["b", "a", "c"], "同到期按登记序");
    }

    /// clearTimeout 阻止执行；对已跑完的一次性定时器清除是无操作。
    #[test]
    fn clear_timeout_prevents_run() {
        let mut runtime = runtime();
        let log = log_new();
        let gate = runtime.gate().clone();

        let log_a = Rc::clone(&log);
        let id = gate
            .set_timeout(
                Box::new(FakeTimer::new(Box::new(move |_| {
                    log_a.borrow_mut().push("a".into());
                }))),
                0,
            )
            .unwrap();
        assert!(gate.clear_timeout(id).unwrap());
        assert!(!gate.clear_timeout(id).unwrap(), "重复清除返回 false");

        let log_b = Rc::clone(&log);
        let id_b = gate
            .set_timeout(
                Box::new(FakeTimer::new(Box::new(move |_| {
                    log_b.borrow_mut().push("b".into());
                }))),
                0,
            )
            .unwrap();
        runtime.pump().unwrap();
        assert_eq!(*log.borrow(), ["b"]);
        assert!(!gate.clear_timeout(id_b).unwrap(), "已执行条目已清除");
    }

    /// setInterval 重复执行、clearInterval 终止（回调内自清）。
    #[test]
    fn interval_repeats_until_cleared() {
        let clock = Rc::new(Cell::new(1_000u64));
        let mut runtime = JsrtRuntime::with_timer_clock(
            Document::new(),
            Box::new(FakeHost::noop()),
            Box::new({
                let clock = Rc::clone(&clock);
                move || clock.get()
            }),
        );
        let log = log_new();
        let ticks = Rc::new(Cell::new(0u32));
        let gate = runtime.gate().clone();
        let id_cell = Rc::new(Cell::new(0u32));

        let log_i = Rc::clone(&log);
        let ticks_i = Rc::clone(&ticks);
        let gate_i = gate.clone();
        let id_i = Rc::clone(&id_cell);
        let id = gate
            .set_interval(
                Box::new(FakeTimer::new(Box::new(move |_| {
                    ticks_i.set(ticks_i.get() + 1);
                    log_i.borrow_mut().push(format!("tick{}", ticks_i.get()));
                    if ticks_i.get() >= 3 {
                        gate_i.clear_interval(id_i.get()).unwrap();
                    }
                }))),
                10,
            )
            .unwrap();
        id_cell.set(id);

        assert_eq!(runtime.pump().unwrap(), PumpStatus::Waiting(1_010));
        clock.set(1_010);
        assert_eq!(runtime.pump().unwrap(), PumpStatus::Waiting(1_020));
        clock.set(1_030);
        assert_eq!(
            runtime.pump().unwrap(),
            PumpStatus::Idle,
            "回调内自清后终止"
        );
        assert_eq!(*log.borrow(), ["tick1", "tick2", "tick3"]);
    }

    /// 嵌套钳制：定时器链第 7 层起延迟下限 5ms（任务卡简化档）。
    #[test]
    fn nested_timeout_chain_clamped_after_five_levels() {
        let clock = Rc::new(Cell::new(1_000u64));
        let mut runtime = JsrtRuntime::with_timer_clock(
            Document::new(),
            Box::new(FakeHost::noop()),
            Box::new({
                let clock = Rc::clone(&clock);
                move || clock.get()
            }),
        );
        let fired: Rc<RefCell<Vec<(u32, u64)>>> = Rc::new(RefCell::new(Vec::new()));
        let gate = runtime.gate().clone();

        schedule_chain(&gate, &fired, &clock, 0, 6);
        runtime.pump().unwrap();
        assert_eq!(
            fired.borrow().len(),
            6,
            "第 0–5 层延迟 0 不受钳制，第 6 层被钳到 +5ms"
        );
        assert_eq!(runtime.pump().unwrap(), PumpStatus::Waiting(1_005));

        clock.set(1_005);
        runtime.pump().unwrap();
        let fired = fired.borrow();
        assert_eq!(fired.len(), 7);
        assert_eq!(fired.last().unwrap(), &(6, 1_005), "第 6 层在 +5ms 跑");
        assert!(
            fired.iter().take(6).all(|&(_, at)| at == 1_000),
            "前 6 层同刻跑完"
        );
    }

    /// 递归构造定时器链：第 `depth` 层回调记录触发时刻并续排下一层。
    fn schedule_chain(
        gate: &JsrtGate,
        fired: &Rc<RefCell<Vec<(u32, u64)>>>,
        clock: &Rc<Cell<u64>>,
        depth: u32,
        max_depth: u32,
    ) {
        let fired = Rc::clone(fired);
        let clock = Rc::clone(clock);
        gate.set_timeout(
            Box::new(FakeTimer::new(Box::new(move |gate| {
                fired.borrow_mut().push((depth, clock.get()));
                if depth < max_depth {
                    schedule_chain(gate, &fired, &clock, depth + 1, max_depth);
                }
            }))),
            0,
        )
        .unwrap();
    }

    /// 定时器回调异常：错误上抛、一次性条目清除、重复定时器不再重排。
    #[test]
    fn timer_callback_error_propagates_and_stops_repeat() {
        struct ErrorTimer;

        impl JsrtTimerCallback for ErrorTimer {
            fn call(&mut self, _gate: &JsrtGate) -> Result<(), JsrtError> {
                Err(JsrtError::Script(ScriptError::new(
                    ScriptErrorKind::Runtime,
                    "boom",
                )))
            }
        }

        let clock = Rc::new(Cell::new(0u64));
        let mut runtime = JsrtRuntime::with_timer_clock(
            Document::new(),
            Box::new(FakeHost::noop()),
            Box::new({
                let clock = Rc::clone(&clock);
                move || clock.get()
            }),
        );
        let gate = runtime.gate().clone();

        gate.set_interval(Box::new(ErrorTimer), 0).unwrap();
        clock.set(1);

        let error = runtime.pump().unwrap_err();
        assert_eq!(
            error,
            JsrtError::Script(ScriptError::new(ScriptErrorKind::Runtime, "boom"))
        );
        assert_eq!(runtime.pump().unwrap(), PumpStatus::Idle, "异常后不再重排");
    }

    // ---- 重入防护 ----

    /// 泵执行任务体期间经 gate 借文档：报 Reentrant 而非 panic / 死锁。
    #[test]
    fn gate_access_during_dom_task_reports_reentrant() {
        let mut runtime = runtime();
        let gate = runtime.gate().clone();
        let observed = Rc::new(RefCell::new(None::<String>));

        let gate_body = gate.clone();
        let observed_body = Rc::clone(&observed);
        runtime
            .post_task(Task::new(move |_| {
                *observed_body.borrow_mut() = gate_body
                    .document_mut()
                    .err()
                    .map(|error| error.to_string());
            }))
            .unwrap();

        runtime.pump().unwrap();
        assert!(
            observed.borrow().is_some(),
            "任务体内经 gate 的文档访问必须拿到 Reentrant"
        );
    }

    /// 派发中的绑定调用安全：回调内改 textContent 走短借用，派发继续。
    #[test]
    fn binding_during_dispatch_is_safe() {
        let mut doc = Document::new();
        let target = element(&mut doc, "div");
        let runtime = JsrtRuntime::new(doc, Box::new(FakeHost::noop()));
        let gate = runtime.gate().clone();

        let gate_listener = gate.clone();
        let registry = *runtime.registry();
        gate.add_listener(
            target,
            "click",
            false,
            Box::new(FakeListener {
                log: log_new(),
                tag: "mutator",
                action: Some(Box::new(move |_| {
                    // 回调内经绑定面改文档：短借用，须成功并置脏位。
                    registry
                        .set_text_content(&gate_listener, target, "changed")
                        .unwrap();
                })),
            }),
        )
        .unwrap();

        runtime.take_mutated();
        runtime
            .registry()
            .dispatch_event(&gate, None, target, &mut Event::new("click", false, true))
            .unwrap();
        assert!(runtime.take_mutated(), "绑定改动置脏位");
        let doc = gate.document().unwrap();
        assert!(matches!(doc.node(target), Some(NodeKind::Element(_))));
        let text = doc.first_child(target).unwrap();
        assert_eq!(
            doc.node(text),
            Some(&NodeKind::Text("changed".into())),
            "回调内的绑定改动生效"
        );
    }

    // ---- T4 补面：documentElement / getElementsByTagName / 拉起载荷 ----

    /// documentElement 返回文档根的第一个元素子节点；无元素时为空。
    #[test]
    fn document_element_returns_first_element_child_of_root() {
        let mut doc = Document::new();
        let html = element(&mut doc, "html");
        doc.append_child(html, doc.root()).unwrap();
        let doctype = doc.create_node(NodeKind::Doctype(crate::DocumentTypeData {
            name: "html".into(),
            public_id: String::new(),
            system_id: String::new(),
        }));
        doc.insert_node(doctype, doc.root(), None);

        let runtime = JsrtRuntime::new(doc, Box::new(FakeHost::noop()));
        let gate = runtime.gate().clone();
        let found = runtime.registry().document_element(&gate).unwrap();
        assert_eq!(found, Some(html));
    }

    /// getElementsByTagName 按文档序收集 HTML 命名空间同名元素（大小写
    /// 不敏感），非 HTML 命名空间不收。
    #[test]
    fn get_elements_by_tag_name_collects_in_document_order() {
        let mut doc = Document::new();
        let html = element(&mut doc, "html");
        doc.append_child(html, doc.root()).unwrap();
        let first = element(&mut doc, "font");
        let second = element(&mut doc, "FONT");
        let svg = doc.create_node(NodeKind::Element(ElementData {
            name: "font".into(),
            namespace: Namespace::Svg,
            attributes: Vec::new(),
        }));
        for node in [first, second, svg] {
            doc.append_child(node, html).unwrap();
        }

        let runtime = JsrtRuntime::new(doc, Box::new(FakeHost::noop()));
        let gate = runtime.gate().clone();
        let found = runtime
            .registry()
            .get_elements_by_tag_name(&gate, "font")
            .unwrap();
        assert_eq!(found, vec![first, second], "文档序 + 仅 HTML 命名空间");
    }

    /// 拉起载荷携带 target / currentTarget 索引（T4：事件门面还原用）。
    #[test]
    fn listener_invocation_carries_target_indices() {
        use std::cell::Cell;

        struct PayloadListener {
            seen: Rc<Cell<Option<(usize, usize)>>>,
        }

        impl JsrtListener for PayloadListener {
            fn call(
                &mut self,
                _gate: &JsrtGate,
                _scope: Option<&dyn JsrtScope>,
                invocation: &ListenerInvocation,
                _event: &mut Event,
            ) -> Result<(), JsrtError> {
                self.seen.set(Some((
                    invocation.target_index,
                    invocation.current_target_index,
                )));
                Ok(())
            }
        }

        let mut doc = Document::new();
        let root_child = element(&mut doc, "div");
        doc.append_child(root_child, doc.root()).unwrap();
        let child = element(&mut doc, "button");
        doc.append_child(child, root_child).unwrap();

        let runtime = JsrtRuntime::new(doc, Box::new(FakeHost::noop()));
        let gate = runtime.gate().clone();
        let seen = Rc::new(Cell::new(None::<(usize, usize)>));
        gate.add_listener(
            child,
            "click",
            false,
            Box::new(PayloadListener {
                seen: Rc::clone(&seen),
            }),
        )
        .unwrap();

        runtime
            .registry()
            .dispatch_event(&gate, None, child, &mut Event::new("click", true, true))
            .unwrap();
        // target 阶段：target 与 currentTarget 同为监听器所在节点。
        assert_eq!(
            seen.get(),
            Some((child.index(), child.index())),
            "target/currentTarget 均指向命中节点"
        );
    }
}
