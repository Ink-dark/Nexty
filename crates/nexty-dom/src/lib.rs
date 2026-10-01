//! Nexty 自研 arena DOM。
//!
//! 本层不依赖任何外部 crate。选型时排除了现成方案：`kuchiki` 的 Rc 不可变树
//! 与 DOM 可变性冲突，`ego-tree` / `indextree` 仍需自接 JSRT 绑定层。
//! 决策见 `docs/decisions/2026-10-01-crate-selection.md`。
//!
//! 行为 ground truth：[WHATWG DOM Standard](https://dom.spec.whatwg.org/)。
//!
//! 当前为骨架：节点存储已落地，树变更语义（插入 / 移除 / 替换算法）尚未实现，
//! 需先读规范对应章节再动手。

#![forbid(unsafe_code)]

/// arena 内节点的索引。
///
/// 在所属 [`Document`] 生命周期内保持稳定；节点被移除后索引不复用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(usize);

impl NodeId {
    /// 返回索引值。仅用于诊断与调试输出。
    #[must_use]
    pub fn index(self) -> usize {
        self.0
    }
}

/// 节点数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    /// 文档节点。
    Document,
    /// 元素节点。
    Element(ElementData),
    /// 文本节点。
    Text(String),
    /// 注释节点。
    Comment(String),
}

/// 元素节点的名称与属性。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementData {
    /// 元素名。HTML 命名空间下为小写。
    pub name: String,
    /// 属性列表，保持插入顺序。同名属性由 HTML 解析层去重。
    pub attributes: Vec<(String, String)>,
}

/// 文档树。
///
/// 节点存储在连续 arena 中，由 [`NodeId`] 引用。
#[derive(Debug, Default)]
pub struct Document {
    nodes: Vec<NodeKind>,
}

impl Document {
    /// 创建只含根文档节点的空文档。
    #[must_use]
    pub fn new() -> Self {
        Self {
            nodes: vec![NodeKind::Document],
        }
    }

    /// 在 arena 中追加一个游离节点，返回其索引。
    ///
    /// 追加的节点尚未接入树；树关系由后续的变更算法建立。
    pub fn create_node(&mut self, kind: NodeKind) -> NodeId {
        self.nodes.push(kind);
        NodeId(self.nodes.len() - 1)
    }

    /// 按索引读取节点。
    #[must_use]
    pub fn node(&self, id: NodeId) -> Option<&NodeKind> {
        self.nodes.get(id.0)
    }

    /// 按索引可变读取节点。
    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut NodeKind> {
        self.nodes.get_mut(id.0)
    }

    /// arena 中的节点总数。
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// 根文档节点索引。
    #[must_use]
    pub fn root(&self) -> NodeId {
        NodeId(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_document_contains_root_only() {
        let doc = Document::new();
        assert_eq!(doc.node_count(), 1);
        assert_eq!(doc.node(doc.root()), Some(&NodeKind::Document));
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
    }

    #[test]
    fn element_data_keeps_attribute_order() {
        let mut doc = Document::new();
        let id = doc.create_node(NodeKind::Element(ElementData {
            name: "a".into(),
            attributes: vec![("href".into(), "/x".into()), ("id".into(), "y".into())],
        }));
        let Some(NodeKind::Element(data)) = doc.node(id) else {
            panic!("expected element node");
        };
        assert_eq!(data.attributes[0].0, "href");
        assert_eq!(data.attributes[1].0, "id");
    }
}
