//! Nexty 绘制 facade：自有绘制指令 + 可替换光栅后端。
//!
//! 上游：`vello_cpu`（CPU）与 `vello_hybrid`（CPU/GPU 混合）。两者是**同形不同名**
//! 的 API（`RenderContext` 与 `Scene`），本 crate 负责适配。决策与调研见
//! `docs/decisions/2026-10-01-crate-selection.md`。
//!
//! 上游类型一律不得出现在本 crate 的 pub 导出中（AGENTS.md 硬规则）。
//!
//! **渲染隔离**：上游对未支持特性是 panic 而非返回错误，故渲染必须在独立线程
//! 执行并由 `catch_unwind` 兜底。因此 workspace 与各 crate 一律不得设置
//! `panic = "abort"`（会禁用 unwind，使兜底失效）。

#![forbid(unsafe_code)]

/// 像素尺寸。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    /// 宽度，px。
    pub width: u32,
    /// 高度，px。
    pub height: u32,
}

/// 非预乘 RGBA 颜色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    /// 红通道。
    pub r: u8,
    /// 绿通道。
    pub g: u8,
    /// 蓝通道。
    pub b: u8,
    /// alpha 通道。
    pub a: u8,
}

/// 轴对齐矩形。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// 左上角 x，px。
    pub x: f32,
    /// 左上角 y，px。
    pub y: f32,
    /// 宽，px。
    pub width: f32,
    /// 高，px。
    pub height: f32,
}

/// 一条绘制指令。
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// 用纯色填充矩形。
    FillRect {
        /// 目标矩形。
        rect: Rect,
        /// 填充色。
        color: Color,
    },
}

/// 一帧的绘制指令列表。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    /// 按绘制顺序排列的指令。
    pub commands: Vec<Command>,
}

/// 光栅化结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pixmap {
    /// 尺寸。
    pub size: Size,
    /// 非预乘 RGBA8 像素，行优先。
    pub data: Vec<u8>,
}

/// 光栅化后端。
///
/// 实现方可以是 `vello_cpu`（CPU）或 `vello_hybrid`（CPU/GPU 混合）。
/// 调用方须在独立线程中执行并在 `catch_unwind` 内兜底。
pub trait Rasterizer {
    /// 把 [`Scene`] 光栅化为 [`Pixmap`]。
    ///
    /// # Errors
    ///
    /// 尺寸非法或后端不支持场景中的指令时返回 [`RasterError`]。
    fn rasterize(&self, scene: &Scene, size: Size) -> Result<Pixmap, RasterError>;
}

/// 光栅化错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RasterError {
    /// 宽或高为 0。
    EmptySize,
    /// 后端不支持场景中的指令。
    UnsupportedCommand,
}
