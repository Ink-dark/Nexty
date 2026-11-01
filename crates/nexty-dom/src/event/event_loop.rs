//! 事件循环：FIFO 宏任务队列（[HTML §8.1 event loops]
//! (https://html.spec.whatwg.org/multipage/webappapis.html#event-loops) 简化档）。
//!
//! - [`Task`]：宏任务的自身 `FnOnce` 包装类型，运行时拿到 [`TaskContext`]——
//!   Document 访问入口 + 继续投递任务的唯一通道。
//! - [`EventLoop::post_task`] 投递到队尾；[`EventLoop::run_pending_tasks`]
//!   FIFO 逐个执行，当前任务跑完才取下一个——任务执行中投递的任务只入队，
//!   由同一循环在其后继续执行，不重入（任务拿不到队列的执行权，结构上排除
//!   递归排空）。
//! - 微任务队列已落地（`jsrt` feature）：[`EventLoop::post_microtask`] 投递、
//!   [`EventLoop::run_microtasks`] 在检查点清空到尽（微任务内新投的微任务同轮
//!   继续跑）；[`TaskContext::post_microtask`] 供任务体内投递。与引擎微任务源
//!   的交替清空由 `jsrt` 模块的泵合流（微任务先于下一个宏任务）。

use std::collections::VecDeque;
use std::fmt;

use crate::Document;

/// 宏任务：自身 `FnOnce` 包装类型（HTML
/// [task](https://html.spec.whatwg.org/multipage/webappapis.html#concept-task)
/// 简化档）。
///
/// 任务体签名固定为 `FnOnce(&mut TaskContext<'_>)`：运行时拿 Document 访问与
/// 继续投递任务的入口。
pub struct Task(Box<dyn for<'a, 'b> FnOnce(&'b mut TaskContext<'a>) + 'static>);

impl Task {
    /// 把闭包包装为任务。
    #[must_use]
    pub fn new<F>(run: F) -> Self
    where
        F: for<'a, 'b> FnOnce(&'b mut TaskContext<'a>) + 'static,
    {
        Self(Box::new(run))
    }
}

impl fmt::Debug for Task {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Task").finish_non_exhaustive()
    }
}

/// 单个任务的运行上下文：Document 访问 + 任务内投递后续任务（宏/微）的通道。
pub struct TaskContext<'a> {
    doc: &'a mut Document,
    tasks: &'a mut VecDeque<Task>,
    /// DOM 微任务队列（`jsrt` feature）：任务体内的微任务投递通道。
    #[cfg(feature = "jsrt")]
    microtasks: &'a mut VecDeque<Task>,
}

impl TaskContext<'_> {
    /// Document 访问入口（派发生命周期事件等）。
    pub fn document(&mut self) -> &mut Document {
        self.doc
    }

    /// 任务执行中投递后续任务：只入队尾，排在当前任务之后执行（不重入）。
    pub fn post_task(&mut self, task: Task) {
        self.tasks.push_back(task);
    }

    /// 任务执行中投递 DOM 微任务（`jsrt` feature）：在下一个微任务检查点执行，
    /// 先于下一个宏任务。
    #[cfg(feature = "jsrt")]
    pub fn post_microtask(&mut self, task: Task) {
        self.microtasks.push_back(task);
    }
}

/// FIFO 宏任务队列 + 微任务队列（HTML event loop 简化档：单源、同线程、无选择步）。
#[derive(Debug, Default)]
pub struct EventLoop {
    tasks: VecDeque<Task>,
    /// DOM 微任务队列（`jsrt` feature）：检查点清空到尽，先于下一个宏任务。
    #[cfg(feature = "jsrt")]
    microtasks: VecDeque<Task>,
}

impl EventLoop {
    /// 空队列。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 投递任务到队尾。
    pub fn post_task(&mut self, task: Task) {
        self.tasks.push_back(task);
    }

    /// FIFO 排空任务队列：每次取队首任务独占执行，当前任务跑完才取下一个；
    /// 执行中经 [`TaskContext::post_task`] 投递的任务追加到队尾，由本循环继续
    /// 按序执行——派发中 post 排队不重入。
    pub fn run_pending_tasks(&mut self, doc: &mut Document) {
        while let Some(task) = self.tasks.pop_front() {
            let mut ctx = TaskContext {
                doc: &mut *doc,
                tasks: &mut self.tasks,
                #[cfg(feature = "jsrt")]
                microtasks: &mut self.microtasks,
            };
            (task.0)(&mut ctx);
        }
    }

    /// 队列中是否还有宏任务。
    #[must_use]
    pub fn has_pending_tasks(&self) -> bool {
        !self.tasks.is_empty()
    }

    /// 取一个宏任务执行（`jsrt` feature：泵按「一个宏任务 → 微任务检查点」节奏
    /// 驱动时的单步入口）。返回是否执行了任务；队列空返回 false。
    pub fn run_next_task(&mut self, doc: &mut Document) -> bool {
        let Some(task) = self.tasks.pop_front() else {
            return false;
        };
        let mut ctx = TaskContext {
            doc,
            tasks: &mut self.tasks,
            #[cfg(feature = "jsrt")]
            microtasks: &mut self.microtasks,
        };
        (task.0)(&mut ctx);
        true
    }

    /// 微任务检查点：清空 DOM 微任务队列到尽（`jsrt` feature）。微任务内新投的
    /// 微任务同轮继续执行；返回本轮执行的微任务数。与引擎微任务源的交替清空
    /// 由 `jsrt` 模块的泵合流。
    #[cfg(feature = "jsrt")]
    pub fn run_microtasks(&mut self, doc: &mut Document) -> usize {
        let mut ran = 0;
        while let Some(task) = self.microtasks.pop_front() {
            let mut ctx = TaskContext {
                doc,
                tasks: &mut self.tasks,
                microtasks: &mut self.microtasks,
            };
            (task.0)(&mut ctx);
            ran += 1;
        }
        ran
    }

    /// 投递 DOM 微任务（`jsrt` feature）。
    #[cfg(feature = "jsrt")]
    pub fn post_microtask(&mut self, task: Task) {
        self.microtasks.push_back(task);
    }

    /// 是否还有待执行的 DOM 微任务（`jsrt` feature）。
    #[cfg(feature = "jsrt")]
    #[must_use]
    pub fn has_pending_microtasks(&self) -> bool {
        !self.microtasks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    type Log = Rc<RefCell<Vec<&'static str>>>;

    /// T2 ① FIFO 顺序。
    #[test]
    fn tasks_run_in_fifo_order() {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let mut event_loop = EventLoop::new();
        let mut doc = Document::new();

        for label in ["a", "b", "c"] {
            let log = Rc::clone(&log);
            event_loop.post_task(Task::new(move |_| {
                log.borrow_mut().push(label);
            }));
        }

        event_loop.run_pending_tasks(&mut doc);
        assert_eq!(*log.borrow(), ["a", "b", "c"]);
    }

    /// T2 ② 派发中 post_task 排队不重入：b 只能在 a 跑完之后执行。
    #[test]
    fn task_posted_during_run_executes_after_current_completes() {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let mut event_loop = EventLoop::new();
        let mut doc = Document::new();

        let log_a = Rc::clone(&log);
        event_loop.post_task(Task::new(move |ctx| {
            log_a.borrow_mut().push("a-start");
            let log_b = Rc::clone(&log_a);
            ctx.post_task(Task::new(move |_| {
                log_b.borrow_mut().push("b");
            }));
            log_a.borrow_mut().push("a-end");
        }));

        event_loop.run_pending_tasks(&mut doc);
        assert_eq!(
            *log.borrow(),
            ["a-start", "a-end", "b"],
            "b 不得在 a 执行期间重入"
        );
    }
}

#[cfg(all(test, feature = "jsrt"))]
mod jsrt_tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    type Log = Rc<RefCell<Vec<&'static str>>>;

    /// 微任务先于下一个宏任务：宏任务 A 投递微任务 M 与宏任务 B，泵节奏
    /// 「宏任务 → 微任务检查点 → 宏任务」下 M 先于 B。
    #[test]
    fn microtasks_drain_before_next_macrotask() {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let mut event_loop = EventLoop::new();
        let mut doc = Document::new();

        let log_a = Rc::clone(&log);
        event_loop.post_task(Task::new(move |ctx| {
            log_a.borrow_mut().push("a");
            let log_m = Rc::clone(&log_a);
            ctx.post_microtask(Task::new(move |_| {
                log_m.borrow_mut().push("m");
            }));
            let log_b = Rc::clone(&log_a);
            ctx.post_task(Task::new(move |_| {
                log_b.borrow_mut().push("b");
            }));
        }));

        assert!(event_loop.run_next_task(&mut doc));
        event_loop.run_microtasks(&mut doc);
        assert!(event_loop.run_next_task(&mut doc));
        assert_eq!(*log.borrow(), ["a", "m", "b"], "微任务先于下一个宏任务");
    }

    /// 微任务清空到尽：微任务内新投的微任务同轮继续执行（不留在队列里）。
    #[test]
    fn microtasks_drain_until_empty() {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let mut event_loop = EventLoop::new();
        let mut doc = Document::new();

        let log_m1 = Rc::clone(&log);
        event_loop.post_microtask(Task::new(move |ctx| {
            log_m1.borrow_mut().push("m1");
            let log_m2 = Rc::clone(&log_m1);
            ctx.post_microtask(Task::new(move |_| {
                log_m2.borrow_mut().push("m2");
            }));
        }));

        let ran = event_loop.run_microtasks(&mut doc);
        assert_eq!(ran, 2, "同轮清空到尽");
        assert_eq!(*log.borrow(), ["m1", "m2"]);
        assert!(!event_loop.has_pending_microtasks());
    }
}
