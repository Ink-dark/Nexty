//! 片段树：布局的输出模型。
//!
//! 布局把 arena DOM + computed style 转换为一片「盒子片段」树：每个生成盒
//! （元素盒或匿名块盒）携带边框盒几何、盒模型宽度与行内文本行，供 paint 层
//! 直接消费。几何坐标系：每个片段的 [`Fragment::border_box`] 相对**其包含块的
//! 内容盒**左上角；文本行相对片段自身内容盒。

use nexty_css::{ComputedStyle, Rgba};
use nexty_dom::NodeId;
use nexty_text::{Glyph, TextStyle};

/// 几何矩形（CSS px）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// 左上角 x。
    pub x: f32,
    /// 左上角 y。
    pub y: f32,
    /// 宽。
    pub width: f32,
    /// 高。
    pub height: f32,
}

/// 四边尺寸（CSS px）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Edges {
    /// 上。
    pub top: f32,
    /// 右。
    pub right: f32,
    /// 下。
    pub bottom: f32,
    /// 左。
    pub left: f32,
}

/// 一个生成盒的片段。
#[derive(Debug, Clone, PartialEq)]
pub struct Fragment {
    /// 对应元素节点；匿名块盒指向其所属的父元素。
    pub node: NodeId,
    /// 是否为匿名块盒（CSS 2.1 §9.2.1.1；无对应元素，绘制时按透明处理）。
    pub anonymous: bool,
    /// 边框盒，相对包含块内容盒左上角。
    pub border_box: Rect,
    /// 四边 border 宽度。
    pub border: Edges,
    /// 四边 padding（used value）。
    pub padding: Edges,
    /// 生成该盒的元素的 computed style（匿名盒为其父元素样式）。
    pub style: ComputedStyle,
    /// 块级子片段。
    pub children: Vec<Fragment>,
    /// 行内内容（该盒的文本行；与块级子片段互斥出现）。
    pub lines: Vec<LineFragment>,
}

/// 一行文本。
#[derive(Debug, Clone, PartialEq)]
pub struct LineFragment {
    /// 基线相对片段内容盒顶部的 y。
    pub baseline: f32,
    /// 行高（strut 与行内盒共同决定）。
    pub height: f32,
    /// 行内文本 run（按绘制顺序）。
    pub runs: Vec<TextRun>,
    /// 行内原子图片（替换元素，按基线对齐：底边落在基线上）。
    pub images: Vec<ImageRun>,
}

/// 行内一张图片的落点。
///
/// 坐标系与 [`LineFragment`] 一致：x/y 相对所属片段**内容盒**左上角，
/// y 已按基线对齐折算（`baseline - height`）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageRun {
    /// 图片来源元素（`<img>` 节点）。
    pub node: NodeId,
    /// 左缘 x。
    pub x: f32,
    /// 顶缘 y。
    pub y: f32,
    /// 显示宽。
    pub width: f32,
    /// 显示高。
    pub height: f32,
}

/// 行内一段同源样式的文本 run。
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    /// 产生这些字形的元素节点（样式来源）。
    pub node: NodeId,
    /// 该 run 的文本样式（字体族/字号，供 paint 选字体）。
    pub style: TextStyle,
    /// 文本颜色（run 元素的 computed `color`）。
    pub color: Rgba,
    /// 字形：x 相对片段内容盒左缘，y 相对**行基线**。
    pub glyphs: Vec<Glyph>,
}
