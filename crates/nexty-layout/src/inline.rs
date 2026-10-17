//! 行内格式化：行内项收集、空白折叠、贪心断行与行盒组装。
//!
//! 行为 ground truth：[CSS 2.1 §9.2.2](https://www.w3.org/TR/CSS21/visuren.html#inline-formatting)
//! （行内格式化上下文）、[§10.8](https://www.w3.org/TR/CSS21/visudet.html#line-height)
//! （行高与 strut）、[CSS Text §3](https://drafts.csswg.org/css-text-3/#white-space-processing)
//! （空白折叠）与 [§7](https://drafts.csswg.org/css-text-3/#line-breaking)（断行）。
//!
//! 已知偏差：行内贪心断行（`build_lines`）仍按 ASCII 空白切词；UAX#14
//! 断点、glyph cluster 映射与 min-content 由文件末尾的 conformance API
//! （`break_opportunities` / `cluster_map` / `min_content_width` /
//! `split_spans`）提供，尚未接入 `build_lines`。
//! `vertical-align` 仅 baseline（原子盒/图片底边对齐基线）。
//!
//! feature `block` 关闭时本模块无消费方（行内流的唯一下游是 taffy 管线），
//! 按模块整体抑制 dead_code。

#![cfg_attr(not(feature = "block"), allow(dead_code))]

use std::collections::HashMap;

use nexty_css::{ComputedStyle, DisplayValue, LineHeightValue};
use nexty_dom::{Document, ElementData, Namespace, NodeId, NodeKind};
use nexty_text::{FontMetrics, Glyph, ResolvedFont, ShapedText, TextShaper, TextStyle};

use crate::Context;
use crate::fragment::{Fragment, ImageRun, LineFragment, TextRun};

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

/// 行内流中的一个原子盒（`inline-block` 等）：片段在 run 叶的 measure 期按
/// taffy 给出的可用宽惰性布局（见 `tree_build::InlineRunNode`），收集期仅登记
/// 节点与样式（占位片段的 `node`/`style` 字段供后续布局使用）。
pub(crate) struct AtomicBlock {
    /// 布局完成的子片段（border_box.x/y 由行盒组装阶段定位）。
    pub fragment: Fragment,
    /// 该项前面是否隔着空白（行首空白不渲染）。
    pub preceded_by_space: bool,
    /// 样式来源（其后空白按它计宽）。
    pub style: ComputedStyle,
}

/// 行内流中的一项：文本词、原子图片或原子盒。
pub(crate) enum InlineItem {
    Text(Word),
    Image(AtomicImage),
    Block(Box<AtomicBlock>),
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
    ctx: &'a Context<'a>,
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
        let (width, height) = image_box_size(self.ctx.document, node, self.ctx.image_sizes);
        self.flush_word();
        self.items.push(InlineItem::Image(AtomicImage {
            node,
            width,
            height,
            preceded_by_space: std::mem::take(&mut self.pending_space),
            style: style.clone(),
        }));
    }

    /// 收集一个原子盒（inline-block 等）：登记节点与样式，布局延迟到
    /// run 叶的 measure 期（届时才知道 taffy 分配的可用宽）。
    fn push_atomic_block(&mut self, node: NodeId, style: &ComputedStyle) {
        self.flush_word();
        self.items.push(InlineItem::Block(Box::new(AtomicBlock {
            fragment: Fragment {
                node,
                anonymous: false,
                border_box: crate::fragment::Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                },
                border: Default::default(),
                padding: Default::default(),
                style: style.clone(),
                children: Vec::new(),
                lines: Vec::new(),
            },
            preceded_by_space: std::mem::take(&mut self.pending_space),
            style: style.clone(),
        })));
    }
}

/// 一段连续行内流的收集器。
///
/// 块容器按树序把行内内容喂给它：直接文本子节点走 [`InlineRun::push_text`]
/// （不递归子树），行内级子元素走 [`InlineRun::push_element`]（递归其子树，
/// 原子行内盒则独立布局）。空白折叠状态（词缓冲、前导空白）跨子节点持续，
/// 保证 `text <b>bold</b>` 这类跨界序列不重复收集、不丢空白。
pub(crate) struct InlineRun<'a> {
    collector: Collector<'a>,
}

impl<'a> InlineRun<'a> {
    pub(crate) fn new(ctx: &'a Context<'a>) -> Self {
        Self {
            collector: Collector {
                ctx,
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
    /// v1 不做失败回退渲染）；原子行内盒（inline-block 等）登记为原子项，
    /// 在 run 叶的 measure 期布局。空白折叠状态（词缓冲、前导空白）跨子节点
    /// 持续，保证 `text <b>bold</b>` 这类跨界序列不重复收集、不丢空白。
    pub(crate) fn push_element(&mut self, element: NodeId, style: &ComputedStyle) {
        if is_image_node(self.collector.ctx.document, element) {
            self.collector.push_image(element, style);
            return;
        }
        match style.display {
            DisplayValue::InlineBlock | DisplayValue::InlineFlex | DisplayValue::InlineTable => {
                self.collector.push_atomic_block(element, style);
            }
            _ => collect_children(&mut self.collector, element, style),
        }
    }

    /// 结束收集，刷出词缓冲。
    pub(crate) fn finish(mut self) -> Vec<InlineItem> {
        self.collector.flush_word();
        self.collector.items
    }
}

fn collect_children(collector: &mut Collector<'_>, node: NodeId, style: &ComputedStyle) {
    let document = collector.ctx.document;
    for child in document.children(node) {
        match document.node(child) {
            Some(NodeKind::Text(text)) => collector.push_text(text, node, style),
            Some(NodeKind::Element(data)) => {
                let Some(child_style) = collector.ctx.styles.get(&child) else {
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
                    // 原子行内盒：独立布局，不递归进其子树
                    DisplayValue::InlineBlock
                    | DisplayValue::InlineFlex
                    | DisplayValue::InlineTable => {
                        collector.push_atomic_block(child, child_style);
                    }
                    // 行内级：递归进入
                    DisplayValue::Inline => {
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
                // 原子盒底边对齐基线（v1 简化：不含 margin）
                InlineItem::Block(atomic) => {
                    baseline = baseline.max(atomic.fragment.border_box.height)
                }
            }
        }

        let mut runs: Vec<TextRun> = Vec::new();
        let mut images: Vec<ImageRun> = Vec::new();
        let mut boxes: Vec<Fragment> = Vec::new();
        // 同源词合并进同一 run，但不得跨越中间的图片（绘制顺序会被打乱）
        let mut mergeable = false;
        // 把字形（font 指向源 ShapedText 的字体表）并入 run：按实例 id
        // 重映射到 run 自己的字体表（跨 run 合并时避免字节比较）
        fn remap_glyph(
            mut glyph: Glyph,
            source_fonts: &[ResolvedFont],
            run_fonts: &mut Vec<ResolvedFont>,
        ) -> Glyph {
            let source = &source_fonts[glyph.font as usize];
            let index = run_fonts
                .iter()
                .position(|font| font.id() == source.id())
                .unwrap_or_else(|| {
                    run_fonts.push(source.clone());
                    run_fonts.len() - 1
                });
            glyph.font = index as u32;
            glyph
        }
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
                            font: glyph.font,
                            cluster: glyph.cluster,
                        })
                        .collect();
                    match runs.last_mut() {
                        Some(run) if mergeable && run.node == word.node => {
                            for glyph in glyphs {
                                run.glyphs
                                    .push(remap_glyph(glyph, &shaped.fonts, &mut run.fonts));
                            }
                        }
                        _ => runs.push(TextRun {
                            node: word.node,
                            style: text_style(&word.style),
                            color: word.style.color,
                            fonts: shaped.fonts.clone(),
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
                InlineItem::Block(atomic) => {
                    let mut fragment = atomic.fragment.clone();
                    // 底边对齐基线：x/y 相对所属片段内容盒
                    fragment.border_box.x = placed_item.x;
                    fragment.border_box.y = baseline - fragment.border_box.height;
                    boxes.push(fragment);
                    mergeable = false;
                }
            }
        }
        lines.push(LineFragment {
            baseline,
            height: baseline + below,
            runs,
            images,
            boxes,
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
            InlineItem::Block(atomic) => (atomic.fragment.node, &atomic.style),
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
            InlineItem::Block(atomic) => atomic.preceded_by_space,
        };
        let space_width = if preceded_by_space && !placed.is_empty() {
            space_after_previous(items, &placed, &mut space_cache, shaper)
        } else {
            0.0
        };

        // (项宽, 是否发生了断行)：断行后本项是新行首项，前导空白不落入新行
        let (item_width, broke) = match item {
            InlineItem::Text(word) => {
                let shaped = cache
                    .entry((word.node, word.text.clone()))
                    .or_insert_with(|| {
                        shaper
                            .shape(&word.text, &text_style(&word.style))
                            .unwrap_or(ShapedText {
                                glyphs: Vec::new(),
                                fonts: Vec::new(),
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
                    x: cursor + if needs_break { 0.0 } else { space_width },
                    shaped: Some(shaped),
                });
                (word_width, needs_break)
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
                    x: cursor + if needs_break { 0.0 } else { space_width },
                    shaped: None,
                });
                (image.width, needs_break)
            }
            InlineItem::Block(atomic) => {
                // 原子盒不可拆：放不下先换行，行首仍放不下则溢出放置
                let width = atomic.fragment.border_box.width;
                let needs_break = !placed.is_empty() && cursor + space_width + width > max_width;
                if needs_break {
                    flush(&mut placed, &mut lines);
                    cursor = 0.0;
                }
                placed.push(PlacedItem {
                    item_index: index,
                    x: cursor + if needs_break { 0.0 } else { space_width },
                    shaped: None,
                });
                (width, needs_break)
            }
        };
        cursor += if broke { 0.0 } else { space_width } + item_width;
    }
    flush(&mut placed, &mut lines);
    lines
}

/// 一行行内内容的宽度（取文本 / 图片 / 原子盒右缘最大值）。
pub(crate) fn line_width(line: &LineFragment) -> f32 {
    let mut width = 0.0_f32;
    for run in &line.runs {
        if let Some(glyph) = run.glyphs.last() {
            width = width.max(glyph.x + glyph.advance);
        }
    }
    for image in &line.images {
        width = width.max(image.x + image.width);
    }
    for atomic in &line.boxes {
        width = width.max(atomic.border_box.x + atomic.border_box.width);
    }
    width
}

// ---------------------------------------------------------------------------
// UAX#14 断行机会 / cluster 映射 / min-content（tests/inline_conformance.rs）
// ---------------------------------------------------------------------------

/// 断行一致性 API。
///
/// 这里自研 UAX#14 规则引擎而不是继续使用 `unicode-linebreak`：后者 0.1.5
/// 的 Unicode 数据停在 15.0，对 Unicode 18.0.0 官方语料会有 1,117 条差异
/// （HH 类、AK/AP/AS/VF/VI、LB12a/LB15c/LB15d/LB19a/LB25/LB28a 等新规则）。
/// 规则与类数据以 Unicode 18.0.0 的 UAX #14 与 UCD 数据文件为准，
/// 数据来源见 `src/data/README.md`。
#[cfg(feature = "inline")]
mod conformance {
    use std::collections::BTreeSet;
    use std::sync::OnceLock;

    use nexty_text::ShapedText;
    use unicode_segmentation::UnicodeSegmentation;

    /// 源文本中的一段字节区间。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Span {
        /// 起始字节偏移（含）。
        pub byte_start: usize,
        /// 结束字节偏移（不含）。
        pub byte_end: usize,
    }

    /// CSS `word-break`（CSS Text 3 §5.1）。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub enum WordBreak {
        /// 按 UAX#14 的常规断点。
        #[default]
        Normal,
        /// 允许在单词内部断行（但不切开字形簇）。
        BreakAll,
        /// 禁止在单词内部断行。
        KeepAll,
    }

    /// CSS `overflow-wrap`（CSS Text 3 §5.4）。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub enum OverflowWrap {
        /// 只在 UAX#14 允许的位置断行。
        #[default]
        Normal,
        /// 溢出时可在任意字形簇之间断行；不参与 min-content。
        BreakWord,
        /// 溢出时可在任意字形簇之间断行；参与 min-content。
        Anywhere,
    }

    /// 断行选项。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct BreakOptions {
        /// CSS `word-break`。
        pub word_break: WordBreak,
        /// CSS `overflow-wrap`。
        pub overflow_wrap: OverflowWrap,
    }

    // -----------------------------------------------------------------------
    // 数据表（Unicode 18.0.0）
    // -----------------------------------------------------------------------

    const LIMIT: usize = 0x11_0000;

    const LINE_BREAK_DATA: &str = include_str!("data/DerivedLineBreak-18.0.0.txt");
    const EAST_ASIAN_DATA: &str = include_str!("data/EastAsianWidth-18.0.0.txt");
    const PI_PF_DATA: &str = include_str!("data/PiPf-18.0.0.txt");
    const COMBINING_DATA: &str = include_str!("data/Combining-18.0.0.txt");
    const EXT_PICT_DATA: &str = include_str!("data/ExtPict-18.0.0.txt");
    const UNASSIGNED_DATA: &str = include_str!("data/Unassigned-18.0.0.txt");

    /// UAX#14 的 Line_Break 类（Unicode 18.0.0，含 AK/AP/AS/VF/VI/HH）。
    #[allow(clippy::upper_case_acronyms)] // 类名沿用 UAX#14 的缩写（ZWJ/BK/...）
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum LineBreakClass {
        BK,
        CR,
        LF,
        CM,
        NL,
        SG,
        WJ,
        ZW,
        GL,
        SP,
        ZWJ,
        B2,
        BA,
        BB,
        HY,
        CB,
        CL,
        CP,
        EX,
        IN,
        NS,
        OP,
        QU,
        IS,
        NU,
        PO,
        PR,
        SY,
        AI,
        AL,
        CJ,
        EB,
        EM,
        H2,
        H3,
        HL,
        ID,
        JL,
        JV,
        JT,
        RI,
        SA,
        XX,
        AK,
        AP,
        AS,
        VF,
        VI,
        HH,
    }

    impl LineBreakClass {
        fn from_name(name: &str) -> Option<Self> {
            Some(match name {
                "BK" => Self::BK,
                "CR" => Self::CR,
                "LF" => Self::LF,
                "CM" => Self::CM,
                "NL" => Self::NL,
                "SG" => Self::SG,
                "WJ" => Self::WJ,
                "ZW" => Self::ZW,
                "GL" => Self::GL,
                "SP" => Self::SP,
                "ZWJ" => Self::ZWJ,
                "B2" => Self::B2,
                "BA" => Self::BA,
                "BB" => Self::BB,
                "HY" => Self::HY,
                "CB" => Self::CB,
                "CL" => Self::CL,
                "CP" => Self::CP,
                "EX" => Self::EX,
                "IN" => Self::IN,
                "NS" => Self::NS,
                "OP" => Self::OP,
                "QU" => Self::QU,
                "IS" => Self::IS,
                "NU" => Self::NU,
                "PO" => Self::PO,
                "PR" => Self::PR,
                "SY" => Self::SY,
                "AI" => Self::AI,
                "AL" => Self::AL,
                "CJ" => Self::CJ,
                "EB" => Self::EB,
                "EM" => Self::EM,
                "H2" => Self::H2,
                "H3" => Self::H3,
                "HL" => Self::HL,
                "ID" => Self::ID,
                "JL" => Self::JL,
                "JV" => Self::JV,
                "JT" => Self::JT,
                "RI" => Self::RI,
                "SA" => Self::SA,
                "XX" => Self::XX,
                "AK" => Self::AK,
                "AP" => Self::AP,
                "AS" => Self::AS,
                "VF" => Self::VF,
                "VI" => Self::VI,
                "HH" => Self::HH,
                _ => return None,
            })
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum QuoteKind {
        None,
        Pi,
        Pf,
    }

    struct RangeTable {
        ranges: Vec<(u32, u32, LineBreakClass)>,
        default: LineBreakClass,
    }

    impl RangeTable {
        fn get(&self, codepoint: u32) -> LineBreakClass {
            let index = self
                .ranges
                .partition_point(|&(start, _, _)| start <= codepoint);
            if index > 0 {
                let (_, end, class) = self.ranges[index - 1];
                if codepoint <= end {
                    return class;
                }
            }
            self.default
        }
    }

    #[derive(Default)]
    struct BoolRanges {
        ranges: Vec<(u32, u32)>,
    }

    impl BoolRanges {
        fn new(mut ranges: Vec<(u32, u32)>) -> Self {
            ranges.sort_unstable();
            let mut merged: Vec<(u32, u32)> = Vec::with_capacity(ranges.len());
            for (start, end) in ranges {
                if let Some(last) = merged.last_mut()
                    && start <= last.1.saturating_add(1)
                {
                    last.1 = last.1.max(end);
                } else {
                    merged.push((start, end));
                }
            }
            Self { ranges: merged }
        }

        fn contains(&self, codepoint: u32) -> bool {
            let index = self
                .ranges
                .partition_point(|&(start, _)| start <= codepoint);
            index > 0 && codepoint <= self.ranges[index - 1].1
        }
    }

    struct Tables {
        line_break: RangeTable,
        east_asian: BoolRanges,
        pi: BoolRanges,
        pf: BoolRanges,
        combining: BoolRanges,
        extended_pictographic: BoolRanges,
        unassigned: BoolRanges,
    }

    static TABLES: OnceLock<Tables> = OnceLock::new();

    fn tables() -> &'static Tables {
        TABLES.get_or_init(parse_tables)
    }

    fn parse_tables() -> Tables {
        assert!(
            LINE_BREAK_DATA.contains("18.0.0")
                && EAST_ASIAN_DATA.contains("18.0.0")
                && COMBINING_DATA.contains("18.0.0")
                && EXT_PICT_DATA.contains("18.0.0")
                && UNASSIGNED_DATA.contains("18.0.0"),
            "line-break data files must stay pinned to Unicode 18.0.0"
        );
        let (pi, pf) = parse_pi_pf();
        Tables {
            line_break: parse_line_break(),
            east_asian: parse_east_asian(),
            pi,
            pf,
            combining: parse_bool_property(COMBINING_DATA),
            extended_pictographic: parse_bool_property(EXT_PICT_DATA),
            unassigned: parse_bool_property(UNASSIGNED_DATA),
        }
    }

    /// 解析只含单一布尔属性的 `range ; Name` 数据文件（所有行都入表）。
    fn parse_bool_property(data: &str) -> BoolRanges {
        let ranges = data
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(parse_property_line)
            .map(|(range, _)| range)
            .collect();
        BoolRanges::new(ranges)
    }

    fn parse_line_break() -> RangeTable {
        let mut dense = vec![LineBreakClass::XX; LIMIT];
        // @missing 提供未列出码点的默认类（XX/ID/PR）。先铺默认值，
        // 再用显式数据行覆盖。
        for line in LINE_BREAK_DATA.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("# @missing:") else {
                continue;
            };
            let Some((range, name)) = parse_property_line(rest) else {
                continue;
            };
            if let Some(class) = missing_class(name) {
                fill_line_break(&mut dense, range, class);
            }
        }
        for line in LINE_BREAK_DATA.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((range, name)) = parse_property_line(line) else {
                continue;
            };
            let class = LineBreakClass::from_name(name)
                .unwrap_or_else(|| panic!("unknown Line_Break class {name:?}"));
            fill_line_break(&mut dense, range, class);
        }
        RangeTable {
            ranges: compress_line_break(&dense),
            default: LineBreakClass::XX,
        }
    }

    fn parse_east_asian() -> BoolRanges {
        let mut dense = vec![false; LIMIT];
        for line in EAST_ASIAN_DATA.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("# @missing:") else {
                continue;
            };
            let Some((range, name)) = parse_property_line(rest) else {
                continue;
            };
            fill_bool(&mut dense, range, is_east_asian(name));
        }
        for line in EAST_ASIAN_DATA.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((range, name)) = parse_property_line(line) else {
                continue;
            };
            fill_bool(&mut dense, range, is_east_asian(name));
        }
        BoolRanges::new(compress_bool(&dense))
    }

    fn parse_pi_pf() -> (BoolRanges, BoolRanges) {
        let mut pi = Vec::new();
        let mut pf = Vec::new();
        for line in PI_PF_DATA.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((range, name)) = parse_property_line(line) else {
                continue;
            };
            match name {
                "Pi" => pi.push(range),
                "Pf" => pf.push(range),
                _ => {}
            }
        }
        (BoolRanges::new(pi), BoolRanges::new(pf))
    }

    fn parse_property_line(text: &str) -> Option<((u32, u32), &str)> {
        let (range_part, rest) = text.split_once(';')?;
        let name = rest.split('#').next()?.trim();
        let range_part = range_part.trim();
        let (start, end) = match range_part.split_once("..") {
            Some((start, end)) => (
                u32::from_str_radix(start.trim(), 16).ok()?,
                u32::from_str_radix(end.trim(), 16).ok()?,
            ),
            None => {
                let value = u32::from_str_radix(range_part, 16).ok()?;
                (value, value)
            }
        };
        Some(((start, end), name))
    }

    fn missing_class(name: &str) -> Option<LineBreakClass> {
        Some(match name {
            "Unknown" => LineBreakClass::XX,
            "Prefix_Numeric" => LineBreakClass::PR,
            "Ideographic" => LineBreakClass::ID,
            "Alphabetic" => LineBreakClass::AL,
            "Ambiguous" => LineBreakClass::AI,
            "Combining_Mark" => LineBreakClass::CM,
            "Contingent_Break" => LineBreakClass::CB,
            "Complex_Context" => LineBreakClass::SA,
            "Conditional_Japanese_Starter" => LineBreakClass::CJ,
            "Exclamation" => LineBreakClass::EX,
            "Hyphen" => LineBreakClass::HY,
            "Infix_Separator" => LineBreakClass::IS,
            "Numeric" => LineBreakClass::NU,
            "Postfix_Numeric" => LineBreakClass::PO,
            "Variation_Selector" => LineBreakClass::CM,
            _ => return None,
        })
    }

    fn is_east_asian(name: &str) -> bool {
        matches!(name, "F" | "W" | "H")
    }

    fn fill_line_break(
        dense: &mut [LineBreakClass],
        (start, end): (u32, u32),
        class: LineBreakClass,
    ) {
        let start = start as usize;
        let end = (end as usize).min(dense.len() - 1);
        if start <= end {
            dense[start..=end].fill(class);
        }
    }

    fn fill_bool(dense: &mut [bool], (start, end): (u32, u32), value: bool) {
        let start = start as usize;
        let end = (end as usize).min(dense.len() - 1);
        if start <= end {
            dense[start..=end].fill(value);
        }
    }

    fn compress_line_break(dense: &[LineBreakClass]) -> Vec<(u32, u32, LineBreakClass)> {
        let mut ranges = Vec::new();
        let mut start = 0usize;
        for index in 1..=dense.len() {
            if index == dense.len() || dense[index] != dense[start] {
                ranges.push((start as u32, (index - 1) as u32, dense[start]));
                start = index;
            }
        }
        ranges
    }

    fn compress_bool(dense: &[bool]) -> Vec<(u32, u32)> {
        let mut ranges = Vec::new();
        let mut start = 0usize;
        for index in 1..=dense.len() {
            if index == dense.len() || dense[index] != dense[start] {
                if dense[start] {
                    ranges.push((start as u32, (index - 1) as u32));
                }
                start = index;
            }
        }
        ranges
    }

    // -----------------------------------------------------------------------
    // 字符属性 / LB9-LB10 单元
    // -----------------------------------------------------------------------

    #[derive(Clone, Copy)]
    struct CharProps {
        class: LineBreakClass,
        east_asian: bool,
        quote: QuoteKind,
        dotted_circle: bool,
        /// LB9：该字符并入前一个 unit（CM/ZWJ，或 SA 类中的组合符）。
        /// VF/VI 等有独立断行类的组合符**不**并入（它们参与 LB28a）。
        lb9_joins_previous: bool,
        /// LB30b 的“potential emoji”：未分配的 Extended_Pictographic。
        potential_emoji: bool,
    }

    fn char_props(tables: &Tables, character: char) -> CharProps {
        let codepoint = character as u32;
        let raw = tables.line_break.get(codepoint);
        let quote = if raw == LineBreakClass::QU {
            if tables.pi.contains(codepoint) {
                QuoteKind::Pi
            } else if tables.pf.contains(codepoint) {
                QuoteKind::Pf
            } else {
                QuoteKind::None
            }
        } else {
            QuoteKind::None
        };
        // LB1 默认解析：AI/SA/SG/XX → AL，CJ → NS；CB 保持未解析（LB20）。
        let class = match raw {
            LineBreakClass::AI | LineBreakClass::SA | LineBreakClass::SG | LineBreakClass::XX => {
                LineBreakClass::AL
            }
            LineBreakClass::CJ => LineBreakClass::NS,
            other => other,
        };
        CharProps {
            class,
            east_asian: tables.east_asian.contains(codepoint),
            quote,
            dotted_circle: codepoint == 0x25CC,
            // LB9 的“组合字符序列”：CM/ZWJ 按 Line_Break 类并入；SA 类里的
            // 泰文/缅甸文组合符（如 U+0E31 SA_Mn）也要按 GC=Mark 并入。
            // VF/VI 之类有独立断行类的组合符不并入，否则 LB28a 会失效。
            lb9_joins_previous: raw == LineBreakClass::CM
                || (raw == LineBreakClass::SA && tables.combining.contains(codepoint)),
            // 已分配的 ExtPict（如 U+2757）不是 LB30b 的 emoji base；只有
            // 未分配的 ExtPictUnassigned 才算“potential emoji”。
            potential_emoji: tables.extended_pictographic.contains(codepoint)
                && tables.unassigned.contains(codepoint),
        }
    }

    #[derive(Clone, Copy)]
    struct Unit {
        start: usize,
        end: usize,
        class: LineBreakClass,
        east_asian: bool,
        quote: QuoteKind,
        dotted_circle: bool,
        potential_emoji: bool,
        /// LB8a：unit 的最后一个字符是 ZWJ。
        no_break_after: bool,
    }

    fn build_units(text: &str) -> Vec<Unit> {
        let tables = tables();
        let mut units: Vec<Unit> = Vec::new();
        for (offset, character) in text.char_indices() {
            let props = char_props(tables, character);
            let end = offset + character.len_utf8();
            if props.lb9_joins_previous || props.class == LineBreakClass::ZWJ {
                if let Some(unit) = units.last_mut()
                    && !matches!(
                        unit.class,
                        LineBreakClass::BK
                            | LineBreakClass::CR
                            | LineBreakClass::LF
                            | LineBreakClass::NL
                            | LineBreakClass::SP
                            | LineBreakClass::ZW
                    )
                {
                    // LB9：X (CM|ZWJ)* 视为 X；只更新 unit 末尾与 LB8a 标记。
                    unit.end = end;
                    unit.no_break_after = props.class == LineBreakClass::ZWJ;
                    continue;
                }
                // LB10：孤立的 CM/ZWJ 按 AL 处理。
                units.push(Unit {
                    start: offset,
                    end,
                    class: LineBreakClass::AL,
                    east_asian: props.east_asian,
                    quote: QuoteKind::None,
                    dotted_circle: false,
                    potential_emoji: false,
                    no_break_after: props.class == LineBreakClass::ZWJ,
                });
                continue;
            }
            units.push(Unit {
                start: offset,
                end,
                class: props.class,
                east_asian: props.east_asian,
                quote: props.quote,
                dotted_circle: props.dotted_circle,
                potential_emoji: props.potential_emoji,
                no_break_after: false,
            });
        }
        units
    }

    // -----------------------------------------------------------------------
    // UAX#14 规则（Unicode 18.0.0，TR14 §6）
    // -----------------------------------------------------------------------

    fn last_non_sp(units: &[Unit], mut index: usize) -> Option<usize> {
        loop {
            if units[index].class != LineBreakClass::SP {
                return Some(index);
            }
            if index == 0 {
                return None;
            }
            index -= 1;
        }
    }

    /// 从边界的左侧匹配 `NU (SY|IS)* (CL|CP)?`。
    fn numeric_before(units: &[Unit], index: usize) -> bool {
        let mut cursor = index as isize;
        if cursor >= 0
            && matches!(
                units[cursor as usize].class,
                LineBreakClass::CL | LineBreakClass::CP
            )
        {
            cursor -= 1;
        }
        while cursor >= 0
            && matches!(
                units[cursor as usize].class,
                LineBreakClass::SY | LineBreakClass::IS
            )
        {
            cursor -= 1;
        }
        cursor >= 0 && units[cursor as usize].class == LineBreakClass::NU
    }

    fn uax14_break_between(units: &[Unit], index: usize) -> bool {
        use LineBreakClass::*;

        let previous = units[index];
        let next = units[index + 1];

        // LB4 / LB5 / LB6 / LB7 / LB8 / LB8a
        if previous.class == BK {
            return true;
        }
        if previous.class == CR && next.class == LF {
            return false;
        }
        if matches!(previous.class, CR | LF | NL) {
            return true;
        }
        if matches!(next.class, BK | CR | LF | NL) {
            return false;
        }
        if matches!(next.class, SP | ZW) {
            return false;
        }
        if let Some(last) = last_non_sp(units, index)
            && units[last].class == ZW
        {
            return true;
        }
        if previous.no_break_after {
            return false;
        }

        // LB11 / LB12 / LB12a
        if next.class == WJ || previous.class == WJ {
            return false;
        }
        if previous.class == GL {
            return false;
        }
        if next.class == GL && !matches!(previous.class, SP | HY | HH) {
            return false;
        }

        // LB14（OP SP* ×）与 LB15a/b 先判；LB15c 是允许断行的例外，必须在
        // LB13 之前判，否则 LB13 的 × IS 会把它吃掉。
        if let Some(last) = last_non_sp(units, index)
            && units[last].class == OP
        {
            return false;
        }
        if let Some(last) = last_non_sp(units, index)
            && units[last].quote == QuoteKind::Pi
        {
            let qualifies = last == 0
                || matches!(
                    units[last - 1].class,
                    BK | CR | LF | NL | OP | QU | GL | SP | ZW
                );
            if qualifies {
                return false;
            }
        }
        if next.quote == QuoteKind::Pf {
            let after = units.get(index + 2).map(|unit| unit.class);
            let prohibited = match after {
                None => true,
                Some(class) => matches!(
                    class,
                    SP | GL | WJ | CL | QU | CP | EX | IS | SY | BK | CR | LF | NL | ZW
                ),
            };
            if prohibited {
                return false;
            }
        }
        // LB15c：SP ÷ IS NU（覆盖 LB13/LB15d 的禁止）。
        if previous.class == SP
            && next.class == IS
            && units.get(index + 2).is_some_and(|unit| unit.class == NU)
        {
            return true;
        }

        // LB13
        if matches!(next.class, CL | CP | EX | SY) {
            return false;
        }
        // LB15d
        if next.class == IS {
            return false;
        }
        // LB16
        if next.class == NS
            && let Some(last) = last_non_sp(units, index)
            && matches!(units[last].class, CL | CP)
        {
            return false;
        }
        // LB17
        if next.class == B2
            && let Some(last) = last_non_sp(units, index)
            && units[last].class == B2
        {
            return false;
        }
        // LB18
        if previous.class == SP {
            return true;
        }

        // LB19
        if next.class == QU && next.quote != QuoteKind::Pi {
            return false;
        }
        if previous.class == QU && previous.quote != QuoteKind::Pf {
            return false;
        }
        // LB19a
        if next.class == QU && !previous.east_asian {
            return false;
        }
        if next.class == QU {
            let after_non_east_asian = units.get(index + 2).is_none_or(|unit| !unit.east_asian);
            if after_non_east_asian {
                return false;
            }
        }
        if previous.class == QU && !next.east_asian {
            return false;
        }
        if previous.class == QU {
            let before_non_east_asian = index == 0 || !units[index - 1].east_asian;
            if before_non_east_asian {
                return false;
            }
        }

        // LB20
        if next.class == CB || previous.class == CB {
            return true;
        }
        // LB20a
        if matches!(previous.class, HY | HH) && matches!(next.class, AL | HL) {
            let word_initial = index == 0
                || matches!(
                    units[index - 1].class,
                    BK | CR | LF | NL | SP | ZW | CB | GL
                );
            if word_initial {
                return false;
            }
        }
        // LB21
        if matches!(next.class, BA | HH | HY | NS) || previous.class == BB {
            return false;
        }
        // LB21a
        if matches!(previous.class, HY | HH)
            && next.class != HL
            && index > 0
            && units[index - 1].class == HL
        {
            return false;
        }
        // LB21b
        if previous.class == SY && next.class == HL {
            return false;
        }
        // LB22
        if next.class == IN {
            return false;
        }
        // LB23
        if matches!(previous.class, AL | HL) && next.class == NU {
            return false;
        }
        if previous.class == NU && matches!(next.class, AL | HL) {
            return false;
        }
        // LB23a
        if previous.class == PR && matches!(next.class, ID | EB | EM) {
            return false;
        }
        if matches!(previous.class, ID | EB | EM) && next.class == PO {
            return false;
        }
        // LB24
        if matches!(previous.class, PR | PO) && matches!(next.class, AL | HL) {
            return false;
        }
        if matches!(previous.class, AL | HL) && matches!(next.class, PR | PO) {
            return false;
        }
        // LB25
        if matches!(next.class, PO | PR) && numeric_before(units, index) {
            return false;
        }
        if previous.class == PO || previous.class == PR {
            if next.class == NU {
                return false;
            }
            if next.class == OP {
                let after_op = units.get(index + 2).map(|unit| unit.class);
                if after_op == Some(NU) {
                    return false;
                }
                if after_op == Some(IS) && units.get(index + 3).is_some_and(|unit| unit.class == NU)
                {
                    return false;
                }
            }
        }
        if matches!(previous.class, HY | IS) && next.class == NU {
            return false;
        }
        if next.class == NU
            && matches!(previous.class, NU | SY | IS)
            && numeric_before(units, index)
        {
            return false;
        }
        // LB26
        if previous.class == JL && matches!(next.class, JL | JV | H2 | H3) {
            return false;
        }
        if matches!(previous.class, JV | H2) && matches!(next.class, JV | JT) {
            return false;
        }
        if matches!(previous.class, JT | H3) && next.class == JT {
            return false;
        }
        // LB27
        if matches!(previous.class, JL | JV | JT | H2 | H3) && next.class == PO {
            return false;
        }
        if previous.class == PR && matches!(next.class, JL | JV | JT | H2 | H3) {
            return false;
        }
        // LB28
        if matches!(previous.class, AL | HL) && matches!(next.class, AL | HL) {
            return false;
        }
        // LB28a
        let previous_brahmic = matches!(previous.class, AK | AS) || previous.dotted_circle;
        let next_brahmic = matches!(next.class, AK | AS) || next.dotted_circle;
        if previous.class == AP && (matches!(next.class, AK | AS) || next.dotted_circle) {
            return false;
        }
        if previous_brahmic && matches!(next.class, VF | VI) {
            return false;
        }
        if previous.class == VI
            && (next.class == AK || next.dotted_circle)
            && index > 0
            && (matches!(units[index - 1].class, AK | AS) || units[index - 1].dotted_circle)
        {
            return false;
        }
        if previous_brahmic
            && next_brahmic
            && units.get(index + 2).is_some_and(|unit| unit.class == VF)
        {
            return false;
        }
        // LB29
        if previous.class == IS && matches!(next.class, AL | HL) {
            return false;
        }
        // LB30
        if matches!(previous.class, AL | HL | NU) && next.class == OP && !next.east_asian {
            return false;
        }
        if previous.class == CP && !previous.east_asian && matches!(next.class, AL | HL | NU) {
            return false;
        }
        // LB30a
        if previous.class == RI && next.class == RI {
            let mut count = 0usize;
            let mut cursor = index as isize;
            while cursor >= 0 && units[cursor as usize].class == RI {
                count += 1;
                cursor -= 1;
            }
            if count % 2 == 1 {
                return false;
            }
        }
        // LB30b（“emoji base (or potential emoji)”）：ID_ExtPictUnassigned
        // 这类潜在 emoji 即使 Line_Break=ID 也要与 EM 粘连。
        if (previous.class == EB || previous.potential_emoji) && next.class == EM {
            return false;
        }
        // LB31
        true
    }

    fn uax14_breaks(text: &str, units: &[Unit]) -> Vec<usize> {
        if units.is_empty() {
            return vec![0];
        }
        let mut breaks = Vec::new();
        for index in 0..units.len().saturating_sub(1) {
            if uax14_break_between(units, index) {
                breaks.push(units[index + 1].start);
            }
        }
        breaks.push(text.len());
        breaks
    }

    // -----------------------------------------------------------------------
    // CSS 选项（word-break / overflow-wrap）
    // -----------------------------------------------------------------------

    fn unit_boundary(units: &[Unit], offset: usize) -> Option<(usize, usize)> {
        let next = units.partition_point(|unit| unit.start < offset);
        if next == 0 || next >= units.len() {
            return None;
        }
        if units[next].start != offset || units[next - 1].end != offset {
            return None;
        }
        Some((next - 1, next))
    }

    fn add_cluster_breaks(text: &str, units: &[Unit], breaks: &mut Vec<usize>, word_only: bool) {
        use LineBreakClass::{AL, HL, ID, NU};
        for (offset, _) in text.grapheme_indices(true) {
            if offset == 0 {
                continue;
            }
            let Some((previous, next)) = unit_boundary(units, offset) else {
                continue;
            };
            if word_only {
                let previous = units[previous].class;
                let next = units[next].class;
                let word = |class| matches!(class, AL | HL | NU | ID);
                if !(word(previous) && word(next)) {
                    continue;
                }
            }
            breaks.push(offset);
        }
    }

    fn remove_word_internal_breaks(units: &[Unit], breaks: &mut Vec<usize>) {
        use LineBreakClass::{AL, HL, ID, NU};
        breaks.retain(|&offset| {
            let Some((previous, next)) = unit_boundary(units, offset) else {
                return true;
            };
            let previous = units[previous].class;
            let next = units[next].class;
            let word = |class| matches!(class, AL | HL | NU | ID);
            !(word(previous) && word(next))
        });
    }

    // -----------------------------------------------------------------------
    // 公共 API
    // -----------------------------------------------------------------------

    /// 按 UAX#14（默认解析）返回断行机会的字节偏移，升序、去重，含
    /// `text.len()`（LB3 的 eot 断点）。
    ///
    /// CSS `word-break` / `overflow-wrap` 只在此结果上增删：`break-all` 在
    /// 相邻 typographic letter unit 之间加断点；`anywhere`/`break-word` 在
    /// 每个字形簇边界加紧急断点（后者是否参与 min-content 由调用方决定）；
    /// `keep-all` 移除单词内部的隐式断点。
    #[must_use]
    pub fn break_opportunities(text: &str, options: &BreakOptions) -> Vec<usize> {
        let units = build_units(text);
        let mut breaks = uax14_breaks(text, &units);
        match options.word_break {
            WordBreak::Normal => {}
            WordBreak::BreakAll => add_cluster_breaks(text, &units, &mut breaks, true),
            WordBreak::KeepAll => remove_word_internal_breaks(&units, &mut breaks),
        }
        if options.overflow_wrap != OverflowWrap::Normal {
            add_cluster_breaks(text, &units, &mut breaks, false);
        }
        breaks.push(text.len());
        breaks.sort_unstable();
        breaks.dedup();
        breaks
    }

    /// 把一次整形结果的 glyph cluster 映射为源文本字节区间。
    ///
    /// 边界是「UAX#29 字形簇边界」与「harfrust 实际 glyph cluster 起点」的
    /// 并集：从不出现在字形簇内部；若整形把多个字形簇合成一个 cluster
    /// （连字），则合并（不切开）。因此 1 char ≠ 1 cluster。
    #[must_use]
    pub fn cluster_map(text: &str, shaped: &ShapedText) -> Vec<Span> {
        if text.is_empty() {
            return Vec::new();
        }
        let mut grapheme_starts = BTreeSet::new();
        let mut graphemes = Vec::new();
        for (offset, grapheme) in text.grapheme_indices(true) {
            grapheme_starts.insert(offset);
            graphemes.push((offset, offset + grapheme.len()));
        }
        grapheme_starts.insert(text.len());

        let mut starts = BTreeSet::new();
        starts.insert(0);
        starts.insert(text.len());
        for glyph in &shaped.glyphs {
            let cluster = glyph.cluster as usize;
            if cluster <= text.len() && grapheme_starts.contains(&cluster) {
                starts.insert(cluster);
            }
        }
        if shaped.glyphs.is_empty() {
            starts.extend(grapheme_starts.iter().copied());
        }

        let starts: Vec<usize> = starts.into_iter().collect();
        let group_of = |offset: usize| -> usize {
            let index = starts.partition_point(|&start| start <= offset);
            if index == 0 { 0 } else { starts[index - 1] }
        };

        let mut spans = Vec::new();
        let mut group_start = graphemes[0].0;
        let mut group_cluster = group_of(group_start);
        for &(grapheme_start, _) in &graphemes[1..] {
            let cluster = group_of(grapheme_start);
            if cluster != group_cluster {
                spans.push(Span {
                    byte_start: group_start,
                    byte_end: grapheme_start,
                });
                group_start = grapheme_start;
                group_cluster = cluster;
            }
        }
        spans.push(Span {
            byte_start: group_start,
            byte_end: text.len(),
        });
        spans
    }

    /// 按 min-content 语义计算最窄宽度（CSS px）。
    ///
    /// `overflow-wrap: break-word` 的紧急断点不计入 min-content（CSS Text 3
    /// §5.4）；`anywhere` 与 `word-break: break-all` 的断点计入（§5.1 末段）。
    #[must_use]
    pub fn min_content_width(text: &str, shaped: &ShapedText, options: &BreakOptions) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let clusters = cluster_map(text, shaped);
        let mut widths = vec![0.0_f32; clusters.len()];
        for glyph in &shaped.glyphs {
            let cluster = glyph.cluster as usize;
            if let Some(index) = clusters
                .iter()
                .position(|span| span.byte_start <= cluster && cluster < span.byte_end)
            {
                widths[index] += glyph.advance;
            }
        }

        let effective = if options.overflow_wrap == OverflowWrap::BreakWord {
            BreakOptions {
                overflow_wrap: OverflowWrap::Normal,
                ..*options
            }
        } else {
            *options
        };
        let breaks = break_opportunities(text, &effective);
        let mut max_width = 0.0_f32;
        let mut start = 0usize;
        for &offset in &breaks {
            if offset <= start {
                continue;
            }
            let width: f32 = clusters
                .iter()
                .zip(&widths)
                .filter(|(span, _)| span.byte_start >= start && span.byte_start < offset)
                .map(|(_, width)| *width)
                .sum();
            max_width = max_width.max(width);
            start = offset;
        }
        max_width
    }

    /// 在 cluster 边界上按断行机会切分文本；输出无缝覆盖输入。
    ///
    /// 不是 cluster 边界的断点会被忽略（cluster 不可拆）。逐段 `&text[start..end]`
    /// 拼回必须与输入逐字节相同。
    #[must_use]
    pub fn split_spans(text: &str, clusters: &[Span], breaks: &[usize]) -> Vec<Span> {
        if text.is_empty() {
            return Vec::new();
        }
        let boundaries: BTreeSet<usize> = clusters
            .iter()
            .flat_map(|span| [span.byte_start, span.byte_end])
            .collect();
        let mut points = BTreeSet::new();
        points.insert(0);
        points.insert(text.len());
        for &offset in breaks {
            if offset > 0 && offset < text.len() && boundaries.contains(&offset) {
                points.insert(offset);
            }
        }
        let points: Vec<usize> = points.into_iter().collect();
        points
            .windows(2)
            .map(|window| Span {
                byte_start: window[0],
                byte_end: window[1],
            })
            .collect()
    }
}

#[cfg(feature = "inline")]
pub use conformance::{
    BreakOptions, OverflowWrap, Span, WordBreak, break_opportunities, cluster_map,
    min_content_width, split_spans,
};
