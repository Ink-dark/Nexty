//! Nexty HTML 解析层 facade。
//!
//! 上游：`html5ever`（MIT/Apache-2.0），按 WHATWG HTML §13 做词法与树构建。
//! 本层的职责是实现 html5ever 的 `TreeSink`，把解析结果写进 [`nexty_dom`] 的
//! arena，使上游类型不越过本层边界。决策见
//! `docs/decisions/2026-10-01-crate-selection.md`。
//!
//! 行为 ground truth：WHATWG HTML Standard §13（Parsing）。
//!
//! 公共 API 只有 [`parse_document`] 与 [`parse_fragment`]：输入 UTF-8 字符串，
//! 输出 [`Document`]。`TreeSink` 实现类型不对外暴露，`QualName` / `Attribute` /
//! `StrTendril` 等上游类型均在本层内转换为自有类型。
//!
//! 另提供 [`collect_style_resources`]：把 `<link rel=stylesheet>` 与 `<style>`
//! 按树序收集为资源清单（只收集不解析），供上层抓取外链 CSS 后按源顺序级联。

#![forbid(unsafe_code)]

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashSet;

use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::tree_builder::{
    AppendNode, AppendText, ElemName, ElementFlags, NodeOrText, QuirksMode, TreeSink,
};
use html5ever::{Attribute as HtmlAttribute, LocalName, Namespace as HtmlNamespace, QualName};

use nexty_dom::{
    Attribute, Document, DocumentTypeData, ElementData, Namespace, NodeId, NodeKind,
    ProcessingInstructionData, QuirksMode as DocumentQuirksMode,
};

const HTML_NAMESPACE: &str = "http://www.w3.org/1999/xhtml";
const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";
const MATHML_NAMESPACE: &str = "http://www.w3.org/1998/Math/MathML";

/// 解析选项。
///
/// 对应 WHATWG HTML §13.2.3 的
/// [scripting flag](https://html.spec.whatwg.org/multipage/parsing.html#scripting-flag)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseOptions {
    /// 脚本是否启用。
    ///
    /// 影响 `<noscript>` 的解析：启用时其内容按单个文本节点处理，禁用时按普通
    /// 元素树处理。
    pub scripting_enabled: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            scripting_enabled: true,
        }
    }
}

/// HTML 片段解析的上下文元素。
///
/// 对应 WHATWG HTML §13.4
/// [parsing HTML fragments](https://html.spec.whatwg.org/multipage/parsing.html#parsing-html-fragments)
/// 里的 context element：决定 tokenizer 的初始状态与外来内容的命名空间切换。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentContext {
    /// 上下文元素局部名。
    pub name: String,
    /// 上下文元素命名空间。
    pub namespace: Namespace,
}

impl FragmentContext {
    /// HTML 命名空间下的上下文元素。
    #[must_use]
    pub fn html(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            namespace: Namespace::Html,
        }
    }

    /// SVG 命名空间下的上下文元素。
    #[must_use]
    pub fn svg(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            namespace: Namespace::Svg,
        }
    }

    /// MathML 命名空间下的上下文元素。
    #[must_use]
    pub fn mathml(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            namespace: Namespace::MathMl,
        }
    }
}

/// 解析一份完整的 HTML 文档。
///
/// 输入按 UTF-8 处理；返回解析得到的 arena DOM，根节点是 `Document`。
/// 树构建遵循 WHATWG HTML §13（Parsing）：隐式的 `<html>` / `<head>` / `<body>`
/// 补齐、表格结构重建、错误恢复等行为与浏览器一致。
///
/// 文档的 quirks 模式由解析结果决定，见 [`Document::quirks_mode`]。
#[must_use]
pub fn parse_document(html: &str, options: ParseOptions) -> Document {
    let parser = html5ever::parse_document(HtmlTreeSink::new(), to_html_options(options));
    parser.one(html)
}

/// 按 WHATWG HTML §13.4 解析一段 HTML 片段。
///
/// `context` 是片段解析的上下文元素。返回承载结果的文档与其中的
/// `DocumentFragment`：片段节点列表即该片段的子节点，与
/// `Range.createContextualFragment` 的语义一致。
///
/// 与完整文档不同，片段解析不补齐 `<html>` / `<head>` / `<body>`，也不产生
/// doctype。
#[must_use]
pub fn parse_fragment(
    html: &str,
    context: &FragmentContext,
    options: ParseOptions,
) -> (Document, NodeId) {
    let parser = html5ever::parse_fragment(
        HtmlTreeSink::new(),
        to_html_options(options),
        QualName::new(
            None,
            to_html_namespace(&context.namespace),
            LocalName::from(context.name.as_str()),
        ),
        Vec::new(),
        options.scripting_enabled,
    );
    let mut document = parser.one(html);
    let fragment = extract_fragment(&mut document);
    (document, fragment)
}

/// 把片段解析结果从合成的 `<html>` 根搬进一个游离的 `DocumentFragment`。
///
/// html5ever 的片段解析把节点挂在合成的 `<html>` 根下；WHATWG HTML §13.4 规定
/// 返回的是 root 的子节点列表，这里用 `DocumentFragment` 承载。
fn extract_fragment(document: &mut Document) -> NodeId {
    let fragment = document.create_node(NodeKind::DocumentFragment);
    if let Some(html_root) = document.first_child(document.root()) {
        let children: Vec<NodeId> = document.children(html_root).collect();
        for child in children {
            document.insert_node(child, fragment, None);
        }
        document.remove_node(html_root);
    }
    fragment
}

/// 从文档树按树序收集的样式表资源。
///
/// 只收集不解析：外链的抓取与文本的解析由上层（network / css 层）负责。
/// 偏差：`media` / `type` 属性与 alternate 样式表集暂不建模，凡
/// `rel` 含 `stylesheet` token 的 `<link>` 一律收集。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StyleResource {
    /// `<link rel=stylesheet href=…>` 声明的外链样式表，字节需另行抓取。
    External {
        /// `href` 属性原值（可能是相对 URL，由上层解析）。
        href: String,
    },
    /// `<style>` 元素的文本内容，可直接交给 CSS 层解析。
    Inline {
        /// 元素内全部文本的拼接（可能为空串）。
        css: String,
    },
}

/// 按树序收集文档中的样式表资源清单。
///
/// 遍历整棵树，`<link rel=stylesheet>` 与 `<style>` 按文档出现顺序交错产出
/// ——级联优先级即此顺序（CSS Cascade：源顺序靠后者胜出）。规则：
///
/// - 只认 HTML 命名空间的 `link` / `style`；外来元素（SVG 等）里的同名元素不算。
/// - `rel` 是 ASCII 大小写不敏感、空白分隔的 token 列表，含 `stylesheet`
///   token 即命中（WHATWG HTML §4.6.6 link types）。
/// - `href` 缺失或为空串的 `<link>` 跳过（WHATWG HTML §4.6.6：空 href 不抓取）。
/// - `<style>` 的内容是全部子文本节点的拼接；空元素产出空串。
/// - `<template>` 内容游离于主树之外，天然不会被收集到。
#[must_use]
pub fn collect_style_resources(document: &Document) -> Vec<StyleResource> {
    let mut resources = Vec::new();
    collect_from(document, document.root(), &mut resources);
    resources
}

/// 以先序遍历收集子树里的样式表资源。
fn collect_from(document: &Document, node: NodeId, resources: &mut Vec<StyleResource>) {
    for child in document.children(node) {
        let Some(NodeKind::Element(data)) = document.node(child) else {
            continue;
        };
        if data.namespace != Namespace::Html {
            continue;
        }
        match data.name.as_str() {
            "link" => {
                if is_stylesheet_link(data)
                    && let Some(href) = attribute_value(data, "href")
                    && !href.is_empty()
                {
                    resources.push(StyleResource::External {
                        href: href.to_string(),
                    });
                }
            }
            "style" => {
                let mut css = String::new();
                for text in document.children(child) {
                    if let Some(NodeKind::Text(content)) = document.node(text) {
                        css.push_str(content);
                    }
                }
                resources.push(StyleResource::Inline { css });
            }
            _ => collect_from(document, child, resources),
        }
    }
}

/// `rel` 属性是否含 `stylesheet` token（ASCII 大小写不敏感）。
fn is_stylesheet_link(data: &ElementData) -> bool {
    attribute_value(data, "rel").is_some_and(|rel| {
        rel.split_ascii_whitespace()
            .any(|token| token.eq_ignore_ascii_case("stylesheet"))
    })
}

/// 取 HTML 元素的指定属性值。
fn attribute_value<'a>(data: &'a ElementData, name: &str) -> Option<&'a str> {
    data.attributes
        .iter()
        .find(|attr| attr.name == name)
        .map(|attr| attr.value.as_str())
}

/// 文档中收集到的一个图片资源。
///
/// 带元素节点 id：同一 src 可出现在多个 `<img>` 上（抓取按 src 去重，
/// 布局与绘制按 node 归位），节点身份不可丢。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageResource {
    /// `<img>` 元素节点。
    pub node: NodeId,
    /// `src` 属性原值（可能是相对 URL，由上层解析）。
    pub src: String,
}

/// 按树序收集文档中的 `<img src=…>` 清单。
///
/// 只收集不抓取、不解码：URL 解析与字节获取由上层（network / chrome 层）
/// 负责。规则：
///
/// - 只认 HTML 命名空间的 `img`；`src` 缺失或为空串的跳过。
/// - `srcset` / `<picture>` / `<source>` 暂不建模，只看 `src`（偏差）。
#[must_use]
pub fn collect_image_resources(document: &Document) -> Vec<ImageResource> {
    let mut resources = Vec::new();
    collect_images_from(document, document.root(), &mut resources);
    resources
}

/// 以先序遍历收集子树里的 `<img>`。
fn collect_images_from(document: &Document, node: NodeId, resources: &mut Vec<ImageResource>) {
    for child in document.children(node) {
        let Some(NodeKind::Element(data)) = document.node(child) else {
            continue;
        };
        if data.namespace == Namespace::Html && data.name == "img" {
            if let Some(src) = attribute_value(data, "src")
                && !src.is_empty()
            {
                resources.push(ImageResource {
                    node: child,
                    src: src.to_string(),
                });
            }
        } else {
            collect_images_from(document, child, resources);
        }
    }
}

/// 自有解析选项 → 上游解析选项。
fn to_html_options(options: ParseOptions) -> html5ever::ParseOpts {
    html5ever::ParseOpts {
        tree_builder: html5ever::tree_builder::TreeBuilderOpts {
            scripting_enabled: options.scripting_enabled,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// html5ever 的 `TreeSink` 实现：把解析事件写进 `nexty_dom` 的 arena。
///
/// 该类型不对外暴露，避免上游类型越过本层边界。
struct HtmlTreeSink {
    document: RefCell<Document>,
    /// MathML `annotation-xml` 集成点元素。
    ///
    /// `TreeSink::is_mathml_annotation_xml_integration_point` 只在建元素时拿到
    /// 该标志，故在此单独记录。
    mathml_integration_points: RefCell<HashSet<NodeId>>,
}

impl HtmlTreeSink {
    fn new() -> Self {
        Self {
            document: RefCell::new(Document::new()),
            mathml_integration_points: RefCell::new(HashSet::new()),
        }
    }
}

impl TreeSink for HtmlTreeSink {
    type Handle = NodeId;
    type Output = Document;
    type ElemName<'a> = ElementName;

    fn finish(self) -> Document {
        self.document.into_inner()
    }

    fn parse_error(&self, _msg: Cow<'static, str>) {
        // 解析错误暂不对外暴露：本层公共 API 只返回 DOM。
    }

    fn get_document(&self) -> NodeId {
        self.document.borrow().root()
    }

    fn elem_name<'a>(&'a self, target: &'a NodeId) -> ElementName {
        let document = self.document.borrow();
        let Some(NodeKind::Element(data)) = document.node(*target) else {
            panic!("elem_name called on a non-element node");
        };
        ElementName {
            namespace: to_html_namespace(&data.namespace),
            local: LocalName::from(data.name.as_str()),
        }
    }

    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<HtmlAttribute>,
        flags: ElementFlags,
    ) -> NodeId {
        let element = {
            let mut document = self.document.borrow_mut();
            let element = document.create_node(NodeKind::Element(ElementData {
                name: name.local.to_string(),
                namespace: to_dom_namespace(&name.ns),
                attributes: attrs
                    .into_iter()
                    .map(|attr| Attribute {
                        name: attr.name.local.to_string(),
                        namespace: to_dom_namespace(&attr.name.ns),
                        value: attr.value.to_string(),
                    })
                    .collect(),
            }));
            if flags.template {
                let contents = document.create_node(NodeKind::DocumentFragment);
                document.set_template_contents(element, contents);
            }
            element
        };

        if flags.mathml_annotation_xml_integration_point {
            self.mathml_integration_points.borrow_mut().insert(element);
        }
        element
    }

    fn create_comment(&self, text: StrTendril) -> NodeId {
        self.document
            .borrow_mut()
            .create_node(NodeKind::Comment(text.to_string()))
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> NodeId {
        self.document
            .borrow_mut()
            .create_node(NodeKind::ProcessingInstruction(ProcessingInstructionData {
                target: target.to_string(),
                data: data.to_string(),
            }))
    }

    fn append(&self, parent: &NodeId, child: NodeOrText<NodeId>) {
        let mut document = self.document.borrow_mut();
        match child {
            AppendText(text) => {
                if let Some(last) = document.last_child(*parent)
                    && let Some(NodeKind::Text(data)) = document.node_mut(last)
                {
                    data.push_str(&text);
                    return;
                }
                let node = document.create_node(NodeKind::Text(text.to_string()));
                document.insert_node(node, *parent, None);
            }
            AppendNode(node) => {
                document.insert_node(node, *parent, None);
            }
        }
    }

    fn append_before_sibling(&self, sibling: &NodeId, new_node: NodeOrText<NodeId>) {
        let mut document = self.document.borrow_mut();
        let reference = *sibling;
        let parent = document
            .parent(reference)
            .expect("append_before_sibling called on a parentless node");
        match new_node {
            AppendText(text) => {
                if let Some(previous) = document.previous_sibling(reference)
                    && let Some(NodeKind::Text(data)) = document.node_mut(previous)
                {
                    data.push_str(&text);
                    return;
                }
                let node = document.create_node(NodeKind::Text(text.to_string()));
                document.insert_node(node, parent, Some(reference));
            }
            AppendNode(node) => {
                document.insert_node(node, parent, Some(reference));
            }
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &NodeId,
        prev_element: &NodeId,
        child: NodeOrText<NodeId>,
    ) {
        if self.document.borrow().parent(*element).is_some() {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    ) {
        let mut document = self.document.borrow_mut();
        let node = document.create_node(NodeKind::Doctype(DocumentTypeData {
            name: name.to_string(),
            public_id: public_id.to_string(),
            system_id: system_id.to_string(),
        }));
        let root = document.root();
        document.insert_node(node, root, None);
    }

    fn get_template_contents(&self, target: &NodeId) -> NodeId {
        self.document
            .borrow()
            .template_contents(*target)
            .expect("get_template_contents called on a non-template element")
    }

    fn same_node(&self, x: &NodeId, y: &NodeId) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.document.borrow_mut().set_quirks_mode(match mode {
            QuirksMode::Quirks => DocumentQuirksMode::Quirks,
            QuirksMode::LimitedQuirks => DocumentQuirksMode::LimitedQuirks,
            QuirksMode::NoQuirks => DocumentQuirksMode::NoQuirks,
        });
    }

    fn add_attrs_if_missing(&self, target: &NodeId, attrs: Vec<HtmlAttribute>) {
        let mut document = self.document.borrow_mut();
        let Some(NodeKind::Element(data)) = document.node_mut(*target) else {
            panic!("add_attrs_if_missing called on a non-element node");
        };
        for attr in attrs {
            let name = attr.name.local.to_string();
            let namespace = to_dom_namespace(&attr.name.ns);
            let exists = data
                .attributes
                .iter()
                .any(|existing| existing.name == name && existing.namespace == namespace);
            if !exists {
                data.attributes.push(Attribute {
                    name,
                    namespace,
                    value: attr.value.to_string(),
                });
            }
        }
    }

    fn remove_from_parent(&self, target: &NodeId) {
        self.document.borrow_mut().remove_node(*target);
    }

    fn reparent_children(&self, node: &NodeId, new_parent: &NodeId) {
        let mut document = self.document.borrow_mut();
        let children: Vec<NodeId> = document.children(*node).collect();
        for child in children {
            document.insert_node(child, *new_parent, None);
        }
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &NodeId) -> bool {
        self.mathml_integration_points.borrow().contains(handle)
    }
}

/// `TreeSink::elem_name` 的返回类型：元素局部名与命名空间。
///
/// 上游要求返回借用形式，而 arena 里存的是字符串，故这里持有转换出的原子名。
#[derive(Debug)]
struct ElementName {
    namespace: HtmlNamespace,
    local: LocalName,
}

impl ElemName for ElementName {
    fn ns(&self) -> &HtmlNamespace {
        &self.namespace
    }

    fn local_name(&self) -> &LocalName {
        &self.local
    }
}

/// 上游命名空间 → 自有命名空间。
fn to_dom_namespace(namespace: &HtmlNamespace) -> Namespace {
    match &**namespace {
        "" => Namespace::None,
        HTML_NAMESPACE => Namespace::Html,
        SVG_NAMESPACE => Namespace::Svg,
        MATHML_NAMESPACE => Namespace::MathMl,
        other => Namespace::Other(other.to_owned()),
    }
}

/// 自有命名空间 → 上游命名空间。
fn to_html_namespace(namespace: &Namespace) -> HtmlNamespace {
    match namespace {
        Namespace::None => HtmlNamespace::from(""),
        Namespace::Html => HtmlNamespace::from(HTML_NAMESPACE),
        Namespace::Svg => HtmlNamespace::from(SVG_NAMESPACE),
        Namespace::MathMl => HtmlNamespace::from(MATHML_NAMESPACE),
        Namespace::Other(uri) => HtmlNamespace::from(uri.as_str()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 以默认选项解析完整文档。
    fn parse(html: &str) -> Document {
        parse_document(html, ParseOptions::default())
    }

    /// 按树序展开 `root` 的全部后代（含自身）。
    fn descendants(document: &Document, root: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            out.push(id);
            let children: Vec<NodeId> = document.children(id).collect();
            for child in children.into_iter().rev() {
                stack.push(child);
            }
        }
        out
    }

    /// 按树序找第一个局部名匹配的元素。
    fn find_element(document: &Document, root: NodeId, name: &str) -> Option<NodeId> {
        descendants(document, root).into_iter().find(
            |id| matches!(document.node(*id), Some(NodeKind::Element(data)) if data.name == name),
        )
    }

    fn text_of(document: &Document, root: NodeId) -> String {
        let mut out = String::new();
        for id in descendants(document, root) {
            if let Some(NodeKind::Text(data)) = document.node(id) {
                out.push_str(data);
            }
        }
        out
    }

    fn element_data(document: &Document, id: NodeId) -> &ElementData {
        match document.node(id) {
            Some(NodeKind::Element(data)) => data,
            other => panic!("expected element, got {other:?}"),
        }
    }

    #[test]
    fn implied_structure_and_doctype() {
        let document = parse("<!DOCTYPE html><p>hello");
        let root = document.root();

        let children: Vec<NodeId> = document.children(root).collect();
        assert_eq!(children.len(), 2);
        assert!(matches!(
            document.node(children[0]),
            Some(NodeKind::Doctype(DocumentTypeData { name, .. })) if name == "html"
        ));
        assert_eq!(document.quirks_mode(), DocumentQuirksMode::NoQuirks);

        let body = find_element(&document, root, "body").expect("implied body");
        let paragraph = find_element(&document, body, "p").expect("paragraph");
        assert_eq!(text_of(&document, paragraph), "hello");
    }

    #[test]
    fn missing_doctype_sets_quirks_mode() {
        let document = parse("<p>x</p>");
        assert_eq!(document.quirks_mode(), DocumentQuirksMode::Quirks);
    }

    #[test]
    fn character_tokens_merge_into_one_text_node() {
        let document = parse("<p>a&amp;b</p>");
        let paragraph = find_element(&document, document.root(), "p").unwrap();
        let texts: Vec<NodeId> = document
            .children(paragraph)
            .filter(|id| matches!(document.node(*id), Some(NodeKind::Text(_))))
            .collect();
        assert_eq!(texts.len(), 1, "adjacent character tokens must merge");
        assert_eq!(document.node(texts[0]), Some(&NodeKind::Text("a&b".into())));
    }

    #[test]
    fn attributes_keep_order_namespace_and_values() {
        let document = parse("<div id=\"a\" class=\"b\"></div>");
        let div = find_element(&document, document.root(), "div").unwrap();
        let data = element_data(&document, div);
        assert_eq!(data.name, "div");
        assert_eq!(data.namespace, Namespace::Html);
        let attributes: Vec<(&str, &str)> = data
            .attributes
            .iter()
            .map(|attr| (attr.name.as_str(), attr.value.as_str()))
            .collect();
        assert_eq!(attributes, vec![("id", "a"), ("class", "b")]);
        assert!(
            data.attributes
                .iter()
                .all(|a| a.namespace == Namespace::None)
        );
    }

    #[test]
    fn comment_node_is_preserved() {
        let document = parse("<div><!-- hi --></div>");
        let div = find_element(&document, document.root(), "div").unwrap();
        let first = document.first_child(div).unwrap();
        assert_eq!(
            document.node(first),
            Some(&NodeKind::Comment(" hi ".into()))
        );
    }

    #[test]
    fn foreign_content_keeps_namespace() {
        let document = parse("<svg><circle r=\"1\"/></svg>");
        let circle = find_element(&document, document.root(), "circle").unwrap();
        assert_eq!(element_data(&document, circle).namespace, Namespace::Svg);
    }

    #[test]
    fn mathml_integration_point_switches_back_to_html() {
        let document = parse(
            "<math><annotation-xml encoding=\"text/html\"><div>x</div></annotation-xml></math>",
        );
        let div = find_element(&document, document.root(), "div").unwrap();
        assert_eq!(element_data(&document, div).namespace, Namespace::Html);
    }

    #[test]
    fn template_children_go_into_template_contents() {
        let document = parse("<template><span>x</span></template>");
        let template = find_element(&document, document.root(), "template").unwrap();
        assert_eq!(document.children(template).count(), 0);

        let contents = document
            .template_contents(template)
            .expect("template contents");
        assert!(matches!(
            document.node(contents),
            Some(NodeKind::DocumentFragment)
        ));
        let span = document.first_child(contents).expect("span");
        assert_eq!(element_data(&document, span).name, "span");
    }

    #[test]
    fn table_structure_is_rebuilt() {
        let document = parse("<table><tr><td>x</td></tr></table>");
        let root = document.root();
        let table = find_element(&document, root, "table").unwrap();
        let tbody = find_element(&document, table, "tbody").expect("implied tbody");
        let row = find_element(&document, tbody, "tr").expect("row");
        let cell = find_element(&document, row, "td").expect("cell");
        assert_eq!(text_of(&document, cell), "x");
    }

    #[test]
    fn misnested_tags_recover_like_a_browser() {
        let document = parse("<b><i>x</b>y</i>");
        let body = find_element(&document, document.root(), "body").unwrap();
        assert_eq!(text_of(&document, body), "xy");
    }

    #[test]
    fn reparent_children_moves_subtree_in_order() {
        let document = parse("<div><span>a</span><span>b</span></div>");
        let div = find_element(&document, document.root(), "div").unwrap();
        let spans: Vec<NodeId> = descendants(&document, div)
            .into_iter()
            .filter(|id| matches!(document.node(*id), Some(NodeKind::Element(data)) if data.name == "span"))
            .collect();
        assert_eq!(spans.len(), 2);
        assert_eq!(text_of(&document, spans[0]), "a");
        assert_eq!(text_of(&document, spans[1]), "b");
    }

    #[test]
    fn fragment_does_not_grow_html_head_body() {
        let (document, fragment) = parse_fragment(
            "<td>x</td>",
            &FragmentContext::html("tr"),
            ParseOptions::default(),
        );
        assert!(matches!(
            document.node(fragment),
            Some(NodeKind::DocumentFragment)
        ));

        let cell = document.first_child(fragment).expect("td");
        assert_eq!(element_data(&document, cell).name, "td");
        assert_eq!(text_of(&document, cell), "x");
    }

    #[test]
    fn fragment_context_switches_namespace() {
        let (document, fragment) = parse_fragment(
            "<circle/>",
            &FragmentContext::svg("svg"),
            ParseOptions::default(),
        );
        let circle = document.first_child(fragment).expect("circle");
        assert_eq!(element_data(&document, circle).namespace, Namespace::Svg);
    }

    #[test]
    fn fragment_context_drives_tokenizer_state() {
        let (document, fragment) = parse_fragment(
            "<b>x</b>",
            &FragmentContext::html("title"),
            ParseOptions::default(),
        );
        let text = document.first_child(fragment).expect("text");
        assert_eq!(
            document.node(text),
            Some(&NodeKind::Text("<b>x</b>".into()))
        );
    }

    #[test]
    fn scripting_flag_controls_noscript_parsing() {
        let disabled = parse_document(
            "<head><noscript><div>x</div></noscript></head>",
            ParseOptions {
                scripting_enabled: false,
            },
        );
        assert!(find_element(&disabled, disabled.root(), "div").is_some());

        let enabled = parse_document(
            "<head><noscript><div>x</div></noscript></head>",
            ParseOptions::default(),
        );
        assert!(find_element(&enabled, enabled.root(), "div").is_none());
    }

    #[test]
    fn style_resources_keep_tree_order() {
        let document = parse(
            "<head><style>p { color: red }</style>\
             <link rel=stylesheet href=a.css><link rel=preload href=b.js></head>\
             <body><style>div { color: blue }</style></body>",
        );
        assert_eq!(
            collect_style_resources(&document),
            vec![
                StyleResource::Inline {
                    css: "p { color: red }".into()
                },
                StyleResource::External {
                    href: "a.css".into()
                },
                StyleResource::Inline {
                    css: "div { color: blue }".into()
                },
            ]
        );
    }

    #[test]
    fn stylesheet_rel_matches_case_insensitive_token_list() {
        let document = parse(
            "<head>\
             <link rel=\"STYLESHEET\" href=a.css>\
             <link rel=\"alternate stylesheet\" href=b.css>\
             <link rel=preload href=c.css>\
             </head>",
        );
        let resources = collect_style_resources(&document);
        let hrefs: Vec<&str> = resources
            .iter()
            .filter_map(|resource| match resource {
                StyleResource::External { href } => Some(href.as_str()),
                StyleResource::Inline { .. } => None,
            })
            .collect();
        assert_eq!(hrefs, vec!["a.css", "b.css"]);
    }

    #[test]
    fn link_without_usable_href_is_skipped() {
        let document = parse("<head><link rel=stylesheet><link rel=stylesheet href=\"\"></head>");
        assert!(collect_style_resources(&document).is_empty());
    }

    #[test]
    fn foreign_namespace_style_is_not_collected() {
        let document = parse("<svg><style>circle { fill: red }</style></svg>");
        assert!(collect_style_resources(&document).is_empty());
    }

    #[test]
    fn style_in_template_is_not_collected() {
        let document = parse("<template><style>p { color: red }</style></template>");
        assert!(collect_style_resources(&document).is_empty());
    }

    #[test]
    fn empty_style_element_yields_empty_css() {
        let document = parse("<head><style></style></head>");
        assert_eq!(
            collect_style_resources(&document),
            vec![StyleResource::Inline { css: String::new() }]
        );
    }

    #[test]
    fn image_resources_carry_node_and_tree_order() {
        let document = parse(
            "<body><img src=\"a.png\"><img src=\"\" alt=\"empty skipped\">\
             <div><img src=\"b.png\"></div></body>",
        );
        let resources = collect_image_resources(&document);
        assert_eq!(resources.len(), 2);
        assert_eq!(resources[0].src, "a.png");
        assert_eq!(
            element_data(&document, resources[0].node).name,
            "img",
            "node 指向 img 元素"
        );
        assert_eq!(resources[1].src, "b.png");
        assert_ne!(resources[0].node, resources[1].node);
    }

    #[test]
    fn image_without_src_is_skipped() {
        let document = parse("<body><img alt=\"no src\"></body>");
        assert!(collect_image_resources(&document).is_empty());
    }
}
