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
//!
//! **Feature**：单行整形（`shaping`，含 `TextShaper` 与 parley 实现）与字体文件
//! 解析（`font-resolution`，含 `FontResolver`）分别受feature 控制，默认全开；
//! 二者共享 parley 依赖，故各自隐含 `dep:parley`。

#![forbid(unsafe_code)]

#[cfg(feature = "shaping")]
use std::cell::RefCell;
#[cfg(feature = "font-resolution")]
use std::sync::Mutex;

#[cfg(feature = "shaping")]
use parley::StyleProperty;

/// 字体族与字号。
#[cfg(feature = "shaping")]
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
#[cfg(feature = "shaping")]
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
#[cfg(feature = "shaping")]
#[derive(Debug, Clone, PartialEq)]
pub struct ShapedText {
    /// 按绘制顺序排列的字形。
    pub glyphs: Vec<Glyph>,
    /// 行宽（含行尾空白），CSS px。
    pub width: f32,
    /// 行高，CSS px。
    pub height: f32,
}

/// 字体度量（CSS px；strut / 行盒计算用）。
#[cfg(feature = "shaping")]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontMetrics {
    /// ascent：基线到行内最高点的距离（CSS px，正值）。
    pub ascent: f32,
    /// descent：基线到行内最低点的距离（CSS px，正值）。
    pub descent: f32,
    /// 该字体的默认行高（ascent + descent + leading，CSS px）。
    pub line_height: f32,
}

/// 解析出的字体资源：字体文件字节与 collection 内 face 索引。
///
/// 供光栅层（paint）构造字形渲染所需的字体数据；字节为共享快照，
/// 解析结果可按族列表缓存。
#[cfg(feature = "font-resolution")]
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedFont {
    data: std::sync::Arc<[u8]>,
    index: u32,
}

#[cfg(feature = "font-resolution")]
impl ResolvedFont {
    /// 字体文件字节（TTC 时为整个 collection）。
    #[must_use]
    pub fn data(&self) -> &std::sync::Arc<[u8]> {
        &self.data
    }

    /// face 在 collection 中的索引（单字体文件为 0）。
    #[must_use]
    pub fn index(&self) -> u32 {
        self.index
    }
}

#[cfg(feature = "font-resolution")]
impl std::fmt::Debug for ResolvedFont {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedFont")
            .field("bytes", &self.data.len())
            .field("index", &self.index)
            .finish()
    }
}

/// 文本整形后端。
///
/// 实现方负责把 [`TextStyle`] 映射到具体字体选择与整形引擎，
/// 只对外暴露本 crate 的类型。
#[cfg(feature = "shaping")]
pub trait TextShaper {
    /// 对一段文本整形。
    ///
    /// # Errors
    ///
    /// 字体缺失或整形失败时返回 [`TextError`]。
    fn shape(&self, text: &str, style: &TextStyle) -> Result<ShapedText, TextError>;

    /// 返回字体族列表对应字体的度量。
    ///
    /// # Errors
    ///
    /// 字体族列表为空时返回 [`TextError::FontUnavailable`]。
    fn metrics(&self, style: &TextStyle) -> Result<FontMetrics, TextError>;
}

/// 文本整形错误。
#[cfg(feature = "shaping")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextError {
    /// 请求的字体族列表为空，无法选择字体。
    ///
    /// 单个未知族名不会触发本错误：parley/fontique 会按 CSS Fonts 的回退
    /// 语义选择可用字体。
    FontUnavailable,
}

/// 字体解析器：把 CSS 字体族列表解析为字体文件字节。
///
/// 独立于整形器（无布局上下文），内部用 `Mutex` 提供可变性，
/// 可跨线程共享（`Send + Sync`）。
#[cfg(feature = "font-resolution")]
#[derive(Default)]
pub struct FontResolver {
    context: Mutex<parley::FontContext>,
}

#[cfg(feature = "font-resolution")]
impl std::fmt::Debug for FontResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontResolver").finish()
    }
}

#[cfg(feature = "font-resolution")]
impl FontResolver {
    /// 创建解析器并发现系统字体。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 按字体族列表解析出可用字体的文件字节。
    ///
    /// 族名 / generic 关键字的语义与 [`TextShaper::shape`] 一致；全部族名
    /// 都不可用时按 CSS Fonts 回退语义退到 sans-serif，仍无则返回 `None`。
    /// 空族列表返回 `None`。
    pub fn resolve_font(&self, families: &[String]) -> Option<ResolvedFont> {
        if families.is_empty() {
            return None;
        }
        // 族名 / generic 关键字的解析与整形同源（parlance）；
        // QueryFamily 借用族名字符串，故持有 owned 副本保证生命周期
        enum FamilySpec {
            Named(String),
            Generic(parley::GenericFamily),
        }
        fn build_items(specs: &[FamilySpec]) -> Vec<parley::fontique::QueryFamily<'_>> {
            specs
                .iter()
                .map(|spec| match spec {
                    FamilySpec::Named(name) => parley::fontique::QueryFamily::Named(name),
                    FamilySpec::Generic(generic) => {
                        parley::fontique::QueryFamily::Generic(*generic)
                    }
                })
                .collect()
        }
        let specs: Vec<FamilySpec> = families
            .iter()
            .filter_map(|family| match parley::FontFamilyName::parse(family)? {
                parley::FontFamilyName::Named(name) => Some(FamilySpec::Named(name.into_owned())),
                parley::FontFamilyName::Generic(generic) => Some(FamilySpec::Generic(generic)),
            })
            .collect();

        let mut context = self.context.lock().expect("font resolver poisoned");
        // 拆字段借用：collection 与 source_cache 同时可变借用
        let parley::FontContext {
            collection,
            source_cache,
        } = &mut *context;
        let mut resolve = |specs: &[FamilySpec]| -> Option<ResolvedFont> {
            let items = build_items(specs);
            let mut query = collection.query(source_cache);
            query.set_families(items);
            let mut found = None;
            query.matches_with(|font| {
                // Blob 是共享引用计数，字节取快照供跨线程使用
                found = Some(ResolvedFont {
                    data: std::sync::Arc::from(font.blob.as_ref()),
                    index: font.index,
                });
                parley::fontique::QueryStatus::Stop
            });
            found
        };

        let mut resolved = resolve(&specs);
        if resolved.is_none() {
            // 回退：sans-serif（CSS Fonts 的 last-resort 语义简化）
            resolved = resolve(&[FamilySpec::Generic(parley::GenericFamily::SansSerif)]);
        }
        resolved
    }
}

/// [`TextShaper`] 的 parley 实现。
///
/// 持有 fontique 字体集合（系统字体）与 parley 布局上下文；上下文可复用，
/// 用 `RefCell` 提供内部可变性（`shape` 只需 `&self`）。
#[cfg(feature = "shaping")]
pub struct ParleyTextShaper {
    font_context: RefCell<parley::FontContext>,
    layout_context: RefCell<parley::LayoutContext>,
}

#[cfg(feature = "shaping")]
impl std::fmt::Debug for ParleyTextShaper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParleyTextShaper").finish()
    }
}

#[cfg(feature = "shaping")]
impl Default for ParleyTextShaper {
    fn default() -> Self {
        Self {
            font_context: RefCell::new(parley::FontContext::new()),
            layout_context: RefCell::new(parley::LayoutContext::new()),
        }
    }
}

#[cfg(feature = "shaping")]
impl ParleyTextShaper {
    /// 创建整形器并发现系统字体。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 按字体族列表解析出可用字体的文件字节。
    ///
    /// 语义见 [`FontResolver::resolve_font`]。
    pub fn resolve_font(&self, families: &[String]) -> Option<ResolvedFont> {
        FontResolver::new().resolve_font(families)
    }

    /// 按样式对文本做单行 parley 布局（不折行）。
    fn build_layout(
        &self,
        text: &str,
        style: &TextStyle,
    ) -> Result<parley::Layout<[u8; 4]>, TextError> {
        if style.families.is_empty() {
            return Err(TextError::FontUnavailable);
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
        Ok(layout)
    }
}

#[cfg(feature = "shaping")]
impl TextShaper for ParleyTextShaper {
    fn shape(&self, text: &str, style: &TextStyle) -> Result<ShapedText, TextError> {
        if text.is_empty() {
            return Ok(ShapedText {
                glyphs: Vec::new(),
                width: 0.0,
                height: 0.0,
            });
        }

        let layout = self.build_layout(text, style)?;

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

        let width = layout
            .width()
            .max(glyphs.last().map_or(0.0, |glyph| glyph.x + glyph.advance));

        Ok(ShapedText {
            glyphs,
            // parley 的行宽按断行测量语义**不含行尾空白**，而本层契约是含
            // （调用方按 ShapedText.width 计词间空白的步进，见 inline 模块的
            // space_after_previous）——取字形右缘与行宽的较大者。
            width,
            height: layout.height(),
        })
    }

    fn metrics(&self, style: &TextStyle) -> Result<FontMetrics, TextError> {
        // 用不换行空格探针取度量：NBSP 在几乎所有字体中都有字形，
        // parley 的行度量来自 run 字体的 ascent/descent/leading
        let layout = self.build_layout("\u{00A0}", style)?;
        let line = layout.lines().next().ok_or(TextError::FontUnavailable)?;
        let metrics = line.metrics();
        Ok(FontMetrics {
            ascent: metrics.ascent,
            descent: metrics.descent,
            line_height: metrics.line_height,
        })
    }
}

// 测试覆盖整形与字体解析（shaping 隐含 font-resolution）。
#[cfg(all(test, feature = "shaping"))]
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

    #[test]
    fn trailing_whitespace_is_included_in_width() {
        // parley 行宽裁剪行尾空白；ShapedText.width 契约是含行尾空白
        // （空格的步进供行内布局计词间距，见下一测试）
        let shaper = shaper();
        let style = style(&["serif"], 16.0);
        let space = shaper.shape(" ", &style).expect("shape space");
        assert!(space.width > 0.0, "行尾空白计入宽度：width={}", space.width);
        let word = shaper.shape("a", &style).expect("shape a");
        let trailing = shaper.shape("a ", &style).expect("shape a-space");
        assert!(
            trailing.width > word.width,
            "a 加行尾空格应更宽：{} vs {}",
            trailing.width,
            word.width
        );
    }

    #[test]
    fn inter_word_space_advances_position() {
        // 词间空白的宽度 = 后词起点与前词右缘之差（行内布局的空格语义）
        let shaper = shaper();
        let style = style(&["serif"], 16.0);
        let space = shaper.shape(" ", &style).expect("shape space");
        let text = shaper.shape("a b", &style).expect("shape a b");
        let b = text.glyphs.last().expect("b glyph");
        let a = text.glyphs.first().expect("a glyph");
        let gap = b.x - (a.x + a.advance);
        assert!(
            (gap - space.width).abs() < 1e-3,
            "词间空隙 {gap} 应等于空格宽 {}",
            space.width
        );
    }

    #[test]
    fn metrics_are_positive_and_scale_with_size() {
        let shaper = shaper();
        let small = shaper
            .metrics(&style(&["serif"], 16.0))
            .expect("metrics for serif");
        assert!(small.ascent > 0.0);
        assert!(small.descent > 0.0);
        assert!(small.line_height >= small.ascent + small.descent);

        let large = shaper
            .metrics(&style(&["serif"], 32.0))
            .expect("metrics at 2x size");
        let ratio = large.ascent / small.ascent;
        assert!((ratio - 2.0).abs() < 0.05, "ascent 应约随字号翻倍: {ratio}");
    }

    #[test]
    fn metrics_empty_family_list_is_font_unavailable() {
        let error = shaper()
            .metrics(&style(&[], 16.0))
            .expect_err("no families");
        assert_eq!(error, TextError::FontUnavailable);
    }

    #[test]
    fn resolve_font_returns_bytes_for_known_and_unknown_families() {
        let shaper = shaper();
        let resolved = shaper
            .resolve_font(&["serif".to_owned()])
            .expect("generic family resolves");
        assert!(!resolved.data().is_empty(), "字体字节非空");
        assert_eq!(resolved.index(), 0, "常规字体文件单 face");

        // 全部族名未知 → 回退仍能解析出字体
        let fallback = shaper
            .resolve_font(&["no-such-family-xyz".to_owned()])
            .expect("fallback resolves");
        assert!(!fallback.data().is_empty());
    }

    #[test]
    fn resolve_font_empty_family_list_is_none() {
        assert!(shaper().resolve_font(&[]).is_none());
    }

    #[test]
    fn resolved_font_bytes_are_valid_font_data() {
        // skrifa（parley 的字体解析库依赖路径）能读取为合法 face
        let shaper = shaper();
        let resolved = shaper
            .resolve_font(&["serif".to_owned()])
            .expect("resolves");
        let face_count = skrifa::FontRef::from_index(resolved.data(), resolved.index())
            .map(|_| 1)
            .unwrap_or(0);
        assert_eq!(face_count, 1, "字节可被 skrifa 解析");
    }
}
