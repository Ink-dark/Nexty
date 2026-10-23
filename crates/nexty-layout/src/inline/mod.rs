//! 行内格式化：行内项收集、空白处理、贪心断行与行盒组装。
//!
//! 行为 ground truth：[CSS 2.1 §9.2.2](https://www.w3.org/TR/CSS21/visuren.html#inline-formatting)
//! （行内格式化上下文）、[§10.8](https://www.w3.org/TR/CSS21/visudet.html#line-height)
//! （行高与 strut）、[CSS Text §3](https://drafts.csswg.org/css-text-3/#white-space-processing)
//! （空白处理）与 [§7](https://drafts.csswg.org/css-text-3/#line-breaking)（断行）。
//!
//! # 断行模型
//!
//! 文本项按 computed style 切成**样式一致的文本块**；每块只整形一次，然后：
//! `cluster_map` 切不可拆 cluster → `break_opportunities`（UAX#14 + CSS
//! `word-break` / `overflow-wrap`）取断点 → `split_spans` 在 cluster 边界上
//! 切段 → 贪心放置。软断点来自 UAX#14；`white-space: nowrap` / `pre` 关闭
//! 软断点，`<br>` 与保留的 `\n` 是硬断行项（`InlineItem::Break`）。
//!
//! 空白处理（CSS Text §4.1 Phase I/II 的简化）：折叠模式（`normal` /
//! `nowrap` / `pre-line`）把连续可折叠空白折成一个空格，行首/行尾的折叠
//! 空格在断行时丢弃；保留模式（`pre` / `pre-wrap` / `break-spaces`）逐字
//! 保留，制表按一个空格处理（`tab-size` 偏差）。
//!
//! # 三分离模块
//!
//! UAX#14 相关代码不放在 `build_lines` 同一个文件里，按职责拆为：
//!
//! | 模块 | 职责 | 行数约束 |
//! | --- | --- | --- |
//! | `cluster_map` | `Span`、整形结果 → 源文本 cluster 映射 | ≤200 |
//! | `break_ops` | `BreakOptions` / `WordBreak` / `OverflowWrap`、`break_opportunities` 与 CSS 选项增删 | ≤200 |
//! | `span_split` | `split_spans` 与 `min_content_width` | ≤200 |
//! | `uax14` | Line_Break 类、UCD 数据解析、LB1–LB31 规则（`classes` / `data` / `rules`） | 规则引擎不设 200 行上限 |
//!
//! `uax14` 的 200 行例外理由：LB1–LB31 必须按规范顺序线性判定，规则之间共享
//! 单元与前瞻/回看上下文，拆碎会让「规范条款 ↔ 代码」的对应关系失真；数据
//! 解析（`data`）与类映射（`classes`）已独立，`rules` 保持单文件但不得回到
//! 千行级（当前约 400 行）。
//!
//! 已知偏差：`vertical-align` 仅 baseline（原子盒/图片底边对齐基线）；
//! `tab-size` 未实现（制表按一个空格处理）；`pre-wrap` 行尾空格未做悬挂
//! （按占宽处理）；bidi 未实现；跨行内元素边界的断行上下文只回看两个字符
//! （见 `uax14::rules::breaks_between_text`）。
//!
//! feature `block` 关闭时本模块无消费方（行内流的唯一下游是 taffy 管线），
//! 按模块整体抑制 dead_code；`replaced` 单独开启时 `lib.rs` 不 re-export 本模块
//! 的公共项，故一并抑制 unused_imports。

#![cfg_attr(not(feature = "block"), allow(dead_code, unused_imports))]

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use nexty_css::{ComputedStyle, DisplayValue, LineHeightValue, OverflowWrapValue, WordBreakValue};
use nexty_dom::{Document, ElementData, Namespace, NodeId, NodeKind};
use nexty_text::{FontMetrics, Glyph, ResolvedFont, ShapedText, TextShaper, TextStyle};

use crate::Context;
use crate::fragment::{Fragment, ImageRun, LineFragment, LineStart, SourceChar, TextRun};

mod break_ops;
mod cluster_map;
mod span_split;
mod uax14;

pub use break_ops::{BreakOptions, OverflowWrap, WordBreak, break_opportunities};
pub use cluster_map::{Span, cluster_map};
pub use span_split::{min_content_width, split_spans};

/// 行内流中的一个原子图片项（替换元素，v1 仅 `<img>`）。
pub(crate) struct AtomicImage {
    /// 图片来源元素。
    pub node: NodeId,
    /// 显示宽。
    pub width: f32,
    /// 显示高。
    pub height: f32,
    /// 样式来源（行首/行尾空白与断行选项按它取）。
    pub style: ComputedStyle,
}

/// 行内流中的一个原子盒（`inline-block` 等）：片段在 run 叶的 measure 期按
/// taffy 给出的可用宽惰性布局（见 `tree_build::InlineRunNode`），收集期仅登记
/// 节点与样式（占位片段的 `node`/`style` 字段供后续布局使用）。
pub(crate) struct AtomicBlock {
    /// 布局完成的子片段（border_box.x/y 由行盒组装阶段定位）。
    pub fragment: Fragment,
    /// 样式来源（行首/行尾空白与断行选项按它取）。
    pub style: ComputedStyle,
}

/// 行内流中的一项：文本块、原子图片、原子盒或硬断行。
pub(crate) enum InlineItem {
    Text(TextChunk),
    Image(AtomicImage),
    Block(Box<AtomicBlock>),
    /// 硬断行（`<br>` 或保留模式下的 `\n`）。
    Break,
}

/// 一段样式一致的文本（空白已按 `white-space` 处理）。
pub(crate) struct TextChunk {
    /// 样式来源元素。
    pub node: NodeId,
    /// 产生这段文本的文本节点（选区端点的定位依据）。
    pub text_node: NodeId,
    /// 文本（不含硬断行字符；折叠模式下连续空白已折成一个空格）。
    pub text: Arc<str>,
    /// `text` 中每个字符回指 `text_node` 源文本的字节区间。
    pub source_map: Arc<[SourceChar]>,
    /// 该文本的样式（直接包含它的元素的 computed style）。
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

/// `node` 是否为 HTML 命名空间的 `<br>`。
fn is_br_node(document: &Document, node: NodeId) -> bool {
    matches!(
        document.node(node),
        Some(NodeKind::Element(data))
            if data.namespace == Namespace::Html && data.name == "br"
    )
}

/// CSS 可折叠空白（空格 / 制表 / 段内换行 / 换页）。
fn is_collapsible(character: char) -> bool {
    matches!(character, ' ' | '\t' | '\n' | '\r' | '\u{000C}')
}

/// 行内项收集状态。
struct Collector<'a> {
    ctx: &'a Context<'a>,
    items: Vec<InlineItem>,
    /// 上一个发出的字符是否为折叠空格（跨文本节点去重）。
    last_collapsible_space: bool,
    /// 上一个折叠空格所属的文本节点（用于把源区间扩展到整段空白）。
    last_space_node: Option<NodeId>,
    text_buffer: String,
    text_source: Vec<SourceChar>,
    /// 当前缓冲的文本节点。
    text_node: Option<NodeId>,
    /// 当前缓冲的样式来源元素。
    text_style_node: Option<NodeId>,
    text_style: Option<ComputedStyle>,
}

impl Collector<'_> {
    /// 若当前缓冲区属于别的文本节点/样式，先刷出（一个文本块只带一种样式）。
    fn ensure_style(&mut self, text_node: NodeId, style_node: NodeId, style: &ComputedStyle) {
        if self.text_node == Some(text_node)
            && self.text_style_node == Some(style_node)
            && self.text_style.as_ref() == Some(style)
        {
            return;
        }
        self.flush_text();
        self.text_node = Some(text_node);
        self.text_style_node = Some(style_node);
        self.text_style = Some(style.clone());
    }

    /// 把缓冲中的文本刷入结果。
    fn flush_text(&mut self) {
        if self.text_buffer.is_empty() {
            return;
        }
        self.items.push(InlineItem::Text(TextChunk {
            node: self
                .text_style_node
                .take()
                .expect("text has a style source"),
            text_node: self.text_node.take().expect("text has a source node"),
            text: Arc::from(std::mem::take(&mut self.text_buffer)),
            source_map: Arc::from(std::mem::take(&mut self.text_source)),
            style: self.text_style.take().expect("text has a style"),
        }));
        self.last_space_node = None;
    }

    /// 把一个字符及其源区间写入缓冲区。
    fn push_char(
        &mut self,
        character: char,
        text_node: NodeId,
        style_node: NodeId,
        style: &ComputedStyle,
        source: Range<usize>,
    ) {
        self.ensure_style(text_node, style_node, style);
        let chunk = self.text_buffer.len() as u32;
        self.text_buffer.push(character);
        self.text_source.push(SourceChar {
            chunk,
            source: source.start as u32,
            source_end: source.end as u32,
        });
    }

    /// 收集一段文本（按当前 `white-space` 处理空白）。
    fn push_text(
        &mut self,
        text: &str,
        text_node: NodeId,
        style_node: NodeId,
        style: &ComputedStyle,
    ) {
        let white_space = style.white_space;
        for (start, character) in text.char_indices() {
            let source = start..start + character.len_utf8();
            if white_space.collapses() {
                if is_collapsible(character) {
                    if white_space.preserves_newlines() && matches!(character, '\n' | '\r') {
                        // pre-line：换行保留为硬断行，空格仍折叠
                        self.push_break();
                        continue;
                    }
                    // 折叠：连续空白合成一个空格；跨文本节点去重
                    if self.last_collapsible_space {
                        // 同一文本节点内把源区间扩展到整段空白
                        if self.last_space_node == Some(text_node)
                            && let Some(last) = self.text_source.last_mut()
                        {
                            last.source_end = source.end as u32;
                        }
                        continue;
                    }
                    self.push_char(' ', text_node, style_node, style, source);
                    self.last_collapsible_space = true;
                    self.last_space_node = Some(text_node);
                } else {
                    self.push_char(character, text_node, style_node, style, source);
                    self.last_collapsible_space = false;
                    self.last_space_node = None;
                }
            } else {
                match character {
                    '\n' | '\r' => self.push_break(),
                    // 偏差：tab-size 不实现，制表按一个空格保留
                    '\t' => {
                        self.push_char(' ', text_node, style_node, style, source);
                        self.last_collapsible_space = false;
                        self.last_space_node = None;
                    }
                    _ => {
                        self.push_char(character, text_node, style_node, style, source);
                        self.last_collapsible_space = false;
                        self.last_space_node = None;
                    }
                }
            }
        }
    }

    /// 收集一个硬断行（`<br>` 或保留的 `\n`）。
    fn push_break(&mut self) {
        self.flush_text();
        self.items.push(InlineItem::Break);
        self.last_collapsible_space = false;
        self.last_space_node = None;
    }

    /// 收集一个行内原子图片。
    fn push_image(&mut self, node: NodeId, style: &ComputedStyle) {
        let (width, height) = image_box_size(self.ctx.document, node, self.ctx.image_sizes);
        self.flush_text();
        self.items.push(InlineItem::Image(AtomicImage {
            node,
            width,
            height,
            style: style.clone(),
        }));
        self.last_collapsible_space = false;
        self.last_space_node = None;
    }

    /// 收集一个原子盒（inline-block 等）：登记节点与样式，布局延迟到
    /// run 叶的 measure 期（届时才知道 taffy 分配的可用宽）。
    fn push_atomic_block(&mut self, node: NodeId, style: &ComputedStyle) {
        self.flush_text();
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
            style: style.clone(),
        })));
        self.last_collapsible_space = false;
        self.last_space_node = None;
    }
}

/// 一段连续行内流的收集器。
///
/// 块容器按树序把行内内容喂给它：直接文本子节点走 [`InlineRun::push_text`]
/// （不递归子树），行内级子元素走 [`InlineRun::push_element`]（递归其子树，
/// 原子行内盒则独立布局）。空白处理状态（缓冲、跨节点折叠）跨子节点持续，
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
                last_collapsible_space: false,
                last_space_node: None,
                text_buffer: String::new(),
                text_source: Vec::new(),
                text_node: None,
                text_style_node: None,
                text_style: None,
            },
        }
    }

    /// 收集一个直接文本子节点（不递归）。
    ///
    /// `text_node` 是文本节点自身（选区端点定位），`style_node` 是提供样式的
    /// 父元素。
    pub(crate) fn push_text(
        &mut self,
        text: &str,
        text_node: NodeId,
        style_node: NodeId,
        style: &ComputedStyle,
    ) {
        self.collector.push_text(text, text_node, style_node, style);
    }

    /// 收集一个行内级子元素的子树。
    ///
    /// 元素本身是行内 `<img>` / `<br>` 时按原子图片 / 硬断行收集（`<img>`
    /// 的子树只有 alt 文本，v1 不做失败回退渲染）；原子行内盒（inline-block
    /// 等）登记为原子项，在 run 叶的 measure 期布局。空白处理状态跨子节点
    /// 持续，保证 `text <b>bold</b>` 这类跨界序列不重复收集、不丢空白。
    pub(crate) fn push_element(&mut self, element: NodeId, style: &ComputedStyle) {
        let document = self.collector.ctx.document;
        if is_image_node(document, element) {
            self.collector.push_image(element, style);
            return;
        }
        if is_br_node(document, element) {
            self.collector.push_break();
            return;
        }
        match style.display {
            DisplayValue::InlineBlock | DisplayValue::InlineFlex | DisplayValue::InlineTable => {
                self.collector.push_atomic_block(element, style);
            }
            _ => collect_children(&mut self.collector, element, style),
        }
    }

    /// 结束收集，刷出文本缓冲。纯折叠空白（且没有其他行内项）不产 run。
    pub(crate) fn finish(mut self) -> Vec<InlineItem> {
        self.collector.flush_text();
        let items = &self.collector.items;
        let has_content = items.iter().any(|item| match item {
            InlineItem::Text(chunk) => {
                !chunk.style.white_space.collapses()
                    || !chunk.text.chars().all(|character| character == ' ')
            }
            _ => true,
        });
        if has_content {
            self.collector.items
        } else {
            Vec::new()
        }
    }
}

fn collect_children(collector: &mut Collector<'_>, node: NodeId, style: &ComputedStyle) {
    let document = collector.ctx.document;
    for child in document.children(node) {
        match document.node(child) {
            Some(NodeKind::Text(text)) => collector.push_text(text, child, node, style),
            Some(NodeKind::Element(data)) => {
                let Some(child_style) = collector.ctx.styles.get(&child) else {
                    continue;
                };
                if child_style.display == DisplayValue::None {
                    continue;
                }
                // 硬断行不受 white-space / display 影响（display:none 已在上面跳过）
                if data.namespace == Namespace::Html && data.name == "br" {
                    collector.push_break();
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

/// computed style → UAX#14/CSS 断行选项。
fn break_options(style: &ComputedStyle, min_content: bool) -> BreakOptions {
    BreakOptions {
        word_break: match style.word_break {
            WordBreakValue::Normal => WordBreak::Normal,
            WordBreakValue::BreakAll => WordBreak::BreakAll,
            WordBreakValue::KeepAll => WordBreak::KeepAll,
        },
        overflow_wrap: match style.overflow_wrap {
            OverflowWrapValue::Normal => OverflowWrap::Normal,
            // break-word 的紧急断点不参与 min-content（CSS Text 3 §5.4）
            OverflowWrapValue::BreakWord if min_content => OverflowWrap::Normal,
            OverflowWrapValue::BreakWord => OverflowWrap::BreakWord,
            OverflowWrapValue::Anywhere => OverflowWrap::Anywhere,
        },
    }
}

/// 文本项样式（原子项取自身样式）。
fn item_style(item: &InlineItem) -> Option<&ComputedStyle> {
    match item {
        InlineItem::Text(chunk) => Some(&chunk.style),
        InlineItem::Image(image) => Some(&image.style),
        InlineItem::Block(atomic) => Some(&atomic.style),
        InlineItem::Break => None,
    }
}

/// 两个相邻行内项之间是否允许软断行。
///
/// 原子行内盒按 U+FFFC（Line_Break=CB）处理：两侧都可断（UAX#14 LB20）。
/// 文本之间跑 UAX#14 的边界判定；任一侧 `white-space` 禁止换行则不产生
/// 软断点（跨元素边界的近似，见 `uax14::rules::breaks_between_text`）。
fn boundary_break_allowed(items: &[InlineItem], previous: usize, next: usize) -> bool {
    let (Some(previous_style), Some(next_style)) =
        (item_style(&items[previous]), item_style(&items[next]))
    else {
        return false;
    };
    if !previous_style.white_space.wraps() || !next_style.white_space.wraps() {
        return false;
    }
    match (&items[previous], &items[next]) {
        (InlineItem::Text(left), InlineItem::Text(right)) => {
            uax14::breaks_between_text(&left.text, &right.text)
        }
        _ => true,
    }
}

/// 一段文本在整形结果中的 glyph 区间（cluster 落在 `start..end`）。
fn glyph_range(shaped: &ShapedText, start: usize, end: usize) -> Range<usize> {
    if start >= end {
        return 0..0;
    }
    let first = shaped
        .glyphs
        .partition_point(|glyph| (glyph.cluster as usize) < start);
    let last = shaped
        .glyphs
        .partition_point(|glyph| (glyph.cluster as usize) < end);
    first..last.max(first)
}

/// glyph 区间的视觉宽度（首字形 x 到末字形右缘）。
fn glyph_width(shaped: &ShapedText, glyphs: &Range<usize>) -> f32 {
    let Some(first) = shaped.glyphs.get(glyphs.start) else {
        return 0.0;
    };
    let Some(last) = shaped.glyphs.get(glyphs.end.saturating_sub(1)) else {
        return 0.0;
    };
    last.x + last.advance - first.x
}

/// 折叠模式下裁掉行首空格。
fn trim_leading_spaces(text: &str, start: usize, end: usize) -> (usize, usize) {
    let bytes = text.as_bytes();
    let mut start = start;
    while start < end && bytes[start] == b' ' {
        start += 1;
    }
    (start, end)
}

/// 折叠模式下裁掉行尾空格后的字节位置。
fn trailing_space_start(text: &str, start: usize, end: usize) -> usize {
    let bytes = text.as_bytes();
    let mut end = end;
    while end > start && bytes[end - 1] == b' ' {
        end -= 1;
    }
    end
}

/// 行内流中的一个已放置片段。
struct PlacedItem {
    /// 来源项下标（`InlineItem`）。
    item_index: usize,
    /// 文本项的字节区间；原子项为 `None`。
    span: Option<(usize, usize)>,
    /// 文本项的 glyph 区间；原子项为 `None`。
    glyphs: Option<Range<usize>>,
    /// 片段在行内的 x 起点。
    x: f32,
}

/// 贪心断行与行盒组装。
///
/// `block_style` 提供 strut（行盒的高度下限与基线位置）。原子图片按基线
/// 对齐（底边落在基线上，CSS 2.1 §10.8.1），不参与断行（不可拆）。
/// `min_content` 为真时按 min-content 语义断行（`overflow-wrap: break-word`
/// 的紧急断点不参与）。
pub(crate) fn build_lines(
    shaper: &dyn TextShaper,
    items: &[InlineItem],
    max_width: f32,
    block_style: &ComputedStyle,
    min_content: bool,
) -> Vec<LineFragment> {
    if items.is_empty() {
        return Vec::new();
    }

    // 每块文本只整形一次（cluster 映射与断点都基于同一结果）
    let shaped_items: Vec<Option<ShapedText>> = items
        .iter()
        .map(|item| match item {
            InlineItem::Text(chunk) => Some(
                shaper
                    .shape(&chunk.text, &text_style(&chunk.style))
                    .unwrap_or(ShapedText {
                        glyphs: Vec::new(),
                        fonts: Vec::new(),
                        width: 0.0,
                        height: 0.0,
                    }),
            ),
            _ => None,
        })
        .collect();

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

    let mut builder = LineBuilder {
        shaper,
        items,
        shaped_items,
        max_width,
        strut_above,
        strut_below,
        lines: Vec::new(),
        placed: Vec::new(),
        cursor: 0.0,
        current_start: LineStart::Soft { space: false },
        next_start: LineStart::Soft { space: false },
    };

    let mut previous: Option<usize> = None;
    for index in 0..items.len() {
        match &items[index] {
            InlineItem::Break => {
                // 硬断行：结束当前行（空行也保留），下一项从行首开始。
                builder.next_start = LineStart::Hard;
                builder.flush();
                previous = None;
            }
            InlineItem::Image(image) => {
                let break_before =
                    previous.is_some_and(|previous| boundary_break_allowed(items, previous, index));
                builder.place_atomic(index, image.width, break_before);
                previous = Some(index);
            }
            InlineItem::Block(atomic) => {
                let break_before =
                    previous.is_some_and(|previous| boundary_break_allowed(items, previous, index));
                builder.place_atomic(index, atomic.fragment.border_box.width, break_before);
                previous = Some(index);
            }
            InlineItem::Text(chunk) => {
                let shaped = builder.shaped_items[index]
                    .as_ref()
                    .expect("text item shaped");
                let clusters = cluster_map(&chunk.text, shaped);
                let normal_options = break_options(&chunk.style, min_content);
                let normal_spans = if chunk.style.white_space.wraps() {
                    split_spans(
                        &chunk.text,
                        &clusters,
                        &break_opportunities(&chunk.text, &normal_options),
                    )
                } else {
                    vec![Span {
                        byte_start: 0,
                        byte_end: chunk.text.len(),
                    }]
                };
                // 紧急断点（break-word / anywhere）：仅在整段放不下一行时使用；
                // min-content 下 break-word 的紧急断点不参与（CSS Text 3 §5.4）。
                let emergency_spans =
                    if !min_content && chunk.style.overflow_wrap != OverflowWrapValue::Normal {
                        let mut options = normal_options;
                        options.overflow_wrap = match chunk.style.overflow_wrap {
                            OverflowWrapValue::BreakWord => OverflowWrap::BreakWord,
                            OverflowWrapValue::Anywhere => OverflowWrap::Anywhere,
                            OverflowWrapValue::Normal => OverflowWrap::Normal,
                        };
                        Some(split_spans(
                            &chunk.text,
                            &clusters,
                            &break_opportunities(&chunk.text, &options),
                        ))
                    } else {
                        None
                    };
                for (position, span) in normal_spans.iter().enumerate() {
                    let break_before = if position == 0 {
                        previous
                            .is_some_and(|previous| boundary_break_allowed(items, previous, index))
                    } else {
                        true
                    };
                    builder.place_text(index, span, break_before, emergency_spans.as_deref());
                }
                previous = Some(index);
            }
        }
    }
    // 末尾：有内容才成行；末尾是硬断行时保留其后的空行（\n 数 + 1）
    let trailing_break = matches!(items.last(), Some(InlineItem::Break));
    if !builder.placed.is_empty() || trailing_break {
        builder.flush();
    }
    builder.lines
}

/// 断行状态：已放置片段、行内游标与行盒输出。
struct LineBuilder<'a> {
    shaper: &'a dyn TextShaper,
    items: &'a [InlineItem],
    shaped_items: Vec<Option<ShapedText>>,
    max_width: f32,
    strut_above: f32,
    strut_below: f32,
    lines: Vec<LineFragment>,
    placed: Vec<PlacedItem>,
    cursor: f32,
    /// 当前行的起点来源（写入正在构建的行）。
    current_start: LineStart,
    /// 下一次 `flush` 之后新行的起点来源。
    next_start: LineStart,
}

impl LineBuilder<'_> {
    /// 一个文本块样式下的字体度量。
    fn metrics(&self, style: &ComputedStyle) -> FontMetrics {
        self.shaper
            .metrics(&text_style(style))
            .unwrap_or(FontMetrics {
                ascent: 0.0,
                descent: 0.0,
                line_height: 0.0,
            })
    }

    /// 文本片段在「行首裁前导空格」后的有效区间与宽度。
    fn prepare_span(
        &self,
        item_index: usize,
        span: &Span,
        at_line_start: bool,
    ) -> Option<(usize, usize, f32)> {
        let InlineItem::Text(chunk) = &self.items[item_index] else {
            return None;
        };
        let (start, end) = if at_line_start && chunk.style.white_space.collapses() {
            trim_leading_spaces(&chunk.text, span.byte_start, span.byte_end)
        } else {
            (span.byte_start, span.byte_end)
        };
        if start >= end {
            return None;
        }
        let shaped = self.shaped_items[item_index].as_ref()?;
        let glyphs = glyph_range(shaped, start, end);
        if glyphs.is_empty() {
            return None;
        }
        Some((start, end, glyph_width(shaped, &glyphs)))
    }

    /// 放置一个文本片段（行内游标处；调用方已处理断行决策）。
    fn place_span_at(&mut self, item_index: usize, start: usize, end: usize) {
        let glyphs = {
            let shaped = self.shaped_items[item_index]
                .as_ref()
                .expect("text item shaped");
            glyph_range(shaped, start, end)
        };
        if glyphs.is_empty() {
            return;
        }
        let width = glyph_width(
            self.shaped_items[item_index].as_ref().expect("shaped"),
            &glyphs,
        );
        self.placed.push(PlacedItem {
            item_index,
            span: Some((start, end)),
            glyphs: Some(glyphs),
            x: self.cursor,
        });
        self.cursor += width;
    }

    /// 放置一段文本：按需在当前行放不下时换行；整段比一行还宽且启用了
    /// 紧急断点时按 cluster 切开放置。
    fn place_text(
        &mut self,
        item_index: usize,
        span: &Span,
        break_before: bool,
        emergency: Option<&[Span]>,
    ) {
        let at_line_start = self.placed.is_empty();
        let Some((mut start, end, width)) = self.prepare_span(item_index, span, at_line_start)
        else {
            return;
        };
        if !self.placed.is_empty() && self.cursor + width > self.max_width {
            if break_before {
                self.mark_soft_break(Some(item_index), span.byte_start);
                self.flush();
                let Some((new_start, new_end, new_width)) =
                    self.prepare_span(item_index, span, true)
                else {
                    return;
                };
                start = new_start;
                if new_width > self.max_width
                    && let Some(emergency) = emergency
                {
                    self.place_emergency(item_index, new_start, new_end, emergency);
                    return;
                }
                self.place_span_at(item_index, start, new_end);
                return;
            }
            if let Some(emergency) = emergency {
                self.place_emergency(item_index, start, end, emergency);
                return;
            }
            // 不可断且放不下：溢出放置（保留旧行为）
        } else if self.placed.is_empty()
            && width > self.max_width
            && let Some(emergency) = emergency
        {
            self.place_emergency(item_index, start, end, emergency);
            return;
        }
        self.place_span_at(item_index, start, end);
    }

    /// 把一段放不下的文本按紧急断点（cluster 边界）切开贪心放置。
    fn place_emergency(&mut self, item_index: usize, start: usize, end: usize, spans: &[Span]) {
        let mut points = vec![start];
        for span in spans {
            if span.byte_start > start && span.byte_start < end {
                points.push(span.byte_start);
            }
        }
        points.push(end);
        points.sort_unstable();
        points.dedup();
        for window in points.windows(2) {
            let part = Span {
                byte_start: window[0],
                byte_end: window[1],
            };
            if part.byte_start >= part.byte_end {
                continue;
            }
            let at_line_start = self.placed.is_empty();
            let Some((start, end, width)) = self.prepare_span(item_index, &part, at_line_start)
            else {
                continue;
            };
            if !self.placed.is_empty() && self.cursor + width > self.max_width {
                self.mark_soft_break(Some(item_index), part.byte_start);
                self.flush();
                // 新行是空的：本段即使超宽也直接放置（单个 cluster 不可再拆）
                let Some((start, end, _)) = self.prepare_span(item_index, &part, true) else {
                    continue;
                };
                self.place_span_at(item_index, start, end);
                continue;
            }
            self.place_span_at(item_index, start, end);
        }
    }

    /// 放置一个原子项（图片 / 原子盒）：不可拆，放不下先换行。
    fn place_atomic(&mut self, item_index: usize, width: f32, break_before: bool) {
        if !self.placed.is_empty() && self.cursor + width > self.max_width && break_before {
            self.mark_soft_break(None, 0);
            self.flush();
        }
        self.placed.push(PlacedItem {
            item_index,
            span: None,
            glyphs: None,
            x: self.cursor,
        });
        self.cursor += width;
    }

    /// 记录软换行，并判断断点处是否消费了一个可折叠空格。
    ///
    /// 软换行不插换行符；若断点处丢掉了行尾折叠空格，复制文本时补回一个空格。
    fn mark_soft_break(&mut self, item_index: Option<usize>, next_start: usize) {
        let space = self.break_consumed_space(item_index, next_start);
        self.next_start = LineStart::Soft { space };
    }

    fn break_consumed_space(&self, item_index: Option<usize>, next_start: usize) -> bool {
        if let Some(index) = item_index
            && let InlineItem::Text(chunk) = &self.items[index]
            && chunk.style.white_space.collapses()
            && chunk
                .text
                .get(next_start..)
                .is_some_and(|tail| tail.starts_with(' '))
        {
            return true;
        }
        if let Some(placed) = self.placed.last()
            && let InlineItem::Text(chunk) = &self.items[placed.item_index]
            && let Some((_, end)) = placed.span
        {
            return chunk.style.white_space.collapses()
                && chunk
                    .text
                    .get(..end)
                    .is_some_and(|head| head.ends_with(' '));
        }
        false
    }

    /// 结束当前行（空行也保留：硬断行与 `<pre>` 的 `\n`）。
    fn flush(&mut self) {
        // 行盒几何：baseline = max(各 run 上半部, strut 上半部)（CSS 2.1 §10.8）
        let mut baseline = self.strut_above;
        let mut below = self.strut_below;
        for placed_item in &self.placed {
            match &self.items[placed_item.item_index] {
                InlineItem::Text(chunk) => {
                    let metrics = self.metrics(&chunk.style);
                    let line_height = used_line_height(&chunk.style, metrics);
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
                InlineItem::Break => {}
            }
        }

        let mut runs: Vec<TextRun> = Vec::new();
        let mut images: Vec<ImageRun> = Vec::new();
        let mut boxes: Vec<Fragment> = Vec::new();
        // 同源文本合并进同一 run，但不得跨越中间的图片（绘制顺序会被打乱）
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
        let last_index = self.placed.len().saturating_sub(1);
        for (position, placed_item) in self.placed.iter().enumerate() {
            match &self.items[placed_item.item_index] {
                InlineItem::Text(chunk) => {
                    let shaped = self.shaped_items[placed_item.item_index]
                        .as_ref()
                        .expect("text item shaped");
                    let mut glyphs = placed_item.glyphs.clone().expect("text has glyphs");
                    // 行尾：折叠模式丢弃尾部空格（CSS Text §4.1 Phase II）
                    if position == last_index
                        && chunk.style.white_space.collapses()
                        && let Some((start, end)) = placed_item.span
                    {
                        let stop = trailing_space_start(&chunk.text, start, end);
                        if stop < end {
                            let cut = shaped
                                .glyphs
                                .partition_point(|glyph| (glyph.cluster as usize) < stop);
                            glyphs.end = cut.max(glyphs.start);
                        }
                    }
                    if glyphs.is_empty() {
                        mergeable = false;
                        continue;
                    }
                    let base = shaped.glyphs[glyphs.start].x;
                    let glyphs: Vec<Glyph> = shaped.glyphs[glyphs.clone()]
                        .iter()
                        .map(|glyph| Glyph {
                            id: glyph.id,
                            x: glyph.x - base + placed_item.x,
                            y: glyph.y,
                            advance: glyph.advance,
                            font: glyph.font,
                            cluster: glyph.cluster,
                        })
                        .collect();
                    match runs.last_mut() {
                        Some(run)
                            if mergeable
                                && run.node == chunk.node
                                && run.text_node == chunk.text_node =>
                        {
                            for glyph in glyphs {
                                run.glyphs
                                    .push(remap_glyph(glyph, &shaped.fonts, &mut run.fonts));
                            }
                        }
                        _ => runs.push(TextRun {
                            node: chunk.node,
                            text_node: chunk.text_node,
                            style: text_style(&chunk.style),
                            color: chunk.style.color,
                            text: Arc::clone(&chunk.text),
                            source_map: Arc::clone(&chunk.source_map),
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
                InlineItem::Break => {}
            }
        }
        self.lines.push(LineFragment {
            baseline,
            height: baseline + below,
            start: std::mem::replace(
                &mut self.current_start,
                std::mem::replace(&mut self.next_start, LineStart::Soft { space: false }),
            ),
            runs,
            images,
            boxes,
        });
        self.placed.clear();
        self.cursor = 0.0;
    }
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
