//! 交互事件接线与生命周期派发（K1 T3）——chrome 侧事件接线的集中点。
//!
//! 从 `app` 的输入路由迁出的事件派发部分全在本模块：
//!
//! - **交互事件**：`hit_test_ex` 命中节点（无命中退回文档根）上构造数据面
//!   事件（`MouseEvent` / `WheelEvent` / `KeyboardEvent`，bubbles=true、
//!   cancelable=true，字段由派发方填）并派发；`Document::dispatch` 返回
//!   「未取消」（true）→ 调用方执行默认动作。链接导航、滚轮滚动、键盘
//!   滚动与快捷键全部走「先派发后默认」——无监听器时行为与接线前一致。
//! - **生命周期**：页面安装点按跨线程信号（HTML 解析完成 / 首轮子资源
//!   编排 settle，见 [`PendingLifecycle`]）经 pipeline 的单处钩子
//!   `Page::post_lifecycle_tasks` post 进 [`EventLoop`]，`run_pending_tasks`
//!   排空派发（FIFO：DOMContentLoaded 先于 load；派发中 post 不重入）。
//!
//! 数据面构造与派发是纯函数（可无头单测）；winit → chrome UI 的路由
//! （地址栏、滚动条、悬停）仍在 `app.rs`，不属于页面 DOM 事件。
//!
//! 偏差（K1 简化档）：监听器回调只见核心 [`Event`] 面，读不到类型化数据
//! 字段（K5 扩回调签名）；`screenX/Y` 未跟踪（置 0）；无 mousemove/悬停
//! 派发（悬停只更新光标）；地址栏聚焦时按键属 chrome UI，不派发到页面。

use nexty_dom::{DeltaMode, Event, KeyboardEvent, MouseEvent, NodeId, WheelEvent};

use super::BrowserApp;
use crate::pipeline::{self, Page, PendingLifecycle};
use crate::scrollbar;
use crate::ui::{self, BAR_HEIGHT, Modifiers};

/// 派发鼠标事件（mousedown / mouseup / click）到 `target`。
///
/// `client` 为页面视口坐标（UI Events clientX/Y：相对视口，不含滚动偏移）；
/// button 固定主键 0，`buttons` 为按住位掩码（UI Events §5.2.2：按下主键
/// 为 1，释放后为 0）。bubbles=true、cancelable=true。返回「未取消」：
/// false = 已被 [`prevent_default`](nexty_dom::Event::prevent_default)，
/// 默认动作必须跳过。
pub(super) fn dispatch_mouse(
    page: &mut Page,
    target: NodeId,
    type_: &'static str,
    client: (f32, f32),
    buttons: u16,
    modifiers: Modifiers,
) -> bool {
    let mut event = MouseEvent::new(type_, true, true);
    event.detail = 1;
    event.client_x = client.0 as i32;
    event.client_y = client.1 as i32;
    event.button = 0;
    event.buttons = buttons;
    event.ctrl_key = modifiers.ctrl;
    event.shift_key = modifiers.shift;
    event.alt_key = modifiers.alt;
    page.document
        .borrow_mut()
        .dispatch(target, &mut event.event)
}

/// 派发滚轮事件（wheel）到 `target`。
///
/// `delta` 为 UI Events 语义（正值 deltaY = 页面向下滚），单位由 `mode`
/// 标注；简化档的 `WheelEvent` 数据面无坐标与修饰键字段。bubbles=true、
/// cancelable=true。返回「未取消」。
pub(super) fn dispatch_wheel(
    page: &mut Page,
    target: NodeId,
    delta: (f64, f64),
    mode: DeltaMode,
) -> bool {
    let mut event = WheelEvent::new("wheel", true, true);
    event.delta_x = delta.0;
    event.delta_y = delta.1;
    event.delta_mode = mode;
    page.document
        .borrow_mut()
        .dispatch(target, &mut event.event)
}

/// 派发键盘事件（keydown）到 `target`。
///
/// `key` / `code` 为 UI Events 键值与物理键位字符串；bubbles=true、
/// cancelable=true。返回「未取消」。
pub(super) fn dispatch_key(
    page: &mut Page,
    target: NodeId,
    key: &str,
    code: &str,
    modifiers: Modifiers,
    repeat: bool,
) -> bool {
    let mut event = KeyboardEvent::new("keydown", true, true);
    event.key = key.to_owned();
    event.code = code.to_owned();
    event.ctrl_key = modifiers.ctrl;
    event.shift_key = modifiers.shift;
    event.alt_key = modifiers.alt;
    event.repeat = repeat;
    page.document
        .borrow_mut()
        .dispatch(target, &mut event.event)
}

/// chrome 抽象键 → DOM `key`/`code` 字符串（KeyboardEvent 数据面填充用）。
///
/// 字符键 `key` 取原文、`code` 按 `Key<大写>`；命名键两处同名——物理键位
/// 的精确 `code` 面记入偏差清单（简化档不读 winit physical_key）。
pub(super) fn dom_key_names(key: ui::Key) -> (String, String) {
    match key {
        ui::Key::Character(character) => {
            let key = character.to_string();
            let code = format!("Key{}", character.to_ascii_uppercase());
            (key, code)
        }
        ui::Key::Left => ("ArrowLeft".to_owned(), "ArrowLeft".to_owned()),
        ui::Key::Right => ("ArrowRight".to_owned(), "ArrowRight".to_owned()),
        ui::Key::Home => ("Home".to_owned(), "Home".to_owned()),
        ui::Key::End => ("End".to_owned(), "End".to_owned()),
        ui::Key::F5 => ("F5".to_owned(), "F5".to_owned()),
        ui::Key::Escape => ("Escape".to_owned(), "Escape".to_owned()),
    }
}

impl BrowserApp {
    /// 当前修饰键快照（winit ModifiersChanged 维护的三个布尔）。
    pub(super) fn modifiers(&self) -> Modifiers {
        Modifiers {
            ctrl: self.ctrl_down,
            shift: self.shift_down,
            alt: self.alt_down,
        }
    }

    /// 在命中节点（`node` 为 None 时退回文档根）派发页面鼠标事件。
    ///
    /// `x`/`y` 为窗口坐标；client 取页面视口坐标（扣工具带高度，不含滚动
    /// 偏移）。无页面时视为未取消（默认动作照常——与无监听器时一致）。
    pub(super) fn dispatch_page_mouse(
        &mut self,
        node: Option<NodeId>,
        type_: &'static str,
        x: f32,
        y: f32,
        buttons: u16,
    ) -> bool {
        let modifiers = self.modifiers();
        let Some(page) = self.page.as_mut() else {
            return true;
        };
        let target = match node {
            Some(node) => node,
            None => page.document.borrow().root(),
        };
        let dispatched =
            dispatch_mouse(page, target, type_, (x, y - BAR_HEIGHT), buttons, modifiers);
        // JS 监听器表同事件派发 + 事件间隙任务泵（K5 T3）：click 监听器改
        // textContent → 泵后脏位 → 重排。
        let js_dispatched = self.dispatch_script_event(target, type_);
        self.pump_scripts();
        dispatched && js_dispatched
    }

    /// 派发滚轮事件（光标命中节点或文档根）。返回「未取消」。
    pub(super) fn dispatch_page_wheel(&mut self, delta: (f64, f64), mode: DeltaMode) -> bool {
        let (x, y) = self.cursor;
        let node = self.hit_node_at(x, y);
        let Some(page) = self.page.as_mut() else {
            return true;
        };
        let target = match node {
            Some(node) => node,
            None => page.document.borrow().root(),
        };
        let dispatched = dispatch_wheel(page, target, delta, mode);
        let js_dispatched = self.dispatch_script_event(target, "wheel");
        self.pump_scripts();
        dispatched && js_dispatched
    }

    /// 窗口点 `(x, y)` 在页面里的命中节点（工具带高度已扣除并叠加滚动；
    /// K6：计入容器滚动偏移，命中的是滚动后实际显示的内容）。
    pub(super) fn hit_node_at(&self, x: f32, y: f32) -> Option<NodeId> {
        let page_y = y - BAR_HEIGHT + self.scroll_y;
        let offsets = self
            .page
            .as_ref()
            .map(|page| page.scroll_offsets.clone())
            .unwrap_or_default();
        self.root
            .as_ref()
            .and_then(|root| pipeline::hit_test_scrolled(root, x, page_y, &offsets))
            .map(|hit| hit.node)
    }

    /// 滚轮默认动作（K6）：由内向外链式消费——光标定位最内可滚动容器，
    /// 未到边界则消费并停；到边界冒泡祖先；全链到边界回落视口滚动
    /// （`scroll_by` 保持 `Scene::translated` 零 emit 不变量）。容器消费
    /// 重建场景（偏差，见 `set_container_scroll`）。
    ///
    /// `dy` 为 px（正 = 向下滚，与 DOM `WheelEvent.deltaY` 同向）。
    pub(super) fn wheel_scroll(&mut self, dy: f32) {
        let chain = match (self.page.as_ref(), self.root.as_ref()) {
            (Some(page), Some(root)) => {
                let (x, y) = self.cursor;
                let page_y = y - BAR_HEIGHT + self.scroll_y;
                pipeline::scroll_chain_at(root, x, page_y, &page.scroll_offsets)
            }
            _ => Vec::new(),
        };
        match scrollbar::wheel_chain_target(chain, dy) {
            Some((node, offset)) => self.set_container_scroll(node, offset),
            None => self.scroll_by(dy),
        }
    }

    /// 派发页面键盘事件（keydown，文档根上）。返回「未取消」。
    ///
    /// 调用边界：地址栏聚焦时键属 chrome UI，不派发到页面（调用方保证）；
    /// 无页面时视为未取消。
    pub(super) fn dispatch_page_key(&mut self, key: &str, code: &str, repeat: bool) -> bool {
        let modifiers = self.modifiers();
        let Some(page) = self.page.as_mut() else {
            return true;
        };
        let target = page.document.borrow().root();
        let dispatched = dispatch_key(page, target, key, code, modifiers, repeat);
        let js_dispatched = self.dispatch_script_event(target, "keydown");
        self.pump_scripts();
        dispatched && js_dispatched
    }

    /// 滚动键的「先派发后默认」：keydown 未取消才执行滚动。
    pub(super) fn dispatch_key_scroll(&mut self, key: &'static str, delta: f32) {
        if self.dispatch_page_key(key, key, false) {
            self.scroll_by(delta);
        }
    }

    /// 页面安装点的生命周期派发：每个事件按「K1 表（EventLoop post + 排空）
    /// → JS 监听器表（jsrt 会话）」成对派发，DOMContentLoaded 先于 load。
    /// 信号未到的事件不派发（K3/K4 异步 settle 的接缝）。K5 T3 语义：defer
    /// 档脚本已在页面安装期执行完，DCL 此后才派发（K1 时序保持）。
    ///
    /// 跨表偏差：K1 的 Rust 监听器与 jsrt 的 JS 监听器是两张表，同一事件
    /// 各走一遍派发（先后成对，表内保序）——单表语义已在 jsrt gate 模块
    /// 文档登记。
    pub(super) fn dispatch_page_lifecycle(&mut self, pending: PendingLifecycle) {
        if pending.dom_content_loaded {
            self.dispatch_lifecycle_event(true);
        }
        if pending.load {
            self.dispatch_lifecycle_event(false);
        }
    }

    /// 单个生命周期事件的成对派发 + 事件间隙任务泵。
    fn dispatch_lifecycle_event(&mut self, dom_content_loaded: bool) {
        let pending = PendingLifecycle {
            dom_content_loaded,
            load: false,
        };
        if let Some(page) = self.page.as_mut() {
            page.post_lifecycle_tasks(&mut self.dom_loop, pending);
        }
        self.run_dom_tasks();
        let mut event = if dom_content_loaded {
            Event::dom_content_loaded()
        } else {
            Event::load()
        };
        self.dispatch_script_lifecycle(&mut event);
        self.pump_scripts();
    }

    /// JS 监听器表上的生命周期事件派发（无脚本会话为无操作）。
    fn dispatch_script_lifecycle(&mut self, event: &mut Event) {
        let Some(script) = self.script.as_ref() else {
            return;
        };
        let Some(page) = self.page.as_ref() else {
            return;
        };
        let target = page.document.borrow().root();
        if let Err(error) = script.dispatch(target, event) {
            eprintln!("script lifecycle error: {error}");
        }
    }

    /// JS 监听器表上的输入事件派发（无脚本会话视为未取消，与无监听器一致）。
    /// 返回「未取消」。
    fn dispatch_script_event(&mut self, target: NodeId, type_: &str) -> bool {
        let Some(script) = self.script.as_ref() else {
            return true;
        };
        let mut event = Event::new(type_, true, true);
        match script.dispatch(target, &mut event) {
            Ok(not_prevented) => not_prevented,
            Err(error) => {
                eprintln!("script event error: {error}");
                true
            }
        }
    }

    /// 事件间隙任务泵（K5 T3）：清空微任务 / 到期定时器；绑定面改过文档则
    /// 重排一轮 + 请求重绘（「DOM 变更后触发重排」的接缝）。泵错误记日志
    /// 不 panic（偏差：错误后本轮泵终止）。
    fn pump_scripts(&mut self) {
        let outcome = match self.script.as_mut() {
            Some(script) => {
                let error = script.pump().err();
                let mutated = script.take_mutated();
                (error, mutated)
            }
            None => return,
        };
        if let Some(error) = outcome.0 {
            eprintln!("script task error: {error}");
        }
        if outcome.1 {
            self.relayout();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    /// 排空 DOM 宏任务队列（任务在当前页面文档上执行）。
    pub(super) fn run_dom_tasks(&mut self) {
        if let Some(page) = self.page.as_mut() {
            self.dom_loop
                .run_pending_tasks(&mut page.document.borrow_mut());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline;
    use nexty_dom::{Event, EventListener};
    use std::cell::RefCell;
    use std::rc::Rc;

    type Log = Rc<RefCell<Vec<&'static str>>>;

    /// 鼠标事件记录器：按事件类型记入日志；`prevent_click` 为真时对 click
    /// 调 [`Event::prevent_default`]（模拟页面监听器取消默认动作）。
    struct ClickLogger {
        log: Log,
        prevent_click: bool,
    }

    impl EventListener for ClickLogger {
        fn handle_event(&mut self, event: &mut Event) {
            let label = match event.type_() {
                "mousedown" => "mousedown",
                "mouseup" => "mouseup",
                "click" => "click",
                _ => "other",
            };
            self.log.borrow_mut().push(label);
            if self.prevent_click && event.type_() == "click" {
                event.prevent_default();
            }
        }
    }

    /// 块级链接页 + 命中节点（布局 200px 视口，点 (10, 5) 命中 `<a>` 盒）。
    fn link_page(log: &Log, prevent_click: bool) -> (Page, NodeId) {
        let page = pipeline::load_page(
            "<body><a href=\"/target\" style=\"display: block; height: 20px\">block link</a></body>",
            "body { margin: 0 }",
        );
        let shaper = nexty_text::ParleyTextShaper::new();
        let root = pipeline::layout_page(&page, &shaper, 200.0).expect("root");
        let node = pipeline::hit_test(&root, 10.0, 5.0).expect("命中链接");
        page.document.borrow_mut().add_listener(
            node,
            "mousedown",
            false,
            Box::new(ClickLogger {
                log: Rc::clone(log),
                prevent_click: false,
            }),
        );
        page.document.borrow_mut().add_listener(
            node,
            "mouseup",
            false,
            Box::new(ClickLogger {
                log: Rc::clone(log),
                prevent_click: false,
            }),
        );
        page.document.borrow_mut().add_listener(
            node,
            "click",
            false,
            Box::new(ClickLogger {
                log: Rc::clone(log),
                prevent_click,
            }),
        );
        (page, node)
    }

    /// ① click 派发且默认导航仍发生：三事件按序派发、click 未取消——
    /// app 侧据此执行链接导航（默认动作）。
    #[test]
    fn click_sequence_dispatches_and_default_navigation_runs() {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let (mut page, target) = link_page(&log, false);
        let modifiers = Modifiers::default();

        // app.rs 的点击序列：mousedown（主键按住 buttons=1）→ mouseup →
        // click（释放且未拖动才派发，现语义）。
        assert!(dispatch_mouse(
            &mut page,
            target,
            "mousedown",
            (10.0, 5.0),
            1,
            modifiers
        ));
        assert!(dispatch_mouse(
            &mut page,
            target,
            "mouseup",
            (10.0, 5.0),
            0,
            modifiers
        ));
        assert!(
            dispatch_mouse(&mut page, target, "click", (10.0, 5.0), 0, modifiers),
            "无取消监听器：click 未取消 → 默认导航执行"
        );
        assert_eq!(
            *log.borrow(),
            ["mousedown", "mouseup", "click"],
            "点击序列 mousedown → mouseup → click 完整派发"
        );
    }

    /// ② 注入测试监听器 preventDefault 后默认导航被跳过——「先派发后默认」
    /// 顺序的证据：监听器在 click 派发中置 defaultPrevented，导航判定
    /// （dispatch 返回值）在其后读到 false。mousedown/mouseup 不受影响。
    #[test]
    fn prevent_default_click_skips_default_navigation() {
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let (mut page, target) = link_page(&log, true);
        let modifiers = Modifiers::default();

        assert!(dispatch_mouse(
            &mut page,
            target,
            "mousedown",
            (10.0, 5.0),
            1,
            modifiers
        ));
        assert!(dispatch_mouse(
            &mut page,
            target,
            "mouseup",
            (10.0, 5.0),
            0,
            modifiers
        ));
        assert!(
            !dispatch_mouse(&mut page, target, "click", (10.0, 5.0), 0, modifiers),
            "preventDefault 后 dispatch 返回已取消 → 默认导航被跳过"
        );
        assert_eq!(
            *log.borrow(),
            ["mousedown", "mouseup", "click"],
            "事件照常派发，被取消的只是默认动作"
        );
    }

    /// ③ 生命周期（pipeline 单处钩子）：按信号 post、排空派发——
    /// DOMContentLoaded 在解析后、load 在子资源 settle 后，各恰一次、FIFO
    /// 保序。与 tests/chrome.rs 的集成测试互补（那里走完整抓取编排）。
    #[test]
    fn lifecycle_posts_dispatch_exactly_once_per_signal_in_fifo_order() {
        let mut page = pipeline::load_page("<body>hi</body>", "");
        let log: Log = Rc::new(RefCell::new(Vec::new()));
        let root = page.document.borrow().root();
        let recorder = |log: &Log, label: &'static str| {
            let log = Rc::clone(log);
            Box::new(move |_event: &mut Event| log.borrow_mut().push(label))
                as Box<dyn EventListener>
        };
        page.document.borrow_mut().add_listener(
            root,
            nexty_dom::DOM_CONTENT_LOADED,
            false,
            recorder(&log, "dcl"),
        );
        page.document.borrow_mut().add_listener(
            root,
            nexty_dom::LOAD,
            false,
            recorder(&log, "load"),
        );

        // 无信号：不 post、不派发
        let mut event_loop = nexty_dom::EventLoop::new();
        page.post_lifecycle_tasks(&mut event_loop, PendingLifecycle::default());
        event_loop.run_pending_tasks(&mut page.document.borrow_mut());
        assert!(log.borrow().is_empty(), "信号未到不派发");

        // 只到解析信号：仅 DOMContentLoaded
        page.post_lifecycle_tasks(
            &mut event_loop,
            PendingLifecycle {
                dom_content_loaded: true,
                load: false,
            },
        );
        event_loop.run_pending_tasks(&mut page.document.borrow_mut());
        assert_eq!(*log.borrow(), ["dcl"], "仅 DOMContentLoaded 派发");

        // settle 信号到：load 恰一次
        page.post_lifecycle_tasks(
            &mut event_loop,
            PendingLifecycle {
                dom_content_loaded: false,
                load: true,
            },
        );
        event_loop.run_pending_tasks(&mut page.document.borrow_mut());
        assert_eq!(
            *log.borrow(),
            ["dcl", "load"],
            "DOMContentLoaded 先于 load，各恰一次"
        );

        // 队列已空：重复排空不重复派发
        event_loop.run_pending_tasks(&mut page.document.borrow_mut());
        assert_eq!(log.borrow().len(), 2, "排空后无重复派发");
    }

    /// 滚轮/键盘事件在命中节点与文档根上的派发与取消语义（字段填充对
    /// 监听器不可见是 K1 登记偏差，字段面由检视覆盖）。
    #[test]
    fn wheel_and_key_dispatch_gating() {
        let mut page = pipeline::load_page("<body>x</body>", "");
        let root_node = page.document.borrow().root();
        let hits = Rc::new(RefCell::new(Vec::new()));
        let wheel_hits = Rc::clone(&hits);
        page.document.borrow_mut().add_listener(
            root_node,
            "wheel",
            false,
            Box::new(move |event: &mut Event| {
                wheel_hits.borrow_mut().push(event.type_().to_owned());
                event.prevent_default();
            }),
        );
        assert!(
            !dispatch_wheel(&mut page, root_node, (0.0, -40.0), DeltaMode::Line),
            "wheel 被 preventDefault → 默认滚动跳过"
        );
        assert_eq!(*hits.borrow(), ["wheel"]);

        let key_hits = Rc::new(RefCell::new(Vec::new()));
        page.document.borrow_mut().add_listener(
            root_node,
            "keydown",
            false,
            Box::new(move |_event: &mut Event| key_hits.borrow_mut().push("keydown")),
        );
        assert!(
            dispatch_key(
                &mut page,
                root_node,
                "PageDown",
                "PageDown",
                Modifiers::default(),
                false
            ),
            "keydown 未取消 → 默认滚动执行"
        );
        assert_eq!(*hits.borrow(), ["wheel"], "wheel 监听器不收 keydown");
    }

    /// chrome 抽象键 → DOM key/code 映射（字符键大写进 code）。
    #[test]
    fn dom_key_names_map_characters_and_named_keys() {
        let (key, code) = dom_key_names(ui::Key::Character('a'));
        assert_eq!((key.as_str(), code.as_str()), ("a", "KeyA"));
        let (key, code) = dom_key_names(ui::Key::F5);
        assert_eq!((key.as_str(), code.as_str()), ("F5", "F5"));
        let (key, _) = dom_key_names(ui::Key::Left);
        assert_eq!(key.as_str(), "ArrowLeft");
    }
}
