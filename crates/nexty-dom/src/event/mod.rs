//! 事件模型：Event / 监听器注册表 / 传播算法（WHATWG DOM Standard §2.9 简化档）。
//!
//! 行为 ground truth：[DOM Standard §2.9 Dispatching events]
//! (https://dom.spec.whatwg.org/#dispatching-events)。
//!
//! 结构：
//! - [`Event`]：事件核心（type / bubbles / cancelable / eventPhase /
//!   defaultPrevented / timeStamp）与三个干预方法。
//! - 监听器为自身 trait 类型 [`EventListener`]（另含 `FnMut(&mut Event)` 的
//!   便捷实现），按（事件类型，capture 标志）登记在
//!   [`Document`](crate::Document) 侧的注册表容器——arena 节点不挂堆结构；
//!   EventTarget 语义（add / remove / dispatch）由 `Document` 方法承载。
//! - 传播（[`Document::dispatch`]）：capture 沿祖先链下行 → target → bubble
//!   上行；target 上 capture 与非 capture 监听器都触发且按注册顺序；
//!   `bubbles=false` 停在 target。
//! - 事件数据面见 [`data`] 模块；任务队列见 [`event_loop`] 模块。
//!
//! ## 偏差清单（DOM §2.9 简化档）
//!
//! - 无 shadow tree / retargeting / composed path；无 trusted 语义（无 isTrusted）。
//! - 监听器身份是 [`ListenerId`] 句柄而非回调对象地址——`Box` 移交注册表后
//!   对象地址身份不可用；允许重复注册（同 DOM 语义：触发两次，各按句柄独立移除）。
//! - 无 passive / once / AbortSignal 选项。
//! - 监听器回调只接触核心 [`Event`] 面；类型化数据（[`MouseEvent`] 等）由派发方
//!   持有，回调读不到——K5 接 JS 绑定时再扩回调签名。
//! - 监听器不随节点从树中摘下而清理（游离节点按 DOM 语义保留监听器，随
//!   `Document` 一起释放）。
//! - 事件对象按单次派发使用：stop / canceled 标志不随派发重置（同规范），
//!   复用同一对象再派发会携带旧标志。
//! - [`FocusEvent`] 仅类型面，无派发点（K10 焦点模型补）；无 pointer capture /
//!   合成事件 / IME（本轮明确不做）。
//! - 微任务队列只留接口空位（K5 填充），见 [`EventLoop`] 模块文档。

mod data;
mod event_loop;

pub use data::{DeltaMode, FocusEvent, KeyboardEvent, MouseEvent, UiEvent, WheelEvent};
pub use event_loop::{EventLoop, Task, TaskContext};

use std::collections::HashMap;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{Document, NodeId};

/// `DOMContentLoaded` 生命周期事件类型字符串。
///
/// bubbles=true、cancelable=false，在 Document 上派发（HTML 语法章节
/// [the end](https://html.spec.whatwg.org/multipage/syntax.html#the-end) 的
/// DOMContentLoaded 报告步骤）；派发时机由 chrome 接线（K1 T3）。
pub const DOM_CONTENT_LOADED: &str = "DOMContentLoaded";

/// `load` 生命周期事件类型字符串。
///
/// bubbles=false、cancelable=false，在 Document 上派发（HTML 语法章节
/// [the end](https://html.spec.whatwg.org/multipage/syntax.html#the-end) 的
/// load 事件步骤）；派发时机由 chrome 接线（K1 T3，子资源编排 settle 点）。
pub const LOAD: &str = "load";

/// 事件传播阶段（DOM Standard
/// [eventPhase](https://dom.spec.whatwg.org/#dom-event-eventphase)）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventPhase {
    /// 未在派发中（构造后、派发结束后）。
    None,
    /// 捕获阶段：祖先链下行，只触发 capture 监听器。
    Capturing,
    /// 位于 target：capture 与非 capture 监听器都触发、按注册顺序。
    AtTarget,
    /// 冒泡阶段：祖先链上行，只触发非 capture 监听器。
    Bubbling,
}

/// 事件核心（DOM Standard [Event](https://dom.spec.whatwg.org/#interface-event)
/// 简化档：无 shadow、无 trusted）。
///
/// 生命周期事件直接用 [`Event::dom_content_loaded`] / [`Event::load`] 构造；
/// 带数据的事件见数据面类型（[`MouseEvent`] 等，各自组合 `event: Event`）。
#[derive(Debug)]
pub struct Event {
    type_: String,
    bubbles: bool,
    cancelable: bool,
    phase: EventPhase,
    default_prevented: bool,
    stop_propagation_flag: bool,
    stop_immediate_propagation_flag: bool,
    time_stamp: u64,
}

impl Event {
    /// 构造事件。
    ///
    /// `timeStamp` 取构造时刻的 Unix 毫秒数（饱和）；标志位全部初始为假，
    /// 阶段为 [`EventPhase::None`]。
    #[must_use]
    pub fn new(type_: impl Into<String>, bubbles: bool, cancelable: bool) -> Self {
        let time_stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since_epoch| {
                u64::try_from(since_epoch.as_millis()).unwrap_or(u64::MAX)
            });
        Self {
            type_: type_.into(),
            bubbles,
            cancelable,
            phase: EventPhase::None,
            default_prevented: false,
            stop_propagation_flag: false,
            stop_immediate_propagation_flag: false,
            time_stamp,
        }
    }

    /// DOM `type` 属性：事件类型字符串。
    #[must_use]
    pub fn type_(&self) -> &str {
        &self.type_
    }

    /// `bubbles` 标志：是否沿祖先链冒泡。
    #[must_use]
    pub fn bubbles(&self) -> bool {
        self.bubbles
    }

    /// `cancelable` 标志：[`prevent_default`](Self::prevent_default) 是否有效。
    #[must_use]
    pub fn cancelable(&self) -> bool {
        self.cancelable
    }

    /// 当前传播阶段；派发结束后回到 [`EventPhase::None`]。
    #[must_use]
    pub fn event_phase(&self) -> EventPhase {
        self.phase
    }

    /// `defaultPrevented`：默认动作是否已被取消（仅 cancelable 事件可为真）。
    #[must_use]
    pub fn default_prevented(&self) -> bool {
        self.default_prevented
    }

    /// `timeStamp`：构造时刻的 Unix 毫秒数。
    #[must_use]
    pub fn time_stamp(&self) -> u64 {
        self.time_stamp
    }

    /// `DOMContentLoaded` 生命周期事件（HTML 语法章节
    /// [the end](https://html.spec.whatwg.org/multipage/syntax.html#the-end)）：
    /// bubbles=true、cancelable=false，在 Document 上派发；派发时机由 chrome
    /// 接线（K1 T3）。
    #[must_use]
    pub fn dom_content_loaded() -> Self {
        Self::new(DOM_CONTENT_LOADED, true, false)
    }

    /// `load` 生命周期事件（同上）：bubbles=false、cancelable=false，在
    /// Document 上派发；派发时机由 chrome 接线（K1 T3）。
    #[must_use]
    pub fn load() -> Self {
        Self::new(LOAD, false, false)
    }

    /// 阻断事件向后续节点与阶段传播；当前节点已排到的剩余监听器照常触发
    /// （DOM Standard
    /// [stopPropagation](https://dom.spec.whatwg.org/#dom-event-stoppropagation)）。
    pub fn stop_propagation(&mut self) {
        self.stop_propagation_flag = true;
    }

    /// 在 [`stop_propagation`](Self::stop_propagation) 的基础上同时终止当前
    /// 节点剩余监听器（DOM Standard
    /// [stopImmediatePropagation](https://dom.spec.whatwg.org/#dom-event-stopimmediatepropagation)）。
    pub fn stop_immediate_propagation(&mut self) {
        self.stop_propagation_flag = true;
        self.stop_immediate_propagation_flag = true;
    }

    /// 取消默认动作；cancelable=false 的事件上无效（DOM Standard
    /// [preventDefault](https://dom.spec.whatwg.org/#dom-event-preventdefault)）。
    pub fn prevent_default(&mut self) {
        if self.cancelable {
            self.default_prevented = true;
        }
    }
}

/// 事件监听器（自身 trait 类型，不外泄依赖类型——本 crate 本就零依赖）。
///
/// 回调只接触核心 [`Event`] 面（阶段与干预标志）；类型化数据由派发方持有。
/// 闭包可经 `FnMut(&mut Event)` 便捷实现直接当监听器用——闭包参数须标注
/// `&mut Event` 类型，未标注时 rustc 的高阶生命周期推断可能报
/// "not general enough"。
pub trait EventListener {
    /// 处理事件。可在回调内调用 [`stop_propagation`](Event::stop_propagation) /
    /// [`stop_immediate_propagation`](Event::stop_immediate_propagation) /
    /// [`prevent_default`](Event::prevent_default)。
    fn handle_event(&mut self, event: &mut Event);
}

impl<F> EventListener for F
where
    F: FnMut(&mut Event),
{
    fn handle_event(&mut self, event: &mut Event) {
        self(event);
    }
}

/// 监听器句柄：[`Document::add_listener`] 返回，[`Document::remove_listener`]
/// 凭句柄移除。
///
/// 用句柄而非对象地址作身份——监听器 `Box` 移交注册表后地址身份不可用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ListenerId(u64);

/// 单条监听器登记（注册表内部）。
struct ListenerEntry {
    capture: bool,
    id: ListenerId,
    listener: Box<dyn EventListener>,
}

/// 监听器注册表：节点 → 事件类型 → 注册序条目。
///
/// 同一（节点，类型）下的条目保持注册顺序（DOM 的 event listener list 是
/// 单条有序表），capture 标志记录在条目上——capture / bubble 阶段按标志过滤，
/// target 阶段两种混序按同一注册顺序触发（DOM §2.9「invoke」）。事件类型作
/// 哈希键、条目为小 Vec，与（type, capture）存取语义等价且省一次排序。
#[derive(Default)]
pub(crate) struct ListenerRegistry {
    by_node: HashMap<NodeId, HashMap<String, Vec<ListenerEntry>>>,
    next_id: u64,
}

impl ListenerRegistry {
    /// 登记监听器，返回移除用句柄。允许重复注册（同 DOM：触发两次）。
    fn add(
        &mut self,
        target: NodeId,
        event_type: &str,
        capture: bool,
        listener: Box<dyn EventListener>,
    ) -> ListenerId {
        let id = ListenerId(self.next_id);
        self.next_id += 1;
        self.by_node
            .entry(target)
            .or_default()
            .entry(event_type.to_string())
            .or_default()
            .push(ListenerEntry {
                capture,
                id,
                listener,
            });
        id
    }

    /// 移除指定条目；未找到返回 false。
    fn remove(&mut self, target: NodeId, event_type: &str, id: ListenerId) -> bool {
        let Some(entries) = self
            .by_node
            .get_mut(&target)
            .and_then(|by_type| by_type.get_mut(event_type))
        else {
            return false;
        };
        let Some(position) = entries.iter().position(|entry| entry.id == id) else {
            return false;
        };
        entries.remove(position);
        if entries.is_empty()
            && let Some(by_type) = self.by_node.get_mut(&target)
        {
            by_type.remove(event_type);
            if by_type.is_empty() {
                self.by_node.remove(&target);
            }
        }
        true
    }

    /// 按（节点，类型）取条目表。
    fn for_type_mut(
        &mut self,
        target: NodeId,
        event_type: &str,
    ) -> Option<&mut Vec<ListenerEntry>> {
        self.by_node.get_mut(&target)?.get_mut(event_type)
    }
}

impl fmt::Debug for ListenerRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let listener_count: usize = self
            .by_node
            .values()
            .map(|by_type| by_type.values().map(Vec::len).sum::<usize>())
            .sum();
        f.debug_struct("ListenerRegistry")
            .field("nodes", &self.by_node.len())
            .field("listeners", &listener_count)
            .finish()
    }
}

impl Document {
    /// 在 `target` 上登记 `event_type` 监听器（DOM
    /// [addEventListener](https://dom.spec.whatwg.org/#dom-eventtarget-addeventlistener)
    /// 简化档：无 passive / once / signal）。`capture` 为真时该监听器在捕获
    /// 阶段触发（在 target 上则随注册顺序参与 target 阶段）。
    ///
    /// 返回 [`ListenerId`] 供 [`remove_listener`](Self::remove_listener) 使用；
    /// 允许重复注册（同 DOM 语义：触发两次）。
    pub fn add_listener(
        &mut self,
        target: NodeId,
        event_type: &str,
        capture: bool,
        listener: Box<dyn EventListener>,
    ) -> ListenerId {
        self.listeners.add(target, event_type, capture, listener)
    }

    /// 移除监听器（DOM
    /// [removeEventListener](https://dom.spec.whatwg.org/#dom-eventtarget-removeeventlistener)
    /// 简化档）。返回是否确有移除；同一句柄重复移除返回 false。
    pub fn remove_listener(&mut self, target: NodeId, event_type: &str, id: ListenerId) -> bool {
        self.listeners.remove(target, event_type, id)
    }

    /// 派发事件（DOM Standard
    /// [dispatch](https://dom.spec.whatwg.org/#concept-event-dispatch) 简化档：
    /// 无 shadow、无 retargeting）。
    ///
    /// 传播：capture 沿祖先链自根下行 → target（capture 与非 capture 监听器
    /// 都触发、按注册顺序）→ bubble 上行；`bubbles=false` 停在 target；target
    /// 无祖先链（含 Document 根）时仅 target 阶段。
    ///
    /// 返回「未取消」：false 表示 cancelable 事件已被
    /// [`prevent_default`](Event::prevent_default) 取消，调用方应跳过默认动作
    /// （先派发后默认契约）。
    pub fn dispatch(&mut self, target: NodeId, event: &mut Event) -> bool {
        // 祖先链（根 → target 的直接父节点，自上而下）。深度受 MAX_TREE_DEPTH
        // 约束，迭代收集不递归。
        let mut chain = Vec::new();
        let mut current = self.parent(target);
        while let Some(node) = current {
            chain.push(node);
            current = self.parent(node);
        }
        chain.reverse();

        for &node in &chain {
            if event.stop_propagation_flag {
                break;
            }
            event.phase = EventPhase::Capturing;
            self.invoke(node, event);
        }
        if !event.stop_propagation_flag {
            event.phase = EventPhase::AtTarget;
            self.invoke(target, event);
        }
        if event.bubbles {
            for &node in chain.iter().rev() {
                if event.stop_propagation_flag {
                    break;
                }
                event.phase = EventPhase::Bubbling;
                self.invoke(node, event);
            }
        }
        event.phase = EventPhase::None;
        !event.default_prevented
    }

    /// invoke（DOM §2.9「invoke an event listener」简化档）：按事件当前阶段
    /// 过滤条目——Capturing 只触发 capture 条目、Bubbling 只触发非 capture、
    /// AtTarget 全部按注册序触发；stopImmediatePropagation 终止当前节点剩余
    /// 监听器，stopPropagation 不影响当前节点、只阻断后续节点与阶段。
    fn invoke(&mut self, node: NodeId, event: &mut Event) {
        let (capture_only, all_entries) = match event.phase {
            EventPhase::Capturing => (true, false),
            EventPhase::AtTarget => (false, true),
            EventPhase::Bubbling => (false, false),
            EventPhase::None => return,
        };
        let type_ = event.type_();
        let Some(entries) = self.listeners.for_type_mut(node, type_) else {
            return;
        };
        for entry in entries.iter_mut() {
            if event.stop_immediate_propagation_flag {
                break;
            }
            if !all_entries && entry.capture != capture_only {
                continue;
            }
            entry.listener.handle_event(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ElementData, Namespace, NodeKind};
    use std::cell::RefCell;
    use std::rc::Rc;

    fn element(doc: &mut Document, name: &str) -> NodeId {
        doc.create_node(NodeKind::Element(ElementData {
            name: name.into(),
            namespace: Namespace::Html,
            attributes: Vec::new(),
        }))
    }

    type Log = Rc<RefCell<Vec<(&'static str, EventPhase)>>>;

    /// 监听动作：记录（标签， 阶段）后可选注入一种干预。
    #[derive(Clone, Copy)]
    enum Action {
        None,
        StopPropagation,
        StopImmediate,
        PreventDefault,
    }

    struct Recorder {
        label: &'static str,
        action: Action,
        log: Log,
    }

    impl EventListener for Recorder {
        fn handle_event(&mut self, event: &mut Event) {
            self.log
                .borrow_mut()
                .push((self.label, event.event_phase()));
            match self.action {
                Action::None => {}
                Action::StopPropagation => event.stop_propagation(),
                Action::StopImmediate => event.stop_immediate_propagation(),
                Action::PreventDefault => event.prevent_default(),
            }
        }
    }

    fn recorder(label: &'static str, action: Action, log: &Log) -> Box<dyn EventListener> {
        Box::new(Recorder {
            label,
            action,
            log: Rc::clone(log),
        })
    }

    fn labels_of(log: &Log) -> Vec<&'static str> {
        log.borrow().iter().map(|(label, _)| *label).collect()
    }

    // ---- T1 退出条件 ----

    /// ① 三阶段顺序与注册顺序（含 target 双捕获语义）。
    #[test]
    fn dispatch_runs_capture_target_bubble_in_registration_order() {
        let mut doc = Document::new();
        let outer = element(&mut doc, "outer");
        let inner = element(&mut doc, "inner");
        doc.append_child(outer, doc.root()).unwrap();
        doc.append_child(inner, outer).unwrap();

        let log: Log = Rc::new(RefCell::new(Vec::new()));
        doc.add_listener(
            doc.root(),
            "click",
            true,
            recorder("root-capture", Action::None, &log),
        );
        doc.add_listener(
            outer,
            "click",
            true,
            recorder("outer-capture", Action::None, &log),
        );
        doc.add_listener(
            outer,
            "click",
            false,
            recorder("outer-bubble", Action::None, &log),
        );
        // target 上 capture 与非 capture 混合注册：触发顺序 = 注册顺序。
        doc.add_listener(
            inner,
            "click",
            true,
            recorder("inner-cap-1", Action::None, &log),
        );
        doc.add_listener(
            inner,
            "click",
            false,
            recorder("inner-bubble", Action::None, &log),
        );
        doc.add_listener(
            inner,
            "click",
            true,
            recorder("inner-cap-2", Action::None, &log),
        );

        let mut event = Event::new("click", true, true);
        assert!(doc.dispatch(inner, &mut event));

        assert_eq!(
            labels_of(&log),
            [
                "root-capture",
                "outer-capture",
                "inner-cap-1",
                "inner-bubble",
                "inner-cap-2",
                "outer-bubble"
            ]
        );
        for (label, phase) in log.borrow().iter() {
            let expected = match *label {
                "root-capture" | "outer-capture" => EventPhase::Capturing,
                "outer-bubble" => EventPhase::Bubbling,
                _ => EventPhase::AtTarget,
            };
            assert_eq!(*phase, expected, "{label} 观察到的阶段不符");
        }
    }

    /// ② stopPropagation：同节点剩余监听器照跑，后续节点与冒泡阶段阻断。
    #[test]
    fn stop_propagation_keeps_remaining_target_listeners_and_blocks_ancestors() {
        let mut doc = Document::new();
        let parent = element(&mut doc, "parent");
        let target = element(&mut doc, "target");
        doc.append_child(parent, doc.root()).unwrap();
        doc.append_child(target, parent).unwrap();

        let log: Log = Rc::new(RefCell::new(Vec::new()));
        doc.add_listener(
            parent,
            "click",
            true,
            recorder("parent-capture", Action::None, &log),
        );
        doc.add_listener(
            parent,
            "click",
            false,
            recorder("parent-bubble", Action::None, &log),
        );
        doc.add_listener(target, "click", false, recorder("t1", Action::None, &log));
        doc.add_listener(
            target,
            "click",
            false,
            recorder("t2", Action::StopPropagation, &log),
        );
        doc.add_listener(target, "click", false, recorder("t3", Action::None, &log));

        let mut event = Event::new("click", true, true);
        doc.dispatch(target, &mut event);

        assert_eq!(
            labels_of(&log),
            ["parent-capture", "t1", "t2", "t3"],
            "stopPropagation：t3 照跑，parent-bubble 不触发"
        );
    }

    /// ② stopImmediatePropagation：额外终止当前节点剩余监听器（与上一条对照）。
    #[test]
    fn stop_immediate_propagation_additionally_stops_current_node() {
        let mut doc = Document::new();
        let parent = element(&mut doc, "parent");
        let target = element(&mut doc, "target");
        doc.append_child(parent, doc.root()).unwrap();
        doc.append_child(target, parent).unwrap();

        let log: Log = Rc::new(RefCell::new(Vec::new()));
        doc.add_listener(
            parent,
            "click",
            true,
            recorder("parent-capture", Action::None, &log),
        );
        doc.add_listener(target, "click", false, recorder("t1", Action::None, &log));
        doc.add_listener(
            target,
            "click",
            false,
            recorder("t2", Action::StopImmediate, &log),
        );
        doc.add_listener(target, "click", false, recorder("t3", Action::None, &log));

        let mut event = Event::new("click", true, true);
        doc.dispatch(target, &mut event);

        assert_eq!(
            labels_of(&log),
            ["parent-capture", "t1", "t2"],
            "stopImmediatePropagation：t3 不再触发"
        );
    }

    /// ③ preventDefault：cancelable=false 时无效；取消后 defaultPrevented=true
    /// 且 dispatch 返回 false。
    #[test]
    fn prevent_default_only_takes_effect_on_cancelable_events() {
        let mut doc = Document::new();
        let node = element(&mut doc, "a");
        doc.append_child(node, doc.root()).unwrap();

        let log: Log = Rc::new(RefCell::new(Vec::new()));
        doc.add_listener(
            node,
            "submit",
            false,
            recorder("prevent", Action::PreventDefault, &log),
        );

        let mut non_cancelable = Event::new("submit", true, false);
        assert!(
            doc.dispatch(node, &mut non_cancelable),
            "cancelable=false 时 preventDefault 无效，dispatch 返回未取消"
        );
        assert!(!non_cancelable.default_prevented());

        let mut cancelable = Event::new("submit", true, true);
        assert!(
            !doc.dispatch(node, &mut cancelable),
            "取消后 dispatch 返回 false（默认动作应跳过）"
        );
        assert!(cancelable.default_prevented());
    }

    /// ④ bubbles=false 停在 target（capture 链照常、无冒泡）。
    #[test]
    fn non_bubbling_event_stops_after_target() {
        let mut doc = Document::new();
        let outer = element(&mut doc, "outer");
        let inner = element(&mut doc, "inner");
        doc.append_child(outer, doc.root()).unwrap();
        doc.append_child(inner, outer).unwrap();

        let log: Log = Rc::new(RefCell::new(Vec::new()));
        doc.add_listener(
            doc.root(),
            "click",
            true,
            recorder("root-capture", Action::None, &log),
        );
        doc.add_listener(
            outer,
            "click",
            true,
            recorder("outer-capture", Action::None, &log),
        );
        doc.add_listener(
            outer,
            "click",
            false,
            recorder("outer-bubble", Action::None, &log),
        );
        doc.add_listener(
            inner,
            "click",
            false,
            recorder("inner-bubble", Action::None, &log),
        );

        let mut event = Event::new("click", false, true);
        doc.dispatch(inner, &mut event);

        assert_eq!(
            labels_of(&log),
            ["root-capture", "outer-capture", "inner-bubble"],
            "bubbles=false：capture 链照常、target 照常、无冒泡"
        );
    }

    /// ⑤ remove_listener 后不再触发；重复移除返回 false。
    #[test]
    fn remove_listener_takes_effect_on_subsequent_dispatch() {
        let mut doc = Document::new();
        let node = element(&mut doc, "a");
        doc.append_child(node, doc.root()).unwrap();

        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let id = doc.add_listener(node, "click", false, recorder("a", Action::None, &log));

        let mut first = Event::new("click", true, false);
        doc.dispatch(node, &mut first);
        assert_eq!(log.borrow().len(), 1);

        assert!(doc.remove_listener(node, "click", id));
        let mut second = Event::new("click", true, false);
        doc.dispatch(node, &mut second);
        assert_eq!(log.borrow().len(), 1, "移除后不再触发");

        assert!(
            !doc.remove_listener(node, "click", id),
            "重复移除返回 false"
        );
    }

    // ---- T2 退出条件（生命周期事件字段与 bubbles）----

    #[test]
    fn lifecycle_events_carry_documented_flags() {
        let dom_content_loaded = Event::dom_content_loaded();
        assert_eq!(dom_content_loaded.type_(), DOM_CONTENT_LOADED);
        assert!(dom_content_loaded.bubbles());
        assert!(!dom_content_loaded.cancelable());
        assert!(!dom_content_loaded.default_prevented());
        assert_eq!(dom_content_loaded.event_phase(), EventPhase::None);

        let load = Event::load();
        assert_eq!(load.type_(), LOAD);
        assert!(!load.bubbles(), "load 不冒泡");
        assert!(!load.cancelable());
        assert!(!load.default_prevented());
    }

    #[test]
    fn lifecycle_events_dispatch_on_document_root_at_target_only() {
        let mut doc = Document::new();
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        doc.add_listener(
            doc.root(),
            DOM_CONTENT_LOADED,
            false,
            recorder("dcl", Action::None, &log),
        );

        let mut event = Event::dom_content_loaded();
        assert!(doc.dispatch(doc.root(), &mut event));
        assert_eq!(
            *log.borrow(),
            [("dcl", EventPhase::AtTarget)],
            "根节点无祖先链：仅 target 阶段，恰好一次"
        );

        // capture 监听器在 target（根节点）上同样按注册序参与 target 阶段。
        doc.add_listener(
            doc.root(),
            LOAD,
            true,
            recorder("load-capture", Action::None, &log),
        );
        let mut load = Event::load();
        assert!(doc.dispatch(doc.root(), &mut load));
        assert_eq!(
            log.borrow().get(1),
            Some(&("load-capture", EventPhase::AtTarget))
        );
    }

    // ---- 补充钉住的语义 ----

    #[test]
    fn closure_listeners_invoke_through_blanket_impl() {
        let mut doc = Document::new();
        let node = element(&mut doc, "a");
        doc.append_child(node, doc.root()).unwrap();

        let hits = Rc::new(RefCell::new(0u32));
        let counter = Rc::clone(&hits);
        doc.add_listener(
            node,
            "click",
            false,
            Box::new(move |_event: &mut Event| {
                *counter.borrow_mut() += 1;
            }),
        );

        let mut event = Event::new("click", true, false);
        assert!(doc.dispatch(node, &mut event));
        assert_eq!(*hits.borrow(), 1);
    }

    #[test]
    fn dispatch_without_listeners_completes_and_resets_phase() {
        let mut doc = Document::new();
        let node = element(&mut doc, "div");
        doc.append_child(node, doc.root()).unwrap();

        let mut event = Event::new("hover", true, true);
        assert!(doc.dispatch(node, &mut event));
        assert_eq!(event.event_phase(), EventPhase::None, "派发结束后回到 None");
    }
}
