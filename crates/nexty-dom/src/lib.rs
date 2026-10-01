//! Nexty 自研 arena DOM。
//!
//! 本层不依赖任何外部 crate。选型时排除了现成方案：`kuchiki` 式的 Rc 不可变树
//! 与 DOM 可变性冲突，`ego-tree` / `indextree` 仍需自接 JSRT 绑定层；性能评估
//! 显示 arena 在构建、遍历、随机访问、兄弟遍历与内存分配上全面领先。
//! 决策见 `docs/decisions/2026-10-01-crate-selection.md`。
//!
//! 行为 ground truth：[WHATWG DOM Standard](https://dom.spec.whatwg.org/)。
//! 节点类型见 `node` 模块，树变更算法见 `tree` 模块。
//!
//! 已落地：节点类型（含 doctype / fragment / 处理指令）与元素命名空间、
//! 树变更算法（insert / remove / replace / clone / normalize）。JS 绑定层尚未接入。

#![forbid(unsafe_code)]

mod node;
mod tree;

pub use node::{
    Attribute, DocumentTypeData, DomError, ElementData, Namespace, NodeId, NodeKind,
    ProcessingInstructionData, QuirksMode,
};
pub use tree::{Children, Document};

#[cfg(test)]
mod tests {
    use super::*;

    fn element(doc: &mut Document, name: &str) -> NodeId {
        doc.create_node(NodeKind::Element(ElementData {
            name: name.into(),
            namespace: Namespace::Html,
            attributes: Vec::new(),
        }))
    }

    fn text(doc: &mut Document, data: &str) -> NodeId {
        doc.create_node(NodeKind::Text(data.into()))
    }

    fn children_of(doc: &Document, id: NodeId) -> Vec<NodeId> {
        doc.children(id).collect()
    }

    #[test]
    fn new_document_contains_root_only() {
        let doc = Document::new();
        assert_eq!(doc.node_count(), 1);
        assert_eq!(doc.node(doc.root()), Some(&NodeKind::Document));
        assert_eq!(doc.quirks_mode(), QuirksMode::NoQuirks);
    }

    #[test]
    fn created_node_is_readable_and_distinct_from_root() {
        let mut doc = Document::new();
        let id = doc.create_node(NodeKind::Text("hi".into()));
        assert_ne!(id, doc.root());
        assert_eq!(doc.node(id), Some(&NodeKind::Text("hi".into())));
        assert_eq!(doc.node_count(), 2);
    }

    #[test]
    fn node_mut_replaces_kind() {
        let mut doc = Document::new();
        let id = doc.create_node(NodeKind::Comment("old".into()));
        if let Some(kind) = doc.node_mut(id) {
            *kind = NodeKind::Comment("new".into());
        }
        assert_eq!(doc.node(id), Some(&NodeKind::Comment("new".into())));
    }

    #[test]
    fn unknown_index_returns_none() {
        let doc = Document::new();
        assert_eq!(doc.node(NodeId(99)), None);
        assert_eq!(doc.parent(NodeId(99)), None);
    }

    #[test]
    fn element_data_keeps_attribute_order_and_namespace() {
        let mut doc = Document::new();
        let id = doc.create_node(NodeKind::Element(ElementData {
            name: "a".into(),
            namespace: Namespace::Svg,
            attributes: vec![
                Attribute {
                    name: "href".into(),
                    namespace: Namespace::None,
                    value: "/x".into(),
                },
                Attribute {
                    name: "href".into(),
                    namespace: Namespace::Other("http://www.w3.org/1999/xlink".into()),
                    value: "/y".into(),
                },
            ],
        }));
        let Some(NodeKind::Element(data)) = doc.node(id) else {
            panic!("expected element node");
        };
        assert_eq!(data.namespace, Namespace::Svg);
        assert_eq!(data.attributes[0].namespace, Namespace::None);
        assert_eq!(
            data.attributes[1].namespace,
            Namespace::Other("http://www.w3.org/1999/xlink".into())
        );
    }

    #[test]
    fn append_child_links_parent_and_siblings() {
        let mut doc = Document::new();
        let parent = element(&mut doc, "div");
        let a = text(&mut doc, "a");
        let b = text(&mut doc, "b");
        let c = text(&mut doc, "c");

        doc.append_child(a, parent).unwrap();
        doc.append_child(b, parent).unwrap();
        doc.append_child(c, parent).unwrap();

        assert_eq!(doc.parent(a), Some(parent));
        assert_eq!(doc.first_child(parent), Some(a));
        assert_eq!(doc.last_child(parent), Some(c));
        assert_eq!(children_of(&doc, parent), vec![a, b, c]);
        assert_eq!(doc.next_sibling(a), Some(b));
        assert_eq!(doc.previous_sibling(b), Some(a));
        assert_eq!(doc.next_sibling(c), None);
    }

    #[test]
    fn insert_before_places_node_at_reference() {
        let mut doc = Document::new();
        let parent = element(&mut doc, "ul");
        let a = element(&mut doc, "li");
        let b = element(&mut doc, "li");
        let mid = element(&mut doc, "li");

        doc.append_child(a, parent).unwrap();
        doc.append_child(b, parent).unwrap();
        doc.insert_before(mid, parent, Some(b)).unwrap();

        assert_eq!(children_of(&doc, parent), vec![a, mid, b]);
    }

    #[test]
    fn insert_before_self_is_noop() {
        let mut doc = Document::new();
        let parent = element(&mut doc, "div");
        let a = element(&mut doc, "span");
        let b = element(&mut doc, "span");
        doc.append_child(a, parent).unwrap();
        doc.append_child(b, parent).unwrap();

        doc.insert_before(a, parent, Some(a)).unwrap();
        assert_eq!(children_of(&doc, parent), vec![a, b]);
    }

    #[test]
    fn moving_node_detaches_from_old_parent() {
        let mut doc = Document::new();
        let first = element(&mut doc, "div");
        let second = element(&mut doc, "div");
        let moved = element(&mut doc, "p");

        doc.append_child(moved, first).unwrap();
        doc.append_child(moved, second).unwrap();

        assert_eq!(children_of(&doc, first), Vec::<NodeId>::new());
        assert_eq!(children_of(&doc, second), vec![moved]);
        assert_eq!(doc.parent(moved), Some(second));
    }

    #[test]
    fn fragment_insert_expands_children_in_order() {
        let mut doc = Document::new();
        let parent = element(&mut doc, "div");
        let fragment = doc.create_node(NodeKind::DocumentFragment);
        let a = text(&mut doc, "a");
        let b = text(&mut doc, "b");
        doc.append_child(a, fragment).unwrap();
        doc.append_child(b, fragment).unwrap();

        doc.append_child(fragment, parent).unwrap();

        assert_eq!(children_of(&doc, fragment), Vec::<NodeId>::new());
        assert_eq!(children_of(&doc, parent), vec![a, b]);
        assert_eq!(doc.parent(a), Some(parent));
    }

    #[test]
    fn empty_fragment_insert_is_noop() {
        let mut doc = Document::new();
        let parent = element(&mut doc, "div");
        let fragment = doc.create_node(NodeKind::DocumentFragment);
        doc.append_child(fragment, parent).unwrap();
        assert_eq!(children_of(&doc, parent), Vec::<NodeId>::new());
    }

    #[test]
    fn remove_child_clears_links_and_reports_missing_parent() {
        let mut doc = Document::new();
        let parent = element(&mut doc, "div");
        let a = element(&mut doc, "span");
        let b = element(&mut doc, "span");
        doc.append_child(a, parent).unwrap();
        doc.append_child(b, parent).unwrap();

        doc.remove_child(a, parent).unwrap();
        assert_eq!(doc.parent(a), None);
        assert_eq!(doc.next_sibling(a), None);
        assert_eq!(children_of(&doc, parent), vec![b]);
        assert_eq!(doc.first_child(parent), Some(b));
        assert_eq!(doc.previous_sibling(b), None);
        assert_eq!(doc.remove_child(a, parent), Err(DomError::NotFound));
    }

    #[test]
    fn replace_child_swaps_in_place() {
        let mut doc = Document::new();
        let parent = element(&mut doc, "div");
        let a = element(&mut doc, "span");
        let b = element(&mut doc, "span");
        let replacement = element(&mut doc, "em");
        doc.append_child(a, parent).unwrap();
        doc.append_child(b, parent).unwrap();

        doc.replace_child(a, replacement, parent).unwrap();
        assert_eq!(children_of(&doc, parent), vec![replacement, b]);
        assert_eq!(doc.parent(a), None);
    }

    #[test]
    fn replace_all_clears_then_inserts() {
        let mut doc = Document::new();
        let parent = element(&mut doc, "div");
        let a = element(&mut doc, "span");
        let b = element(&mut doc, "span");
        let fresh = text(&mut doc, "fresh");
        doc.append_child(a, parent).unwrap();
        doc.append_child(b, parent).unwrap();

        doc.replace_all(Some(fresh), parent);
        assert_eq!(children_of(&doc, parent), vec![fresh]);
        assert_eq!(doc.parent(a), None);

        doc.replace_all(None, parent);
        assert_eq!(children_of(&doc, parent), Vec::<NodeId>::new());
    }

    #[test]
    fn insert_into_own_subtree_is_rejected() {
        let mut doc = Document::new();
        let parent = element(&mut doc, "div");
        let child = element(&mut doc, "span");
        doc.append_child(child, parent).unwrap();

        assert_eq!(
            doc.append_child(parent, child),
            Err(DomError::HierarchyRequest)
        );
    }

    #[test]
    fn insert_with_foreign_reference_is_not_found() {
        let mut doc = Document::new();
        let parent = element(&mut doc, "div");
        let other = element(&mut doc, "div");
        let loose = element(&mut doc, "span");

        assert_eq!(
            doc.insert_before(loose, parent, Some(other)),
            Err(DomError::NotFound)
        );
    }

    #[test]
    fn document_rejects_second_element_and_text_children() {
        let mut doc = Document::new();
        let root = doc.root();
        let html = element(&mut doc, "html");
        doc.append_child(html, root).unwrap();

        let second = element(&mut doc, "html");
        assert_eq!(
            doc.append_child(second, root),
            Err(DomError::HierarchyRequest)
        );

        let text = text(&mut doc, "nope");
        assert_eq!(
            doc.append_child(text, root),
            Err(DomError::HierarchyRequest)
        );
    }

    #[test]
    fn document_rejects_doctype_after_element() {
        let mut doc = Document::new();
        let root = doc.root();
        let html = element(&mut doc, "html");
        doc.append_child(html, root).unwrap();

        let doctype = doc.create_node(NodeKind::Doctype(DocumentTypeData {
            name: "html".into(),
            public_id: String::new(),
            system_id: String::new(),
        }));
        assert_eq!(
            doc.append_child(doctype, root),
            Err(DomError::HierarchyRequest)
        );
    }

    #[test]
    fn doctype_before_element_is_allowed_once() {
        let mut doc = Document::new();
        let root = doc.root();
        let doctype = doc.create_node(NodeKind::Doctype(DocumentTypeData {
            name: "html".into(),
            public_id: String::new(),
            system_id: String::new(),
        }));
        doc.append_child(doctype, root).unwrap();

        let html = element(&mut doc, "html");
        doc.append_child(html, root).unwrap();
        assert_eq!(children_of(&doc, root), vec![doctype, html]);

        let duplicate = doc.create_node(NodeKind::Doctype(DocumentTypeData::default()));
        assert_eq!(
            doc.append_child(duplicate, root),
            Err(DomError::HierarchyRequest)
        );
    }

    #[test]
    fn doctype_rejected_outside_document() {
        let mut doc = Document::new();
        let parent = element(&mut doc, "div");
        let doctype = doc.create_node(NodeKind::Doctype(DocumentTypeData::default()));
        assert_eq!(
            doc.append_child(doctype, parent),
            Err(DomError::HierarchyRequest)
        );
    }

    #[test]
    fn clone_shallow_copies_data_but_not_children() {
        let mut doc = Document::new();
        let source = doc.create_node(NodeKind::Element(ElementData {
            name: "a".into(),
            namespace: Namespace::Svg,
            attributes: vec![Attribute {
                name: "href".into(),
                namespace: Namespace::None,
                value: "/x".into(),
            }],
        }));
        let child = text(&mut doc, "text");
        doc.append_child(child, source).unwrap();

        let copy = doc.clone_node(source, false);
        assert_ne!(copy, source);
        assert_eq!(doc.node(copy), doc.node(source));
        assert_eq!(children_of(&doc, copy), Vec::<NodeId>::new());
        assert_eq!(doc.parent(copy), None);
    }

    #[test]
    fn clone_subtree_copies_descendants() {
        let mut doc = Document::new();
        let source = element(&mut doc, "ul");
        let item = element(&mut doc, "li");
        let label = text(&mut doc, "one");
        doc.append_child(item, source).unwrap();
        doc.append_child(label, item).unwrap();

        let copy = doc.clone_node(source, true);
        let copied_item = doc.first_child(copy).unwrap();
        let copied_label = doc.first_child(copied_item).unwrap();
        assert_eq!(doc.node(copied_item), doc.node(item));
        assert_eq!(doc.node(copied_label), Some(&NodeKind::Text("one".into())));
        assert_ne!(copied_item, item);
    }

    #[test]
    fn clone_template_gets_fresh_contents() {
        let mut doc = Document::new();
        let template = element(&mut doc, "template");
        let contents = doc.create_node(NodeKind::DocumentFragment);
        let inner = element(&mut doc, "span");
        doc.append_child(inner, contents).unwrap();
        doc.set_template_contents(template, contents);

        let shallow = doc.clone_node(template, false);
        let shallow_contents = doc.template_contents(shallow).unwrap();
        assert_ne!(shallow_contents, contents);
        assert_eq!(children_of(&doc, shallow_contents), Vec::<NodeId>::new());

        let deep = doc.clone_node(template, true);
        let deep_contents = doc.template_contents(deep).unwrap();
        assert_eq!(children_of(&doc, deep_contents).len(), 1);
    }

    #[test]
    fn normalize_merges_adjacent_text_and_drops_empty() {
        let mut doc = Document::new();
        let parent = element(&mut doc, "p");
        let a = text(&mut doc, "a");
        let empty = text(&mut doc, "");
        let b = text(&mut doc, "b");
        let c = text(&mut doc, "c");
        let br = element(&mut doc, "br");
        let d = text(&mut doc, "d");

        for node in [a, empty, b, c, br, d] {
            doc.append_child(node, parent).unwrap();
        }

        doc.normalize(parent);

        assert_eq!(doc.node(a), Some(&NodeKind::Text("abc".into())));
        assert_eq!(children_of(&doc, parent), vec![a, br, d]);
        assert_eq!(doc.parent(empty), None);
        assert_eq!(doc.parent(c), None);
    }

    #[test]
    fn normalize_descends_into_subtrees() {
        let mut doc = Document::new();
        let outer = element(&mut doc, "div");
        let inner = element(&mut doc, "p");
        doc.append_child(inner, outer).unwrap();
        let a = text(&mut doc, "x");
        let b = text(&mut doc, "y");
        doc.append_child(a, inner).unwrap();
        doc.append_child(b, inner).unwrap();

        doc.normalize(outer);
        assert_eq!(doc.node(a), Some(&NodeKind::Text("xy".into())));
        assert_eq!(children_of(&doc, inner), vec![a]);
    }

    #[test]
    fn quirks_mode_round_trips() {
        let mut doc = Document::new();
        doc.set_quirks_mode(QuirksMode::Quirks);
        assert_eq!(doc.quirks_mode(), QuirksMode::Quirks);
    }
}
