//! arena 文档树与树变更算法。
//!
//! 行为 ground truth：WHATWG DOM Standard 的
//! [mutation algorithms](https://dom.spec.whatwg.org/#mutation-algorithms)。
//! 方法命名与规范一一对应：`insert_before` = *pre-insert*、`insert_node` = *insert*、
//! `append_child` = *append*、`remove_child` = *pre-remove*、`replace_child` =
//! *replace*、`replace_all` = *replace all*、`clone_node` = *clone a node*、
//! `normalize` = *normalize*。
//!
//! 带校验的方法（`insert_before` / `append_child` / `remove_child` /
//! `replace_child`）实现规范里的前置校验并返回 [`DomError`]；不带校验的
//! `insert_node` / `remove_node` 是解析器的写入路径——HTML 树构建器保证其操作
//! 合法，重复校验只会拖慢解析。

use crate::node::{DomError, NodeId, NodeKind, QuirksMode};

/// arena 中的一个节点：节点数据 + 树结构指针。
#[derive(Debug)]
struct Node {
    kind: NodeKind,
    parent: Option<NodeId>,
    first_child: Option<NodeId>,
    last_child: Option<NodeId>,
    previous_sibling: Option<NodeId>,
    next_sibling: Option<NodeId>,
    /// `<template>` 元素的[模板内容](https://html.spec.whatwg.org/multipage/#template-contents)。
    template_contents: Option<NodeId>,
}

impl Node {
    fn detached(kind: NodeKind) -> Self {
        Self {
            kind,
            parent: None,
            first_child: None,
            last_child: None,
            previous_sibling: None,
            next_sibling: None,
            template_contents: None,
        }
    }
}

/// 文档树。
///
/// 节点存储在连续 arena 中，由 [`NodeId`] 引用；树关系用父指针 + 首末子指针 +
/// 双向兄弟链维护，插入与移除均为 O(1)。
#[derive(Debug)]
pub struct Document {
    nodes: Vec<Node>,
    quirks_mode: QuirksMode,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    /// 创建只含根文档节点的空文档。
    #[must_use]
    pub fn new() -> Self {
        Self {
            nodes: vec![Node::detached(NodeKind::Document)],
            quirks_mode: QuirksMode::NoQuirks,
        }
    }

    /// 根文档节点索引。
    #[must_use]
    pub fn root(&self) -> NodeId {
        NodeId(0)
    }

    /// 在 arena 中追加一个游离节点，返回其索引。
    ///
    /// 追加的节点尚未接入树；树关系由后续的变更算法建立。
    pub fn create_node(&mut self, kind: NodeKind) -> NodeId {
        self.nodes.push(Node::detached(kind));
        NodeId(self.nodes.len() - 1)
    }

    /// 按索引读取节点数据。
    #[must_use]
    pub fn node(&self, id: NodeId) -> Option<&NodeKind> {
        self.nodes.get(id.0).map(|node| &node.kind)
    }

    /// 按索引可变读取节点数据。
    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut NodeKind> {
        self.nodes.get_mut(id.0).map(|node| &mut node.kind)
    }

    /// arena 中的节点总数。
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// 文档的 quirks 模式。
    #[must_use]
    pub fn quirks_mode(&self) -> QuirksMode {
        self.quirks_mode
    }

    /// 设置文档的 quirks 模式。
    pub fn set_quirks_mode(&mut self, mode: QuirksMode) {
        self.quirks_mode = mode;
    }

    /// `<template>` 元素的模板内容片段索引。
    #[must_use]
    pub fn template_contents(&self, id: NodeId) -> Option<NodeId> {
        self.nodes.get(id.0).and_then(|node| node.template_contents)
    }

    /// 设置 `<template>` 元素的模板内容片段。
    pub fn set_template_contents(&mut self, id: NodeId, contents: NodeId) {
        if let Some(node) = self.nodes.get_mut(id.0) {
            node.template_contents = Some(contents);
        }
    }

    /// 父节点。
    #[must_use]
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.nodes.get(id.0).and_then(|node| node.parent)
    }

    /// 首个子节点。
    #[must_use]
    pub fn first_child(&self, id: NodeId) -> Option<NodeId> {
        self.nodes.get(id.0).and_then(|node| node.first_child)
    }

    /// 末个子节点。
    #[must_use]
    pub fn last_child(&self, id: NodeId) -> Option<NodeId> {
        self.nodes.get(id.0).and_then(|node| node.last_child)
    }

    /// 后一个兄弟节点。
    #[must_use]
    pub fn next_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.nodes.get(id.0).and_then(|node| node.next_sibling)
    }

    /// 前一个兄弟节点。
    #[must_use]
    pub fn previous_sibling(&self, id: NodeId) -> Option<NodeId> {
        self.nodes.get(id.0).and_then(|node| node.previous_sibling)
    }

    /// 按树序迭代子节点。
    #[must_use]
    pub fn children(&self, id: NodeId) -> Children<'_> {
        Children {
            doc: self,
            next: self.first_child(id),
        }
    }

    /// [insert](https://dom.spec.whatwg.org/#concept-node-insert)：把 `node` 插入
    /// `parent` 中 `child` 之前（`child` 为 `None` 时追加到末尾），不做前置校验。
    ///
    /// `node` 为 `DocumentFragment` 时展开其子节点。返回 `node` 本身。
    pub fn insert_node(&mut self, node: NodeId, parent: NodeId, child: Option<NodeId>) -> NodeId {
        let nodes: Vec<NodeId> = if matches!(self.node(node), Some(NodeKind::DocumentFragment)) {
            self.children(node).collect()
        } else {
            vec![node]
        };
        if nodes.is_empty() {
            return node;
        }
        for &n in &nodes {
            self.detach(n);
            self.link_before(n, parent, child);
        }
        node
    }

    /// [remove](https://dom.spec.whatwg.org/#concept-node-remove)：把 `node` 从其
    /// 父节点摘下，不做前置校验。游离节点调用无副作用。
    pub fn remove_node(&mut self, node: NodeId) {
        self.detach(node);
    }

    /// [pre-insert](https://dom.spec.whatwg.org/#concept-node-pre-insert)：校验后把
    /// `node` 插入 `parent` 中 `child` 之前（`child` 为 `None` 时追加到末尾）。
    ///
    /// # Errors
    ///
    /// 校验不通过时返回 [`DomError::HierarchyRequest`] 或 [`DomError::NotFound`]。
    pub fn insert_before(
        &mut self,
        node: NodeId,
        parent: NodeId,
        child: Option<NodeId>,
    ) -> Result<NodeId, DomError> {
        self.ensure_pre_insert_validity(node, parent, child, &[])?;
        let reference = if child == Some(node) {
            self.next_sibling(node)
        } else {
            child
        };
        Ok(self.insert_node(node, parent, reference))
    }

    /// [append](https://dom.spec.whatwg.org/#concept-node-append)：等价于
    /// `insert_before(node, parent, None)`。
    ///
    /// # Errors
    ///
    /// 校验不通过时返回 [`DomError::HierarchyRequest`] 或 [`DomError::NotFound`]。
    pub fn append_child(&mut self, node: NodeId, parent: NodeId) -> Result<NodeId, DomError> {
        self.insert_before(node, parent, None)
    }

    /// [pre-remove](https://dom.spec.whatwg.org/#concept-node-pre-remove)：校验后把
    /// `child` 从 `parent` 摘下。
    ///
    /// # Errors
    ///
    /// `child` 的父节点不是 `parent` 时返回 [`DomError::NotFound`]。
    pub fn remove_child(&mut self, child: NodeId, parent: NodeId) -> Result<NodeId, DomError> {
        if self.parent(child) != Some(parent) {
            return Err(DomError::NotFound);
        }
        self.detach(child);
        Ok(child)
    }

    /// [replace](https://dom.spec.whatwg.org/#concept-node-replace)：用 `node` 替换
    /// `parent` 下的 `child`，返回被替换的 `child`。
    ///
    /// # Errors
    ///
    /// 校验不通过时返回 [`DomError::HierarchyRequest`] 或 [`DomError::NotFound`]。
    pub fn replace_child(
        &mut self,
        child: NodeId,
        node: NodeId,
        parent: NodeId,
    ) -> Result<NodeId, DomError> {
        self.ensure_pre_insert_validity(node, parent, Some(child), &[child])?;
        let mut reference = self.next_sibling(child);
        if reference == Some(node) {
            reference = self.next_sibling(node);
        }
        if self.parent(child).is_some() {
            self.detach(child);
        }
        self.insert_node(node, parent, reference);
        Ok(child)
    }

    /// [replace all](https://dom.spec.whatwg.org/#concept-node-replace-all)：清空
    /// `parent` 的全部子节点，再插入 `node`（为 `None` 时只清空）。
    pub fn replace_all(&mut self, node: Option<NodeId>, parent: NodeId) {
        let removed: Vec<NodeId> = self.children(parent).collect();
        for child in removed {
            self.detach(child);
        }
        if let Some(node) = node {
            self.insert_node(node, parent, None);
        }
    }

    /// [clone a node](https://dom.spec.whatwg.org/#concept-node-clone)：复制 `node`
    /// 到本文档，`subtree` 为真时连同后代一起复制。返回复制出的游离节点。
    ///
    /// 元素的模板内容按 HTML 的 cloning steps 处理：复制时总是新建一个模板内容片段，
    /// `subtree` 为真时连片段内容一并复制。
    pub fn clone_node(&mut self, node: NodeId, subtree: bool) -> NodeId {
        let Some(kind) = self.node(node).cloned() else {
            return node;
        };
        let copy = self.create_node(kind);

        if let Some(contents) = self.template_contents(node) {
            let new_contents = self.create_node(NodeKind::DocumentFragment);
            if subtree {
                for child in self.children(contents).collect::<Vec<_>>() {
                    let child_copy = self.clone_node(child, true);
                    self.link_before(child_copy, new_contents, None);
                }
            }
            self.set_template_contents(copy, new_contents);
        }

        if subtree {
            for child in self.children(node).collect::<Vec<_>>() {
                let child_copy = self.clone_node(child, true);
                self.link_before(child_copy, copy, None);
            }
        }
        copy
    }

    /// [normalize](https://dom.spec.whatwg.org/#dom-node-normalize)：移除 `node` 后代
    /// 中的空文本节点，并把相邻文本节点合并到前一个。
    pub fn normalize(&mut self, node: NodeId) {
        let mut texts = Vec::new();
        self.collect_text_descendants(node, &mut texts);

        for text in texts {
            // 已被前一轮合并摘除的节点跳过。
            if self.parent(text).is_none() {
                continue;
            }
            let empty = matches!(self.node(text), Some(NodeKind::Text(data)) if data.is_empty());
            if empty {
                self.detach(text);
                continue;
            }

            let mut merged = String::new();
            let mut following = Vec::new();
            let mut current = self.next_sibling(text);
            while let Some(id) = current {
                match self.node(id) {
                    Some(NodeKind::Text(data)) => {
                        merged.push_str(data);
                        following.push(id);
                    }
                    _ => break,
                }
                current = self.next_sibling(id);
            }

            if let Some(NodeKind::Text(data)) = self.node_mut(text) {
                data.push_str(&merged);
            }
            for id in following {
                self.detach(id);
            }
        }
    }

    /// 把 `node` 从当前父节点摘下，并清空其兄弟链。
    fn detach(&mut self, node: NodeId) {
        let Some(parent) = self.parent(node) else {
            return;
        };
        let previous = self.previous_sibling(node);
        let next = self.next_sibling(node);

        match previous {
            Some(id) => self.nodes[id.0].next_sibling = next,
            None => self.nodes[parent.0].first_child = next,
        }
        match next {
            Some(id) => self.nodes[id.0].previous_sibling = previous,
            None => self.nodes[parent.0].last_child = previous,
        }

        let entry = &mut self.nodes[node.0];
        entry.parent = None;
        entry.previous_sibling = None;
        entry.next_sibling = None;
    }

    /// 把游离节点 `node` 挂到 `parent` 中 `child` 之前（`child` 为 `None` 时挂到末尾）。
    fn link_before(&mut self, node: NodeId, parent: NodeId, child: Option<NodeId>) {
        let (previous, next) = match child {
            Some(id) => (self.previous_sibling(id), Some(id)),
            None => (self.last_child(parent), None),
        };

        {
            let entry = &mut self.nodes[node.0];
            entry.parent = Some(parent);
            entry.previous_sibling = previous;
            entry.next_sibling = next;
        }

        match previous {
            Some(id) => self.nodes[id.0].next_sibling = Some(node),
            None => self.nodes[parent.0].first_child = Some(node),
        }
        match next {
            Some(id) => self.nodes[id.0].previous_sibling = Some(node),
            None => self.nodes[parent.0].last_child = Some(node),
        }
    }

    /// [ensure pre-insert validity](https://dom.spec.whatwg.org/#concept-node-ensure-pre-insertion-validity)。
    fn ensure_pre_insert_validity(
        &self,
        node: NodeId,
        parent: NodeId,
        child: Option<NodeId>,
        children_to_exclude: &[NodeId],
    ) -> Result<(), DomError> {
        // 1. 父节点必须是 Document / DocumentFragment / Element。
        match self.node(parent) {
            Some(NodeKind::Document | NodeKind::DocumentFragment | NodeKind::Element(_)) => {}
            _ => return Err(DomError::HierarchyRequest),
        }

        // 2. 不能把节点插进自己的子树。
        if self.is_inclusive_ancestor(node, parent) {
            return Err(DomError::HierarchyRequest);
        }

        // 3. 参考节点必须已经是父节点的子节点。
        if let Some(child) = child
            && self.parent(child) != Some(parent)
        {
            return Err(DomError::NotFound);
        }

        // 4. 只允许插入 DocumentFragment / DocumentType / Element / CharacterData。
        match self.node(node) {
            Some(
                NodeKind::DocumentFragment
                | NodeKind::Doctype(_)
                | NodeKind::Element(_)
                | NodeKind::Text(_)
                | NodeKind::Comment(_)
                | NodeKind::ProcessingInstruction(_),
            ) => {}
            _ => return Err(DomError::HierarchyRequest),
        }

        // 5. 父节点不是文档时，只额外禁止插入 doctype。
        if !matches!(self.node(parent), Some(NodeKind::Document)) {
            if matches!(self.node(node), Some(NodeKind::Doctype(_))) {
                return Err(DomError::HierarchyRequest);
            }
            return Ok(());
        }

        // 6. 文档下不允许插入文本节点。
        if matches!(self.node(node), Some(NodeKind::Text(_))) {
            return Err(DomError::HierarchyRequest);
        }

        // 7. 其余 CharacterData（注释 / 处理指令）直接放行。
        if matches!(
            self.node(node),
            Some(NodeKind::Comment(_) | NodeKind::ProcessingInstruction(_))
        ) {
            return Ok(());
        }

        // 8. DocumentFragment：最多带一个元素子节点，且不能带文本子节点。
        if matches!(self.node(node), Some(NodeKind::DocumentFragment)) {
            let mut element_children = 0usize;
            for child in self.children(node) {
                match self.node(child) {
                    Some(NodeKind::Element(_)) => element_children += 1,
                    Some(NodeKind::Text(_)) => return Err(DomError::HierarchyRequest),
                    _ => {}
                }
            }
            if element_children > 1 {
                return Err(DomError::HierarchyRequest);
            }
            if element_children == 0 {
                return Ok(());
            }
        }

        // 9. DocumentFragment / Element：文档下只能有一个元素子节点，且须在 doctype 之后。
        if matches!(
            self.node(node),
            Some(NodeKind::DocumentFragment | NodeKind::Element(_))
        ) {
            let conflict = self.has_child_where(parent, children_to_exclude, |kind| {
                matches!(kind, NodeKind::Element(_))
            }) || child.is_some_and(|child| self.has_following_doctype(child))
                || child.is_some_and(|child| {
                    matches!(self.node(child), Some(NodeKind::Doctype(_)))
                        && !children_to_exclude.contains(&child)
                });
            return if conflict {
                Err(DomError::HierarchyRequest)
            } else {
                Ok(())
            };
        }

        // 10. 此处 node 必为 doctype。
        // 11. 文档下只能有一个 doctype，且须在所有元素之前。
        let conflict = self.has_child_where(parent, children_to_exclude, |kind| {
            matches!(kind, NodeKind::Doctype(_))
        }) || child.is_some_and(|child| self.has_preceding_element(child))
            || (child.is_none()
                && self.has_child_where(parent, children_to_exclude, |kind| {
                    matches!(kind, NodeKind::Element(_))
                }));
        if conflict {
            return Err(DomError::HierarchyRequest);
        }
        Ok(())
    }

    /// `node` 是否为 `descendant` 的祖先（含自身）。
    fn is_inclusive_ancestor(&self, node: NodeId, descendant: NodeId) -> bool {
        let mut current = Some(descendant);
        while let Some(id) = current {
            if id == node {
                return true;
            }
            current = self.parent(id);
        }
        false
    }

    /// `parent` 的子节点中是否存在满足 `predicate` 且不在 `exclude` 里的节点。
    fn has_child_where(
        &self,
        parent: NodeId,
        exclude: &[NodeId],
        predicate: impl Fn(&NodeKind) -> bool,
    ) -> bool {
        self.children(parent)
            .filter(|child| !exclude.contains(child))
            .any(|child| self.node(child).is_some_and(&predicate))
    }

    /// `child` 之后是否存在 doctype 兄弟节点。
    fn has_following_doctype(&self, child: NodeId) -> bool {
        let mut current = self.next_sibling(child);
        while let Some(id) = current {
            if matches!(self.node(id), Some(NodeKind::Doctype(_))) {
                return true;
            }
            current = self.next_sibling(id);
        }
        false
    }

    /// `child` 之前是否存在元素兄弟节点。
    fn has_preceding_element(&self, child: NodeId) -> bool {
        let Some(parent) = self.parent(child) else {
            return false;
        };
        let mut current = self.first_child(parent);
        while let Some(id) = current {
            if id == child {
                return false;
            }
            if matches!(self.node(id), Some(NodeKind::Element(_))) {
                return true;
            }
            current = self.next_sibling(id);
        }
        false
    }

    /// 按树序收集 `root` 的全部文本后代。
    fn collect_text_descendants(&self, root: NodeId, out: &mut Vec<NodeId>) {
        let mut current = self.first_child(root);
        while let Some(id) = current {
            if matches!(self.node(id), Some(NodeKind::Text(_))) {
                out.push(id);
            }
            current = match self.first_child(id) {
                Some(first) => Some(first),
                None => self.next_in_tree_order(id, root),
            };
        }
    }

    /// 树序下 `node` 的后继，`root` 的后代耗尽时返回 `None`。
    fn next_in_tree_order(&self, node: NodeId, root: NodeId) -> Option<NodeId> {
        let mut current = node;
        loop {
            if current == root {
                return None;
            }
            if let Some(sibling) = self.next_sibling(current) {
                return Some(sibling);
            }
            current = self.parent(current)?;
        }
    }
}

/// [`Document::children`] 返回的子节点迭代器，按树序产出 [`NodeId`]。
pub struct Children<'a> {
    doc: &'a Document,
    next: Option<NodeId>,
}

impl Iterator for Children<'_> {
    type Item = NodeId;

    fn next(&mut self) -> Option<NodeId> {
        let id = self.next?;
        self.next = self.doc.next_sibling(id);
        Some(id)
    }
}
