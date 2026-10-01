//! Nexty 文本整形与字体 facade。
//!
//! 上游：`parley` + `swash` + `fontique`（linebender 系，Apache-2.0/MIT）。
//! 决策见 `docs/decisions/2026-10-01-crate-selection.md`。
//!
//! 行为 ground truth：CSS Text、CSS Fonts 与 Unicode 断行算法。
//!
//! 上游类型一律不得出现在本 crate 的 pub 导出中（AGENTS.md 硬规则）。

#![forbid(unsafe_code)]

/// 字体族与字号。
#[derive(Debug, Clone, PartialEq)]
pub struct TextStyle {
    /// 字体族名，如 `"serif"`、`"sans-serif"`。
    pub family: String,
    /// 字号，CSS px。
    pub size: f32,
}

/// 整形后的单个字形。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    /// 字体内部字形编号。
    pub id: u16,
    /// 相对行首的水平偏移，CSS px。
    pub x: f32,
    /// 相对基线的垂直偏移，CSS px。
    pub y: f32,
    /// 步进宽度，CSS px。
    pub advance: f32,
}

/// 一段文本的整形结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ShapedText {
    /// 按绘制顺序排列的字形。
    pub glyphs: Vec<Glyph>,
    /// 总宽度，CSS px。
    pub width: f32,
    /// 行高，CSS px。
    pub height: f32,
}

/// 文本整形后端。
///
/// 实现方负责把 [`TextStyle`] 映射到具体字体选择与整形引擎，
/// 只对外暴露本 crate 的类型。
pub trait TextShaper {
    /// 对一段文本整形。
    ///
    /// # Errors
    ///
    /// 字体缺失或整形失败时返回 [`TextError`]。
    fn shape(&self, text: &str, style: &TextStyle) -> Result<ShapedText, TextError>;
}

/// 文本整形错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextError {
    /// 请求的字体族没有可用字体。
    FontUnavailable {
        /// 请求的字体族名。
        family: String,
    },
    /// 整形引擎拒绝该输入。
    ShapeFailed,
}
