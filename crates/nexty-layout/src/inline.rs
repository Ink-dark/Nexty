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
use nexty_dom::{Document, ElementData, Namespace, NodeId, NodeKind};
use nexty_text::{FontMetrics, Glyph, ShapedText, TextShaper, TextStyle};

use crate::fragment::{ImageRun, LineFragment, TextRun};

/// 行内流中的一个原子图片项（替换元素，v1 仅 `<img>`）。
pub(crate) struct AtomicImage {
    /// 图片来源元素。
    pub node: NodeId,
    /// 显示宽。
    pub width: f32,
    /// 显示高。
    pub height: f32,
    /// 该项前面是否隔着空白（行首空白不渲染）。
    pub preceded_by_space: bool,
    /// 样式来源（其后空白按它计宽）。
    pub style: ComputedStyle,
}

/// 行内流中的一项：文本词或原子图片。
pub(crate) enum InlineItem {
    Text(Word),
    Image(AtomicImage),
}

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

/// `<img>` 的盒尺寸（CSS px）。
///
/// 逐维独立解析：`width` / `height` 属性优先，缺失回退自然尺寸（已解码
/// 图片的像素尺寸），再回退 CSS Images §5.1 的默认对象尺寸 300×150。
/// 偏差：单边指定时不按内在比例保持，宽高比可能失真。
pub(crate) fn image_box_size(
    document: &Document,
    node: NodeId,
    image_sizes: &HashMap<NodeId, (f32, f32)>,
) -> (f32, f32) {
    let (natural_width, natural_height) = image_sizes.get(&node).copied().unwrap_or((300.0, 150.0));
    let Some(NodeKind::Element(data)) = document.node(node) else {
        return (natural_width, natural_height);
    };
    let width = attribute_px(data, "width").unwrap_or(natural_width);
    let height = attribute_px(data, "height").unwrap_or(natural_height);
    (width, height)
}

/// 尺寸属性 → px（HTML 尺寸属性是无符号整数；宽松解析前导数字）。
fn attribute_px(data: &ElementData, name: &str) -> Option<f32> {
    let raw = data
        .attributes
        .iter()
        .find(|attr| attr.name == name)?
        .value
        .trim();
    let digits: String = raw
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    digits.parse::<f32>().ok().filter(|value| *value > 0.0)
}

/// `node` 是否为 HTML 命名空间的 `<img>`。
fn is_image_node(document: &Document, node: NodeId) -> bool {
    matches!(
        document.node(node),
        Some(NodeKind::Element(data))
            if data.namespace == Namespace::Html && data.name == "img"
    )
}

/// 行内项收集状态。
struct Collector<'a> {
    document: &'a Document,
    styles: &'a HashMap<NodeId, ComputedStyle>,
    image_sizes: &'a HashMap<NodeId, (f32, f32)>,
    items: Vec<InlineItem>,
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
        self.items.push(InlineItem::Text(Word {
            node: self.word_node.take().expect("word has a style source"),
            preceded_by_space: std::mem::take(&mut self.pending_space),
            text: std::mem::take(&mut self.word_buffer),
            style: self.word_style.take().expect("word has a style"),
        }));
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

    /// 收集一个行内原子图片。
    fn push_image(&mut self, node: NodeId, style: &ComputedStyle) {
        let (width, height) = image_box_size(self.document, node, self.image_sizes);
        self.flush_word();
        self.items.push(InlineItem::Image(AtomicImage {
            node,
            width,
            height,
            preceded_by_space: std::mem::take(&mut self.pending_space),
            style: style.clone(),
        }));
    }
}

/// 一段连续行内流的收集器。
///
/// 块容器按树序把行内内容喂给它：直接文本子节点走 [`InlineRun::push_text`]
/// （不递归子树），行内级子元素走 [`InlineRun::push_element`]（递归其子树）。
/// 空白折叠状态（词缓冲、前导空白）跨子节点持续，保证
/// `text <b>bold</b>` 这类跨界序列不重复收集、不丢空白。
pub(crate) struct InlineRun<'a> {
    collector: Collector<'a>,
}

impl<'a> InlineRun<'a> {
    pub(crate) fn new(
        document: &'a Document,
        styles: &'a HashMap<NodeId, ComputedStyle>,
        image_sizes: &'a HashMap<NodeId, (f32, f32)>,
    ) -> Self {
        Self {
            collector: Collector {
                document,
                styles,
                image_sizes,
                items: Vec::new(),
                pending_space: false,
                word_buffer: String::new(),
                word_node: None,
                word_style: None,
            },
        }
    }

    /// 收集一个直接文本子节点（不递归）。
    pub(crate) fn push_text(&mut self, text: &str, node: NodeId, style: &ComputedStyle) {
        self.collector.push_text(text, node, style);
    }

    /// 收集一个行内级子元素的子树。
    ///
    /// 元素本身是行内 `<img>` 时按原子图片收集（其子树只有 alt 文本，
    /// v1 不做失败回退渲染）。
    pub(crate) fn push_element(&mut self, element: NodeId, style: &ComputedStyle) {
        if is_image_node(self.collector.document, element) {
            self.collector.push_image(element, style);
            return;
        }
        collect_children(&mut self.collector, element, style);
    }

    /// 结束收集，刷出词缓冲。
    pub(crate) fn finish(mut self) -> Vec<InlineItem> {
        self.collector.flush_word();
        self.collector.items
    }
}

fn collect_children(collector: &mut Collector<'_>, node: NodeId, style: &ComputedStyle) {
    let document = collector.document;
    for child in document.children(node) {
        match document.node(child) {
            Some(NodeKind::Text(text)) => collector.push_text(text, node, style),
            Some(NodeKind::Element(data)) => {
                let Some(child_style) = collector.styles.get(&child) else {
                    continue;
                };
                if child_style.display == DisplayValue::None {
                    continue;
                }
                // 行内替换元素：原子图片不进入子树（子树只有 alt 文本，
                // v1 不做失败回退渲染）
                if data.namespace == Namespace::Html
                    && data.name == "img"
                    && child_style.display == DisplayValue::Inline
                {
                    collector.push_image(child, child_style);
                    continue;
                }
                match child_style.display {
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
/// `block_style` 提供 strut（行盒的高度下限与基线位置）。原子图片按基线
/// 对齐（底边落在基线上，CSS 2.1 §10.8.1），不参与断行（不可拆）。
pub(crate) fn build_lines(
    shaper: &dyn TextShaper,
    items: &[InlineItem],
    max_width: f32,
    block_style: &ComputedStyle,
) -> Vec<LineFragment> {
    if items.is_empty() {
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

    struct PlacedItem {
        item_index: usize,
        /// 行内 x 起点（相对行左缘，含前导空白宽度）。
        x: f32,
        /// 文本项的整形结果；原子图片无。
        shaped: Option<ShapedText>,
    }

    let mut lines: Vec<LineFragment> = Vec::new();
    let mut placed: Vec<PlacedItem> = Vec::new();
    let mut cursor = 0.0;

    let flush = |placed: &mut Vec<PlacedItem>, lines: &mut Vec<LineFragment>| {
        if placed.is_empty() {
            return;
        }
        // 行盒几何：baseline = max(各 run 上半部, strut 上半部)（CSS 2.1 §10.8）
        let mut baseline = strut_above;
        let mut below = strut_below;
        for placed_item in placed.iter() {
            match &items[placed_item.item_index] {
                InlineItem::Text(word) => {
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
                // 替换元素底边对齐基线：整个盒高在基线之上
                InlineItem::Image(image) => baseline = baseline.max(image.height),
            }
        }

        let mut runs: Vec<TextRun> = Vec::new();
        let mut images: Vec<ImageRun> = Vec::new();
        // 同源词合并进同一 run，但不得跨越中间的图片（绘制顺序会被打乱）
        let mut mergeable = false;
        for placed_item in placed.iter() {
            match &items[placed_item.item_index] {
                InlineItem::Text(word) => {
                    let shaped = placed_item
                        .shaped
                        .as_ref()
                        .expect("text item carries shaped glyphs");
                    let glyphs: Vec<Glyph> = shaped
                        .glyphs
                        .iter()
                        .map(|glyph| Glyph {
                            id: glyph.id,
                            x: glyph.x + placed_item.x,
                            y: glyph.y,
                            advance: glyph.advance,
                        })
                        .collect();
                    match runs.last_mut() {
                        Some(run) if mergeable && run.node == word.node => {
                            run.glyphs.extend(glyphs)
                        }
                        _ => runs.push(TextRun {
                            node: word.node,
                            style: text_style(&word.style),
                            color: word.style.color,
                            glyphs,
                        }),
                    }
                    mergeable = true;
                }
                InlineItem::Image(image) => {
                    images.push(ImageRun {
                        node: image.node,
                        x: placed_item.x,
                        y: baseline - image.height,
                        width: image.width,
                        height: image.height,
                    });
                    mergeable = false;
                }
            }
        }
        lines.push(LineFragment {
            baseline,
            height: baseline + below,
            runs,
            images,
        });
        placed.clear();
    };

    /// 前一项之后、当前项之前的空白宽度（按前一项样式计宽）。
    fn space_after_previous(
        items: &[InlineItem],
        placed: &[PlacedItem],
        space_cache: &mut HashMap<NodeId, f32>,
        shaper: &dyn TextShaper,
    ) -> f32 {
        let previous = &items[placed.last().expect("non-empty").item_index];
        let (node, style) = match previous {
            InlineItem::Text(word) => (word.node, &word.style),
            InlineItem::Image(image) => (image.node, &image.style),
        };
        *space_cache.entry(node).or_insert_with(|| {
            shaper
                .shape(" ", &text_style(style))
                .map(|shaped| shaped.width)
                .unwrap_or(0.0)
        })
    }

    for (index, item) in items.iter().enumerate() {
        let preceded_by_space = match item {
            InlineItem::Text(word) => word.preceded_by_space,
            InlineItem::Image(image) => image.preceded_by_space,
        };
        let space_width = if preceded_by_space && !placed.is_empty() {
            space_after_previous(items, &placed, &mut space_cache, shaper)
        } else {
            0.0
        };

        let item_width = match item {
            InlineItem::Text(word) => {
                let shaped = cache
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
                // 断行：行上有内容且放不下 → 换行（行首超长词直接溢出放置）
                let needs_break =
                    !placed.is_empty() && cursor + space_width + shaped.width > max_width;
                if needs_break {
                    flush(&mut placed, &mut lines);
                    cursor = 0.0;
                }
                let word_width = shaped.width;
                placed.push(PlacedItem {
                    item_index: index,
                    x: cursor + space_width,
                    shaped: Some(shaped),
                });
                word_width
            }
            InlineItem::Image(image) => {
                // 图片不可拆：放不下先换行，行首仍放不下则溢出放置
                let needs_break =
                    !placed.is_empty() && cursor + space_width + image.width > max_width;
                if needs_break {
                    flush(&mut placed, &mut lines);
                    cursor = 0.0;
                }
                placed.push(PlacedItem {
                    item_index: index,
                    x: cursor + space_width,
                    shaped: None,
                });
                image.width
            }
        };
        cursor += space_width + item_width;
    }
    flush(&mut placed, &mut lines);
    lines
}
