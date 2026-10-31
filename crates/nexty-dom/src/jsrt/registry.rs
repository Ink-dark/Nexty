//! 绑定注册表：JSRT v0 的 DOM 操作面（引擎无关）。
//!
//! [`DomRegistry`] 以 [`NodeId`](crate::NodeId) 数值与回调句柄为操作对象，由
//! 引擎适配 crate 在绑定层调用；所有树变更都走 `tree` 模块的算法入口
//! （insert / remove / replace_all），不绕过深度维护。行为 ground truth 为
//! WHATWG DOM 对应算法的简化档。
//!
//! querySelector 与 getComputedStyle 经注入钩子（[`QuerySelectorHook`] /
//! [`GetComputedStyleHook`]）：由 chrome 在装配期注入 css 选择器匹配与级联
//! 只读快照，本 crate 不反向依赖 css（层隔离）；测试可注入桩实现。
//!
//! ## 偏差清单（v0）
//!
//! - `create_element` / `set_attribute` / classList 写回的名字统一 ASCII
//!   小写（HTML 文档语义）；SVG / MathML 的大小写敏感名 v0 不支持。
//! - `get_attribute` 按 ASCII 大小写不敏感匹配属性名（HTML 简化档）；
//!   只匹配无命名空间属性（解析器对 HTML 属性即此形态）。
//! - `get_element_by_id` 线性先序遍历（无 id 索引），id 精确匹配，返回文档序
//!   第一个命中。
//! - `text_content`：Document / Doctype 返回 `None`（规范为 null）；
//!   `set_text_content` 不接受 Document / Doctype 目标（规范的异常类别化）。
//! - classList 空白切分按 ASCII 空白；空 token 报
//!   [`BindingError::InvalidArgument`]（规范 SyntaxError 的类别化）。
//! - `remove_event_listener` 按（目标，登记句柄）移除——监听器身份是句柄
//!   而非函数对象地址（同 `event` 模块偏差）。
//! - querySelector / getComputedStyle 钩子未装配时报
//!   [`BindingError::HookUnavailable`]；getComputedStyle 返回属性序对快照
//!   （css 类型不进本 crate）。

use std::fmt;

use super::JsrtError;
use super::gate::{JsrtGate, JsrtListener, ListenerHandle};
use crate::event::Event;
use crate::node::{Attribute, ElementData, Namespace, NodeKind};
use crate::{Document, NodeId};

/// querySelector 注入钩子：在 `scope` 子树（scope 传文档根即全文档）内匹配
/// 选择器，返回首个命中的元素。由 chrome 装配 css 选择器匹配，测试可注入桩。
pub type QuerySelectorHook = Box<dyn Fn(&Document, NodeId, &str) -> Option<NodeId>>;

/// getComputedStyle 注入钩子：返回元素的计算样式快照（属性名 → 值，序对顺序
/// 即迭代序）。由 chrome 装配级联只读快照，测试可注入桩。
pub type GetComputedStyleHook = Box<dyn Fn(&Document, NodeId) -> Vec<(String, String)>>;

/// 绑定操作错误（[`JsrtError::Binding`] 的载荷）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingError {
    /// 目标节点不存在（索引越界）。
    NotFound,
    /// 目标节点不是元素（绑定操作要求 Element）。
    NotElement,
    /// 参数非法（空名字 / 空 class token 等）。
    InvalidArgument,
    /// 树层级约束不满足（DOM `HierarchyRequestError` 的类别化）。
    HierarchyRequest,
    /// 注入钩子未装配（querySelector / getComputedStyle）。
    HookUnavailable,
}

impl fmt::Display for BindingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::NotFound => "node not found",
            Self::NotElement => "target is not an element",
            Self::InvalidArgument => "invalid argument",
            Self::HierarchyRequest => "hierarchy request violated",
            Self::HookUnavailable => "hook not installed",
        };
        f.write_str(message)
    }
}

impl std::error::Error for BindingError {}

/// 绑定注册表（v0 全清单）：引擎适配 crate 的绑定层调用入口。
///
/// 无状态门面：全部操作经 [`JsrtGate`] 短借用完成；文档改动后置重排脏位
/// （chrome 读取后触发重排）。
#[derive(Debug, Clone, Copy, Default)]
pub struct DomRegistry;

impl DomRegistry {
    /// `createElement`（DOM
    /// [create an element](https://dom.spec.whatwg.org/#concept-create-element)
    /// 简化档）：HTML 命名空间元素，名字 ASCII 小写化，游离不挂树。
    ///
    /// # Errors
    ///
    /// 名字为空返回 [`BindingError::InvalidArgument`]；文档借用冲突返回
    /// [`JsrtError::Reentrant`]。
    pub fn create_element(&self, gate: &JsrtGate, name: &str) -> Result<NodeId, JsrtError> {
        if name.is_empty() {
            return Err(BindingError::InvalidArgument.into());
        }
        let name = name.to_ascii_lowercase();
        let mut doc = gate.document_mut()?;
        Ok(doc.create_node(NodeKind::Element(ElementData {
            name,
            namespace: Namespace::Html,
            attributes: Vec::new(),
        })))
    }

    /// `appendChild`（DOM *append*）：把 `child` 追加到 `parent` 末尾，走
    /// 树变更算法入口（含 pre-insert 校验与深度维护）。
    ///
    /// # Errors
    ///
    /// 层级约束 / 目标缺失见 [`BindingError`]；借用冲突见 [`JsrtError::Reentrant`]。
    pub fn append_child(
        &self,
        gate: &JsrtGate,
        parent: NodeId,
        child: NodeId,
    ) -> Result<(), JsrtError> {
        let mut doc = gate.document_mut()?;
        doc.append_child(child, parent).map_err(map_dom_error)?;
        drop(doc);
        gate.mark_mutated();
        Ok(())
    }

    /// `remove`（DOM [`ChildNode.remove`](https://dom.spec.whatwg.org/#dom-childnode-remove)
    /// 简化档）：从父节点摘下；游离节点为无副作用的成功。
    ///
    /// # Errors
    ///
    /// 节点不存在返回 [`BindingError::NotFound`]；借用冲突见
    /// [`JsrtError::Reentrant`]。
    pub fn remove(&self, gate: &JsrtGate, node: NodeId) -> Result<(), JsrtError> {
        let mut doc = gate.document_mut()?;
        if doc.node(node).is_none() {
            return Err(BindingError::NotFound.into());
        }
        let mut removed = false;
        if let Some(parent) = doc.parent(node) {
            doc.remove_child(node, parent).map_err(map_dom_error)?;
            removed = true;
        }
        drop(doc);
        if removed {
            gate.mark_mutated();
        }
        Ok(())
    }

    /// `textContent` 读（DOM
    /// [textContent](https://dom.spec.whatwg.org/#dom-node-textcontent) 简化档）：
    /// 元素 / 片段返回后代文本拼接，文本 / 注释返回自身数据，
    /// Document / Doctype 返回 `None`。
    ///
    /// # Errors
    ///
    /// 节点不存在返回 [`BindingError::NotFound`]。
    pub fn text_content(&self, gate: &JsrtGate, node: NodeId) -> Result<Option<String>, JsrtError> {
        let doc = gate.document()?;
        match doc.node(node) {
            None => Err(BindingError::NotFound.into()),
            Some(NodeKind::Text(data) | NodeKind::Comment(data)) => Ok(Some(data.clone())),
            Some(NodeKind::Element(_) | NodeKind::DocumentFragment) => {
                Ok(Some(descendant_text(&doc, node)))
            }
            Some(
                NodeKind::Document | NodeKind::Doctype(_) | NodeKind::ProcessingInstruction(_),
            ) => Ok(None),
        }
    }

    /// `textContent` 写（DOM
    /// [replace all](https://dom.spec.whatwg.org/#dom-node-textcontent) 简化档）：
    /// 元素 / 片段清空后插入单个文本节点；文本 / 注释改写数据；处理指令改写
    /// 数据；Document / Doctype 不接受。
    ///
    /// # Errors
    ///
    /// 目标缺失 / 不接受的目标见 [`BindingError`]；借用冲突见
    /// [`JsrtError::Reentrant`]。
    pub fn set_text_content(
        &self,
        gate: &JsrtGate,
        node: NodeId,
        value: &str,
    ) -> Result<(), JsrtError> {
        enum Target {
            Character,
            ProcessingInstruction,
            Container,
        }
        let mut doc = gate.document_mut()?;
        let target = match doc.node(node) {
            None => return Err(BindingError::NotFound.into()),
            Some(NodeKind::Document | NodeKind::Doctype(_)) => {
                return Err(BindingError::InvalidArgument.into());
            }
            Some(NodeKind::Text(_) | NodeKind::Comment(_)) => Target::Character,
            Some(NodeKind::ProcessingInstruction(_)) => Target::ProcessingInstruction,
            Some(NodeKind::Element(_) | NodeKind::DocumentFragment) => Target::Container,
        };
        match target {
            Target::Character => {
                if let Some(NodeKind::Text(data) | NodeKind::Comment(data)) = doc.node_mut(node) {
                    *data = value.to_string();
                }
            }
            Target::ProcessingInstruction => {
                if let Some(NodeKind::ProcessingInstruction(data)) = doc.node_mut(node) {
                    data.data = value.to_string();
                }
            }
            Target::Container => {
                doc.replace_all(None, node);
                if !value.is_empty() {
                    let text = doc.create_node(NodeKind::Text(value.to_string()));
                    doc.append_child(text, node).map_err(map_dom_error)?;
                }
            }
        }
        drop(doc);
        gate.mark_mutated();
        Ok(())
    }

    /// `getAttribute`（DOM
    /// [get an attribute](https://dom.spec.whatwg.org/#concept-element-attributes-get-value)
    /// 简化档）：按 ASCII 大小写不敏感匹配无命名空间属性；不存在返回 `None`。
    ///
    /// # Errors
    ///
    /// 目标缺失 / 非元素见 [`BindingError`]。
    pub fn get_attribute(
        &self,
        gate: &JsrtGate,
        node: NodeId,
        name: &str,
    ) -> Result<Option<String>, JsrtError> {
        let doc = gate.document()?;
        let data = element_data(&doc, node)?;
        Ok(data
            .attributes
            .iter()
            .find(|attr| attr.namespace == Namespace::None && attr.name.eq_ignore_ascii_case(name))
            .map(|attr| attr.value.clone()))
    }

    /// `setAttribute`（DOM
    /// [set an attribute](https://dom.spec.whatwg.org/#concept-element-attributes-set-value)
    /// 简化档）：名字 ASCII 小写化，已存在则改值，不存在则追加。
    ///
    /// # Errors
    ///
    /// 名字为空返回 [`BindingError::InvalidArgument`]；目标缺失 / 非元素见
    /// [`BindingError`]。
    pub fn set_attribute(
        &self,
        gate: &JsrtGate,
        node: NodeId,
        name: &str,
        value: &str,
    ) -> Result<(), JsrtError> {
        if name.is_empty() {
            return Err(BindingError::InvalidArgument.into());
        }
        let name = name.to_ascii_lowercase();
        let mut doc = gate.document_mut()?;
        element_data(&doc, node)?;
        if let Some(data) = element_data_mut(&mut doc, node)? {
            match data
                .attributes
                .iter_mut()
                .find(|attr| attr.namespace == Namespace::None && attr.name == name)
            {
                Some(attr) => attr.value = value.to_string(),
                None => data.attributes.push(Attribute {
                    name,
                    namespace: Namespace::None,
                    value: value.to_string(),
                }),
            }
        }
        drop(doc);
        gate.mark_mutated();
        Ok(())
    }

    /// `classList` 读：按 ASCII 空白切分 `class` 属性（DOM
    /// [DOMTokenList](https://dom.spec.whatwg.org/#domtokenlist) 简化档）。
    ///
    /// # Errors
    ///
    /// 目标缺失 / 非元素见 [`BindingError`]。
    pub fn class_list(&self, gate: &JsrtGate, node: NodeId) -> Result<Vec<String>, JsrtError> {
        let doc = gate.document()?;
        let data = element_data(&doc, node)?;
        Ok(parse_class_tokens(&data.attributes))
    }

    /// `classList.add`：已存在的 token 跳过，新 token 追加（保序）。
    ///
    /// # Errors
    ///
    /// 空 token 返回 [`BindingError::InvalidArgument`]；目标缺失 / 非元素见
    /// [`BindingError`]。
    pub fn class_list_add(
        &self,
        gate: &JsrtGate,
        node: NodeId,
        tokens: &[&str],
    ) -> Result<(), JsrtError> {
        Self::validate_tokens(tokens)?;
        self.update_class_tokens(gate, node, |mut current| {
            let mut changed = false;
            for token in tokens {
                let token = (*token).to_string();
                if !current.contains(&token) {
                    current.push(token);
                    changed = true;
                }
            }
            (current, changed)
        })
    }

    /// `classList.remove`：移除全部同名 token。
    ///
    /// # Errors
    ///
    /// 空 token 返回 [`BindingError::InvalidArgument`]；目标缺失 / 非元素见
    /// [`BindingError`]。
    pub fn class_list_remove(
        &self,
        gate: &JsrtGate,
        node: NodeId,
        tokens: &[&str],
    ) -> Result<(), JsrtError> {
        Self::validate_tokens(tokens)?;
        self.update_class_tokens(gate, node, |current| {
            let before = current.len();
            let remaining: Vec<String> = current
                .into_iter()
                .filter(|token| !tokens.contains(&token.as_str()))
                .collect();
            let changed = remaining.len() != before;
            (remaining, changed)
        })
    }

    /// `classList.contains`：是否含该 token。
    ///
    /// # Errors
    ///
    /// 空 token 返回 [`BindingError::InvalidArgument`]；目标缺失 / 非元素见
    /// [`BindingError`]。
    pub fn class_list_contains(
        &self,
        gate: &JsrtGate,
        node: NodeId,
        token: &str,
    ) -> Result<bool, JsrtError> {
        Self::validate_tokens(&[token])?;
        Ok(self
            .class_list(gate, node)?
            .iter()
            .any(|entry| entry == token))
    }

    /// `classList.toggle`：含则移除返回 `false`，不含则添加返回 `true`
    /// （无 force 参数，v0 偏差）。
    ///
    /// # Errors
    ///
    /// 空 token 返回 [`BindingError::InvalidArgument`]；目标缺失 / 非元素见
    /// [`BindingError`]。
    pub fn class_list_toggle(
        &self,
        gate: &JsrtGate,
        node: NodeId,
        token: &str,
    ) -> Result<bool, JsrtError> {
        Self::validate_tokens(&[token])?;
        if self.class_list_contains(gate, node, token)? {
            self.class_list_remove(gate, node, &[token])?;
            Ok(false)
        } else {
            self.class_list_add(gate, node, &[token])?;
            Ok(true)
        }
    }

    /// `getElementById`（DOM
    /// [get an element by id](https://html.spec.whatwg.org/multipage/dom.html#dom-document-getelementbyid)
    /// 简化档）：先序遍历找第一个 id 属性精确匹配的元素。
    ///
    /// # Errors
    ///
    /// 文档借用冲突见 [`JsrtError::Reentrant`]。
    pub fn get_element_by_id(
        &self,
        gate: &JsrtGate,
        id: &str,
    ) -> Result<Option<NodeId>, JsrtError> {
        let doc = gate.document()?;
        let mut stack: Vec<NodeId> = vec![doc.root()];
        while let Some(node) = stack.pop() {
            if let Some(NodeKind::Element(data)) = doc.node(node)
                && data.attributes.iter().any(|attr| {
                    attr.namespace == Namespace::None && attr.name == "id" && attr.value == id
                })
            {
                return Ok(Some(node));
            }
            let mut children: Vec<NodeId> = doc.children(node).collect();
            children.reverse();
            stack.extend(children);
        }
        Ok(None)
    }

    /// `addEventListener`：登记进 JS 监听器表（与 K1 的 Rust 监听器注册表
    /// 分离，见 [`super::gate`] 模块文档偏差）。允许重复注册（同 DOM：触发两次）。
    ///
    /// # Errors
    ///
    /// 目标不存在返回 [`BindingError::NotFound`]；借用冲突见
    /// [`JsrtError::Reentrant`]。
    pub fn add_event_listener(
        &self,
        gate: &JsrtGate,
        node: NodeId,
        type_: &str,
        capture: bool,
        callback: Box<dyn JsrtListener>,
    ) -> Result<ListenerHandle, JsrtError> {
        {
            let doc = gate.document()?;
            if doc.node(node).is_none() {
                return Err(BindingError::NotFound.into());
            }
        }
        gate.add_listener(node, type_, capture, callback)
    }

    /// `removeEventListener`（句柄形态）：句柄不属于目标时返回 `false`。
    ///
    /// # Errors
    ///
    /// 目标不存在返回 [`BindingError::NotFound`]；借用冲突见
    /// [`JsrtError::Reentrant`]。
    pub fn remove_event_listener(
        &self,
        gate: &JsrtGate,
        node: NodeId,
        handle: ListenerHandle,
    ) -> Result<bool, JsrtError> {
        {
            let doc = gate.document()?;
            if doc.node(node).is_none() {
                return Err(BindingError::NotFound.into());
            }
        }
        gate.remove_listener_of(node, handle)
    }

    /// `dispatchEvent`（DOM §2.9 简化档，见 [`super::gate`] 模块文档偏差）：
    /// 返回 `!defaultPrevented`。`scope` 为引擎作用域——JS 侧派发（引擎执行
    /// 期间）由绑定传入；泵驱动的派发传 `None`。
    ///
    /// # Errors
    ///
    /// 目标不存在返回 [`BindingError::NotFound`]；回调异常原样上抛。
    pub fn dispatch_event(
        &self,
        gate: &JsrtGate,
        scope: Option<&dyn crate::jsrt::JsrtScope>,
        node: NodeId,
        event: &mut Event,
    ) -> Result<bool, JsrtError> {
        gate.dispatch(node, event, scope)
    }

    /// `querySelector`：经注入钩子匹配；钩子未装配时报
    /// [`BindingError::HookUnavailable`]。
    ///
    /// # Errors
    ///
    /// 目标不存在 / 钩子未装配见 [`BindingError`]；借用冲突见
    /// [`JsrtError::Reentrant`]。
    pub fn query_selector(
        &self,
        gate: &JsrtGate,
        scope: NodeId,
        selector: &str,
    ) -> Result<Option<NodeId>, JsrtError> {
        {
            let doc = gate.document()?;
            if doc.node(scope).is_none() {
                return Err(BindingError::NotFound.into());
            }
        }
        let state = gate.borrow_state("query_selector")?;
        let Some(hook) = state.query_selector.as_ref() else {
            return Err(BindingError::HookUnavailable.into());
        };
        let doc = gate.document()?;
        Ok(hook(&doc, scope, selector))
    }
    /// `getComputedStyle`：经注入钩子取级联只读快照（属性序对）；元素校验后
    /// 调用，钩子未装配时报 [`BindingError::HookUnavailable`]。
    ///
    /// # Errors
    ///
    /// 目标缺失 / 非元素 / 钩子未装配见 [`BindingError`]。
    pub fn get_computed_style(
        &self,
        gate: &JsrtGate,
        node: NodeId,
    ) -> Result<Vec<(String, String)>, JsrtError> {
        {
            let doc = gate.document()?;
            element_data(&doc, node)?;
        }
        let state = gate.borrow_state("get_computed_style")?;
        let Some(hook) = state.get_computed_style.as_ref() else {
            return Err(BindingError::HookUnavailable.into());
        };
        let doc = gate.document()?;
        Ok(hook(&doc, node))
    }

    fn validate_tokens(tokens: &[&str]) -> Result<(), JsrtError> {
        if tokens.iter().any(|token| token.is_empty()) {
            return Err(BindingError::InvalidArgument.into());
        }
        Ok(())
    }

    /// 读改写 `class` 属性的公共路径：解析现有 token → 更新 → 序列化写回。
    fn update_class_tokens(
        &self,
        gate: &JsrtGate,
        node: NodeId,
        update: impl FnOnce(Vec<String>) -> (Vec<String>, bool),
    ) -> Result<(), JsrtError> {
        let mut doc = gate.document_mut()?;
        element_data(&doc, node)?;
        let changed = match element_data_mut(&mut doc, node)? {
            Some(data) => {
                let (updated, changed) = update(parse_class_tokens(&data.attributes));
                if changed {
                    write_class_attr(data, &updated);
                }
                changed
            }
            // element_data 已校验元素存在，此分支不可达。
            None => false,
        };
        drop(doc);
        if changed {
            gate.mark_mutated();
        }
        Ok(())
    }
}

/// 元素节点数据读取；目标不存在 / 非元素分别报错。
fn element_data(doc: &Document, node: NodeId) -> Result<&ElementData, BindingError> {
    match doc.node(node) {
        Some(NodeKind::Element(data)) => Ok(data),
        Some(_) => Err(BindingError::NotElement),
        None => Err(BindingError::NotFound),
    }
}

/// 元素节点数据可变读取；调用方先用 [`element_data`] 完成存在性 / 类型校验。
fn element_data_mut(
    doc: &mut Document,
    node: NodeId,
) -> Result<Option<&mut ElementData>, BindingError> {
    match doc.node_mut(node) {
        Some(NodeKind::Element(data)) => Ok(Some(data)),
        Some(_) => Err(BindingError::NotElement),
        None => Ok(None),
    }
}

/// 解析 `class` 属性为 token 列表（ASCII 空白切分，保序）。
fn parse_class_tokens(attributes: &[Attribute]) -> Vec<String> {
    attributes
        .iter()
        .find(|attr| attr.namespace == Namespace::None && attr.name == "class")
        .map(|attr| {
            attr.value
                .split_ascii_whitespace()
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 把 token 列表序列化写回 `class` 属性（空格连接；无属性则追加）。
fn write_class_attr(data: &mut ElementData, tokens: &[String]) {
    let value = tokens.join(" ");
    match data
        .attributes
        .iter_mut()
        .find(|attr| attr.namespace == Namespace::None && attr.name == "class")
    {
        Some(attr) => attr.value = value,
        None => data.attributes.push(Attribute {
            name: "class".to_string(),
            namespace: Namespace::None,
            value,
        }),
    }
}

/// 先序收集 `root` 的全部文本后代（栈实现，深树不递归）。
fn descendant_text(doc: &Document, root: NodeId) -> String {
    let mut out = String::new();
    let mut stack: Vec<NodeId> = doc.children(root).collect();
    stack.reverse();
    while let Some(node) = stack.pop() {
        if let Some(NodeKind::Text(data)) = doc.node(node) {
            out.push_str(data);
        }
        let mut children: Vec<NodeId> = doc.children(node).collect();
        children.reverse();
        stack.extend(children);
    }
    out
}

/// [`DomError`](crate::DomError) → [`BindingError`] 映射。
fn map_dom_error(error: crate::DomError) -> JsrtError {
    match error {
        crate::DomError::HierarchyRequest => BindingError::HierarchyRequest.into(),
        crate::DomError::NotFound => BindingError::NotFound.into(),
    }
}
