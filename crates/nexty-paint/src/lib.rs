//! Nexty 绘制 facade：自有绘制指令 + 可替换光栅后端。
//!
//! 上游：`vello_cpu`（CPU 光栅，sparse strips）。GPU 后端 `vello_hybrid` 依赖
//! wgpu device/surface，由 chrome 层提供后接入——本轮从本 crate 依赖中摘除，
//! [`Rasterizer`] trait 即双后端接缝。决策与调研见
//! `docs/decisions/2026-10-01-crate-selection.md`。
//!
//! 指令集保持最小（当前仅 [`Command::FillRect`]），后续消费方（layout）需要
//! 什么指令再扩展什么。
//!
//! 上游类型一律不得出现在本 crate 的 pub 导出中（AGENTS.md 硬规则）。
//!
//! **渲染隔离**：上游对未支持特性是 panic 而非返回错误，故渲染必须在独立线程
//! 执行并由 `catch_unwind` 兜底（由 chrome 层持有）。因此 workspace 与各 crate
//! 一律不得设置 `panic = "abort"`（会禁用 unwind，使兜底失效）。

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

impl Color {
    /// 不透明纯色。
    #[must_use]
    pub const fn opaque(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 0xff }
    }
}

/// 轴对齐矩形。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// 左上角 x，px（可为负，越界部分被裁剪）。
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
    /// 用纯色填充矩形；后绘制的矩形按 src-over 混合叠加。
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
    /// 非预乘 RGBA8 像素，行优先；未被指令覆盖的区域为全透明。
    pub data: Vec<u8>,
}

impl Pixmap {
    /// 读取 `(x, y)` 处像素的 RGBA 分量。
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.size.width || y >= self.size.height {
            return None;
        }
        let index = (y as usize * self.size.width as usize + x as usize) * 4;
        Some([
            self.data[index],
            self.data[index + 1],
            self.data[index + 2],
            self.data[index + 3],
        ])
    }
}

/// 光栅化后端。
///
/// 实现方可以是 `vello_cpu`（CPU）或 `vello_hybrid`（CPU/GPU 混合，待接入）。
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RasterError {
    /// 宽或高为 0。
    EmptySize,
    /// 尺寸超出后端上限（vello_cpu 的渲染目标为 u16 像素）。
    SizeExceedsLimit,
    /// 后端不支持场景中的指令。
    UnsupportedCommand,
}

/// [`Rasterizer`] 的 vello_cpu 实现（CPU 光栅）。
///
/// `vello_cpu` 对未支持特性是 panic 而非返回错误；按渲染隔离规则，
/// 调用方须把本实现放进独立线程并以 `catch_unwind` 兜底。
#[derive(Debug, Default, Clone, Copy)]
pub struct VelloCpuRasterizer;

impl VelloCpuRasterizer {
    /// 创建 CPU 光栅后端。
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl Rasterizer for VelloCpuRasterizer {
    fn rasterize(&self, scene: &Scene, size: Size) -> Result<Pixmap, RasterError> {
        if size.width == 0 || size.height == 0 {
            return Err(RasterError::EmptySize);
        }
        let Ok(width) = u16::try_from(size.width) else {
            return Err(RasterError::SizeExceedsLimit);
        };
        let Ok(height) = u16::try_from(size.height) else {
            return Err(RasterError::SizeExceedsLimit);
        };

        let mut context = vello_cpu::RenderContext::new(width, height);
        for command in &scene.commands {
            match command {
                Command::FillRect { rect, color } => {
                    context.set_paint(vello_cpu::peniko::Color::from_rgba8(
                        color.r, color.g, color.b, color.a,
                    ));
                    context.fill_rect(&vello_cpu::kurbo::Rect::new(
                        f64::from(rect.x),
                        f64::from(rect.y),
                        f64::from(rect.x) + f64::from(rect.width),
                        f64::from(rect.y) + f64::from(rect.height),
                    ));
                }
            }
        }

        let mut target = vello_cpu::Pixmap::new(width, height);
        let mut resources = vello_cpu::Resources::new();
        context.render(&mut target, &mut resources);

        // vello_cpu 输出预乘像素；facade 契约为非预乘，用上游自带转换
        let data = target
            .take_unpremultiplied()
            .iter()
            .flat_map(|pixel| [pixel.r, pixel.g, pixel.b, pixel.a])
            .collect();
        Ok(Pixmap { size, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(width: u32, height: u32) -> Size {
        Size { width, height }
    }

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn fill(x: f32, y: f32, width: f32, height: f32, color: Color) -> Command {
        Command::FillRect {
            rect: rect(x, y, width, height),
            color,
        }
    }

    #[test]
    fn fill_rect_covers_exactly_its_area() {
        let scene = Scene {
            commands: vec![fill(2.0, 1.0, 4.0, 3.0, Color::opaque(255, 0, 0))],
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(10, 8))
            .expect("rasterize");

        assert_eq!(pixmap.size, size(10, 8));
        assert_eq!(pixmap.data.len(), 10 * 8 * 4);

        for y in 0..8u32 {
            for x in 0..10u32 {
                let pixel = pixmap.pixel(x, y).expect("in bounds");
                let inside = (2..6).contains(&x) && (1..4).contains(&y);
                if inside {
                    assert_eq!(pixel, [255, 0, 0, 255], "({x}, {y}) should be red");
                } else {
                    assert_eq!(pixel, [0, 0, 0, 0], "({x}, {y}) should stay transparent");
                }
            }
        }
    }

    #[test]
    fn later_rects_composite_over_earlier_ones() {
        let scene = Scene {
            commands: vec![
                fill(0.0, 0.0, 4.0, 4.0, Color::opaque(255, 0, 0)),
                fill(2.0, 0.0, 4.0, 4.0, Color::opaque(0, 255, 0)),
            ],
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(8, 4))
            .expect("rasterize");

        // 像素中心采样：红色覆盖 [0,4)，绿色覆盖 [2,6)
        assert_eq!(pixmap.pixel(0, 0), Some([255, 0, 0, 255]));
        assert_eq!(pixmap.pixel(2, 0), Some([0, 255, 0, 255]));
        assert_eq!(pixmap.pixel(5, 0), Some([0, 255, 0, 255]));
        assert_eq!(pixmap.pixel(6, 0), Some([0, 0, 0, 0]));
    }

    #[test]
    fn alpha_blends_with_src_over() {
        // 半透明黑叠在不透明白上 → 灰
        let scene = Scene {
            commands: vec![
                fill(0.0, 0.0, 2.0, 2.0, Color::opaque(255, 255, 255)),
                Command::FillRect {
                    rect: rect(0.0, 0.0, 2.0, 2.0),
                    color: Color {
                        r: 0,
                        g: 0,
                        b: 0,
                        a: 128,
                    },
                },
            ],
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(2, 2))
            .expect("rasterize");

        let [r, g, b, a] = pixmap.pixel(0, 0).expect("pixel");
        assert_eq!(a, 255);
        // 255 * (1 - 128/255) ≈ 127，光栅化允许 ±1 量化差
        assert!((i16::from(r) - 127).abs() <= 1, "r = {r}");
        assert_eq!(g, r);
        assert_eq!(b, r);
    }

    #[test]
    fn fractional_rects_conserve_coverage() {
        // 分数坐标矩形按几何面积做覆盖抗锯齿：部分覆盖 → 部分 alpha
        let scene = Scene {
            commands: vec![fill(0.5, 0.5, 3.5, 1.0, Color::opaque(0, 0, 255))],
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(6, 2))
            .expect("rasterize");

        // 行内被覆盖的 alpha 总量守恒：x 覆盖 3.5px × y 覆盖 0.5px ≈ 1.75
        for y in [0u32, 1] {
            let coverage: f32 = (0..6u32)
                .filter_map(|x| pixmap.pixel(x, y))
                .map(|pixel| f32::from(pixel[3]) / 255.0)
                .sum();
            assert!(
                (coverage - 1.75).abs() < 0.05,
                "row {y} coverage = {coverage}"
            );
        }
        // 未触及的区域保持全透明
        assert_eq!(pixmap.pixel(5, 0), Some([0, 0, 0, 0]));
    }

    #[test]
    fn empty_size_is_rejected() {
        let error = VelloCpuRasterizer::new()
            .rasterize(&Scene::default(), size(0, 10))
            .expect_err("zero width");
        assert_eq!(error, RasterError::EmptySize);

        let error = VelloCpuRasterizer::new()
            .rasterize(&Scene::default(), size(10, 0))
            .expect_err("zero height");
        assert_eq!(error, RasterError::EmptySize);
    }

    #[test]
    fn oversize_is_rejected() {
        let error = VelloCpuRasterizer::new()
            .rasterize(&Scene::default(), size(u32::from(u16::MAX) + 1, 10))
            .expect_err("width over u16");
        assert_eq!(error, RasterError::SizeExceedsLimit);
    }

    #[test]
    fn empty_scene_yields_transparent_pixmap() {
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&Scene::default(), size(3, 3))
            .expect("rasterize");
        assert!(pixmap.data.iter().all(|byte| *byte == 0));
    }
}
