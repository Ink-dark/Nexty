//! 节点数据类型。
//!
//! 行为 ground truth：[WHATWG DOM Standard](https://dom.spec.whatwg.org/) 的
//! [Nodes](https://dom.spec.whatwg.org/#nodes) 与
//! [Elements](https://dom.spec.whatwg.org/#elements) 章节。

/// arena 内节点的索引。
///
/// 在所属 [`Document`](crate::Document) 生命周期内保持稳定；节点被移除后索引不复用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub(crate) usize);

impl NodeId {
    /// 返回索引值。仅用于诊断与调试输出。
    #[must_use]
    pub fn index(self) -> usize {
        self.0
    }
}

/// 元素或属性的命名空间。
///
/// 对应 DOM 的 namespace（可为 null）。HTML 的三个内容命名空间单列，其余命名空间
/// 按 URI 原样保留。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Namespace {
    /// 空命名空间，对应 DOM 中的 null namespace。
    None,
    /// `http://www.w3.org/1999/xhtml`。
    Html,
    /// `http://www.w3.org/2000/svg`。
    Svg,
    /// `http://www.w3.org/1998/Math/MathML`。
    MathMl,
    /// 其它命名空间 URI。
    Other(String),
}

/// 元素属性。
///
/// 对应 DOM [Attr](https://dom.spec.whatwg.org/#interface-attr) 的局部名、命名空间
/// 与值。XML 前缀不参与匹配，故不保留。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    /// 属性局部名。
    pub name: String,
    /// 属性命名空间。
    pub namespace: Namespace,
    /// 属性值。
    pub value: String,
}

/// 元素节点的数据。
///
/// 对应 DOM [Element](https://dom.spec.whatwg.org/#interface-element) 的 local name、
/// namespace 与 attribute list。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementData {
    /// 元素局部名。HTML 命名空间下为小写。
    pub name: String,
    /// 元素命名空间。
    pub namespace: Namespace,
    /// 属性列表，保持插入顺序。
    pub attributes: Vec<Attribute>,
}

/// 文档类型节点的数据。
///
/// 对应 DOM
/// [DocumentType](https://dom.spec.whatwg.org/#interface-documenttype)。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocumentTypeData {
    /// 文档类型名。
    pub name: String,
    /// public ID。
    pub public_id: String,
    /// system ID。
    pub system_id: String,
}

/// 处理指令节点的数据。
///
/// 对应 DOM
/// [ProcessingInstruction](https://dom.spec.whatwg.org/#interface-processinginstruction)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessingInstructionData {
    /// 处理指令目标。
    pub target: String,
    /// 处理指令数据。
    pub data: String,
}

/// 节点数据。
///
/// 对应 DOM [Node](https://dom.spec.whatwg.org/#interface-node) 的
/// [node type](https://dom.spec.whatwg.org/#concept-node-type)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeKind {
    /// 文档节点。
    Document,
    /// 文档类型节点。
    Doctype(DocumentTypeData),
    /// 文档片段节点。
    DocumentFragment,
    /// 元素节点。
    Element(ElementData),
    /// 文本节点。
    Text(String),
    /// 注释节点。
    Comment(String),
    /// 处理指令节点。
    ProcessingInstruction(ProcessingInstructionData),
}

/// 文档的 quirks 模式。
///
/// 对应 DOM [Document/mode](https://dom.spec.whatwg.org/#dom-document-mode)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum QuirksMode {
    /// 标准模式。
    #[default]
    NoQuirks,
    /// quirks 模式。
    Quirks,
    /// limited quirks 模式。
    LimitedQuirks,
}

/// DOM 树操作可能返回的错误。
///
/// 对应 DOM 规范在树变更算法中抛出的 `DOMException` 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomError {
    /// `HierarchyRequestError`：目标位置不满足树的层级约束。
    HierarchyRequest,
    /// `NotFoundError`：参考节点不是指定父节点的子节点。
    NotFound,
}
