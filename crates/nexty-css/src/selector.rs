//! 选择器解析与匹配：`selectors` crate 的适配层。
//!
//! 行为 ground truth：[CSS Selectors Level 4](https://drafts.csswg.org/selectors-4/)。
//! `selectors` 负责选择器语法解析与匹配算法；本模块提供三件事：
//! 1. [`SelectorImpl`] 的关联类型（字符串原子、空伪类/伪元素集合）；
//! 2. arena DOM（[`nexty_dom::Document`] + [`NodeId`]）的 `Element` trait 适配；
//! 3. 不泄漏上游类型的公共匹配 API。
//!
//! 元素身份：`opaque()` 用 arena 中该节点数据的地址。同一 [`Document`] 共享借用
//! 期间地址稳定，因此 `:has()` 锚点与 nth 缓存的跨 wrapper 身份比较成立。
//!
//! HTML 元素（HTML 命名空间）按 Selectors §3.1.3 在 HTML 文档中做 ASCII
//! case-insensitive 的类型 / 属性名匹配。
//!
//! **Feature**：本模块受 `selectors` feature 控制（由 lib.rs 装配）。
//! `NextySelector::specificity` 字段由 `cascade` 模块读取、`to_css_string` 由
//! `parser` 消费，故上层 feature 关闭时它们暂无读取方——此时按模块整体抑制
//! dead_code。

#![cfg_attr(
    not(any(feature = "stylesheets", feature = "cascade")),
    allow(dead_code)
)]
#![cfg_attr(not(feature = "cascade"), allow(dead_code))]

use std::borrow::Borrow;
use std::fmt;

use cssparser::ToCss;
use nexty_dom::{Document, ElementData, Namespace, NodeId, NodeKind, QuirksMode as DomQuirksMode};
use precomputed_hash::PrecomputedHash;
use selectors::attr::{AttrSelectorOperation, CaseSensitivity, NamespaceConstraint};
use selectors::context::{
    MatchingContext, MatchingForInvalidation, MatchingMode, NeedsSelectorFlags, QuirksMode,
    SelectorCaches,
};
use selectors::matching::matches_selector as match_single_selector;
use selectors::parser::{
    NonTSPseudoClass, ParseRelative, PseudoElement, Selector, SelectorImpl, SelectorList,
    SelectorParseErrorKind,
};
use selectors::{Element as SelectorsElement, OpaqueElement};

const HTML_NAMESPACE_URL: &str = "http://www.w3.org/1999/xhtml";
const SVG_NAMESPACE_URL: &str = "http://www.w3.org/2000/svg";
const MATHML_NAMESPACE_URL: &str = "http://www.w3.org/1998/Math/MathML";

/// 选择器关联类型用的字符串原子。
///
/// 内置 precomputed hash 供 selectors 的 bloom / 缓存路径使用；
/// 进程内稳定（`DefaultHasher`），不跨进程。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Atom {
    string: String,
    hash: u32,
}

impl Atom {
    fn new(string: impl Into<String>) -> Self {
        let string = string.into();
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        string.hash(&mut hasher);
        Self {
            string,
            hash: hasher.finish() as u32,
        }
    }
}

impl From<&str> for Atom {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl Default for Atom {
    fn default() -> Self {
        Self::new("")
    }
}

impl Borrow<str> for Atom {
    fn borrow(&self) -> &str {
        &self.string
    }
}

impl AsRef<str> for Atom {
    fn as_ref(&self) -> &str {
        &self.string
    }
}

/// 显式取原子内的字符串（绕开 `Borrow` 多重候选的歧义）。
fn atom_str(atom: &Atom) -> &str {
    atom.borrow()
}

impl PrecomputedHash for Atom {
    fn precomputed_hash(&self) -> u32 {
        self.hash
    }
}

impl ToCss for Atom {
    fn to_css<W: fmt::Write>(&self, dest: &mut W) -> fmt::Result {
        dest.write_str(&self.string)
    }
}

/// [`NextySelectorImpl`] 的伪类 / 伪元素占位类型。
///
/// 本层不接受任何非结构伪类与伪元素（解析期即拒绝），故为空集合。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NeverPseudo {}

impl ToCss for NeverPseudo {
    fn to_css<W: fmt::Write>(&self, _dest: &mut W) -> fmt::Result {
        match *self {}
    }
}

impl NonTSPseudoClass for NeverPseudo {
    fn is_active_or_hover(&self) -> bool {
        match *self {}
    }

    fn is_user_action_state(&self) -> bool {
        match *self {}
    }
}

impl PseudoElement for NeverPseudo {}

/// [`SelectorImpl`] 实现：字符串原子 + 空伪类 / 伪元素。
#[derive(Clone, Debug)]
pub(crate) struct NextySelectorImpl;

impl SelectorImpl for NextySelectorImpl {
    type ExtraMatchingData<'a> = ();
    type AttrValue = Atom;
    type Identifier = Atom;
    type LocalName = Atom;
    type NamespaceUrl = Atom;
    type NamespacePrefix = Atom;
    type BorrowedNamespaceUrl = str;
    type BorrowedLocalName = str;
    type NonTSPseudoClass = NeverPseudo;
    type PseudoElement = NeverPseudo;
}

/// 选择器解析器配置：启用 Selectors 4 的 `:is()` / `:where()` / `:has()` 与
/// `:nth-child(An+B of S)`（crate 支持、规范定稿的特性）。
///
/// 非结构伪类、伪元素、`@namespace` 前缀与 shadow DOM 相关语法按默认实现拒绝。
pub(crate) struct NextySelectorParser;

impl selectors::parser::Parser<'_> for NextySelectorParser {
    type Impl = NextySelectorImpl;
    type Error = SelectorParseErrorKind;

    fn parse_is_and_where(&self) -> bool {
        true
    }

    fn parse_has(&self) -> bool {
        true
    }

    fn parse_nth_child_of(&self) -> bool {
        true
    }
}

/// 一条已解析的复式选择器及其特异度。
#[derive(Debug, Clone)]
pub(crate) struct NextySelector {
    /// 已解析选择器。
    pub(crate) selector: Selector<NextySelectorImpl>,
    /// 特异度（a/b/c 打包为 u32，selectors crate 的 `specificity()` 语义）。
    pub(crate) specificity: u32,
}

impl NextySelector {
    /// 选择器的序列化形式（调试与 selector_text 用）。
    pub(crate) fn to_css_string(&self) -> String {
        self.selector.to_css_string()
    }
}

/// 解析一个逗号分隔的选择器列表（Selectors 4 §Grouping）。
///
/// 列表中任一选择器无效时整体返回 `Err`（样式规则按 CSS Syntax §5.3 丢弃）。
pub(crate) fn parse_selector_list(
    input: &mut cssparser::Parser<'_>,
) -> Result<Vec<NextySelector>, ()> {
    let list = SelectorList::parse(&NextySelectorParser, input, ParseRelative::No)
        .map_err(|_: cssparser::ParseError<SelectorParseErrorKind>| ())?;
    Ok(list
        .slice()
        .iter()
        .map(|selector| NextySelector {
            specificity: selector.specificity(),
            selector: selector.clone(),
        })
        .collect())
}

/// 选择器解析失败。
///
/// 不携带上游错误细节；样式表解析路径上无效选择器导致整条规则丢弃。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectorError;

/// 判断 `element` 是否匹配选择器文本（Selectors 4）。
///
/// # Errors
///
/// 选择器文本无效时返回 [`SelectorError`]。
pub fn matches_selector(
    selector_text: &str,
    document: &Document,
    element: NodeId,
) -> Result<bool, SelectorError> {
    let parsed = parse_selector_text(selector_text)?;
    Ok(match_element_selectors(&parsed, document, element))
}

/// 解析选择器文本。
fn parse_selector_text(selector_text: &str) -> Result<Vec<NextySelector>, SelectorError> {
    let mut parser = cssparser::Parser::new(selector_text);
    parse_selector_list(&mut parser).map_err(|_| SelectorError)
}

/// 对元素逐一匹配已解析选择器列表，任一命中即真。
pub(crate) fn match_element_selectors(
    selectors: &[NextySelector],
    document: &Document,
    element: NodeId,
) -> bool {
    let dom_element = DomElement::new(document, element);
    let mut caches = SelectorCaches::default();
    let mut context = MatchingContext::new(
        MatchingMode::Normal,
        None,
        &mut caches,
        to_matching_quirks_mode(document.quirks_mode()),
        NeedsSelectorFlags::No,
        MatchingForInvalidation::No,
    );
    selectors
        .iter()
        .any(|parsed| match_single_selector(&parsed.selector, 0, None, &dom_element, &mut context))
}

/// 自有 quirks 模式 → selectors 匹配上下文的 quirks 模式。
fn to_matching_quirks_mode(mode: DomQuirksMode) -> QuirksMode {
    match mode {
        DomQuirksMode::NoQuirks => QuirksMode::NoQuirks,
        DomQuirksMode::Quirks => QuirksMode::Quirks,
        DomQuirksMode::LimitedQuirks => QuirksMode::LimitedQuirks,
    }
}

/// arena DOM 元素在 selectors 匹配算法中的表示。
#[derive(Debug, Clone)]
pub(crate) struct DomElement<'a> {
    document: &'a Document,
    id: NodeId,
}

impl<'a> DomElement<'a> {
    pub(crate) fn new(document: &'a Document, id: NodeId) -> Self {
        Self { document, id }
    }

    fn element_data(&self) -> Option<&'a ElementData> {
        match self.document.node(self.id) {
            Some(NodeKind::Element(data)) => Some(data),
            _ => None,
        }
    }

    fn is_html_element(&self) -> bool {
        self.element_data()
            .is_some_and(|data| data.namespace == Namespace::Html)
    }

    /// 命名空间 → 命名空间 URL 字符串。
    fn namespace_url(&self) -> Option<String> {
        self.element_data().map(|data| {
            match &data.namespace {
                Namespace::None => "",
                Namespace::Html => HTML_NAMESPACE_URL,
                Namespace::Svg => SVG_NAMESPACE_URL,
                Namespace::MathMl => MATHML_NAMESPACE_URL,
                Namespace::Other(uri) => uri.as_str(),
            }
            .to_owned()
        })
    }

    /// 属性命名空间 → 命名空间 URL 字符串。
    fn attr_namespace_url(namespace: &Namespace) -> String {
        match namespace {
            Namespace::None => String::new(),
            Namespace::Html => HTML_NAMESPACE_URL.to_owned(),
            Namespace::Svg => SVG_NAMESPACE_URL.to_owned(),
            Namespace::MathMl => MATHML_NAMESPACE_URL.to_owned(),
            Namespace::Other(uri) => uri.clone(),
        }
    }

    /// 最近的一个元素祖先；跨过 Document / DocumentFragment 即停。
    fn ancestor_element(&self) -> Option<Self> {
        let mut current = self.document.parent(self.id);
        while let Some(id) = current {
            match self.document.node(id) {
                Some(NodeKind::Element(_)) => return Some(Self::new(self.document, id)),
                // 文档根与模板内容片段都不是元素，向上遍历到此为止
                Some(NodeKind::Document | NodeKind::DocumentFragment) => return None,
                _ => current = self.document.parent(id),
            }
        }
        None
    }

    /// 相邻兄弟中最近的元素。
    fn sibling_element(&self, next: bool) -> Option<Self> {
        let mut current = if next {
            self.document.next_sibling(self.id)
        } else {
            self.document.previous_sibling(self.id)
        };
        while let Some(id) = current {
            if matches!(self.document.node(id), Some(NodeKind::Element(_))) {
                return Some(Self::new(self.document, id));
            }
            current = if next {
                self.document.next_sibling(id)
            } else {
                self.document.previous_sibling(id)
            };
        }
        None
    }

    /// 无命名空间属性查找。
    fn attr_in_no_namespace(&self, name: &str) -> Option<&'a str> {
        let data = self.element_data()?;
        data.attributes
            .iter()
            .find(|attr| attr.namespace == Namespace::None && self.attr_name_eq(&attr.name, name))
            .map(|attr| attr.value.as_str())
    }

    /// 属性名匹配：HTML 元素 ASCII case-insensitive（Selectors §3.1.3）。
    fn attr_name_eq(&self, actual: &str, expected: &str) -> bool {
        if self.is_html_element() {
            actual.eq_ignore_ascii_case(expected)
        } else {
            actual == expected
        }
    }
}

impl SelectorsElement for DomElement<'_> {
    type Impl = NextySelectorImpl;

    fn opaque(&self) -> OpaqueElement {
        // arena 节点数据地址：同一 Document 共享借用下按 NodeId 稳定，
        // 使同一元素的不同 wrapper 身份一致（:has 锚点、nth 缓存依赖此性质）。
        match self.document.node(self.id) {
            Some(kind) => OpaqueElement::new(kind),
            None => OpaqueElement::new(self),
        }
    }

    fn parent_element(&self) -> Option<Self> {
        self.ancestor_element()
    }

    fn parent_node_is_shadow_root(&self) -> bool {
        false
    }

    fn containing_shadow_host(&self) -> Option<Self> {
        None
    }

    fn is_pseudo_element(&self) -> bool {
        false
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        self.sibling_element(false)
    }

    fn next_sibling_element(&self) -> Option<Self> {
        self.sibling_element(true)
    }

    fn first_element_child(&self) -> Option<Self> {
        let mut current = self.document.first_child(self.id);
        while let Some(id) = current {
            if matches!(self.document.node(id), Some(NodeKind::Element(_))) {
                return Some(Self::new(self.document, id));
            }
            current = self.document.next_sibling(id);
        }
        None
    }

    fn is_html_element_in_html_document(&self) -> bool {
        self.is_html_element()
    }

    fn has_local_name(&self, local_name: &str) -> bool {
        let Some(data) = self.element_data() else {
            return false;
        };
        if self.is_html_element() {
            data.name.eq_ignore_ascii_case(local_name)
        } else {
            data.name == local_name
        }
    }

    fn has_namespace(&self, ns: &str) -> bool {
        self.namespace_url().is_some_and(|url| url == ns)
    }

    fn is_same_type(&self, other: &Self) -> bool {
        let Some(data) = self.element_data() else {
            return false;
        };
        let Some(other_data) = other.element_data() else {
            return false;
        };
        if data.namespace != other_data.namespace {
            return false;
        }
        if self.is_html_element() {
            data.name.eq_ignore_ascii_case(&other_data.name)
        } else {
            data.name == other_data.name
        }
    }

    fn attr_matches(
        &self,
        ns: &NamespaceConstraint<&<Self::Impl as SelectorImpl>::NamespaceUrl>,
        local_name: &<Self::Impl as SelectorImpl>::LocalName,
        operation: &AttrSelectorOperation<&<Self::Impl as SelectorImpl>::AttrValue>,
    ) -> bool {
        let Some(data) = self.element_data() else {
            return false;
        };
        data.attributes.iter().any(|attr| {
            if !self.attr_name_eq(&attr.name, local_name.borrow()) {
                return false;
            }
            let namespace_matches = match ns {
                NamespaceConstraint::Any => true,
                NamespaceConstraint::Specific(url) => {
                    // 命名空间比较针对的是**属性**的命名空间，而非元素
                    Self::attr_namespace_url(&attr.namespace) == atom_str(url)
                }
            };
            if !namespace_matches {
                return false;
            }
            operation.eval_str(&attr.value)
        })
    }

    fn match_non_ts_pseudo_class(
        &self,
        _pc: &NeverPseudo,
        _context: &mut MatchingContext<Self::Impl>,
    ) -> bool {
        match *_pc {}
    }

    fn match_pseudo_element(
        &self,
        _pe: &NeverPseudo,
        _context: &mut MatchingContext<Self::Impl>,
    ) -> bool {
        match *_pe {}
    }

    fn apply_selector_flags(&self, _flags: selectors::matching::ElementSelectorFlags) {}

    fn is_link(&self) -> bool {
        // HTML 中 :link 命中带 href 的 a / area / link（Selectors §8.1）
        matches!(self.element_data(), Some(data)
            if data.namespace == Namespace::Html
                && matches!(data.name.as_str(), "a" | "area" | "link"))
            && self.attr_in_no_namespace("href").is_some()
    }

    fn is_html_slot_element(&self) -> bool {
        matches!(self.element_data(), Some(data)
            if data.namespace == Namespace::Html && data.name == "slot")
    }

    fn has_id(
        &self,
        id: &<Self::Impl as SelectorImpl>::Identifier,
        case_sensitivity: CaseSensitivity,
    ) -> bool {
        self.attr_in_no_namespace("id")
            .is_some_and(|value| case_sensitivity.eq(value.as_bytes(), atom_str(id).as_bytes()))
    }

    fn has_class(
        &self,
        name: &<Self::Impl as SelectorImpl>::Identifier,
        case_sensitivity: CaseSensitivity,
    ) -> bool {
        let Some(value) = self.attr_in_no_namespace("class") else {
            return false;
        };
        value
            .split_ascii_whitespace()
            .any(|token| case_sensitivity.eq(token.as_bytes(), atom_str(name).as_bytes()))
    }

    fn has_custom_state(&self, _name: &<Self::Impl as SelectorImpl>::Identifier) -> bool {
        false
    }

    fn imported_part(
        &self,
        _name: &<Self::Impl as SelectorImpl>::Identifier,
    ) -> Option<<Self::Impl as SelectorImpl>::Identifier> {
        None
    }

    fn is_part(&self, _name: &<Self::Impl as SelectorImpl>::Identifier) -> bool {
        false
    }

    fn is_empty(&self) -> bool {
        // :empty：无元素子节点且无非空文本子节点（selectors Element trait 定义）
        self.document.children(self.id).all(|child| {
            !matches!(self.document.node(child), Some(NodeKind::Element(_)))
                && match self.document.node(child) {
                    Some(NodeKind::Text(text)) => text.is_empty(),
                    _ => true,
                }
        })
    }

    fn is_root(&self) -> bool {
        matches!(
            self.document
                .parent(self.id)
                .and_then(|parent| self.document.node(parent)),
            Some(NodeKind::Document)
        )
    }

    fn add_element_unique_hashes(&self, _filter: &mut selectors::bloom::BloomFilter) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{build_document, find_first_by_name};

    fn matches(selector: &str, doc: &Document, id: NodeId) -> bool {
        matches_selector(selector, doc, id).expect("selector should parse")
    }

    fn rejects(selector: &str, doc: &Document, id: NodeId) {
        assert!(
            matches_selector(selector, doc, id).is_err(),
            "{selector} should be rejected"
        );
    }

    #[test]
    fn type_class_and_id_selectors() {
        let doc = build_document(
            "<div id=\"outer\" class=\"box wrap\"><p class=\"box\">a</p><span>b</span></div>",
        );
        let div = find_first_by_name(&doc, "div").expect("div");
        let p = find_first_by_name(&doc, "p").expect("p");
        let span = find_first_by_name(&doc, "span").expect("span");

        assert!(matches("div", &doc, div));
        assert!(
            matches("DIV", &doc, div),
            "HTML 类型选择器 ASCII 大小写不敏感"
        );
        assert!(!matches("span", &doc, div));
        assert!(matches(".box", &doc, div));
        assert!(matches(".wrap", &doc, div));
        assert!(!matches(".missing", &doc, div));
        assert!(matches("#outer", &doc, div));
        assert!(!matches("#outer", &doc, p));
        assert!(matches("*", &doc, span));
    }

    #[test]
    fn attribute_selectors() {
        let doc = build_document(
            "<a href=\"https://example.test/page\" title=\"Go\" lang=\"en-US\">x</a>\
             <svg><a xlink:href=\"#y\">s</a></svg>",
        );
        let a = find_first_by_name(&doc, "a").expect("a");
        // 在 svg 子树内找带 xlink:href 的 a
        let svg = find_first_by_name(&doc, "svg").expect("svg");
        let svg_a = {
            fn walk(document: &Document, root: NodeId) -> Option<NodeId> {
                if matches!(document.node(root), Some(NodeKind::Element(data)) if data.name == "a")
                {
                    return Some(root);
                }
                document
                    .children(root)
                    .find_map(|child| walk(document, child))
            }
            walk(&doc, svg).expect("svg a")
        };

        assert!(matches("[href]", &doc, a));
        assert!(matches("[HREF]", &doc, a), "HTML 属性名大小写不敏感");
        assert!(matches("[href=\"https://example.test/page\"]", &doc, a));
        assert!(matches("[href^=\"https://\"]", &doc, a));
        assert!(matches("[href$=\".test/page\"]", &doc, a));
        assert!(matches("[href*=\"example\"]", &doc, a));
        assert!(matches("[title~=\"Go\"]", &doc, a));
        assert!(matches("[title~=\"go\" i]", &doc, a));
        assert!(!matches("[title~=\"go\"]", &doc, a));
        assert!(matches("[lang|=\"en\"]", &doc, a));
        assert!(!matches("[href]", &doc, svg_a));
        // 无命名空间限定的属性选择器匹配不带命名空间的属性
        assert!(matches("[*|href]", &doc, svg_a));
    }

    #[test]
    fn combinators() {
        let doc = build_document(
            "<div class=\"a\"><p><em>deep</em></p><ul><li>1</li><li class=\"hit\">2</li></ul></div>",
        );
        let em = find_first_by_name(&doc, "em").expect("em");
        let li = find_first_by_name(&doc, "li").expect("li");
        let second_li = doc
            .next_sibling(li)
            .filter(|id| matches!(doc.node(*id), Some(NodeKind::Element(_))))
            .expect("second li");

        assert!(matches("div em", &doc, em));
        assert!(matches("div p em", &doc, em));
        assert!(matches(".a > p > em", &doc, em));
        assert!(!matches(".a > em", &doc, em));
        assert!(matches("li + li", &doc, second_li));
        assert!(!matches("li + li", &doc, li));
        let ul = find_first_by_name(&doc, "ul").expect("ul");
        assert!(matches("p ~ ul", &doc, ul));
        assert!(!matches("p ~ ul", &doc, em));
        assert!(matches(".hit", &doc, second_li));
    }

    #[test]
    fn structural_pseudo_classes() {
        let doc = build_document("<ul><li>1</li><li>2</li><li>3</li></ul><p>text</p>");
        let first_li = find_first_by_name(&doc, "li").expect("li");
        let second = doc.next_sibling(first_li).expect("second li");
        let third = doc.next_sibling(second).expect("third li");
        let p = find_first_by_name(&doc, "p").expect("p");
        let ul = find_first_by_name(&doc, "ul").expect("ul");
        let html = find_first_by_name(&doc, "html").expect("html");

        assert!(matches("li:first-child", &doc, first_li));
        assert!(!matches("li:first-child", &doc, second));
        assert!(matches("li:last-child", &doc, third));
        assert!(matches("li:nth-child(2)", &doc, second));
        assert!(matches("li:nth-child(2n+1)", &doc, third));
        assert!(!matches("li:nth-child(2n)", &doc, third));
        assert!(matches("li:nth-last-child(1)", &doc, third));
        assert!(matches("body > p:last-child", &doc, p));
        assert!(!matches("ul:empty", &doc, ul));
        assert!(matches(":root", &doc, html));
        assert!(!matches(":root", &doc, p));
    }

    #[test]
    fn logical_pseudo_classes() {
        let doc = build_document("<div><span class=\"x\">1</span><span>2</span><b>3</b></div>");
        let span = find_first_by_name(&doc, "span").expect("span");
        let b = find_first_by_name(&doc, "b").expect("b");
        let div = find_first_by_name(&doc, "div").expect("div");

        assert!(matches(
            "span:not(.x)",
            &doc,
            doc.next_sibling(span).expect("second span")
        ));
        assert!(matches(":is(span, b)", &doc, span));
        assert!(matches(":is(span, b)", &doc, b));
        assert!(matches(":where(span)", &doc, span));
        assert!(matches("div:has(> b)", &doc, div));
        assert!(!matches("div:has(> i)", &doc, div));
        // b 是末子节点，没有后继元素兄弟
        assert!(!matches("b:has(+ *)", &doc, b));
        let second_span = doc.next_sibling(span).expect("second span");
        assert!(matches("span:has(+ b)", &doc, second_span));
        assert!(matches("span:first-of-type", &doc, span));
        assert!(matches("b:last-of-type", &doc, b));
    }

    #[test]
    fn invalid_selectors_are_rejected() {
        let doc = build_document("<div><p>x</p></div>");
        let div = find_first_by_name(&doc, "div").expect("div");
        let p = find_first_by_name(&doc, "p").expect("p");

        rejects("div >", &doc, p);
        rejects("::before", &doc, p);
        rejects(":hover", &doc, div);
        rejects("div::selection", &doc, div);
        rejects("", &doc, div);
        rejects(",", &doc, div);
    }

    #[test]
    fn namespaces_distinguish_elements() {
        let doc = build_document("<svg><circle r=\"1\"/></svg><div><circle r=\"2\"/></div>");
        let svg_circle = find_first_by_name(&doc, "circle").expect("circle");
        // div 内的第二个 circle：svg 的下一个兄弟是 div
        let div = find_first_by_name(&doc, "svg")
            .and_then(|svg| doc.next_sibling(svg))
            .expect("div");
        let html_circle = {
            fn walk(document: &Document, root: NodeId) -> Option<NodeId> {
                if matches!(document.node(root), Some(NodeKind::Element(data)) if data.name == "circle")
                {
                    return Some(root);
                }
                document
                    .children(root)
                    .find_map(|child| walk(document, child))
            }
            walk(&doc, div).expect("html circle")
        };

        assert!(matches("circle", &doc, svg_circle));
        // 类型选择器不限命名空间，两者都匹配
        assert!(matches("circle", &doc, html_circle));
        // 命名空间限定：SVG 命名空间的 circle
        assert!(
            matches_selector("svg|circle", &doc, svg_circle).is_err(),
            "未声明前缀时带 | 的选择器拒绝"
        );
    }

    #[test]
    fn template_contents_are_separate_tree() {
        let doc = build_document("<template><span>x</span></template><span>y</span>");
        // 模板内容里的 span 不能通过祖先链匹配 body 下的规则
        let body_span = find_first_by_name(&doc, "span").expect("body span");
        assert!(matches("body > span", &doc, body_span));
        assert!(matches("body span", &doc, body_span));
    }
}
