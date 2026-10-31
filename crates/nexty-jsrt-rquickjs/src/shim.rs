//! 引擎侧 DOM 形状 shim：在全局 JS 形态上组装 `document` / 元素包装 /
//! 事件门面 / 定时器，全部落在 `__dom_*` 原生绑定（见 [`crate::bindings`]）上。
//!
//! ## 偏差清单（v0）
//!
//! - 元素是普通对象包装（按 nodeId 缓存同一包装）；同一节点的两个包装经
//!   `__wrap` 恒等，但 `===` 比较的是包装对象——同一节点重复取包装返回同一
//!   对象，行为与 DOM 一致。
//! - 事件对象是门面（type/bubbles/cancelable/eventPhase/defaultPrevented +
//!   三个干预方法）；无 target/currentTarget、trusted、Event 构造器。
//!   `dispatchEvent` 接受字符串类型或 `{ type, bubbles?, cancelable? }`。
//! - 无 `createTextNode` / `createDocumentFragment` / `importNode`；
//!   文本内容由 `textContent` 写入。
//! - 无 Promise 之外的微任务绑定（引擎 promise job 由宿主 `run_microtasks`
//!   经 `execute_pending_job` 清空）；queueMicrotask 未绑定。
//! - 无 ESM / fetch / localStorage / worker（T4 登记偏差）。

/// DOM 形状 shim（间接 eval 注入，函数挂到全局）。
pub const SHIM: &str = r#"
var __flags = { stopped: false, immediate: false, prevented: false };
var __fns = [];
var __els = new Map();

function __msg(e) {
  if (e && typeof e === "object" && e.message !== undefined) return String(e.message);
  return String(e);
}
function __store(fn) { __fns.push(fn); return __fns.length - 1; }

function __invoke_listener(idx, phase, type, bubbles, cancelable) {
  __flags = { stopped: false, immediate: false, prevented: false };
  var ev = {
    type: type,
    bubbles: bubbles,
    cancelable: cancelable,
    eventPhase: phase,
    defaultPrevented: false,
    stopPropagation: function () { __flags.stopped = true; },
    stopImmediatePropagation: function () { __flags.stopped = true; __flags.immediate = true; },
    preventDefault: function () { if (cancelable) { __flags.prevented = true; } },
  };
  try { __fns[idx](ev); }
  catch (e) {
    return { error: __msg(e), stopped: __flags.stopped, immediate: __flags.immediate, prevented: __flags.prevented };
  }
  return { error: null, stopped: __flags.stopped, immediate: __flags.immediate, prevented: __flags.prevented };
}

function __invoke_timer(idx) {
  try { __fns[idx](); return null; }
  catch (e) { return __msg(e); }
}

function __dom_eval(src, name) {
  try { (0, eval)(src); return null; }
  catch (e) {
    var kind = (e instanceof SyntaxError) ? "syntax" : "runtime";
    return kind + "\u001f" + (name ? name + ": " : "") + __msg(e);
  }
}

function __wrap(id) {
  if (__els.has(id)) return __els.get(id);
  var el = {
    __node: id,
    appendChild: function (child) { __dom_append_child(id, child.__node); return child; },
    remove: function () { __dom_remove(id); },
    getAttribute: function (name) { var v = __dom_get_attribute(id, String(name)); return v === undefined ? null : v; },
    setAttribute: function (name, value) { __dom_set_attribute(id, String(name), String(value)); },
    addEventListener: function (type, fn, capture) { return __listen(id, type, fn, capture); },
    removeEventListener: function (type, fn, capture) { return __unlisten(id, type, fn, capture); },
    dispatchEvent: function (ev) { return __dispatch(id, ev); },
    querySelector: function (sel) {
      var r = __dom_query_selector(id, String(sel));
      return r === null ? null : __wrap(r);
    },
  };
  Object.defineProperty(el, "textContent", {
    get: function () { var v = __dom_text_content(id); return v === undefined ? null : v; },
    set: function (v) { __dom_set_text_content(id, String(v)); },
  });
  el.classList = {
    add: function () { __dom_class_list_add.apply(null, [id].concat(Array.prototype.slice.call(arguments))); },
    remove: function () { __dom_class_list_remove.apply(null, [id].concat(Array.prototype.slice.call(arguments))); },
    contains: function (token) { return __dom_class_list_contains(id, String(token)); },
    toggle: function (token) { return __dom_class_list_toggle(id, String(token)); },
  };
  __els.set(id, el);
  return el;
}

var __lh = [];

function __listen(node, type, fn, capture) {
  if (typeof fn !== "function") throw new TypeError("addEventListener requires a function");
  var idx = __store(fn);
  var handle = __dom_add_event_listener(node, String(type), idx, !!capture);
  __lh.push({ node: node, type: String(type), fn: fn, capture: !!capture, handle: handle });
  return handle;
}

function __unlisten(node, type, fn, capture) {
  if (typeof fn !== "function") return false;
  for (var i = __lh.length - 1; i >= 0; i--) {
    var entry = __lh[i];
    if (entry.node === node && entry.type === String(type) && entry.fn === fn && entry.capture === !!capture) {
      __lh.splice(i, 1);
      return __dom_remove_event_listener(node, entry.handle);
    }
  }
  return false;
}

function __dispatch(node, ev) {
  if (typeof ev === "string") ev = { type: ev };
  if (!ev || typeof ev.type !== "string") throw new TypeError("dispatchEvent requires an event with a type");
  return __dom_dispatch(node, ev.type, !!ev.bubbles, !!ev.cancelable);
}

var document = {
  appendChild: function (child) { __dom_append_child(0, child.__node); return child; },
  createElement: function (name) { return __wrap(__dom_create_element(String(name))); },
  getElementById: function (id) {
    var r = __dom_get_element_by_id(String(id));
    return r === undefined ? null : __wrap(r);
  },
  querySelector: function (sel) {
    var r = __dom_query_selector(0, String(sel));
    return r === undefined ? null : __wrap(r);
  },
  getComputedStyle: function (el) { return __dom_get_computed_style(el.__node); },
  addEventListener: function (type, fn, capture) { return __listen(0, type, fn, capture); },
  removeEventListener: function (type, fn, capture) { return __unlisten(0, type, fn, capture); },
  dispatchEvent: function (ev) { return __dispatch(0, ev); },
};

function setTimeout(fn, delay) {
  if (typeof fn !== "function") throw new TypeError("setTimeout requires a function");
  return __dom_set_timeout(__store(fn), __norm_delay(delay));
}
function clearTimeout(id) { __dom_clear_timeout(id); }
function setInterval(fn, delay) {
  if (typeof fn !== "function") throw new TypeError("setInterval requires a function");
  return __dom_set_interval(__store(fn), __norm_delay(delay));
}
function clearInterval(id) { __dom_clear_timeout(id); }
function __norm_delay(delay) {
  var n = Number(delay);
  return (typeof n === "number" && isFinite(n) && n > 0) ? n : 0;
}

globalThis.document = document;
globalThis.setTimeout = setTimeout;
globalThis.clearTimeout = clearTimeout;
globalThis.setInterval = setInterval;
globalThis.clearInterval = clearInterval;
"#;
