//! K5 T3 集成测试：页面脚本编排的端到端行为（无头）。
//!
//! 验证面（对应任务退出条件）：
//! 1. click 监听器改 textContent → 泵后脏位 → 重排消费（片段树含新文本）；
//! 2. 定时器改 style → 泵（0 延迟定时器在装配泵执行）→ 重排消费（computed
//!    style 按新内联样式重级联）；
//! 3. 脚本错误上拋为错误类别，不 panic，页面编排继续（后续脚本照常执行）；
//! 4. 外链 src 脚本（defer 档）按文档序执行，DCL 派发先于脚本注册的
//!    JS 监听器（K1 时序保持）。
//!
//! 页面安装点组合（`install_and_run` → 级联 → 布局）与 `app.rs` 的
//! `page_loaded` 完全一致；输入/生命周期派发 + 事件间隙泵的组合在
//! `app::events`（winit 边界，无头不可测，实机验证）。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use nexty_chrome::pipeline::{self, Page};
use nexty_chrome::resources::{self, ScriptSource};
use nexty_chrome::script;
use nexty_css::MediaViewport;
use nexty_dom::{Event, NodeId, NodeKind, ScriptErrorKind};
use nexty_layout::Fragment;
use nexty_text::ParleyTextShaper;

/// 装配一个带 inline 脚本的页面（页面安装点组合：collect → install_and_run
/// → build_page 级联——脚本改动被首轮级联直接消费）。
fn page_with_script(
    html: &str,
    scripts_bytes: &HashMap<String, Vec<u8>>,
    base_url: &str,
) -> (Page, Option<script::ScriptSession>, Vec<(usize, String)>) {
    let document = Rc::new(RefCell::new(nexty_html::parse_document(
        html,
        nexty_html::ParseOptions::default(),
    )));
    let resources = resources::assemble_page_data(&document.borrow(), base_url, &dummy_fetched());
    let scripts = resources::collect_script_resources(&document.borrow());
    let mut errors: Vec<(usize, String)> = Vec::new();
    let resolve = move |src: &str| nexty_network::resolve(base_url, src).ok();
    let session = script::install_and_run(
        &document,
        &scripts,
        scripts_bytes,
        &resolve,
        &resources.stylesheets,
        |index, error| errors.push((index, error.to_string())),
    )
    .expect("engine init");
    let page = pipeline::build_page(document, resources.stylesheets, resources.images);
    (page, session, errors)
}

/// 空 fetched 面（inline 脚本用例不需要外链字节）。
fn dummy_fetched() -> resources::FetchedPageData {
    resources::FetchedPageData {
        html: String::new(),
        css: HashMap::new(),
        images: HashMap::new(),
        scripts: HashMap::new(),
    }
}

/// 按文档序找首个指定标签名的 HTML 元素。
fn find_element(page: &Page, name: &str) -> NodeId {
    let document = page.document.borrow();
    let mut stack = vec![document.root()];
    while let Some(node) = stack.pop() {
        if let Some(NodeKind::Element(data)) = document.node(node)
            && data.namespace == nexty_dom::Namespace::Html
            && data.name == name
        {
            return node;
        }
        let children: Vec<NodeId> = document.children(node).collect();
        children
            .into_iter()
            .rev()
            .for_each(|child| stack.push(child));
    }
    panic!("element <{name}> not found");
}

/// 片段树内全部文本 run 的拼接（重排消费断言用）。
fn fragment_text(fragment: &Fragment) -> String {
    let mut out = String::new();
    for line in &fragment.lines {
        for run in &line.runs {
            out.push_str(&run.text);
        }
        for inner in &line.boxes {
            out.push_str(&fragment_text(inner));
        }
    }
    for child in &fragment.children {
        out.push_str(&fragment_text(child));
    }
    out
}

/// ① click 监听器改 textContent → 事件间隙泵后脏位 → 重排消费新文本。
#[test]
fn click_listener_text_change_triggers_reflow() {
    let html = r#"<body><button id="b">original</button><script>
        var b = document.getElementById("b");
        b.addEventListener("click", function () { b.textContent = "clicked"; });
    </script></body>"#;
    let (mut page, session, errors) = page_with_script(html, &HashMap::new(), "https://page.test/");
    assert!(errors.is_empty(), "no script errors: {errors:?}");
    let mut session = session.expect("scripts present → session installed");

    // 首轮布局：旧文本。
    let shaper = ParleyTextShaper::new();
    let viewport = MediaViewport {
        width: 800.0,
        height: 600.0,
    };
    pipeline::recascade(&mut page, viewport);
    let root = pipeline::layout_page(&page, &shaper, viewport.width).expect("root fragment");
    assert!(
        fragment_text(&root).contains("original"),
        "first layout shows original text"
    );

    // 点击派发（K1 表 + JS 表）+ 事件间隙泵：监听器改 textContent。
    let button = find_element(&page, "button");
    let mut event = Event::new("click", true, true);
    session
        .dispatch(button, &mut event)
        .expect("click dispatch");
    session.pump().expect("pump");
    assert!(session.take_mutated(), "绑定面改动置重排脏位");

    // DOM 文本已变；重排消费改动（新文本进入片段树）。
    let document = page.document.borrow();
    let text = document
        .first_child(button)
        .and_then(|child| match document.node(child) {
            Some(NodeKind::Text(data)) => Some(data.clone()),
            _ => None,
        })
        .expect("button text child");
    drop(document);
    assert_eq!(text, "clicked", "listener mutated the DOM");
    pipeline::recascade(&mut page, viewport);
    let root = pipeline::layout_page(&page, &shaper, viewport.width).expect("root fragment");
    assert!(
        fragment_text(&root).contains("clicked") && !fragment_text(&root).contains("original"),
        "reflow consumed the mutation"
    );
}

/// ② 定时器改 style → 装配泵（0 延迟定时器即期执行）→ 重排消费新样式。
#[test]
fn timer_style_change_triggers_reflow() {
    let html = r#"<body><p id="p">text</p><script>
        setTimeout(function () {
            document.getElementById("p").setAttribute("style", "font-size: 40px");
        }, 0);
    </script></body>"#;
    let (mut page, session, errors) = page_with_script(html, &HashMap::new(), "https://page.test/");
    assert!(errors.is_empty(), "no script errors: {errors:?}");
    let session = session.expect("scripts present → session installed");

    // install_and_run 结尾已泵一轮：0 延迟定时器即期执行。
    assert!(session.take_mutated(), "定时器回调改属性置重排脏位");
    pipeline::recascade(
        &mut page,
        MediaViewport {
            width: 800.0,
            height: 600.0,
        },
    );
    let paragraph = find_element(&page, "p");
    let style = page.styles.get(&paragraph).expect("paragraph styled");
    assert_eq!(style.font_size, 40.0, "重排按脚本改写的内联样式重新级联");
}

/// ③ 脚本错误上拋为错误类别，不 panic；后续脚本照常执行。
#[test]
fn script_errors_are_reported_and_execution_continues() {
    let html = r#"<body><script>this is not javascript(</script><script>
        document.body.setAttribute("data-ok", "yes");
    </script></body>"#;
    let (page, session, errors) = page_with_script(html, &HashMap::new(), "https://page.test/");
    assert_eq!(errors.len(), 1, "语法错误逐条上报: {errors:?}");
    assert!(
        errors[0].1.contains("syntax") || errors[0].1.contains("Syntax"),
        "错误类别可见: {}",
        errors[0].1
    );
    // 后续脚本（data-ok）照常执行。
    let document = page.document.borrow();
    let body = find_element(&page, "body");
    let data = match document.node(body) {
        Some(NodeKind::Element(data)) => data
            .attributes
            .iter()
            .find(|attr| attr.name == "data-ok")
            .map(|attr| attr.value.clone()),
        _ => None,
    };
    assert_eq!(data.as_deref(), Some("yes"), "错误不阻断后续脚本");
    let _ = session;
}

/// ④ 外链 src 脚本（defer 档）按文档序执行；脚本内注册的 DCL 监听器在
/// DCL 派发时触发（K1 时序保持：脚本先跑完才 DCL）。
#[test]
fn external_script_runs_before_dom_content_loaded_dispatch() {
    let html = r#"<head><script src="app.js"></script></head><body><p id="p">idle</p></body>"#;
    let mut bytes = HashMap::new();
    bytes.insert(
        "https://page.test/app.js".to_string(),
        br#"
        var p = document.getElementById("p");
        p.textContent = "from-src";
        document.addEventListener("DOMContentLoaded", function () {
            p.setAttribute("data-dcl", "seen");
        });
        "#
        .to_vec(),
    );
    let (page, session, errors) = page_with_script(html, &bytes, "https://page.test/");
    assert!(errors.is_empty(), "no script errors: {errors:?}");
    let session = session.expect("src script present → session installed");

    // src 脚本已执行（textContent 改写生效）。
    let document = page.document.borrow();
    let p = find_element(&page, "p");
    let text = match document.node(p) {
        Some(NodeKind::Element(_)) => {
            let child = document.first_child(p).expect("text child");
            match document.node(child) {
                Some(NodeKind::Text(data)) => data.clone(),
                other => panic!("expected text, got {other:?}"),
            }
        }
        other => panic!("expected element, got {other:?}"),
    };
    assert_eq!(text, "from-src", "defer 档 src 脚本在 DCL 前执行");

    // DCL 派发进 JS 监听器表（app::events 的成对派发里 K1 先、JS 后）。
    drop(document);
    let root = page.document.borrow().root();
    let mut event = Event::dom_content_loaded();
    session.dispatch(root, &mut event).expect("dcl dispatch");
    let document = page.document.borrow();
    let data = match document.node(p) {
        Some(NodeKind::Element(data)) => data
            .attributes
            .iter()
            .find(|attr| attr.name == "data-dcl")
            .map(|attr| attr.value.clone()),
        _ => None,
    };
    assert_eq!(data.as_deref(), Some("seen"), "DCL 监听器触发");
}

/// ⑤ 脚本清单收集：inline 与外链按文档序、template 内容不收、src 空按
/// inline 处理（解析完成点单处钩子的契约面）。
#[test]
fn script_resources_collect_in_document_order() {
    let document = nexty_html::parse_document(
        r#"<head><script src="a.js"></script><script>inline1()</script></head>
           <body><script src=""></script><template><script>nope()</script></template>
           <script src="b.js"></script></body>"#,
        nexty_html::ParseOptions::default(),
    );
    let scripts = resources::collect_script_resources(&document);
    // 空 src 按 inline（空文本）收集——执行即 no-op；template 内容游离主树
    // 之外，其 script 不收。
    assert_eq!(scripts.len(), 4);
    assert!(matches!(scripts[0].source, ScriptSource::External { ref src } if src == "a.js"));
    assert!(
        matches!(scripts[1].source, ScriptSource::Inline(ref text) if text.contains("inline1"))
    );
    assert!(matches!(scripts[2].source, ScriptSource::Inline(ref text) if text.is_empty()));
    assert!(matches!(scripts[3].source, ScriptSource::External { ref src } if src == "b.js"));
}

/// ⑥ 无脚本页面零引擎成本：install_and_run 返回 None。
#[test]
fn scriptless_page_installs_no_engine() {
    let document = Rc::new(RefCell::new(nexty_html::parse_document(
        "<body><p>plain</p></body>",
        nexty_html::ParseOptions::default(),
    )));
    let scripts = resources::collect_script_resources(&document.borrow());
    assert!(scripts.is_empty());
    let resolve = |_: &str| None;
    let session = script::install_and_run(
        &document,
        &scripts,
        &HashMap::new(),
        &resolve,
        &[],
        |_, _| {},
    )
    .expect("engine init");
    assert!(session.is_none(), "无脚本不装配引擎");
    // 错误类别面：语法错误上报 kind 而非 panic（对照断言）。
    let html = "<body><script>bad(</script></body>";
    let document = Rc::new(RefCell::new(nexty_html::parse_document(
        html,
        nexty_html::ParseOptions::default(),
    )));
    let scripts = resources::collect_script_resources(&document.borrow());
    let mut kinds = Vec::new();
    let _ = script::install_and_run(
        &document,
        &scripts,
        &HashMap::new(),
        &resolve,
        &[],
        |_, error| kinds.push(error.kind),
    )
    .expect("engine init");
    assert_eq!(kinds, vec![ScriptErrorKind::Syntax], "语法错误类别化上抛");
}
