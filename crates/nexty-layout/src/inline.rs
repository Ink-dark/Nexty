//! 行内格式化：行内项收集、空白折叠、贪心断行与行盒组装。
//!
//! 行为 ground truth：[CSS 2.1 §9.2.2](https://www.w3.org/TR/CSS21/visuren.html#inline-formatting)
//! （行内格式化上下文）、[§10.8](https://www.w3.org/TR/CSS21/visudet.html#line-height)
//! （行高与 strut）、[CSS Text §3](https://drafts.csswg.org/css-text-3/#white-space-processing)
//! （空白折叠）与 [§7](https://drafts.csswg.org/css-text-3/#line-breaking)（断行）。
//!
//! 已知偏差：断行只在 ASCII 空白处（无 UAX#14 完整断点，CJK 不折行）；
//! `vertical-align` 仅 baseline；`display: inline-block` 暂按 `inline` 参与
//! 行内流。

use std::collections::HashMap;

use nexty_css::{ComputedStyle, DisplayValue, LineHeightValue};
use nexty_dom::{Document, NodeId, NodeKind};
use nexty_text::{FontMetrics, Glyph, ShapedText, TextShaper, TextStyle};

use crate::fragment::{LineFragment, TextRun};

/// 行内流中的一个词（空白折叠后）。
pub(crate) struct Word {
    /// 样式来源元素。
    pub node: NodeId,
    /// 该词前面是否隔着空白（行首的空白不渲染、不计宽）。
    pub preceded_by_space: bool,
    /// 词文本。
    pub text: String,
    /// 该词的样式（直接包含它的元素的 computed style）。
    pub style: ComputedStyle,
}

/// 行内项收集状态。
struct Collector<'a> {
    document: &'a Document,
    styles: &'a HashMap<NodeId, ComputedStyle>,
    words: Vec<Word>,
    pending_space: bool,
    word_buffer: String,
    word_node: Option<NodeId>,
    word_style: Option<ComputedStyle>,
}

impl Collector<'_> {
    /// 把缓冲中的词刷入结果。
    fn flush_word(&mut self) {
        if self.word_buffer.is_empty() {
            return;
        }
        self.words.push(Word {
            node: self.word_node.take().expect("word has a style source"),
            preceded_by_space: std::mem::take(&mut self.pending_space),
            text: std::mem::take(&mut self.word_buffer),
            style: self.word_style.take().expect("word has a style"),
        });
    }

    /// 收集一段文本（按当前样式）。
    fn push_text(&mut self, text: &str, node: NodeId, style: &ComputedStyle) {
        for character in text.chars() {
            if character.is_ascii_whitespace() {
                self.flush_word();
                self.pending_space = true;
            } else {
                if self.word_buffer.is_empty() {
                    self.word_node = Some(node);
                    self.word_style = Some(style.clone());
                }
                self.word_buffer.push(character);
            }
        }
    }
}

/// 收集 `node` 子树（块容器直接内容）的行内词。
///
/// `style` 是 `node` 自身的 computed style（其直接文本节点使用它）。
/// 行内级子元素递归进入；`display: none` 子树跳过；`display: contents`
/// 的盒子移除、其内容提升到当前行内流。
pub(crate) fn collect_inline_words(
    document: &Document,
    styles: &HashMap<NodeId, ComputedStyle>,
    node: NodeId,
    style: &ComputedStyle,
) -> Vec<Word> {
    let mut collector = Collector {
        document,
        styles,
        words: Vec::new(),
        pending_space: false,
        word_buffer: String::new(),
        word_node: None,
        word_style: None,
    };
    collect_children(&mut collector, node, style);
    collector.flush_word();
    collector.words
}

fn collect_children(collector: &mut Collector<'_>, node: NodeId, style: &ComputedStyle) {
    let document = collector.document;
    for child in document.children(node) {
        match document.node(child) {
            Some(NodeKind::Text(text)) => collector.push_text(text, node, style),
            Some(NodeKind::Element(_)) => {
                let Some(child_style) = collector.styles.get(&child) else {
                    continue;
                };
                match child_style.display {
                    DisplayValue::None => {}
                    // contents：盒子移除，内容提升（保留其样式供直接文本使用）
                    DisplayValue::Contents => {
                        collect_children(collector, child, child_style);
                    }
                    // 行内级（inline-block 暂按 inline 参与行内流，见模块偏差）
                    DisplayValue::Inline
                    | DisplayValue::InlineBlock
                    | DisplayValue::InlineFlex
                    | DisplayValue::InlineGrid
                    | DisplayValue::InlineTable => {
                        collect_children(collector, child, child_style);
                    }
                    // 块级子元素不属于行内流（由块级分组处理），跳过
                    _ => {}
                }
            }
            // 注释与处理指令不产生行内内容
            _ => {}
        }
    }
}

/// computed style → 文本层样式。
pub(crate) fn text_style(style: &ComputedStyle) -> TextStyle {
    TextStyle {
        families: style
            .font_family
            .iter()
            .map(|family| match family {
                nexty_css::FontFamilyValue::Generic(generic) => (*generic).to_owned(),
                nexty_css::FontFamilyValue::Family(name) => name.clone(),
            })
            .collect(),
        size: style.font_size,
    }
}

/// line-height 的 used value（CSS 2.1 §10.8.1；computed 阶段后仅
/// Normal/Number/Length 三种形态出现）。
pub(crate) fn used_line_height(style: &ComputedStyle, metrics: FontMetrics) -> f32 {
    match style.line_height {
        LineHeightValue::Normal => metrics.line_height,
        LineHeightValue::Number(factor) => factor * style.font_size,
        LineHeightValue::Length(px) => px,
        // computed 阶段已折算；防御性处理
        LineHeightValue::Percent(fraction) => fraction * style.font_size,
        LineHeightValue::Em(em) => em * style.font_size,
    }
}

/// 贪心断行与行盒组装。
///
/// `block_style` 提供 strut（行盒的高度下限与基线位置）。
pub(crate) fn build_lines(
    shaper: &dyn TextShaper,
    words: &[Word],
    max_width: f32,
    block_style: &ComputedStyle,
) -> Vec<LineFragment> {
    if words.is_empty() {
        return Vec::new();
    }

    let mut cache: HashMap<(NodeId, String), ShapedText> = HashMap::new();
    let mut space_cache: HashMap<NodeId, f32> = HashMap::new();

    // strut：块自身字体参与每个行盒（CSS 2.1 §10.8.1）
    let strut_metrics = shaper
        .metrics(&text_style(block_style))
        .unwrap_or(FontMetrics {
            ascent: 0.0,
            descent: 0.0,
            line_height: 0.0,
        });
    let strut_line_height = used_line_height(block_style, strut_metrics);
    let strut_half_leading =
        (strut_line_height - (strut_metrics.ascent + strut_metrics.descent)) / 2.0;
    let strut_above = strut_metrics.ascent + strut_half_leading;
    let strut_below = strut_metrics.descent + strut_half_leading;

    struct PlacedWord {
        word_index: usize,
        /// run 内 x 起点（相对行左缘，含前导空白宽度）。
        x: f32,
        shaped: ShapedText,
    }

    let mut lines: Vec<LineFragment> = Vec::new();
    let mut placed: Vec<PlacedWord> = Vec::new();
    let mut cursor = 0.0;

    let flush = |placed: &mut Vec<PlacedWord>, lines: &mut Vec<LineFragment>| {
        if placed.is_empty() {
            return;
        }
        // 行盒几何：baseline = max(各 run 上半部, strut 上半部)（CSS 2.1 §10.8）
        let mut baseline = strut_above;
        let mut below = strut_below;
        for word in placed
            .iter()
            .map(|placed_word| &words[placed_word.word_index])
        {
            let metrics = shaper
                .metrics(&text_style(&word.style))
                .unwrap_or(FontMetrics {
                    ascent: 0.0,
                    descent: 0.0,
                    line_height: 0.0,
                });
            let line_height = used_line_height(&word.style, metrics);
            let half_leading = (line_height - (metrics.ascent + metrics.descent)) / 2.0;
            baseline = baseline.max(metrics.ascent + half_leading);
            below = below.max(metrics.descent + half_leading);
        }

        let mut runs: Vec<TextRun> = Vec::new();
        for placed_word in placed.iter() {
            let word = &words[placed_word.word_index];
            let glyphs: Vec<Glyph> = placed_word
                .shaped
                .glyphs
                .iter()
                .map(|glyph| Glyph {
                    id: glyph.id,
                    x: glyph.x + placed_word.x,
                    y: glyph.y,
                    advance: glyph.advance,
                })
                .collect();
            match runs.last_mut() {
                // 相邻同源词合并进同一 run
                Some(run) if run.node == word.node => run.glyphs.extend(glyphs),
                _ => runs.push(TextRun {
                    node: word.node,
                    style: text_style(&word.style),
                    color: word.style.color,
                    glyphs,
                }),
            }
        }
        lines.push(LineFragment {
            baseline,
            height: baseline + below,
            runs,
        });
        placed.clear();
    };

    for (index, word) in words.iter().enumerate() {
        let shaped_word = cache
            .entry((word.node, word.text.clone()))
            .or_insert_with(|| {
                shaper
                    .shape(&word.text, &text_style(&word.style))
                    .unwrap_or(ShapedText {
                        glyphs: Vec::new(),
                        width: 0.0,
                        height: 0.0,
                    })
            })
            .clone();

        // 词前空白：行首的丢弃（CSS Text §3），行中的按前一词样式计宽
        let space_width = if word.preceded_by_space && !placed.is_empty() {
            let previous = &words[placed.last().expect("non-empty").word_index];
            *space_cache.entry(previous.node).or_insert_with(|| {
                shaper
                    .shape(" ", &text_style(&previous.style))
                    .map(|shaped| shaped.width)
                    .unwrap_or(0.0)
            })
        } else {
            0.0
        };

        // 断行：行上有内容且放不下 → 换行（行首超长词直接溢出放置）
        if !placed.is_empty() && cursor + space_width + shaped_word.width > max_width {
            flush(&mut placed, &mut lines);
            cursor = 0.0;
        }

        let word_width = shaped_word.width;
        placed.push(PlacedWord {
            word_index: index,
            x: cursor + space_width,
            shaped: shaped_word,
        });
        cursor += space_width + word_width;
    }
    flush(&mut placed, &mut lines);
    lines
}
