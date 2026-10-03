//! 内在尺寸测量：shrink-to-fit（CSS 2.1 §10.3.7）与 flex-basis: auto
//! 所需的 min-content / max-content 宽度。
//!
//! 只测量不布局：文本按词整形、替换元素取盒尺寸、行内原子盒递归测量。
//! 偏差：`display: contents` 的提升近似为行内透明穿透；浮动与绝对定位
//! 未实现因此不涉及。

use std::collections::HashMap;

use nexty_css::{ComputedStyle, DisplayValue};
use nexty_dom::{Document, Namespace, NodeId, NodeKind};
use nexty_text::ShapedText;

use crate::block::Context;
use crate::inline::image_box_size;

/// 一个盒的内容内在宽度（CSS px，不含自身 border/padding/margin）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Intrinsic {
    /// min-content：不可拆的最宽单元（最长词 / 最宽图 / 最宽块级子盒）。
    pub min: f32,
    /// max-content：不折行时的总宽。
    pub max: f32,
}

/// 测量 `node`（块容器或行内原子盒）的内容内在宽度。
pub(crate) fn measure_box(ctx: &Context<'_>, node: NodeId, style: &ComputedStyle) -> Intrinsic {
    let mut walk = MeasureWalk {
        ctx,
        text_cache: HashMap::new(),
    };
    walk.container(node, style)
}

struct MeasureWalk<'a> {
    ctx: &'a Context<'a>,
    /// 同一节点同文本的整形缓存（测量对同一文本只做一次）。
    text_cache: HashMap<(NodeId, String), ShapedText>,
}

impl MeasureWalk<'_> {
    /// 块容器：行内序列累计与块级子盒取大。
    fn container(&mut self, node: NodeId, style: &ComputedStyle) -> Intrinsic {
        let mut sequence = 0.0_f32; // 当前行内序列累计宽
        let mut sequence_max = 0.0_f32; // 已结束序列的最大累计
        let mut block_max = 0.0_f32; // 块级子盒 max-content 最大值
        let mut min = 0.0_f32;

        for child in self.ctx.document.children(node) {
            match self.ctx.document.node(child) {
                Some(NodeKind::Text(text)) => {
                    // 直接文本：样式取父元素
                    let measured = self.text(text, node, style);
                    sequence += measured.max;
                    min = min.max(measured.min);
                }
                Some(NodeKind::Element(_)) => {
                    let Some(child_style) = self.ctx.styles.get(&child) else {
                        continue;
                    };
                    match child_style.display {
                        DisplayValue::None => {}
                        DisplayValue::Contents => {
                            // 偏差：contents 提升近似为穿透
                            let inner = self.container(child, child_style);
                            sequence += inner.max;
                            min = min.max(inner.min);
                        }
                        DisplayValue::Inline => {
                            let inner = self.inline_subtree(child, child_style);
                            sequence += inner.max;
                            min = min.max(inner.min);
                        }
                        DisplayValue::InlineBlock
                        | DisplayValue::InlineFlex
                        | DisplayValue::InlineTable => {
                            let inner = self.atomic_box(child, child_style);
                            sequence += inner.max;
                            min = min.max(inner.min);
                        }
                        // 块级子盒：纵向堆叠，序列被打断，宽度取大
                        _ => {
                            sequence_max = sequence_max.max(sequence);
                            sequence = 0.0;
                            let inner = self.atomic_box(child, child_style);
                            block_max = block_max.max(inner.max);
                            min = min.max(inner.min);
                        }
                    }
                }
                _ => {}
            }
        }
        sequence_max = sequence_max.max(sequence);
        Intrinsic {
            min,
            max: sequence_max.max(block_max),
        }
    }

    /// 行内子树：文本与行内替换元素累计，块级子盒取大（近似）。
    fn inline_subtree(&mut self, node: NodeId, style: &ComputedStyle) -> Intrinsic {
        let mut sequence = 0.0_f32;
        let mut min = 0.0_f32;
        for child in self.ctx.document.children(node) {
            match self.ctx.document.node(child) {
                Some(NodeKind::Text(text)) => {
                    let measured = self.text(text, node, style);
                    sequence += measured.max;
                    min = min.max(measured.min);
                }
                Some(NodeKind::Element(_)) => {
                    let Some(child_style) = self.ctx.styles.get(&child) else {
                        continue;
                    };
                    match child_style.display {
                        DisplayValue::None => {}
                        DisplayValue::Contents | DisplayValue::Inline => {
                            let inner = self.inline_subtree(child, child_style);
                            sequence += inner.max;
                            min = min.max(inner.min);
                        }
                        DisplayValue::InlineBlock
                        | DisplayValue::InlineFlex
                        | DisplayValue::InlineTable => {
                            let inner = self.atomic_box(child, child_style);
                            sequence += inner.max;
                            min = min.max(inner.min);
                        }
                        // 行内流里的块级子盒由块级分组承接；测量端忽略（偏差）
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        Intrinsic { min, max: sequence }
    }

    /// 一个原子盒（含自身 border/padding/margin 水平量）。
    fn atomic_box(&mut self, node: NodeId, style: &ComputedStyle) -> Intrinsic {
        // 替换元素：尺寸直接来自属性/自然尺寸
        if is_image(self.ctx.document, node) {
            let (width, _) = image_box_size(self.ctx.document, node, self.ctx.image_sizes);
            return Intrinsic {
                min: width,
                max: width,
            };
        }
        let margin_left = used_margin(style.margin.left);
        let margin_right = used_margin(style.margin.right);
        let fixed = horizontal_fixed(style);
        let inner = self.container(node, style);
        Intrinsic {
            min: inner.min + fixed + margin_left + margin_right,
            max: inner.max + fixed + margin_left + margin_right,
        }
    }

    /// 一段文本：返回内在尺寸（max = 不折行序列总宽，min = 最长词）。
    fn text(&mut self, text: &str, node: NodeId, style: &ComputedStyle) -> Intrinsic {
        let mut sequence = 0.0_f32;
        let mut min = 0.0_f32;
        let mut pending_space = false;
        let mut word = String::new();
        for character in text.chars() {
            if character.is_ascii_whitespace() {
                if !word.is_empty() {
                    // 词前空白（跨节点折叠成一个）先行计入
                    if pending_space {
                        sequence += self.space_width(node, style);
                    }
                    let width = self.word_width(node, style, &word);
                    sequence += width;
                    min = min.max(width);
                    word.clear();
                }
                pending_space = true;
            } else {
                word.push(character);
            }
        }
        if !word.is_empty() {
            if pending_space {
                sequence += self.space_width(node, style);
            }
            let width = self.word_width(node, style, &word);
            sequence += width;
            min = min.max(width);
        }
        Intrinsic { min, max: sequence }
    }

    fn word_width(&mut self, node: NodeId, style: &ComputedStyle, word: &str) -> f32 {
        self.text_cache
            .entry((node, word.to_owned()))
            .or_insert_with(|| {
                self.ctx
                    .shaper
                    .shape(word, &crate::inline::text_style(style))
                    .unwrap_or(ShapedText {
                        glyphs: Vec::new(),
                        width: 0.0,
                        height: 0.0,
                    })
            })
            .width
    }

    fn space_width(&mut self, node: NodeId, style: &ComputedStyle) -> f32 {
        self.word_width(node, style, " ")
    }
}

/// margin 的 used value（auto → 0；测量端不解析百分比基准）。
fn used_margin(value: nexty_css::MarginValue) -> f32 {
    match value {
        nexty_css::MarginValue::Length(px) => px,
        nexty_css::MarginValue::Percent(fraction) => fraction,
        nexty_css::MarginValue::Auto => 0.0,
    }
}

/// 自身水平 border + padding（测量端 padding 百分比无基准，按已解析的
/// 数值近似：padding 存的是 PaddingValue，这里仅取长度；百分比记 0）。
fn horizontal_fixed(style: &ComputedStyle) -> f32 {
    let padding = |value: nexty_css::PaddingValue| match value {
        nexty_css::PaddingValue::Length(px) => px,
        nexty_css::PaddingValue::Percent(_) => 0.0,
    };
    style.border_width.left
        + padding(style.padding.left)
        + style.border_width.right
        + padding(style.padding.right)
}

/// `node` 是否为 HTML 命名空间的 `<img>`。
fn is_image(document: &Document, node: NodeId) -> bool {
    matches!(
        document.node(node),
        Some(NodeKind::Element(data))
            if data.namespace == Namespace::Html && data.name == "img"
    )
}
