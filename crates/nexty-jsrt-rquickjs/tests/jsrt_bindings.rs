//! K5 T2 集成测试：绑定面 v0 全清单 + 错误路径 + 定时器/微任务经泵的顺序。
//!
//! 验证策略：JS 侧断言（eval 抛错即测试失败）+ DOM 状态经接缝读回（引擎无关
//! 通道），不透传引擎值。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use nexty_dom::{Event, JsrtError, NodeKind, PumpStatus, ScriptErrorKind};
use nexty_jsrt_rquickjs::Jsrt;

/// 断言脚本执行成功（失败带脚本原文与错误消息）。
fn assert_ok(jsrt: &mut Jsrt, source: &str) {
    jsrt.eval(source, "test")
        .unwrap_or_else(|error| panic!("eval failed: {error}\n--- script ---\n{source}"));
}

/// 断言脚本执行失败且错误类别为 Runtime。
fn assert_eval_runtime_error(jsrt: &mut Jsrt, source: &str) -> String {
    match jsrt.eval(source, "test") {
        Err(JsrtError::Script(script)) => {
            assert_eq!(script.kind, ScriptErrorKind::Runtime, "source: {source}");
            script.message
        }
        other => panic!("expected runtime error, got {other:?}\n--- script ---\n{source}"),
    }
}

/// 建 html > body > log 结构的辅助脚本（Document 只允许一个元素子节点）。
const SETUP_LOG: &str = r#"
  const html = document.createElement("html");
  document.appendChild(html);
  const body = document.createElement("body");
  body.setAttribute("id", "__body");
  html.appendChild(body);
  const log = document.createElement("div");
  log.setAttribute("id", "__log");
  body.appendChild(log);
"#;

/// 经接缝读回元素文本（引擎无关验证通道）。
fn text_of(jsrt: &Jsrt, id: &str) -> String {
    let gate = jsrt.gate();
    let node = jsrt
        .runtime()
        .registry()
        .get_element_by_id(gate, id)
        .expect("registry lookup")
        .unwrap_or_else(|| panic!("element #{id} not found"));
    jsrt.runtime()
        .registry()
        .text_content(gate, node)
        .expect("text_content")
        .unwrap_or_default()
}

/// 经接缝按 id 取节点。
fn node_by_id(jsrt: &Jsrt, id: &str) -> nexty_dom::NodeId {
    jsrt.runtime()
        .registry()
        .get_element_by_id(jsrt.gate(), id)
        .expect("registry lookup")
        .unwrap_or_else(|| panic!("element #{id} not found"))
}

fn plain_jsrt() -> Jsrt {
    let mut jsrt = Jsrt::new(nexty_dom::Document::new()).unwrap();
    assert_ok(&mut jsrt, SETUP_LOG);
    jsrt
}

// ---- 树变更 + textContent ----

#[test]
fn create_append_remove_and_text_content() {
    let mut jsrt = plain_jsrt();
    assert_ok(
        &mut jsrt,
        r#"
        const div = document.createElement("div");
        div.setAttribute("id", "target");
        document.getElementById("__body").appendChild(div);
        div.textContent = "hello";
        if (div.textContent !== "hello") throw new Error("set/get textContent");
        const span = document.createElement("span");
        span.textContent = "world";
        div.appendChild(span);
        if (div.textContent !== "helloworld") throw new Error("descendant concat: " + div.textContent);
        span.remove();
        if (div.textContent !== "hello") throw new Error("after remove: " + div.textContent);
    "#,
    );
    assert_eq!(text_of(&jsrt, "target"), "hello");
    assert!(jsrt.take_mutated(), "树变更置重排脏位");
}

#[test]
fn append_child_hierarchy_and_not_found_errors() {
    let mut jsrt = plain_jsrt();
    assert_ok(
        &mut jsrt,
        r#"
        const a = document.createElement("div");
        a.setAttribute("id", "a");
        document.getElementById("__body").appendChild(a);
        try { a.appendChild(a); throw new Error("should have thrown"); }
        catch (e) {
            if (!String(e.message).includes("hierarchy")) throw new Error("wrong error: " + e.message);
        }
        // 非法索引经树校验映射为 HierarchyRequest（越界节点不满足可插入类型）。
        try { a.appendChild({ __node: 999999 }); throw new Error("should have thrown"); }
        catch (e) {
            if (!String(e.message).includes("hierarchy")) throw new Error("wrong error: " + e.message);
        }
    "#,
    );
}

// ---- attributes ----

#[test]
fn attributes_get_set_case_insensitive() {
    let mut jsrt = plain_jsrt();
    assert_ok(
        &mut jsrt,
        r#"
        const el = document.createElement("div");
        el.setAttribute("id", "attr-target");
        document.getElementById("__body").appendChild(el);
        el.setAttribute("data-x", "1");
        if (el.getAttribute("data-x") !== "1") throw new Error("get after set");
        if (el.getAttribute("DATA-X") !== "1") throw new Error("case-insensitive get");
        el.setAttribute("DATA-X", "2");
        if (el.getAttribute("data-x") !== "2") throw new Error("set replaces value");
        if (el.getAttribute("missing") !== null) throw new Error("missing returns null");
    "#,
    );
}

#[test]
fn empty_attribute_name_is_js_exception() {
    let mut jsrt = plain_jsrt();
    let message = assert_eval_runtime_error(
        &mut jsrt,
        r#"
        const el = document.createElement("div");
        el.setAttribute("", "1");
    "#,
    );
    assert!(message.contains("invalid argument"), "message: {message}");
}

// ---- classList ----

#[test]
fn class_list_add_remove_contains_toggle() {
    let mut jsrt = plain_jsrt();
    assert_ok(
        &mut jsrt,
        r#"
        const el = document.createElement("div");
        el.setAttribute("id", "cls");
        document.getElementById("__body").appendChild(el);
        el.classList.add("a", "b");
        if (!el.classList.contains("a") || !el.classList.contains("b")) throw new Error("add");
        el.classList.add("a");
        if (el.getAttribute("class") !== "a b") throw new Error("no duplicates: " + el.getAttribute("class"));
        el.classList.toggle("c");
        if (!el.classList.contains("c")) throw new Error("toggle on");
        if (el.classList.toggle("c") !== false) throw new Error("toggle off returns false");
        if (el.classList.contains("c")) throw new Error("toggle off removes");
        el.classList.toggle("c");
        el.classList.remove("a");
        if (el.classList.contains("a")) throw new Error("remove");
        if (el.getAttribute("class") !== "b c") throw new Error("serializes remaining: " + el.getAttribute("class"));
    "#,
    );
}

#[test]
fn class_list_empty_token_is_js_exception() {
    let mut jsrt = plain_jsrt();
    let message = assert_eval_runtime_error(
        &mut jsrt,
        r#"
        const el = document.createElement("div");
        document.getElementById("__body").appendChild(el);
        el.classList.add("");
    "#,
    );
    assert!(message.contains("invalid argument"), "message: {message}");
}

// ---- 查找 ----

#[test]
fn get_element_by_id_and_wrapper_identity() {
    let mut jsrt = plain_jsrt();
    assert_ok(
        &mut jsrt,
        r#"
        const a = document.createElement("div");
        a.setAttribute("id", "x");
        document.getElementById("__body").appendChild(a);
        if (document.getElementById("x") !== a) throw new Error("wrapper identity");
        if (document.getElementById("nope") !== null) throw new Error("miss returns null");
    "#,
    );
    // 文档序第一个命中（经接缝核对）。
    assert_eq!(
        node_by_id(&jsrt, "x").index(),
        node_by_id(&jsrt, "x").index()
    );
}

// ---- 事件 ----

#[test]
fn add_and_dispatch_events_with_phases() {
    let mut jsrt = plain_jsrt();
    assert_ok(
        &mut jsrt,
        r#"
        const parent = document.createElement("div");
        parent.setAttribute("id", "parent");
        document.getElementById("__body").appendChild(parent);
        const child = document.createElement("span");
        child.setAttribute("id", "child");
        parent.appendChild(child);

        const order = [];
        parent.addEventListener("click", (ev) => { order.push("p-capture:" + ev.eventPhase); }, true);
        child.addEventListener("click", (ev) => { order.push("target:" + ev.eventPhase + ":" + ev.type); });
        parent.addEventListener("click", (ev) => { order.push("p-bubble:" + ev.eventPhase); });
        child.dispatchEvent({ type: "click", bubbles: true });
        const expected = "p-capture:1,target:2:click,p-bubble:3";
        if (order.join(",") !== expected) throw new Error("order: " + order.join(","));

        // 事件对象形态：bubbles / cancelable / preventDefault。
        let saw = null;
        child.addEventListener("submit", (ev) => { saw = ev; ev.preventDefault(); });
        if (child.dispatchEvent({ type: "submit", bubbles: false, cancelable: true }) !== false) {
            throw new Error("preventDefault flips dispatch return");
        }
        if (saw.bubbles !== false || saw.cancelable !== true || saw.defaultPrevented !== false) {
            throw new Error("event facade fields");
        }
    "#,
    );
    let _ = jsrt.take_mutated(); // SETUP_LOG 的挂树改动先清零。
    assert!(!jsrt.take_mutated(), "事件派发本身不改树");
}

#[test]
fn stop_propagation_and_immediate() {
    let mut jsrt = plain_jsrt();
    assert_ok(
        &mut jsrt,
        r#"
        const parent = document.createElement("div");
        document.getElementById("__body").appendChild(parent);
        const child = document.createElement("span");
        parent.appendChild(child);

        const order = [];
        child.addEventListener("click", (ev) => { order.push("first"); ev.stopPropagation(); });
        child.addEventListener("click", (ev) => { order.push("second"); });
        parent.addEventListener("click", () => { order.push("bubble"); });
        child.dispatchEvent({ type: "click", bubbles: true });
        if (order.join(",") !== "first,second") throw new Error("stopPropagation: " + order.join(","));

        const order2 = [];
        child.addEventListener("tap", (ev) => { order2.push("first"); ev.stopImmediatePropagation(); });
        child.addEventListener("tap", () => { order2.push("second"); });
        child.dispatchEvent("tap");
        if (order2.join(",") !== "first") throw new Error("stopImmediate: " + order2.join(","));
    "#,
    );
}

#[test]
fn listener_error_propagates_as_script_error() {
    let mut jsrt = plain_jsrt();
    assert_ok(
        &mut jsrt,
        r#"
        const el = document.createElement("div");
        el.setAttribute("id", "boom");
        document.getElementById("__body").appendChild(el);
        el.addEventListener("click", () => { throw new Error("listener-boom"); });
    "#,
    );
    let target = node_by_id(&jsrt, "boom");
    let mut event = Event::new("click", false, true);
    let error = jsrt
        .dispatch(target, &mut event)
        .expect_err("listener error must propagate");
    let rendered = error.to_string();
    assert!(rendered.contains("listener-boom"), "error: {rendered}");
}

#[test]
fn remove_event_listener_stops_delivery() {
    let mut jsrt = plain_jsrt();
    assert_ok(
        &mut jsrt,
        r#"
        const el = document.createElement("div");
        document.getElementById("__body").appendChild(el);
        let calls = 0;
        const fn = () => { calls++; };
        el.addEventListener("click", fn);
        el.dispatchEvent("click");
        if (calls !== 1) throw new Error("before remove");
        el.removeEventListener("click", fn);
        el.dispatchEvent("click");
        if (calls !== 1) throw new Error("after remove: " + calls);
        if (el.removeEventListener("click", fn) !== false) throw new Error("double remove false");
    "#,
    );
}

/// Rust 侧派发（生命周期事件形态）进引擎回调面。
#[test]
fn host_side_dispatch_reaches_js_listener() {
    let mut jsrt = plain_jsrt();
    assert_ok(
        &mut jsrt,
        r#"
        document.addEventListener("DOMContentLoaded", (ev) => {
            document.getElementById("__log").textContent = "dcl:" + ev.eventPhase;
        });
    "#,
    );
    let root = jsrt.gate().document().unwrap().root();
    let mut event = Event::dom_content_loaded();
    let not_prevented = jsrt.dispatch(root, &mut event).unwrap();
    assert!(not_prevented);
    assert_eq!(
        text_of(&jsrt, "__log"),
        "dcl:2",
        "document 上 AtTarget 阶段"
    );
}

// ---- querySelector / getComputedStyle 钩子 ----

#[test]
fn hooks_unavailable_reports_js_exception() {
    let mut jsrt = plain_jsrt();
    let message = assert_eval_runtime_error(&mut jsrt, "document.querySelector('div');");
    assert!(message.contains("hook not installed"), "message: {message}");
    let message = assert_eval_runtime_error(
        &mut jsrt,
        "document.getComputedStyle(document.createElement('div'));",
    );
    assert!(message.contains("hook not installed"), "message: {message}");
}

/// 测试注入点：文档序首个同名元素匹配器（chrome T3 的装配位）。
fn first_element_matcher() -> nexty_dom::QuerySelectorHook {
    Box::new(|doc, _scope, selector| {
        let mut stack: Vec<nexty_dom::NodeId> = vec![doc.root()];
        while let Some(node) = stack.pop() {
            if let Some(NodeKind::Element(data)) = doc.node(node)
                && data.name == selector
            {
                return Some(node);
            }
            let mut children: Vec<nexty_dom::NodeId> = doc.children(node).collect();
            children.reverse();
            stack.extend(children);
        }
        None
    })
}

#[test]
fn query_selector_hook_and_style_hook() {
    let mut jsrt = plain_jsrt();
    jsrt.runtime_mut()
        .set_query_selector_hook(first_element_matcher());
    jsrt.runtime_mut()
        .set_get_computed_style_hook(Box::new(|_doc, _node| {
            vec![
                ("color".to_string(), "red".to_string()),
                ("font-size".to_string(), "12px".to_string()),
            ]
        }));
    assert_ok(
        &mut jsrt,
        r#"
        const first = document.createElement("section");
        first.setAttribute("id", "first");
        document.getElementById("__body").appendChild(first);
        const second = document.createElement("section");
        second.setAttribute("id", "second");
        document.getElementById("__body").appendChild(second);
        if (document.querySelector("section") !== first) throw new Error("document order first");
        const style = document.getComputedStyle(first);
        if (style.color !== "red" || style["font-size"] !== "12px") throw new Error("style snapshot");
    "#,
    );
}

// ---- 定时器：顺序 / 清除 / 嵌套钳制 / 错误 ----

struct Clock {
    handle: Rc<Cell<u64>>,
}

impl Clock {
    fn new(start: u64) -> (Self, Box<dyn Fn() -> u64>) {
        let handle = Rc::new(Cell::new(start));
        let driver = {
            let handle = Rc::clone(&handle);
            Box::new(move || handle.get()) as Box<dyn Fn() -> u64>
        };
        (Self { handle }, driver)
    }
}

#[test]
fn timers_run_in_due_order_and_clear_works() {
    let (clock, driver) = Clock::new(1_000);
    let mut jsrt = Jsrt::with_timer_clock(nexty_dom::Document::new(), driver).unwrap();
    assert_ok(&mut jsrt, SETUP_LOG);
    assert_ok(
        &mut jsrt,
        r#"
        const log = document.getElementById("__log");
        setTimeout(() => { log.textContent += "a;"; }, 50);
        setTimeout(() => { log.textContent += "b;"; }, 10);
        const cancelled = setTimeout(() => { log.textContent += "never;"; }, 10);
        clearTimeout(cancelled);
        let interval = 0;
        const id = setInterval(() => {
            interval++;
            log.textContent += "i" + interval + ";";
            if (interval >= 2) clearInterval(id);
        }, 10);
    "#,
    );

    assert_eq!(
        jsrt.pump().unwrap(),
        PumpStatus::Waiting(1_010),
        "最近到期 10ms"
    );
    clock.handle.set(1_010);
    assert_eq!(jsrt.pump().unwrap(), PumpStatus::Waiting(1_020));
    clock.handle.set(1_020);
    assert_eq!(
        jsrt.pump().unwrap(),
        PumpStatus::Waiting(1_050),
        "interval 已自清"
    );
    clock.handle.set(1_050);
    assert_eq!(jsrt.pump().unwrap(), PumpStatus::Idle);
    assert_eq!(
        text_of(&jsrt, "__log"),
        "b;i1;i2;a;",
        "同到期按登记序、清除不跑"
    );
}

#[test]
fn nested_timeout_chain_is_clamped_after_five_levels() {
    let (clock, driver) = Clock::new(1_000);
    let mut jsrt = Jsrt::with_timer_clock(nexty_dom::Document::new(), driver).unwrap();
    assert_ok(&mut jsrt, SETUP_LOG);
    assert_ok(
        &mut jsrt,
        r#"
        const log = document.getElementById("__log");
        let depth = 0;
        function chain() {
            depth++;
            log.textContent = String(depth);
            if (depth < 7) setTimeout(chain, 0);
        }
        setTimeout(chain, 0);
    "#,
    );
    assert_eq!(
        jsrt.pump().unwrap(),
        PumpStatus::Waiting(1_005),
        "第 7 层起嵌套钳制 5ms"
    );
    assert_eq!(text_of(&jsrt, "__log"), "6", "前 6 层同刻跑完");
    clock.handle.set(1_005);
    assert_eq!(jsrt.pump().unwrap(), PumpStatus::Idle);
    assert_eq!(text_of(&jsrt, "__log"), "7");
}

#[test]
fn timer_callback_error_propagates_without_panic() {
    let (clock, driver) = Clock::new(1_000);
    let mut jsrt = Jsrt::with_timer_clock(nexty_dom::Document::new(), driver).unwrap();
    assert_ok(
        &mut jsrt,
        "setTimeout(() => { throw new Error('timer-boom'); }, 0);",
    );
    clock.handle.set(1_001);
    let error = jsrt.pump().expect_err("timer error must propagate");
    assert!(error.to_string().contains("timer-boom"), "error: {error}");
    assert_eq!(jsrt.pump().unwrap(), PumpStatus::Idle, "泵可继续驱动");
}

// ---- 微任务（promise）经泵 ----

#[test]
fn promise_microtasks_run_in_eval_checkpoint_before_timers() {
    let (clock, driver) = Clock::new(1_000);
    let mut jsrt = Jsrt::with_timer_clock(nexty_dom::Document::new(), driver).unwrap();
    assert_ok(&mut jsrt, SETUP_LOG);
    assert_ok(
        &mut jsrt,
        r#"
        const log = document.getElementById("__log");
        log.textContent += "s;";
        Promise.resolve().then(() => { log.textContent += "p1;"; });
        setTimeout(() => { log.textContent += "t;"; }, 0);
        Promise.resolve().then(() => { log.textContent += "p2;"; });
    "#,
    );
    // eval 的检查点清空引擎微任务（先于泵里的定时器）。
    assert_eq!(text_of(&jsrt, "__log"), "s;p1;p2;");
    clock.handle.set(1_001);
    jsrt.pump().unwrap();
    assert_eq!(text_of(&jsrt, "__log"), "s;p1;p2;t;");
}

#[test]
fn promise_reaction_error_is_contained_and_pump_survives() {
    let mut jsrt = plain_jsrt();
    // 反应回调异常按 promise 语义拒绝派生 promise（偏差：unhandled rejection
    // 不上报宿主；作业级异常路径见 bindings::job_exception_message）。
    assert_ok(
        &mut jsrt,
        "Promise.resolve().then(() => { throw new Error('job-boom'); });",
    );
    assert_eq!(jsrt.pump().unwrap(), PumpStatus::Idle, "泵可继续驱动");
}

// ---- 内联脚本改 DOM → 脏位（重排触发接缝） ----

#[test]
fn script_dom_mutation_sets_dirty_bit() {
    let mut jsrt = plain_jsrt();
    let _ = jsrt.take_mutated(); // SETUP_LOG 的挂树改动先清零。
    // createElement 不挂树：不算文档改动。
    jsrt.eval("document.createElement('div');", "detached")
        .unwrap();
    assert!(!jsrt.take_mutated(), "游离创建不改树");
    jsrt.eval(
        "document.getElementById(\"__body\").appendChild(document.createElement('div'));",
        "attach",
    )
    .unwrap();
    assert!(jsrt.take_mutated(), "挂树置脏位");
    assert!(!jsrt.take_mutated(), "读取即清零");
}

// ---- listener 句柄数值往返（跨界形态） ----

#[test]
fn listener_handle_round_trips_through_js_number() {
    let mut jsrt = plain_jsrt();
    assert_ok(
        &mut jsrt,
        r#"
        const el = document.createElement("div");
        document.getElementById("__body").appendChild(el);
        let calls = 0;
        const fn = () => { calls++; };
        const h1 = el.addEventListener("click", fn);
        const h2 = el.addEventListener("click", fn);
        if (typeof h1 !== "number" || h1 === h2) throw new Error("handles are distinct numbers");
        if (!el.removeEventListener("click", fn)) throw new Error("remove by handle 1");
        if (!el.removeEventListener("click", fn)) throw new Error("remove by handle 2");
        el.dispatchEvent("click");
        if (calls !== 0) throw new Error("both removed");
    "#,
    );
}

// ---- RefCell 共享句柄路径（T3 装配形态冒烟） ----

#[test]
fn from_shared_document_works() {
    let shared = Rc::new(RefCell::new(nexty_dom::Document::new()));
    let mut jsrt = Jsrt::from_shared(Rc::clone(&shared)).unwrap();
    assert_ok(&mut jsrt, SETUP_LOG);
    assert_ok(
        &mut jsrt,
        "document.getElementById('__log').textContent = 'shared';",
    );
    assert_eq!(text_of(&jsrt, "__log"), "shared");
    assert!(shared.borrow().node_count() > 1, "共享句柄即运行时文档");
}
