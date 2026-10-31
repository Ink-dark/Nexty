//! 页面脚本编排（K5 T3）：[`ScriptHost`](nexty_dom::ScriptHost) 装配、注入
//! 钩子（querySelector / getComputedStyle——选择器匹配与级联快照由本层用
//! nexty-css 提供）、事件间隙任务泵与「DOM 变更 → 重排」的脏位消费。
//!
//! 模块放在 crate 根而非 `app`（`app` 是 cfg(window) 的私有模块，winit
//! 不可无头测试）——本模块纯逻辑，集成测试（`tests/script.rs`）与
//! `app.rs` 的页面安装点共用同一入口 [`install_and_run`]。
//!
//! ## 编排时序（WHATWG 简化档）
//!
//! 页面安装点（主线程，文档构建后、级联前）：
//! 1. [`install_and_run`]：inline 与外链（defer / async 均按 defer 档）
//!    脚本按**文档序**同步执行——外链字节来自主文档子资源编排（同预算、
//!    缺席降级跳过）；
//! 2. 装配结束泵一轮（微任务 / 0 延迟定时器清空）；
//! 3. 之后才级联 + 首轮布局——脚本对树的改动直接被首轮消费；
//! 4. `DOMContentLoaded` 在脚本执行完后派发（K1 语义：defer 脚本先跑完）；
//! 5. 输入事件 / 生命周期派发后的「事件间隙」再泵（见 `app::events` 的
//!    `pump_scripts`），绑定面改过文档（脏位）即重排 + 重绘。
//!
//! ## 偏差清单（T3/T4 汇总，登记见执行报告）
//!
//! - 外链脚本无 async 档（defer / async 同档：DCL 前执行完）；无 `type`
//!   检查（一律 classic script）；`integrity` / `nomodule` 未建模。
//! - 引擎异常（eval / 回调 / 定时器）经 [`JsrtError`] 上抛，宿主记日志继续
//!   （HTML 的「报告错误并继续事件循环」简化档；unhandled promise rejection
//!   未上报宿主）。
//! - 定时器只在事件间隙驱动（无独立 `ControlFlow::WaitUntil` 唤醒源）：
//!   延迟定时器等到下一次输入 / 重绘间隙才可能到期。
//! - getComputedStyle 快照语义：每次调用全文档重级联（O(n)，无增量），
//!   序对列表只覆盖已建模属性面；`querySelector` 每元素重新解析选择器文本
//!   （无解析缓存）。
//! - `document.write` 在解析器外的脚本执行中为 no-op（捕获槽未装入）；
//!   解析中的 write 语义由 nexty-html 的 scripted 驱动提供（T4）。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use nexty_css::{ComputedStyle, Rgba, Stylesheet, html_ua_stylesheet};
use nexty_dom::{
    Document, Event, GetComputedStyleHook, JsrtError, NodeId, NodeKind, PumpStatus,
    QuerySelectorHook, ScriptError,
};
use nexty_jsrt_rquickjs::Jsrt;

use crate::resources::{ScriptResource, ScriptSource};

/// 一个页面的脚本会话：引擎实例 + 注入钩子 + 脏位（与 `Page.document` 共享
/// 同一棵树——`Rc<RefCell<Document>>`，见 ADR `docs/decisions/2026-10-31-jsrt.md`）。
///
/// 单线程使用（`Document` !Send 且引擎 `Context` !Send）。随页面同生共死：
/// 导航换页时旧会话整体丢弃（`app` 在页面安装点重建）。
pub struct ScriptSession {
    jsrt: Jsrt,
}

impl ScriptSession {
    /// 装配脚本会话：引擎实例 + querySelector / getComputedStyle 注入钩子
    ///（`stylesheets` 为本页作者样式表——级联快照的级联源）。
    ///
    /// # Errors
    ///
    /// 引擎初始化失败返回 [`JsrtError::Script`]。
    pub fn install(
        document: &Rc<RefCell<Document>>,
        stylesheets: &[Stylesheet],
    ) -> Result<Self, JsrtError> {
        let mut jsrt = Jsrt::from_shared(Rc::clone(document))?;
        jsrt.runtime_mut()
            .set_query_selector_hook(query_selector_hook());
        jsrt.runtime_mut()
            .set_get_computed_style_hook(get_computed_style_hook(stylesheets));
        Ok(Self { jsrt })
    }

    /// 编译并同步执行一段脚本（inline 语义），随后做微任务检查点。
    ///
    /// # Errors
    ///
    /// 编译失败 [`ScriptErrorKind::Syntax`](nexty_dom::ScriptErrorKind)、
    /// 执行异常 [`ScriptErrorKind::Runtime`](nexty_dom::ScriptErrorKind)。
    pub fn eval(&mut self, source: &str, name: &str) -> Result<(), JsrtError> {
        self.jsrt.eval(source, name)
    }

    /// 宿主侧事件派发进引擎回调面（生命周期事件 / 输入事件的 JS 监听器表）。
    ///
    /// # Errors
    ///
    /// 目标不存在 / 回调异常上抛。
    pub fn dispatch(&self, target: NodeId, event: &mut Event) -> Result<bool, JsrtError> {
        self.jsrt.dispatch(target, event)
    }

    /// 任务泵：清空微任务 / 到期定时器（节奏见
    /// [`nexty_dom::JsrtRuntime::pump`]），返回宿主下次驱动时机。
    ///
    /// # Errors
    ///
    /// 引擎回调异常上抛（不 panic）。
    pub fn pump(&mut self) -> Result<PumpStatus, JsrtError> {
        self.jsrt.pump()
    }

    /// 取走重排脏位：脚本经绑定面改过文档即置位，读取即清零。
    #[must_use]
    pub fn take_mutated(&self) -> bool {
        self.jsrt.take_mutated()
    }
}

/// 页面安装点入口（K5 T3）：按文档序执行脚本清单（inline + 外链 defer 档），
/// 装配结束后泵一轮。
///
/// `scripts` 来自 [`collect_script_resources`](crate::resources::collect_script_resources)
/// （解析完成点单处钩子）；`fetched` 是主文档编排抓到的外链脚本字节（绝对
/// URL 键控，缺席 = 抓取失败/超预算/URL 非法——降级跳过该脚本）；`resolve`
/// 把 `src` 属性按文档 base 解析为绝对地址；`stylesheets` 供级联快照钩子。
///
/// 脚本执行错误不中断编排（继续下一个脚本），逐条经 `on_error` 上报——
/// HTML 语义：错误报告后页面继续。返回 `Ok(None)` 当且仅当清单为空（无脚本
/// 页面零引擎成本）。
///
/// # Errors
///
/// 引擎初始化失败返回 [`JsrtError::Script`]（页面降级为无脚本继续渲染）。
pub fn install_and_run(
    document: &Rc<RefCell<Document>>,
    scripts: &[ScriptResource],
    fetched: &HashMap<String, Vec<u8>>,
    resolve: &dyn Fn(&str) -> Option<String>,
    stylesheets: &[Stylesheet],
    mut on_error: impl FnMut(usize, &ScriptError),
) -> Result<Option<ScriptSession>, JsrtError> {
    if scripts.is_empty() {
        return Ok(None);
    }
    let mut session: Option<ScriptSession> = None;
    for (index, script) in scripts.iter().enumerate() {
        let source: String = match &script.source {
            ScriptSource::Inline(source) => source.clone(),
            ScriptSource::External { src } => {
                let Some(url) = resolve(src) else {
                    continue; // URL 非法：降级跳过
                };
                let Some(bytes) = fetched.get(&url) else {
                    continue; // 抓取缺席：降级跳过（该脚本不执行）
                };
                String::from_utf8_lossy(bytes).into_owned()
            }
        };
        if session.is_none() {
            session = Some(ScriptSession::install(document, stylesheets)?);
        }
        let installed = session
            .as_mut()
            .expect("session installed on first script above");
        if let Err(JsrtError::Script(error)) = installed.eval(&source, &format!("script#{index}")) {
            on_error(index, &error);
        }
    }
    let Some(mut session) = session else {
        return Ok(None);
    };
    // 装配后泵一轮：微任务 / 0 延迟定时器清空（DCL 之前）。
    if let Err(JsrtError::Script(error)) = session.pump() {
        on_error(usize::MAX, &error);
    }
    Ok(Some(session))
}

/// querySelector 注入钩子：`scope` 子树内先序首个命中（scope 自身不参与，
/// DOM `element.querySelector` 语义）。选择器解析与匹配全权交给 nexty-css
///（每元素重新解析选择器文本——快照语义偏差，见模块文档）。
fn query_selector_hook() -> QuerySelectorHook {
    Box::new(|document: &Document, scope: NodeId, selector: &str| {
        let mut stack = vec![scope];
        while let Some(node) = stack.pop() {
            let children: Vec<NodeId> = document.children(node).collect();
            stack.extend(children.iter().rev());
            if node != scope
                && matches!(document.node(node), Some(NodeKind::Element(_)))
                && nexty_css::matches_selector(selector, document, node).unwrap_or(false)
            {
                return Some(node);
            }
        }
        None
    })
}

/// getComputedStyle 注入钩子：按当前树与作者样式表重级联一次，取目标元素的
/// 计算样式快照（属性序对，序即迭代序）。css 类型不入 dom（钩子输出纯序对）。
fn get_computed_style_hook(stylesheets: &[Stylesheet]) -> GetComputedStyleHook {
    let stylesheets = stylesheets.to_vec();
    Box::new(move |document: &Document, node: NodeId| {
        let sheet_refs: Vec<&Stylesheet> = stylesheets.iter().collect();
        let styles = nexty_css::compute_document_styles_in(
            document,
            &sheet_refs,
            Some(html_ua_stylesheet()),
            None,
        );
        styles.get(&node).map(style_snapshot).unwrap_or_default()
    })
}

/// 计算样式 → 属性序对快照（getComputedStyle 的已建模属性面；枚举值用
/// Debug 形态透出——快照语义偏差，见模块文档）。
fn style_snapshot(style: &ComputedStyle) -> Vec<(String, String)> {
    let color = |c: &Rgba| format!("rgba({}, {}, {}, {})", c.r, c.g, c.b, c.a);
    vec![
        ("display".into(), format!("{:?}", style.display)),
        ("color".into(), color(&style.color)),
        ("background-color".into(), color(&style.background_color)),
        ("font-size".into(), format!("{}px", style.font_size)),
        ("font-weight".into(), format!("{}", style.font_weight)),
        ("font-style".into(), format!("{:?}", style.font_style)),
        ("text-align".into(), format!("{:?}", style.text_align)),
        ("line-height".into(), format!("{:?}", style.line_height)),
        ("white-space".into(), format!("{:?}", style.white_space)),
        ("overflow-x".into(), format!("{:?}", style.overflow_x)),
        ("overflow-y".into(), format!("{:?}", style.overflow_y)),
        ("position".into(), format!("{:?}", style.position)),
        ("z-index".into(), format!("{:?}", style.z_index)),
    ]
}
