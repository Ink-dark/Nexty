//! 测试辅助：用 nexty-html 构造 arena 测试文档并定位元素。
//!
//! 仅在 `cfg(test)` 下编译；nexty-html 是 dev-dependency，不进入公共 API。

use nexty_dom::{Document, NodeId, NodeKind};
use nexty_html::ParseOptions;

/// 解析完整 HTML 文档。
#[must_use]
pub fn build_document(html: &str) -> Document {
    nexty_html::parse_document(html, ParseOptions::default())
}

/// 按树序找第一个局部名匹配的元素。
#[must_use]
pub fn find_first_by_name(document: &Document, name: &str) -> Option<NodeId> {
    fn walk(document: &Document, root: NodeId, name: &str) -> Option<NodeId> {
        if matches!(document.node(root), Some(NodeKind::Element(data)) if data.name == name) {
            return Some(root);
        }
        for child in document.children(root) {
            if let Some(found) = walk(document, child, name) {
                return Some(found);
            }
        }
        None
    }
    walk(document, document.root(), name)
}
