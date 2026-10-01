//! WPT tree-construction 比对 harness。
//!
//! 语料来自 `web-platform-tests/wpt` 的 tree-construction 测试，钉在 commit
//! `5cd8e3fa0a6c4ca11fa565f7c0956802c8e0045d`，放在
//! `tests/data/tree-construction/`。`.dat` 格式与序列化规则见该目录的
//! `README.md`，本文件的行为对齐上游 runner 的 `test.js`。
//!
//! 每个用例把输入喂给 [`parse_document`] 或 [`parse_fragment`]，把结果按
//! `README.md` 的 tree dump 格式序列化，与 `#document` 段逐字比对。
//!
//! 设 `NEXTY_WPT_FILTER=<子串>` 可只跑文件名匹配的语料，便于定位失败项。
//! 设 `NEXTY_WPT_DUMP=1` 只打印失败用例的 `(文件, 序号)`，用于重建基线。
//!
//! # 基线
//!
//! [`KNOWN_DIVERGENCE`] 列出「Nexty 本层只如实转写上游产出、失败源于上游或语料」
//! 的用例，分两类：html5ever 尚未跟进的规范改动，以及语料期望本身与现行规范冲突。
//! 基线外的失败即回归，会让测试失败；基线内却已通过的用例会被报为 stale
//! （上游或语料可能已更新，需更新基线）。

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use nexty_dom::{Document, Namespace, NodeId, NodeKind};
use nexty_html::{FragmentContext, ParseOptions, parse_document, parse_fragment};

/// 已知差异基线：`(语料文件, 用例序号, 原因)`。
///
/// 每条都经过读 html5ever 源码与 WHATWG 规范确认：失败不在 Nexty 本层——要么
/// html5ever 的词法 / 树构建尚未跟进规范，要么语料期望本身与现行规范冲突。
/// Nexty 的 `TreeSink` / arena DOM 只如实转写上游产出。
const KNOWN_DIVERGENCE: &[(&str, usize, &str)] = &[
    // 语料过时：期望产出 PI 节点，但现行规范无处理指令词法状态（88 条）。
    ("html5test-com.dat", 11, REASON_PI),
    ("tests1.dat", 39, REASON_PI),
    ("tests1.dat", 43, REASON_PI),
    ("tests1.dat", 46, REASON_PI),
    ("processing-instructions.dat", 0, REASON_PI),
    ("processing-instructions.dat", 1, REASON_PI),
    ("processing-instructions.dat", 2, REASON_PI),
    ("processing-instructions.dat", 3, REASON_PI),
    ("processing-instructions.dat", 4, REASON_PI),
    ("processing-instructions.dat", 5, REASON_PI),
    ("processing-instructions.dat", 6, REASON_PI),
    ("processing-instructions.dat", 7, REASON_PI),
    ("processing-instructions.dat", 8, REASON_PI),
    ("processing-instructions.dat", 9, REASON_PI),
    ("processing-instructions.dat", 10, REASON_PI),
    ("processing-instructions.dat", 11, REASON_PI),
    ("processing-instructions.dat", 12, REASON_PI),
    ("processing-instructions.dat", 13, REASON_PI),
    ("processing-instructions.dat", 14, REASON_PI),
    ("processing-instructions.dat", 15, REASON_PI),
    ("processing-instructions.dat", 16, REASON_PI),
    ("processing-instructions.dat", 17, REASON_PI),
    ("processing-instructions.dat", 18, REASON_PI),
    ("processing-instructions.dat", 19, REASON_PI),
    ("processing-instructions.dat", 20, REASON_PI),
    ("processing-instructions.dat", 21, REASON_PI),
    ("processing-instructions.dat", 22, REASON_PI),
    ("processing-instructions.dat", 23, REASON_PI),
    ("processing-instructions.dat", 24, REASON_PI),
    ("processing-instructions.dat", 25, REASON_PI),
    ("processing-instructions.dat", 26, REASON_PI),
    ("processing-instructions.dat", 27, REASON_PI),
    ("processing-instructions.dat", 28, REASON_PI),
    ("processing-instructions.dat", 29, REASON_PI),
    ("processing-instructions.dat", 30, REASON_PI),
    ("processing-instructions.dat", 31, REASON_PI),
    ("processing-instructions.dat", 32, REASON_PI),
    ("processing-instructions.dat", 33, REASON_PI),
    ("processing-instructions.dat", 34, REASON_PI),
    ("processing-instructions.dat", 35, REASON_PI),
    ("processing-instructions.dat", 36, REASON_PI),
    ("processing-instructions.dat", 37, REASON_PI),
    ("processing-instructions.dat", 38, REASON_PI),
    ("processing-instructions.dat", 39, REASON_PI),
    ("processing-instructions.dat", 40, REASON_PI),
    ("processing-instructions.dat", 41, REASON_PI),
    ("processing-instructions.dat", 42, REASON_PI),
    ("processing-instructions.dat", 43, REASON_PI),
    ("processing-instructions.dat", 44, REASON_PI),
    ("processing-instructions.dat", 45, REASON_PI),
    ("processing-instructions.dat", 46, REASON_PI),
    ("processing-instructions.dat", 47, REASON_PI),
    ("processing-instructions.dat", 48, REASON_PI),
    ("processing-instructions.dat", 49, REASON_PI),
    ("processing-instructions.dat", 50, REASON_PI),
    ("processing-instructions.dat", 51, REASON_PI),
    ("processing-instructions.dat", 52, REASON_PI),
    ("processing-instructions.dat", 53, REASON_PI),
    ("processing-instructions.dat", 54, REASON_PI),
    ("processing-instructions.dat", 55, REASON_PI),
    ("processing-instructions.dat", 56, REASON_PI),
    ("processing-instructions.dat", 57, REASON_PI),
    ("processing-instructions.dat", 58, REASON_PI),
    ("processing-instructions.dat", 59, REASON_PI),
    ("processing-instructions.dat", 60, REASON_PI),
    ("processing-instructions.dat", 61, REASON_PI),
    ("processing-instructions.dat", 62, REASON_PI),
    ("processing-instructions.dat", 63, REASON_PI),
    ("processing-instructions.dat", 64, REASON_PI),
    ("processing-instructions.dat", 100, REASON_PI),
    ("processing-instructions.dat", 101, REASON_PI),
    ("processing-instructions.dat", 102, REASON_PI),
    ("processing-instructions.dat", 103, REASON_PI),
    ("processing-instructions.dat", 104, REASON_PI),
    ("processing-instructions.dat", 105, REASON_PI),
    ("processing-instructions.dat", 107, REASON_PI),
    ("processing-instructions.dat", 108, REASON_PI),
    ("processing-instructions.dat", 109, REASON_PI),
    ("processing-instructions.dat", 110, REASON_PI),
    ("processing-instructions.dat", 113, REASON_PI),
    ("processing-instructions.dat", 114, REASON_PI),
    ("processing-instructions.dat", 115, REASON_PI),
    ("processing-instructions.dat", 116, REASON_PI),
    ("processing-instructions.dat", 117, REASON_PI),
    ("processing-instructions.dat", 118, REASON_PI),
    ("processing-instructions.dat", 119, REASON_PI),
    ("processing-instructions.dat", 122, REASON_PI),
    ("processing-instructions.dat", 123, REASON_PI),
    // 需要脚本执行（document.write），HTML 层不含 JSRT（6 条）。
    ("scripted_adoption01.dat", 0, REASON_SCRIPTED),
    ("scripted_ark.dat", 0, REASON_SCRIPTED),
    ("scripted_foster01.dat", 0, REASON_SCRIPTED),
    ("scripted_foster01.dat", 1, REASON_SCRIPTED),
    ("scripted_webkit01.dat", 0, REASON_SCRIPTED),
    ("scripted_webkit01.dat", 1, REASON_SCRIPTED),
    // <template> 的 frameset-ok / form 指针语义（6 条）。
    ("template.dat", 43, REASON_TEMPLATE),
    ("template.dat", 44, REASON_TEMPLATE),
    ("template.dat", 45, REASON_TEMPLATE),
    ("template.dat", 116, REASON_TEMPLATE),
    ("template.dat", 119, REASON_TEMPLATE),
    ("template.dat", 123, REASON_TEMPLATE),
    // <selectedcontent> 克隆钩子（4 条）。
    ("webkit02.dat", 44, REASON_SELECTEDCONTENT),
    ("webkit02.dat", 45, REASON_SELECTEDCONTENT),
    ("webkit02.dat", 46, REASON_SELECTEDCONTENT),
    ("webkit02.dat", 47, REASON_SELECTEDCONTENT),
    // in select 插入模式缺失（1 条）。
    ("tests_innerHTML_1.dat", 75, REASON_IN_SELECT),
];

/// 语料过时：`processing-instructions.dat` 等期望产出 PI 节点，但现行 WHATWG
/// §13.2.5 已无处理指令词法状态——`<?…>` 在 tag open state 触发
/// unexpected-question-mark-instead-of-tag-name，按 bogus comment 处理。
/// html5ever 的产出与规范一致，这些用例属语料（html5lib 遗留）过时。
const REASON_PI: &str = "语料过时：期望 PI 节点，但现行 WHATWG §13.2.5 无处理指令词法状态，<?…> 走 bogus comment，html5ever 与规范一致";

/// `in select` 插入模式：html5ever 无该模式，select 片段下 `<input>` 只报错未忽略。
const REASON_IN_SELECT: &str =
    "html5ever 缺 in select 插入模式：select 片段下 <input> 只记录解析错误、仍被插入";

/// `<selectedcontent>`：html5ever 只在显式 `</option>` 时回调克隆钩子。
const REASON_SELECTEDCONTENT: &str =
    "html5ever 仅在显式 </option> 时回调 selectedcontent 克隆（servo/html5ever#712）";

/// `<template>`：html5ever 的 frameset-ok / form 指针语义未跟上最新规范。
const REASON_TEMPLATE: &str =
    "html5ever 树构建与最新规范漂移：<template> 置 frameset-ok=false、沿用旧 form 指针语义";

/// 需要脚本执行（`document.write`）的用例，HTML 层不含 JSRT。
const REASON_SCRIPTED: &str = "需要脚本执行（document.write），HTML 层不含 JSRT";

/// 一条 tree-construction 用例。
struct Case {
    /// `#data` 段：喂给解析器的输入。
    data: String,
    /// `#document` 段：期望的 tree dump。
    document: String,
    /// `#document-fragment` 段：片段解析的上下文元素描述。
    fragment: Option<String>,
    /// 是否带 `#script-off`。
    script_off: bool,
}

/// 按 `.dat` 格式切分语料文件。
///
/// 逐行扫描，遇到 `#xxx` 段头就切换当前段；段内容原样累积，边界处的空行与
/// 段尾换行按上游 runner 的规则剪掉。
fn parse_dat(text: &str) -> Vec<Case> {
    let trimmed = text.strip_suffix('\n').unwrap_or(text);
    let lines: Vec<&str> = trimmed.split('\n').collect();

    let mut cases = Vec::new();
    let mut sections: Option<HashMap<String, String>> = None;
    let mut key: Option<String> = None;

    for (index, raw) in lines.iter().enumerate() {
        let line = if index + 1 == lines.len() {
            (*raw).to_string()
        } else {
            format!("{raw}\n")
        };

        let heading = line
            .strip_prefix('#')
            .map(str::trim)
            .filter(|heading| !heading.is_empty())
            .map(str::to_string);

        match heading {
            Some(heading) => {
                if heading == "data"
                    && let Some(mut previous_case) = sections.take()
                {
                    if let Some(previous) = key.as_ref()
                        && let Some(value) = previous_case.get_mut(previous)
                        && !value.is_empty()
                    {
                        value.pop();
                    }
                    cases.push(finish_case(previous_case));
                }
                sections
                    .get_or_insert_with(HashMap::new)
                    .insert(heading.clone(), String::new());
                key = Some(heading);
            }
            None => {
                if let (Some(sections), Some(key)) = (sections.as_mut(), key.as_ref()) {
                    sections.entry(key.clone()).or_default().push_str(&line);
                }
            }
        }
    }

    if let Some(sections) = sections {
        cases.push(finish_case(sections));
    }

    cases
}

/// 把累积的各段整理成一条用例，并剪掉段尾换行。
fn finish_case(sections: HashMap<String, String>) -> Case {
    let get = |name: &str| {
        sections
            .get(name)
            .map(|value| value.strip_suffix('\n').unwrap_or(value).to_string())
    };
    Case {
        data: get("data").unwrap_or_default(),
        document: get("document").unwrap_or_default(),
        fragment: get("document-fragment"),
        script_off: sections.contains_key("script-off"),
    }
}

/// 把 `#document-fragment` 段的描述转成上下文元素。
///
/// `svg ` / `math ` 前缀切换命名空间，其余按 HTML 处理。
fn context_from_dat(spec: &str) -> FragmentContext {
    if let Some(name) = spec.strip_prefix("svg ") {
        FragmentContext::svg(name)
    } else if let Some(name) = spec.strip_prefix("math ") {
        FragmentContext::mathml(name)
    } else {
        FragmentContext::html(spec)
    }
}

/// 按 `README.md` 的 tree dump 格式序列化 `root` 子树。
fn serialize_tree(document: &mut Document, root: NodeId) -> String {
    document.normalize(root);
    let mut lines = Vec::new();
    walk(document, root, 0, &mut lines);
    lines.join("\n")
}

fn walk(document: &Document, node: NodeId, depth: usize, lines: &mut Vec<String>) {
    let pad = if depth > 0 {
        " ".repeat(2 * depth - 1)
    } else {
        String::new()
    };
    let inner_pad = " ".repeat(2 * depth + 1);

    match document.node(node) {
        Some(NodeKind::Document) => lines.push("#document".to_string()),
        Some(NodeKind::DocumentFragment) => lines.push("#document-fragment".to_string()),
        Some(NodeKind::Doctype(data)) => {
            if data.name.is_empty() {
                lines.push(format!("|{pad}<!DOCTYPE >"));
            } else if data.public_id.is_empty() && data.system_id.is_empty() {
                lines.push(format!("|{pad}<!DOCTYPE {}>", data.name));
            } else {
                lines.push(format!(
                    "|{pad}<!DOCTYPE {} \"{}\" \"{}\">",
                    data.name, data.public_id, data.system_id
                ));
            }
        }
        Some(NodeKind::Comment(data)) => lines.push(format!("|{pad}<!-- {data} -->")),
        Some(NodeKind::ProcessingInstruction(data)) => {
            lines.push(format!("|{pad}<?{} {}?>", data.target, data.data));
        }
        Some(NodeKind::Text(data)) => lines.push(format!("|{pad}\"{data}\"")),
        Some(NodeKind::Element(data)) => {
            let tag = match &data.namespace {
                Namespace::Html | Namespace::None => data.name.clone(),
                namespace => format!("{} {}", prefix_of(namespace).unwrap_or(""), data.name),
            };
            lines.push(format!("|{pad}<{tag}>"));

            let mut attributes: Vec<(String, &str)> = data
                .attributes
                .iter()
                .map(|attribute| {
                    let name = match prefix_of(&attribute.namespace) {
                        Some(prefix) => format!("{prefix} {}", attribute.name),
                        None => attribute.name.clone(),
                    };
                    (name, attribute.value.as_str())
                })
                .collect();
            attributes.sort_by(|left, right| left.0.cmp(&right.0));
            for (name, value) in attributes {
                lines.push(format!("|{inner_pad}{name}=\"{value}\""));
            }

            if data.namespace == Namespace::Html && data.name == "template" {
                lines.push(format!("|{inner_pad}content"));
                if let Some(contents) = document.template_contents(node) {
                    for child in document.children(contents) {
                        walk(document, child, depth + 2, lines);
                    }
                }
            }
        }
        None => {}
    }

    for child in document.children(node) {
        walk(document, child, depth + 1, lines);
    }
}

/// tree dump 里的命名空间前缀。空命名空间不写前缀。
fn prefix_of(namespace: &Namespace) -> Option<&'static str> {
    match namespace {
        Namespace::None => None,
        Namespace::Html => Some("html"),
        Namespace::Svg => Some("svg"),
        Namespace::MathMl => Some("math"),
        Namespace::Other(uri) => match uri.as_str() {
            "http://www.w3.org/1999/xlink" => Some("xlink"),
            "http://www.w3.org/XML/1998/namespace" => Some("xml"),
            "http://www.w3.org/2000/xmlns/" => Some("xmlns"),
            _ => None,
        },
    }
}

/// 跑一条用例，返回实际序列化结果；与期望不符时由调用方比对。
fn run_case(case: &Case) -> String {
    let options = ParseOptions {
        scripting_enabled: !case.script_off,
    };

    match &case.fragment {
        Some(spec) => {
            let (mut document, fragment) =
                parse_fragment(&case.data, &context_from_dat(spec), options);
            let actual = serialize_tree(&mut document, fragment);
            // 上游 runner 把片段根的标记换成 `#document` 再比对。
            match actual.strip_prefix("#document-fragment") {
                Some(rest) => format!("#document{rest}"),
                None => actual,
            }
        }
        None => {
            let mut document = parse_document(&case.data, options);
            let root = document.root();
            serialize_tree(&mut document, root)
        }
    }
}

/// 查基线：命中则给出失败原因。
fn known_reason(file: &str, index: usize) -> Option<&'static str> {
    KNOWN_DIVERGENCE
        .iter()
        .find(|(name, case, _)| *name == file && *case == index)
        .map(|(_, _, reason)| *reason)
}

/// 逐文件跑完语料，打印通过率与基线命中情况。
///
/// 基线外的失败会让测试失败；基线内已通过的用例报为 stale 但不致命。
#[test]
fn wpt_tree_construction() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/tree-construction");
    let filter = std::env::var("NEXTY_WPT_FILTER").ok();
    let dump = std::env::var_os("NEXTY_WPT_DUMP").is_some();

    let mut files: Vec<_> = fs::read_dir(&dir)
        .expect("tree-construction corpus directory")
        .map(|entry| entry.expect("dir entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "dat"))
        .collect();
    files.sort();

    let mut total = 0usize;
    let mut passed = 0usize;
    let mut known = 0usize;
    let mut report = Vec::new();
    let mut failures = Vec::new();
    let mut dump_pairs = Vec::new();
    let mut known_by_reason: HashMap<&'static str, usize> = HashMap::new();
    let mut stale: Vec<String> = Vec::new();
    let mut unexpected = 0usize;

    for file in files {
        let name = file
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        if let Some(filter) = &filter
            && !name.contains(filter.as_str())
        {
            continue;
        }

        let text = fs::read_to_string(&file).expect("read corpus file");
        let cases = parse_dat(&text);
        let mut file_passed = 0usize;

        for (index, case) in cases.iter().enumerate() {
            total += 1;
            let expected = format!("#document\n{}", case.document);
            let actual = run_case(case);
            let baseline = known_reason(&name, index);

            if actual == expected {
                passed += 1;
                file_passed += 1;
                if baseline.is_some() {
                    stale.push(format!("{name} #{index}"));
                }
            } else if let Some(reason) = baseline {
                known += 1;
                *known_by_reason.entry(reason).or_default() += 1;
            } else {
                unexpected += 1;
                dump_pairs.push((name.clone(), index));
                if failures.len() < MAX_REPORTED_FAILURES {
                    failures.push(format_failure(&name, index, case, &expected, &actual));
                }
            }
        }

        report.push(format!("{name:<32} {file_passed}/{}", cases.len()));
    }

    if dump {
        println!("=== 失败用例 (file, index) ===");
        for (name, index) in &dump_pairs {
            println!("    (\"{name}\", {index}),");
        }
        return;
    }

    let mut summary = format!("{}\n\n总计 {passed}/{total} 通过", report.join("\n"));
    if known > 0 {
        summary.push_str(&format!("\n已知差异 {known} 条："));
        let mut reasons: Vec<_> = known_by_reason.into_iter().collect();
        reasons.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        for (reason, count) in reasons {
            summary.push_str(&format!("\n  - {count:>3}  {reason}"));
        }
    }
    if !stale.is_empty() {
        summary.push_str(&format!(
            "\n基线内已通过（上游或语料可能已更新，请更新 KNOWN_DIVERGENCE）：{}",
            stale.join("、")
        ));
    }

    assert_eq!(
        unexpected,
        0,
        "{summary}\n\n=== 基线外失败 {unexpected} 条（最多列出 {MAX_REPORTED_FAILURES} 条）===\n\n{}",
        failures.join("\n\n")
    );

    println!("{summary}");
}

const MAX_REPORTED_FAILURES: usize = 12;
const MAX_FAILURE_CHARS: usize = 1200;

fn format_failure(file: &str, index: usize, case: &Case, expected: &str, actual: &str) -> String {
    let context = case
        .fragment
        .as_deref()
        .map(|spec| format!("（片段上下文 {spec}）"))
        .unwrap_or_default();
    format!(
        "{file} 用例 #{index}{context}\n输入: {}\n--- 期望 ---\n{}\n--- 实际 ---\n{}",
        truncate(&case.data),
        truncate(expected),
        truncate(actual),
    )
}

fn truncate(text: &str) -> String {
    if text.chars().count() <= MAX_FAILURE_CHARS {
        return text.to_string();
    }
    let head: String = text.chars().take(MAX_FAILURE_CHARS).collect();
    format!("{head}\n…（截断）")
}
