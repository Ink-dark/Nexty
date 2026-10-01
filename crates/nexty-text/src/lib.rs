//! Nexty 文本整形与字体 facade。
//!
//! 上游：`parley`（Apache-2.0/MIT）——内部用 `fontique` 做字体选择与回退、
//! `harfrust`（HarfBuzz 整形算法的 Rust 实现）做字形整形。选型 ADR 中列出的
//! `swash` 自 parley 0.11 起已不参与整形，本层不再依赖（决策见
//! `docs/decisions/2026-10-01-crate-selection.md`）。
//!
//! 行为 ground truth：CSS Text、CSS Fonts 与 Unicode 断行算法；字形定位遵循
//! OpenType 度量（units_per_em 归一到 CSS px，scale = 1）。
//!
//! 本层只整形**单行**文本：white-space 折叠（含换行符处理）是 layout 层的
//! 职责，进入本层的文本应已折叠。
//!
//! 上游类型一律不得出现在本 crate 的 pub 导出中（AGENTS.md 硬规则）。

#![forbid(unsafe_code)]

use std::cell::RefCell;

use parley::StyleProperty;

/// 字体族与字号。
#[derive(Debug, Clone, PartialEq)]
pub struct TextStyle {
    /// 字体族列表，按 CSS `font-family` 优先顺序排列。
    ///
    /// 每一项是一个族名（如 `"Times New Roman"`）或 generic 族关键字
    /// （如 `"serif"` / `"sans-serif"` / `"monospace"`，大小写不敏感），
    /// 解析与回退由 parley/fontique 按 CSS Fonts 语义处理。
    pub families: Vec<String>,
    /// 字号，CSS px。
    pub size: f32,
}

/// 整形后的单个字形。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    /// 字体内部字形编号。
    pub id: u32,
    /// 相对行首的水平偏移，CSS px。
    pub x: f32,
    /// 相对基线的垂直偏移，CSS px（负值在基线上方）。
    pub y: f32,
    /// 步进宽度，CSS px。
    pub advance: f32,
}

/// 一段文本的整形结果。
///
/// 坐标系：`x` 从行首起算，`y` 以基线为 0、向下为正；[`ShapedText::height`]
/// 是 parley 按字体度量算出的行高。
#[derive(Debug, Clone, PartialEq)]
pub struct ShapedText {
    /// 按绘制顺序排列的字形。
    pub glyphs: Vec<Glyph>,
    /// 行宽（含行尾空白），CSS px。
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextError {
    /// 请求的字体族列表为空，无法选择字体。
    ///
    /// 单个未知族名不会触发本错误：parley/fontique 会按 CSS Fonts 的回退
    /// 语义选择可用字体。
    FontUnavailable,
}

/// [`TextShaper`] 的 parley 实现。
///
/// 持有 fontique 字体集合（系统字体）与 parley 布局上下文；上下文可复用，
/// 用 `RefCell` 提供内部可变性（`shape` 只需 `&self`）。
pub struct ParleyTextShaper {
    font_context: RefCell<parley::FontContext>,
    layout_context: RefCell<parley::LayoutContext>,
}

impl std::fmt::Debug for ParleyTextShaper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParleyTextShaper").finish()
    }
}

impl Default for ParleyTextShaper {
    fn default() -> Self {
        Self {
            font_context: RefCell::new(parley::FontContext::new()),
            layout_context: RefCell::new(parley::LayoutContext::new()),
        }
    }
}

impl ParleyTextShaper {
    /// 创建整形器并发现系统字体。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl TextShaper for ParleyTextShaper {
    fn shape(&self, text: &str, style: &TextStyle) -> Result<ShapedText, TextError> {
        if style.families.is_empty() {
            return Err(TextError::FontUnavailable);
        }
        if text.is_empty() {
            return Ok(ShapedText {
                glyphs: Vec::new(),
                width: 0.0,
                height: 0.0,
            });
        }

        // CSS font-family 列表的解析（族名/引号/generic 关键字/回退顺序）
        // 交给 parley 的 FontFamily::Source
        let family_list = style.families.join(", ");

        let mut font_context = self.font_context.borrow_mut();
        let mut layout_context = self.layout_context.borrow_mut();
        let mut builder = layout_context.ranged_builder(&mut font_context, text, 1.0, false);
        builder.push_default(StyleProperty::FontFamily(parley::FontFamily::Source(
            std::borrow::Cow::Borrowed(&family_list),
        )));
        builder.push_default(StyleProperty::FontSize(style.size));

        let mut layout = builder.build(text);
        // max_advance = None：单行整形，不做折行
        layout.break_all_lines(None);

        let mut glyphs = Vec::new();
        for line in layout.lines() {
            let baseline = line.metrics().baseline;
            for item in line.items() {
                let parley::PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                for glyph in glyph_run.positioned_glyphs() {
                    glyphs.push(Glyph {
                        id: glyph.id,
                        // positioned_glyphs 的 y 已含基线偏移，换回相对基线
                        x: glyph.x,
                        y: glyph.y - baseline,
                        advance: glyph.advance,
                    });
                }
            }
        }

        Ok(ShapedText {
            glyphs,
            width: layout.width(),
            height: layout.height(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shaper() -> ParleyTextShaper {
        ParleyTextShaper::new()
    }

    fn style(families: &[&str], size: f32) -> TextStyle {
        TextStyle {
            families: families.iter().map(|family| (*family).to_owned()).collect(),
            size,
        }
    }

    #[test]
    fn empty_text_yields_empty_output() {
        let shaped = shaper()
            .shape("", &style(&["serif"], 16.0))
            .expect("empty text shapes to empty");
        assert!(shaped.glyphs.is_empty());
        assert_eq!(shaped.width, 0.0);
    }

    #[test]
    fn empty_family_list_is_font_unavailable() {
        let error = shaper()
            .shape("hello", &style(&[], 16.0))
            .expect_err("no families");
        assert_eq!(error, TextError::FontUnavailable);
    }

    #[test]
    fn ascii_text_shapes_to_positive_metrics() {
        let shaped = shaper()
            .shape("Hello", &style(&["serif"], 16.0))
            .expect("shaping with system fonts");

        assert!(!shaped.glyphs.is_empty());
        assert!(shaped.width > 0.0);
        assert!(shaped.height > 0.0);
        // 每个字形有非负步进，x 单调不减
        let mut previous_x = 0.0;
        for glyph in &shaped.glyphs {
            assert!(glyph.advance >= 0.0);
            assert!(glyph.x >= previous_x - 1e-4);
            previous_x = glyph.x;
        }
        // 宽度与最后一个字形的右缘一致（容差内）
        let last = shaped.glyphs.last().expect("non-empty");
        assert!((shaped.width - (last.x + last.advance)).abs() < 1e-3);
    }

    #[test]
    fn advances_scale_with_font_size() {
        let shaper = shaper();
        let small = shaper
            .shape("iii", &style(&["serif"], 16.0))
            .expect("small");
        let large = shaper
            .shape("iii", &style(&["serif"], 32.0))
            .expect("large");

        assert_eq!(
            small.glyphs.len(),
            large.glyphs.len(),
            "同字体同文本字形数一致"
        );
        let small_advance: f32 = small.glyphs.iter().map(|glyph| glyph.advance).sum();
        let large_advance: f32 = large.glyphs.iter().map(|glyph| glyph.advance).sum();
        let ratio = large_advance / small_advance;
        assert!(
            (ratio - 2.0).abs() < 0.02,
            "advance 比例应约等于字号比例: {ratio}"
        );
    }

    #[test]
    fn unknown_family_falls_back_gracefully() {
        let shaper = shaper();
        // 未知族名按 CSS Fonts 语义回退到可用字体，不报错
        let shaped = shaper
            .shape("Hi", &style(&["no-such-family-xyz", "serif"], 16.0))
            .expect("fallback");
        assert!(!shaped.glyphs.is_empty());

        // 只有未知族名同样回退
        let shaped = shaper
            .shape("Hi", &style(&["no-such-family-xyz"], 16.0))
            .expect("fallback with single unknown family");
        assert!(!shaped.glyphs.is_empty());
    }

    #[test]
    fn generic_family_keywords_work() {
        let shaper = shaper();
        for family in ["sans-serif", "SANS-SERIF", "monospace", "system-ui"] {
            let shaped = shaper
                .shape("Hi", &style(&[family], 16.0))
                .unwrap_or_else(|error| panic!("{family} should shape: {error:?}"));
            assert!(!shaped.glyphs.is_empty(), "{family} produced glyphs");
        }
    }
}
