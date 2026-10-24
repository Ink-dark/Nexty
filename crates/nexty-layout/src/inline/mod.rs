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
//! 保留，制表按 `tab-size` 推进（见下）。
//!
//! # 间距施加（CSS Text 3 §8）
//!
//! `letter-spacing` / `word-spacing` 是**放置期修正**：每块文本只整形一次，
//! 间距不改整形结果，在 `build_lines` 的簇分组上施加——每个簇的 advance
//! += letter-spacing（word-separator 簇另计 word-spacing，收缩后不小于 0），
//! 行末簇的 letter-spacing 不计入行宽（断行判定与对齐用净宽）。间距不产生
//! 也不消灭断行机会。偏差：字距只在同一文本块的簇边界之间施加，跨行内
//! 元素边界（`<b>x</b>y` 的 x→y）不加距；相邻块间游标保留前块行尾字距
//! （对齐浏览器在元素边界施加字距的行为）。
//!
//! # 制表与行尾悬挂（CSS Text 3 §5 / white-space 处理）
//!
//! 保留模式的制表符按一个空格整形，放置期把该簇的 advance 替换为「推进到
//! `tab-size` × 空格 advance 的下一整数倍」（`tab-size: 0` 零宽；整数形态
//! 依赖字体度量，`tab-size: <length>` 为固定步长）；折叠模式的制表照常
//! 折叠。`pre-wrap` 软折行/硬断行前的行尾空格**悬挂**：字形不进入 run
//! （不占行宽、可溢出边沿、不触发折行），字节区间保留——复制文本与选区
//! 源映射完整；`break-spaces` 不悬挂（照常占宽），`pre` 硬断行前空格不
//! 悬挂（沿用占宽）。悬挂空格的字形簇不在 TextMap 中（命中吸附到相邻
//! 可见簇），选区矩形不包含它们。
//!
//! # 两端对齐（CSS Text 3 §7）
//!
//! `text-align: justify` 同样是放置期修正：先按现有断行成行（断行结果
//! 不变），软折行收尾的非末行在 flush 期把自由空间均匀分配到机会点。
//! 机会点是 CSS §7 `auto` 方法的可执行近似：折叠空格字形（软折行消费的
//! 可折叠空格处）+ 同块簇边界（`split_spans` 只在 UAX#14 断点切段）。
//! 末行与硬断行前的行不分配（§7.3，按 start 对齐）。偏差：不做 inter-word
//! 空格收缩、不做 `text-justify` 分档；保留空白模式（pre-wrap /
//! break-spaces）的空格不设空格机会；跨行内元素边界不加距；
//! `justify-all` / `match-parent` 降级为 start。measure 路径
//! （min/max-content）不分配。
//!
//! # 首行缩进（CSS Text 3 §4）
//!
//! `text-indent` 只作用于块容器的首个格式化行（每容器一次；run 叶按
//! 「容器首 run」判定，flex 匿名 item 全部视为首 run）。缩进从首行可用宽
//! 中扣减（影响首行断行），并与对齐正交：先缩进，后 center/right/justify。
//! min/max-content 测量不含缩进（css-sizing 语义）。percentage 按行盒的
//! 包含块（即块容器自身内容盒宽，= run 叶可用宽）折算——与 Chromium 一致
//! （实机对照：420px 宽元素上 `10%` 两端引擎同为 ~42px）。
//! `each-line` / `hanging` 关键字未实现。
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
//! bidi 未实现；跨行内元素边界的断行上下文只回看两个字符
//! （见 `uax14::rules::breaks_between_text`）；行内盒被嵌套块级子元素拆分
//! 时（CSS 2.2 §9.2.1.1），其 border / background 只画在各半自身范围，
//! 不跨匿名块连续。
//!
//! feature `block` 关闭时本模块无消费方（行内流的唯一下游是 taffy 管线），
//! 按模块整体抑制 dead_code；`replaced` 单独开启时 `lib.rs` 不 re-export 本模块
//! 的公共项，故一并抑制 unused_imports。

#![cfg_attr(not(feature = "block"), allow(dead_code, unused_imports))]

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use nexty_css::{
    ComputedStyle, DisplayValue, LineHeightValue, MAX_CSS_LENGTH, OverflowWrapValue, SpacingValue,
    TabSizeValue, TextAlignValue, WhiteSpaceValue, WordBreakValue,
};
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

/// 行内收集器产出的一段：连续行内项，或行内流中遇到的块级子元素标记
/// （CSS 2.2 §9.2.1.1：块级子盒把行内流拆成两段，各成匿名块）。
pub(crate) enum InlineSegment {
    Inline(Vec<InlineItem>),
    /// 块级子元素节点（按普通块盒布局）。
    Block(NodeId),
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
    /// `text` 中由制表符映射来的空格的字节偏移（保留模式；放置期按
    /// `tab-size` 推进到下一制表位，CSS Text 3 §5）。
    pub tabs: Arc<[u32]>,
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

/// 尺寸属性 → px（WHATWG HTML §2.4.4.2 非负整数规则：跳过前导空白、可选
/// `+` 号、ASCII 数字串、尾部垃圾忽略；小数点不属于非负整数，`40.5` → 40）。
///
/// 结果必须有限、`> 0` 且 ≤ [`MAX_CSS_LENGTH`]（与 css 值层同一上限），否则
/// 返回 `None` 回落自然尺寸——`<img width="999…9">` 式巨数属性注入的 `inf`
/// 在此被挡在布局之外。
fn attribute_px(data: &ElementData, name: &str) -> Option<f32> {
    let raw = data
        .attributes
        .iter()
        .find(|attr| attr.name == name)?
        .value
        .trim();
    let raw = raw.strip_prefix('+').unwrap_or(raw);
    let digits_end = raw
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(raw.len());
    if digits_end == 0 {
        return None;
    }
    let mut integer: u64 = 0;
    for byte in &raw.as_bytes()[..digits_end] {
        integer = integer
            .saturating_mul(10)
            .saturating_add(u64::from(byte - b'0'));
    }
    let value = integer as f32;
    if value.is_finite() && value > 0.0 && value <= MAX_CSS_LENGTH {
        Some(value)
    } else {
        None
    }
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
    /// 收集产物：行内项段（与块级子元素标记按 DOM 序交替）。
    items: Vec<InlineSegment>,
    /// 上一个发出的字符是否为折叠空格（跨文本节点去重）。
    last_collapsible_space: bool,
    /// 上一个折叠空格所属的文本节点（用于把源区间扩展到整段空白）。
    last_space_node: Option<NodeId>,
    text_buffer: String,
    text_source: Vec<SourceChar>,
    /// 当前缓冲文本中制表符映射空格的字节偏移。
    text_tabs: Vec<u32>,
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

    /// 把一个行内项追加到当前 Inline 段（无段则新开一段）。
    fn push_item(&mut self, item: InlineItem) {
        match self.items.last_mut() {
            Some(InlineSegment::Inline(items)) => items.push(item),
            _ => self.items.push(InlineSegment::Inline(vec![item])),
        }
    }

    /// 把缓冲中的文本刷入结果。
    fn flush_text(&mut self) {
        if self.text_buffer.is_empty() {
            return;
        }
        let chunk = TextChunk {
            node: self
                .text_style_node
                .take()
                .expect("text has a style source"),
            text_node: self.text_node.take().expect("text has a source node"),
            text: Arc::from(std::mem::take(&mut self.text_buffer)),
            source_map: Arc::from(std::mem::take(&mut self.text_source)),
            tabs: Arc::from(std::mem::take(&mut self.text_tabs)),
            style: self.text_style.take().expect("text has a style"),
        };
        self.push_item(InlineItem::Text(chunk));
        self.last_space_node = None;
    }

    /// 行内收集途中遇到块级子元素：终止当前 run（CSS 2.2 §9.2.1.1），
    /// 前半 run 与后半 run 各自包装成匿名块，块级子元素按普通块盒布局。
    /// 偏差：拆分处行内盒的 border / background 只画在各半自身范围。
    fn push_block_child(&mut self, node: NodeId) {
        self.flush_text();
        self.items.push(InlineSegment::Block(node));
        self.last_collapsible_space = false;
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
                    // 保留模式的制表符按一个空格整形，放置期按 tab-size
                    // 推进到下一制表位（CSS Text 3 §5）
                    '\t' => {
                        self.ensure_style(text_node, style_node, style);
                        self.text_tabs.push(self.text_buffer.len() as u32);
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
        self.push_item(InlineItem::Break);
        self.last_collapsible_space = false;
        self.last_space_node = None;
    }

    /// 收集一个行内原子图片。
    fn push_image(&mut self, node: NodeId, style: &ComputedStyle) {
        let (width, height) = image_box_size(self.ctx.document, node, self.ctx.image_sizes);
        self.flush_text();
        self.push_item(InlineItem::Image(AtomicImage {
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
        self.push_item(InlineItem::Block(Box::new(AtomicBlock {
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
///
/// `first_line_indent` 记录本 run 是否为其块容器的第一个 run（text-indent
/// 只作用于容器的首个格式化行，CSS Text 3 §4）；flex 走法下每个匿名 item
/// 都是独立块容器，全部按 first 处理。
pub(crate) struct InlineRun<'a> {
    collector: Collector<'a>,
    first_line_indent: bool,
}

impl<'a> InlineRun<'a> {
    pub(crate) fn new(ctx: &'a Context<'a>, first_line_indent: bool) -> Self {
        Self {
            collector: Collector {
                ctx,
                items: Vec::new(),
                last_collapsible_space: false,
                last_space_node: None,
                text_buffer: String::new(),
                text_source: Vec::new(),
                text_tabs: Vec::new(),
                text_node: None,
                text_style_node: None,
                text_style: None,
            },
            first_line_indent,
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

    /// 结束收集，刷出文本缓冲。产出按块级标记切分的段：纯折叠空白
    ///（且没有其他行内项）的 Inline 段丢弃，块级标记保留。
    /// 返回段列表与「是否容器首 run」（供 text-indent 使用）。
    pub(crate) fn finish(mut self) -> (Vec<InlineSegment>, bool) {
        self.collector.flush_text();
        let segments: Vec<InlineSegment> = self
            .collector
            .items
            .into_iter()
            .filter(|segment| match segment {
                InlineSegment::Inline(items) => items.iter().any(|item| match item {
                    InlineItem::Text(chunk) => {
                        !chunk.style.white_space.collapses()
                            || !chunk.text.chars().all(|character| character == ' ')
                            || !chunk.tabs.is_empty()
                    }
                    _ => true,
                }),
                InlineSegment::Block(_) => true,
            })
            .collect();
        (segments, self.first_line_indent)
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
                    // 块级子元素终止当前行内流（CSS 2.2 §9.2.1.1）：前半 run
                    // 与后半 run 各自包装成匿名块，块级子元素按普通块盒布局
                    _ => collector.push_block_child(child),
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

/// 文本间距参数（CSS Text 3 §8）。
struct Spacing {
    /// letter-spacing：每个字形簇 advance 的附加量（CSS px）。
    letter: f32,
    /// word-spacing：word-separator 簇 advance 的附加量（CSS px）。
    word: f32,
}

impl Spacing {
    fn of(style: &ComputedStyle) -> Self {
        Self {
            letter: style.letter_spacing,
            word: match style.word_spacing {
                SpacingValue::Normal => 0.0,
                SpacingValue::Length(px) => px,
                // computed 阶段已折算；防御性处理（同 used_line_height 兜底口径）
                SpacingValue::Em(em) => em * style.font_size,
            },
        }
    }
}

/// CSS Text 3 §8.2 的 word-separator 字符（U+0020 / U+00A0 / U+3000 与
/// 词条列举的其余分隔符；偏离规范原文额外收编 U+3000，服务中文排版）。
fn is_word_separator(character: char) -> bool {
    matches!(
        character,
        ' ' | '\u{00A0}'
            | '\u{3000}'
            | '\u{1361}'
            | '\u{10100}'
            | '\u{10101}'
            | '\u{1039F}'
            | '\u{1091F}'
    )
}

/// glyph 区间内的一个字形簇分组。
struct ClusterGroup {
    /// 簇的字节区间（chunk 文本内）。
    bytes: Range<usize>,
    /// 簇内字形在 glyph 区间中的下标。
    glyphs: Range<usize>,
    /// 簇的基准 advance 合计（无间距）。
    base: f32,
    /// 是否为 word-separator 簇（制表符映射的空格不算）。
    separator: bool,
    /// 是否为制表符映射的空格簇（advance 按 tab-size 推进）。
    tab: bool,
}

/// 把 glyph 区间按 cluster 切组；簇不跨区间（span 只在 cluster 边界切开）。
/// `span_end` 是区间的字节终点（chunk 文本内），用于末簇的字节区间；
/// `tabs` 是制表符映射空格的字节偏移（升序）。
fn cluster_groups(
    shaped: &ShapedText,
    glyphs: &Range<usize>,
    span_end: usize,
    text: &str,
    tabs: &[u32],
) -> Vec<ClusterGroup> {
    let mut groups: Vec<ClusterGroup> = Vec::new();
    for index in glyphs.clone() {
        let glyph = &shaped.glyphs[index];
        match groups.last_mut() {
            Some(group) if shaped.glyphs[group.glyphs.start].cluster == glyph.cluster => {
                group.glyphs.end = index + 1;
                group.base += glyph.advance;
            }
            _ => groups.push(ClusterGroup {
                bytes: (glyph.cluster as usize)..(glyph.cluster as usize),
                glyphs: index..index + 1,
                base: glyph.advance,
                separator: false,
                tab: false,
            }),
        }
    }
    for index in 0..groups.len().saturating_sub(1) {
        groups[index].bytes.end = groups[index + 1].bytes.start;
    }
    if let Some(last) = groups.last_mut() {
        last.bytes.end = span_end.min(text.len());
    }
    for group in &mut groups {
        group.tab = tabs.binary_search(&(group.bytes.start as u32)).is_ok();
        group.separator = !group.tab
            && text
                .get(group.bytes.start..group.bytes.end)
                .is_some_and(|bytes| !bytes.is_empty() && bytes.chars().all(is_word_separator));
    }
    groups
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
/// 的紧急断点不参与）。`first_line_indent` 是已解析的首行缩进（CSS Text
/// 3 §4；测量路径与后续行由调用方保证传 0）——从首行可用宽中扣减。
pub(crate) fn build_lines(
    shaper: &dyn TextShaper,
    items: &[InlineItem],
    max_width: f32,
    block_style: &ComputedStyle,
    min_content: bool,
    first_line_indent: f32,
) -> Vec<LineFragment> {
    if items.is_empty() {
        return Vec::new();
    }

    // 数值卫生兜底：NaN 按 0 降级（污染隔离在单元素）；+inf 是 MaxContent
    // 测量的合法「不折行」语义，保留（游标累加的都是有限宽度，坐标不扩散
    // 非有限值）。
    let max_width = if max_width.is_nan() { 0.0 } else { max_width };

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
        cursor: first_line_indent,
        current_start: LineStart::Soft { space: false },
        next_start: LineStart::Soft { space: false },
        trailing_letter_spacing: 0.0,
        justify: block_style.text_align == TextAlignValue::Justify && !min_content,
    };

    let mut previous: Option<usize> = None;
    for index in 0..items.len() {
        match &items[index] {
            InlineItem::Break => {
                // 硬断行：结束当前行（空行也保留），下一项从行首开始。
                // 该行以硬断行收尾，不做 justify 分配（CSS Text 3 §7.3）
                builder.next_start = LineStart::Hard;
                builder.flush(false);
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
    // 末尾：有内容才成行；末尾是硬断行时保留其后的空行（\n 数 + 1）。
    // 末行不做 justify 分配（CSS Text 3 §7.3：末行走 text-align-last，
    // 本层按 start 对齐）
    let trailing_break = matches!(items.last(), Some(InlineItem::Break));
    if !builder.placed.is_empty() || trailing_break {
        builder.flush(false);
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
    /// 行内最后一个已放置文本簇的 letter-spacing（行末扣减的量；原子项
    /// 落点归零）。游标在放置期含它，行宽在 flush 期扣除。
    trailing_letter_spacing: f32,
    /// `text-align: justify` 生效（measure 路径关闭；软折行 flush 时消费）。
    justify: bool,
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

    /// 一个文本片段的簇分组与每簇的 advance 增量。
    ///
    /// 增量 = letter-spacing（每簇）+ word-spacing（分隔符簇）+ 制表推进
    /// （制表簇 advance 到 `tab-size` 的下一整数倍，CSS Text 3 §5；依赖
    /// 簇的行内 x，故 `start_x` 必须是该片段的放置 x）。
    fn span_groups_deltas(
        &self,
        item_index: usize,
        glyphs: &Range<usize>,
        span_end: usize,
        start_x: f32,
    ) -> (Vec<ClusterGroup>, Vec<f32>) {
        let InlineItem::Text(chunk) = &self.items[item_index] else {
            return (Vec::new(), Vec::new());
        };
        let Some(shaped) = self.shaped_items[item_index].as_ref() else {
            return (Vec::new(), Vec::new());
        };
        let mut groups = cluster_groups(shaped, glyphs, span_end, &chunk.text, &chunk.tabs);
        let spacing = Spacing::of(&chunk.style);
        let mut deltas = Vec::with_capacity(groups.len());
        let mut x = start_x;
        for group in &mut groups {
            let mut delta = spacing.letter;
            if group.separator {
                delta += (group.base + spacing.word).max(0.0) - group.base;
            }
            if group.tab {
                let stop = match chunk.style.tab_size {
                    TabSizeValue::Integer(count) => (count as f32 * group.base).min(MAX_CSS_LENGTH),
                    TabSizeValue::Length(px) => px.min(MAX_CSS_LENGTH),
                    // computed 阶段已折算；防御按一个空格
                    TabSizeValue::Em(_) => group.base,
                };
                let advance = if stop <= 0.0 {
                    0.0
                } else {
                    let remainder = if x.is_finite() && x > 0.0 {
                        x % stop
                    } else {
                        0.0
                    };
                    if remainder < 0.01 {
                        stop
                    } else {
                        stop - remainder
                    }
                };
                delta += advance - group.base;
            }
            x += group.base + delta;
            deltas.push(delta);
        }
        (groups, deltas)
    }

    /// 文本片段在「行首裁前导空格」后的有效区间与**净宽**。
    ///
    /// 净宽已扣除本片段末簇的 letter-spacing（行末字距不计行宽，断行判定
    /// 与对齐都用净宽，CSS Text 3 §8.1）；word-spacing 的分隔符修正与
    /// 制表推进计入。
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
        let spacing = Spacing::of(&chunk.style);
        let (_, deltas) = self.span_groups_deltas(item_index, &glyphs, end, self.cursor);
        let total = glyph_width(shaped, &glyphs) + deltas.iter().sum::<f32>();
        Some((start, end, total - spacing.letter))
    }

    /// 放置一个文本片段（行内游标处；调用方已处理断行决策）。
    ///
    /// 游标推进计全部间距与制表推进（含本片段末簇的 letter-spacing——它与
    /// 下一片段之间的间隙是真实间距）；行末扣减在 `flush` 做。
    fn place_span_at(&mut self, item_index: usize, start: usize, end: usize) {
        let InlineItem::Text(chunk) = &self.items[item_index] else {
            return;
        };
        let glyphs = {
            let shaped = self.shaped_items[item_index]
                .as_ref()
                .expect("text item shaped");
            glyph_range(shaped, start, end)
        };
        if glyphs.is_empty() {
            return;
        }
        let spacing = Spacing::of(&chunk.style);
        let width = {
            let shaped = self.shaped_items[item_index].as_ref().expect("shaped");
            let (_, deltas) = self.span_groups_deltas(item_index, &glyphs, end, self.cursor);
            glyph_width(shaped, &glyphs) + deltas.iter().sum::<f32>()
        };
        self.placed.push(PlacedItem {
            item_index,
            span: Some((start, end)),
            glyphs: Some(glyphs),
            x: self.cursor,
        });
        self.cursor += width;
        self.trailing_letter_spacing = spacing.letter;
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
                self.flush(self.justify);
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
                self.flush(self.justify);
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
            self.flush(self.justify);
        }
        self.placed.push(PlacedItem {
            item_index,
            span: None,
            glyphs: None,
            x: self.cursor,
        });
        self.cursor += width;
        self.trailing_letter_spacing = 0.0;
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

    /// 边界 `index`（已放置项 `index` 与 `index + 1` 之间）是否为 justify
    /// 机会点：折叠空格字形（软折行消费的可折叠空格处），或同块簇边界
    /// （`split_spans` 只在 UAX#14 断点切段，排除行首天然成立）。
    /// 跨块边界不加距；保留空白模式的空格不设空格机会（偏差记账）。
    fn justify_opportunity(&self, index: usize) -> bool {
        let placed = &self.placed[index];
        let InlineItem::Text(chunk) = &self.items[placed.item_index] else {
            return false;
        };
        if chunk.style.white_space.collapses()
            && let Some((_, end)) = placed.span
            && chunk
                .text
                .get(..end)
                .is_some_and(|head| head.ends_with(' '))
        {
            return true;
        }
        self.placed[index + 1].item_index == placed.item_index
    }

    /// 结束当前行（空行也保留：硬断行与 `<pre>` 的 `\n`）。
    ///
    /// `justify` 为真时（调用方保证：软折行收尾、非末行、`text-align:
    /// justify` 且非测量路径）在机会点均匀分配自由空间（CSS Text 3 §7
    /// auto 方法的可执行近似，偏差见模块文档）。
    fn flush(&mut self, justify: bool) {
        // justify 分配：先按现有断行成行（自由空间在放置期已知），再算
        // 每个放置项的 x 平移量与其末字形的 advance 增量
        let mut shifts = vec![0.0_f32; self.placed.len()];
        let mut boosts = vec![0.0_f32; self.placed.len()];
        if justify && self.placed.len() > 1 {
            let mut net = self.cursor - self.trailing_letter_spacing;
            // 行尾被裁掉（折叠）或悬挂（pre-wrap）的空格不占行宽
            if let Some(last) = self.placed.last()
                && let InlineItem::Text(chunk) = &self.items[last.item_index]
                && let (Some((start, end)), Some(range)) = (last.span, last.glyphs.as_ref())
            {
                let stop = trailing_space_start(&chunk.text, start, end);
                if stop < end
                    && let Some(shaped) = self.shaped_items[last.item_index].as_ref()
                {
                    let cut = shaped
                        .glyphs
                        .partition_point(|glyph| (glyph.cluster as usize) < stop);
                    if cut < range.end {
                        let (groups, deltas) = self.span_groups_deltas(
                            last.item_index,
                            &(range.start..range.end),
                            end,
                            last.x,
                        );
                        let mut excluded = 0.0_f32;
                        for (group, delta) in groups.iter().zip(&deltas) {
                            if group.glyphs.start >= cut {
                                excluded += group.base + delta;
                            }
                        }
                        net -= excluded;
                    }
                }
            }
            let free = self.max_width - net;
            if free > 0.0 && free.is_finite() {
                let count = (0..self.placed.len() - 1)
                    .filter(|&boundary| self.justify_opportunity(boundary))
                    .count();
                if count > 0 {
                    let extra = free / count as f32;
                    let mut shift = 0.0_f32;
                    for index in 0..self.placed.len() {
                        shifts[index] = shift;
                        if index + 1 < self.placed.len() && self.justify_opportunity(index) {
                            boosts[index] += extra;
                            shift += extra;
                        }
                    }
                }
            }
        }

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
        // 行末字距扣减：最后一个放置项是文本且实际产字形时，从其 run 的
        // 末字形 advance 里扣掉 letter-spacing（行末簇的字距不计行宽）
        let mut final_letter: Option<f32> = None;
        for (position, placed_item) in self.placed.iter().enumerate() {
            match &self.items[placed_item.item_index] {
                InlineItem::Text(chunk) => {
                    let shaped = self.shaped_items[placed_item.item_index]
                        .as_ref()
                        .expect("text item shaped");
                    let mut glyphs = placed_item.glyphs.clone().expect("text has glyphs");
                    let (span_start, span_end) = placed_item.span.expect("text has span");
                    let mut visible_end = span_end;
                    // 行尾（仅最后一个放置项）：
                    // - 折叠模式丢弃尾部空格（CSS Text §4.1 Phase II）；
                    // - pre-wrap 的行尾空格悬挂（不占行宽可溢出边沿），但
                    //   字节区间保留——复制文本与选区源映射完整
                    if position == last_index {
                        if chunk.style.white_space.collapses() {
                            let stop = trailing_space_start(&chunk.text, span_start, span_end);
                            if stop < span_end {
                                let cut = shaped
                                    .glyphs
                                    .partition_point(|glyph| (glyph.cluster as usize) < stop);
                                glyphs.end = cut.max(glyphs.start);
                                visible_end = stop;
                            }
                        } else if chunk.style.white_space == WhiteSpaceValue::PreWrap {
                            let stop = trailing_space_start(&chunk.text, span_start, span_end);
                            if stop < span_end {
                                let cut = shaped
                                    .glyphs
                                    .partition_point(|glyph| (glyph.cluster as usize) < stop);
                                glyphs.end = cut.max(glyphs.start);
                            }
                        }
                    }
                    if glyphs.is_empty() {
                        mergeable = false;
                        continue;
                    }
                    // 间距与制表推进（CSS Text 3 §8/§5）：每簇 advance =
                    // base + delta（letter/word 间距、制表推进），簇坐标随
                    // 既有簇的增量右移
                    let (groups, deltas) = self.span_groups_deltas(
                        placed_item.item_index,
                        &glyphs,
                        visible_end,
                        placed_item.x,
                    );
                    let spacing = Spacing::of(&chunk.style);
                    let base = shaped.glyphs[glyphs.start].x;
                    let mut glyphs: Vec<Glyph> = Vec::with_capacity(glyphs.len());
                    let mut prior_boost = 0.0_f32;
                    for (group, delta) in groups.iter().zip(&deltas) {
                        let last_in_group = group.glyphs.end - 1;
                        for index in group.glyphs.clone() {
                            let glyph = &shaped.glyphs[index];
                            let advance = if index == last_in_group {
                                glyph.advance + delta
                            } else {
                                glyph.advance
                            };
                            glyphs.push(Glyph {
                                id: glyph.id,
                                x: glyph.x - base + placed_item.x + prior_boost + shifts[position],
                                y: glyph.y,
                                advance,
                                font: glyph.font,
                                cluster: glyph.cluster,
                            });
                        }
                        prior_boost += delta;
                    }
                    // justify 分配：机会点加在该项末字形 advance 上
                    if boosts[position] != 0.0
                        && let Some(glyph) = glyphs.last_mut()
                    {
                        glyph.advance += boosts[position];
                    }
                    if position == last_index {
                        final_letter = Some(spacing.letter);
                    }
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
                            run.text_end = visible_end as u32;
                        }
                        _ => runs.push(TextRun {
                            node: chunk.node,
                            text_node: chunk.text_node,
                            style: text_style(&chunk.style),
                            color: chunk.style.color,
                            text: Arc::clone(&chunk.text),
                            source_map: Arc::clone(&chunk.source_map),
                            text_start: span_start as u32,
                            text_end: visible_end as u32,
                            collapses_whitespace: chunk.style.white_space.collapses(),
                            fonts: shaped.fonts.clone(),
                            glyphs,
                        }),
                    }
                    mergeable = true;
                }
                InlineItem::Image(image) => {
                    images.push(ImageRun {
                        node: image.node,
                        x: placed_item.x + shifts[position],
                        y: baseline - image.height,
                        width: image.width,
                        height: image.height,
                    });
                    mergeable = false;
                }
                InlineItem::Block(atomic) => {
                    let mut fragment = atomic.fragment.clone();
                    // 底边对齐基线：x/y 相对所属片段内容盒
                    fragment.border_box.x = placed_item.x + shifts[position];
                    fragment.border_box.y = baseline - fragment.border_box.height;
                    boxes.push(fragment);
                    mergeable = false;
                }
                InlineItem::Break => {}
            }
        }
        // 行末簇的 letter-spacing 不计行宽：从末 run 的末字形 advance 扣除
        //（对齐与 line_width 由此拿到净宽；字形坐标不受影响）
        if let Some(letter) = final_letter
            && let Some(run) = runs.last_mut()
            && let Some(glyph) = run.glyphs.last_mut()
        {
            glyph.advance -= letter;
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
