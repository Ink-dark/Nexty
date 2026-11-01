//! 定时器注册表：setTimeout / setInterval 的 ID 管理、到期判定与嵌套钳制。
//!
//! 行为 ground truth：HTML §8.1.4
//! [timers](https://html.spec.whatwg.org/multipage/webappapis.html#timers) 简化档。
//!
//! ## 偏差清单（简化档）
//!
//! - 嵌套钳制：规范在嵌套层级 > 5 后把延迟下限钳到 4ms，且按定时器链历史计层；
//!   此处按定时器链层级计层（新定时器层级 = 运行中定时器层级 + 1，链外调度为
//!   0），钳制下限取 **5ms**（任务卡简化档）。层级随条目存储（见
//!   [`TimerEntry::nesting`]），由泵在回调执行期间写入运行状态供
//!   `set_timeout` / `set_interval` 读取。
//! - `clearTimeout` 与 `clearInterval` 共用同一 ID 空间（与浏览器一致，可互换）。
//! - setInterval 的重复按 `due + interval` 排下次（不补齐回调耗时期间漏掉的
//!   次数）；回调异常后不再重排（泵侧语义）。
//! - 回调不携带额外参数（HTML 允许 `setTimeout(fn, delay, ...args)`，v0 未做）。

use std::collections::HashMap;

use super::{JsrtError, JsrtGate};

/// 定时器回调面：到期由泵拉起，`gate` 提供文档与泵的短借用访问。
///
/// 由适配 crate 实现（把引擎侧函数值封装为回调），经
/// [`JsrtGate::set_timeout`](super::JsrtGate::set_timeout) 登记。
///
/// # Errors
///
/// 引擎回调异常时上抛 [`JsrtError`]；重复定时器异常后不再重排。
pub trait JsrtTimerCallback {
    /// 执行回调。
    ///
    /// # Errors
    ///
    /// 引擎回调异常时上抛 [`JsrtError`]；重复定时器异常后不再重排。
    fn call(&mut self, gate: &JsrtGate) -> Result<(), JsrtError>;
}

/// 嵌套钳制阈值：新定时器的嵌套层级超过该层级后，延迟取下限（HTML §8.1.4
/// 简化档）。
pub(crate) const NESTING_CLAMP_THRESHOLD: u32 = 5;

/// 嵌套钳制下限（毫秒）。任务卡简化档取 5ms（规范为 4ms）。
pub(crate) const NESTING_CLAMP_MIN_DELAY_MS: u64 = 5;

/// 单条定时器登记。执行期间 `callback` 被取出（避免跨回调持借用）。
struct TimerEntry {
    due_ms: u64,
    interval: Option<u64>,
    /// 本定时器的嵌套层级：链外调度为 0，定时器回调内调度为运行层 + 1。
    nesting: u32,
    callback: Option<Box<dyn JsrtTimerCallback>>,
}

/// 定时器注册表：ID → 到期时间 / 重复间隔 / 嵌套层级 / 回调。
pub(crate) struct TimerRegistry {
    entries: HashMap<u32, TimerEntry>,
    next_id: u32,
    clock: Box<dyn Fn() -> u64>,
}

/// [`TimerRegistry::pop_due`](TimerRegistry::pop_due) 取出的到期条目：
/// `(id, due, interval, nesting, callback)`。
type DueTimer = (u32, u64, Option<u64>, u32, Box<dyn JsrtTimerCallback>);

impl TimerRegistry {
    /// 以注入时钟构造（宿主以系统时钟或测试时钟注入，手动推进时行为完全确定）。
    pub(crate) fn with_clock(clock: Box<dyn Fn() -> u64>) -> Self {
        Self {
            entries: HashMap::new(),
            next_id: 1,
            clock,
        }
    }

    /// 当前时刻（毫秒）。
    pub(crate) fn now(&self) -> u64 {
        (self.clock)()
    }

    /// 登记定时器，返回 ID（从 1 起，同 JS 语义；0 不是合法 ID）。
    /// `interval` 为 `Some` 时是重复定时器，下次到期 = 本次到期 + 间隔。
    /// `nesting` 为本定时器的嵌套层级（钳制判定由调用方按运行层级完成）。
    pub(crate) fn schedule(
        &mut self,
        delay_ms: u64,
        interval: Option<u64>,
        callback: Box<dyn JsrtTimerCallback>,
        nesting: u32,
    ) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.insert(
            id,
            TimerEntry {
                due_ms: self.now().saturating_add(delay_ms),
                interval,
                nesting,
                callback: Some(callback),
            },
        );
        id
    }

    /// 取消定时器（clearTimeout / clearInterval 共用）。返回是否确有取消。
    pub(crate) fn cancel(&mut self, id: u32) -> bool {
        self.entries.remove(&id).is_some()
    }

    /// 取出到期回调：`(due, id)` 升序下第一条 `due <= now` 的登记，取出其回调。
    /// 取出后条目保留（`callback` 为 `None`），由泵按存活性与重复间隔重排或清除。
    pub(crate) fn pop_due(&mut self, now: u64) -> Option<DueTimer> {
        let id = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.callback.is_some() && entry.due_ms <= now)
            .min_by_key(|(id, entry)| (entry.due_ms, **id))
            .map(|(id, _)| *id)?;
        let entry = self.entries.get_mut(&id)?;
        let callback = entry.callback.take()?;
        Some((id, entry.due_ms, entry.interval, entry.nesting, callback))
    }

    /// 重排取出的回调：条目仍在（未被清除）时放回回调并更新到期时间；
    /// 条目已被清除则丢弃回调。
    pub(crate) fn reschedule(
        &mut self,
        id: u32,
        due_ms: u64,
        callback: Box<dyn JsrtTimerCallback>,
    ) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.due_ms = due_ms;
            entry.callback = Some(callback);
        }
    }

    /// 最近一次到期时刻（仅统计还挂着回调的条目）；无登记返回 `None`。
    /// 泵据此向宿主报告下次驱动时机（[`super::PumpStatus::Waiting`]）。
    pub(crate) fn next_due(&self) -> Option<u64> {
        self.entries
            .values()
            .filter(|entry| entry.callback.is_some())
            .map(|entry| entry.due_ms)
            .min()
    }

    /// 是否还有任何登记（测试断言用）。
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl std::fmt::Debug for TimerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let live = self
            .entries
            .values()
            .filter(|entry| entry.callback.is_some())
            .count();
        f.debug_struct("TimerRegistry")
            .field("entries", &self.entries.len())
            .field("live", &live)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    /// 空回调（只验证注册表的登记 / 取出 / 清除，不执行）。
    struct Noop;

    impl JsrtTimerCallback for Noop {
        fn call(&mut self, _gate: &JsrtGate) -> Result<(), JsrtError> {
            Ok(())
        }
    }

    /// 固定起点 + 手动推进的注入时钟。
    fn clock_at(start: u64) -> (Box<dyn Fn() -> u64>, Rc<Cell<u64>>) {
        let cell = Rc::new(Cell::new(start));
        let handle = Rc::clone(&cell);
        (Box::new(move || cell.get()), handle)
    }

    #[test]
    fn ids_start_at_one_and_increase() {
        let (clock, _handle) = clock_at(0);
        let mut timers = TimerRegistry::with_clock(clock);
        assert_eq!(timers.schedule(0, None, Box::new(Noop), 0), 1);
        assert_eq!(timers.schedule(0, None, Box::new(Noop), 0), 2);
    }

    #[test]
    fn pop_due_picks_earliest_due_then_lowest_id() {
        let (clock, _handle) = clock_at(100);
        let mut timers = TimerRegistry::with_clock(clock);
        let a = timers.schedule(50, None, Box::new(Noop), 0); // due 150
        let _b = timers.schedule(10, None, Box::new(Noop), 0); // due 110
        let c = timers.schedule(50, None, Box::new(Noop), 0); // due 150

        // 时刻 110：只有 b 到期。
        let (id, due, interval, nesting, _callback) = timers.pop_due(110).unwrap();
        assert_eq!(id, _b);
        assert_eq!(due, 110);
        assert_eq!(interval, None);
        assert_eq!(nesting, 0);

        // 时刻 150：a 与 c 同到期，按 id 升序先取 a；b 的条目已取走回调，不再参与。
        let (id, _due, _interval, _nesting, _callback) = timers.pop_due(150).unwrap();
        assert_eq!(id, a);
        let (id, _due, _interval, _nesting, _callback) = timers.pop_due(150).unwrap();
        assert_eq!(id, c);
        assert!(timers.pop_due(150).is_none(), "全部取尽");
    }

    #[test]
    fn cancel_removes_entry_and_reports_repeat() {
        let (clock, _handle) = clock_at(0);
        let mut timers = TimerRegistry::with_clock(clock);
        let id = timers.schedule(10, None, Box::new(Noop), 0);
        assert!(timers.cancel(id));
        assert!(!timers.cancel(id), "重复取消返回 false");
        assert!(timers.pop_due(100).is_none());
        assert!(timers.is_empty());
    }

    #[test]
    fn reschedule_after_clear_drops_callback() {
        let (clock, _handle) = clock_at(0);
        let mut timers = TimerRegistry::with_clock(clock);
        let id = timers.schedule(10, Some(10), Box::new(Noop), 0);
        let (_id, due, _interval, _nesting, callback) = timers.pop_due(10).unwrap();
        // 回调执行期间自身被清除：条目消失，重排丢弃回调。
        assert!(timers.cancel(id));
        timers.reschedule(id, due + 10, callback);
        assert!(timers.pop_due(1000).is_none(), "回调不复活");
        assert!(timers.is_empty(), "清除后条目不残留");
    }

    #[test]
    fn next_due_reports_earliest_live_entry() {
        let (clock, handle) = clock_at(1000);
        let mut timers = TimerRegistry::with_clock(clock);
        assert_eq!(timers.next_due(), None, "空表无到期");
        timers.schedule(50, None, Box::new(Noop), 0);
        let later = timers.schedule(500, None, Box::new(Noop), 0);
        assert_eq!(timers.next_due(), Some(1050));

        // 取走回调的条目（执行中）不参与最近到期。
        let (id, _due, _interval, _nesting, callback) = timers.pop_due(1050).unwrap();
        assert_eq!(timers.next_due(), Some(1500));
        timers.reschedule(id, 1050, callback);
        assert_eq!(timers.next_due(), Some(1050));

        handle.set(2000);
        let third = timers.schedule(0, None, Box::new(Noop), 0);
        assert_eq!(third, later + 1, "ID 单调递增不复用");
        assert_eq!(timers.next_due(), Some(1050), "到期时刻仍是最早的 1050");
    }

    #[test]
    fn due_is_scheduled_relative_to_clock() {
        let (clock, handle) = clock_at(10);
        let mut timers = TimerRegistry::with_clock(clock);
        timers.schedule(5, None, Box::new(Noop), 0);
        handle.set(100);
        timers.schedule(5, None, Box::new(Noop), 0);
        assert_eq!(timers.next_due(), Some(15), "第一条按登记时刻 + 延迟");
        handle.set(15);
        let (_id, _due, _interval, _nesting, _callback) = timers.pop_due(15).unwrap();
        assert_eq!(timers.next_due(), Some(105), "第二条按当前时刻 + 延迟");
    }
}
