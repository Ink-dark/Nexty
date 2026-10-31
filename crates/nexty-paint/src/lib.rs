//! Nexty 绘制 facade：自有绘制指令 + 可替换光栅后端。
//!
//! 上游：`vello_cpu`（CPU 光栅，sparse strips）。GPU 后端 `vello_hybrid` 依赖
//! wgpu device/surface，由 chrome 层提供后接入——本轮从本 crate 依赖中摘除，
//! [`Rasterizer`] trait 即双后端接缝。决策与调研见
//! `docs/decisions/2026-10-01-crate-selection.md`。
//!
//! 指令集：`FillRect` / `StrokeRect` / `FillRoundedRect` / `StrokeRoundedRect` /
//! `FillGradient` / `DrawText` / `DrawImage`，后续消费方需要什么指令再扩展什么。
//!
//! 上游：`vello_cpu`（CPU 光栅，sparse strips）+ `image`（图片解码）。
//! **解码归属**：解码器只在 [`decode`] 内出现，产出自有类型 [`Image`]，
//! 上游类型不外泄（AGENTS.md 硬规则）。只解码不做编码与图像处理——
//! 裁剪缩放由 layout 算矩形、paint 按矩形采样表达。
//!
//! 上游类型一律不得出现在本 crate 的 pub 导出中（AGENTS.md 硬规则）。
//!
//! **渲染隔离**：上游对未支持特性是 panic 而非返回错误，故渲染必须在独立线程
//! 执行并由 `catch_unwind` 兜底（由 chrome 层持有）。因此 workspace 与各 crate
//! 一律不得设置 `panic = "abort"`（会禁用 unwind，使兜底失效）。

#![forbid(unsafe_code)]

use std::sync::Arc;

use nexty_text::ResolvedFont;

/// 图片解码错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// 字节不是受支持的图片格式，或格式数据损坏。
    ///
    /// 上游解码器的错误类型不得外泄，故折叠为单一变体：调用方（chrome 层）
    /// 对解码失败的处理是「跳过该资源」，不需要区分具体原因。
    InvalidImage,
    /// 图像尺寸为 0、超出资源防护上限，或后端不支持（预检失败与不可渲染
    /// 同错误变体）。
    ///
    /// 上限见 [`MAX_DIMENSION`] / [`MAX_PIXEL_AREA`]：几 KB 的 PNG 可声明
    /// 65535×65535，先只读头部校验、再分配像素——分配失败是 abort，渲染
    /// 隔离的 panic 兜底兜不住（超限返回本变体而非尝试分配）。
    SizeUnsupported,
}

/// 解码预检的单边长度上限。
///
/// 旧值为 65535（vello 渲染目标 u16 上限）；收紧到 16384 与光栅目标、
/// 内存预算同量级——任何实际可绘制的图片远小于此。
#[cfg(feature = "cpu")]
pub const MAX_DIMENSION: u32 = 16384;

/// 解码预检的像素面积上限（2^26，约 256 MB RGBA8 缓冲）。
#[cfg(feature = "cpu")]
pub const MAX_PIXEL_AREA: u64 = 1 << 26;

/// 从字节流解码一幅图片（格式由内容嗅探，不依赖扩展名）。
///
/// 支持 PNG / JPEG / GIF / WebP（编译期 feature 决定）。产出非预乘 RGBA8，
/// 供 [`Command::DrawImage`] 消费。
///
/// 解码前先 `into_dimensions()` 只读头部做尺寸预检：边长 ≤ [`MAX_DIMENSION`]
/// 且 `宽 × 高` ≤ [`MAX_PIXEL_AREA`]，超限返回 [`DecodeError::SizeUnsupported`]
/// 而不进入像素解码；预检通过后才 `decode()`。
///
/// # Errors
///
/// 字节无法识别为受支持格式、格式数据损坏、或头部尺寸预检不通过时返回
/// [`DecodeError`]。
///
/// 上游 `image` 系列的错误一律映射为 [`DecodeError::InvalidImage`]，不引入
/// `image::error::ImageError`——依赖类型不得出现在 pub 导出中。
#[cfg(feature = "cpu")]
pub fn decode(bytes: &[u8]) -> Result<Image, DecodeError> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| DecodeError::InvalidImage)?;
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| DecodeError::InvalidImage)?;
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_PIXEL_AREA
    {
        return Err(DecodeError::SizeUnsupported);
    }

    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| DecodeError::InvalidImage)?;
    let decoded = reader.decode().map_err(|_| DecodeError::InvalidImage)?;
    debug_assert_eq!(
        (decoded.width(), decoded.height()),
        (width, height),
        "解码后尺寸与头部预检一致"
    );

    // 统一转RGBA8（灰度 / 调色板 / RGB 均在此归一），行优先连续排列。
    let rgba = decoded.into_rgba8();
    Ok(Image::new(width, height, rgba.into_raw()))
}

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

/// 一个字形（文本 run 内；`y` 相对基线，负值在基线上方）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextGlyph {
    /// 字体内部字形编号。
    pub id: u32,
    /// 相对基线原点的水平偏移，px。
    pub x: f32,
    /// 相对基线的垂直偏移，px。
    pub y: f32,
    /// 所属字体在 [`Command::DrawText::fonts`] 中的下标（text 层按脚本
    /// 回退后一条 run 可能跨多个字体）。
    pub font: u32,
}

/// 线性渐变的色标（CSS Images §3.4）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientStop {
    /// 色标在渐变线上的位置：0.0 = 起点，1.0 = 终点。
    pub offset: f32,
    /// 色标颜色（非预乘）。
    pub color: Color,
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
    /// 用纯色描边矩形（线宽向内向外各半），边框绘制用。
    StrokeRect {
        /// 目标矩形（描边中心线）。
        rect: Rect,
        /// 描边色。
        color: Color,
        /// 线宽，px；非正值不绘制。
        width: f32,
    },
    /// 用纯色填充圆角矩形（CSS Backgrounds §5.5 border-radius 语义）。
    ///
    /// 半径顺序为左上、右上、右下、左下（CSS `border-radius` 简写序）。相邻角
    /// 半径之和超过边长时按 CSS §5.5 等比缩减；负值与非有限半径按 0 处理
    /// （渲染为直角）；矩形坐标非有限或宽高非正时整条指令跳过。
    FillRoundedRect {
        /// 目标矩形。
        rect: Rect,
        /// 四角半径 `[左上, 右上, 右下, 左下]`，px。
        radii: [f32; 4],
        /// 填充色。
        color: Color,
    },
    /// 用纯色描边圆角矩形，圆角边框绘制用。
    ///
    /// 与 [`Command::StrokeRect`]（中心线描边）不同，本指令的 `rect` 是
    /// border-box 外缘，绘制的带落在内侧 `[0, width]`：中心线路径整体内缩
    /// `width/2`、半径减 `width/2`（负值钳 0），CSS §5.5 缩减在内缩后施加。
    /// 线宽非有限或非正、内缩后矩形退化时整条指令跳过。
    StrokeRoundedRect {
        /// 目标矩形（border-box 外缘）。
        rect: Rect,
        /// 四角半径 `[左上, 右上, 右下, 左下]`，px（相对 border-box 外缘）。
        radii: [f32; 4],
        /// 描边色。
        color: Color,
        /// 带宽，px。
        width: f32,
    },
    /// 用线性渐变填充（可带圆角的）矩形——单条指令覆盖渐变 × border-radius
    /// 组合，无需调用方自算裁剪。
    ///
    /// `start` / `end` 是**场景坐标**下的渐变线两端（CSS Images §3.4 的角度 /
    /// 关键字 → 端点换算由调用方完成）；色标交给后端按 sRGB、预乘 alpha 插值，
    /// stop 范围外 Pad 外推取端色——均与 CSS 默认语义一致。offset 非有限的
    /// 色标被丢弃；全部被丢弃、端点或矩形非法时整条指令跳过。
    FillGradient {
        /// 目标矩形。
        rect: Rect,
        /// 四角半径 `[左上, 右上, 右下, 左下]`，px；全 0 即直角。
        radii: [f32; 4],
        /// 渐变线起点（场景坐标）。
        start: [f32; 2],
        /// 渐变线终点（场景坐标）。
        end: [f32; 2],
        /// 色标列表（offset 0 = 起点，1 = 终点）。
        stops: Vec<GradientStop>,
    },
    /// 绘制一行字形。
    ///
    /// 字形必须已经过 text 层整形（字形 id 对应 `fonts[glyph.font]` 字体）；
    /// `glyphs[i].y` 相对基线，`baseline` 是基线的绝对 y。
    DrawText {
        /// 字形序列（x 为绝对坐标，y 相对基线）。
        glyphs: Vec<TextGlyph>,
        /// 基线的绝对 y，px。
        baseline: f32,
        /// 本次整形实际用到的字体（按脚本回退后可能多于一个）。
        fonts: Vec<ResolvedFont>,
        /// 字号，px。
        size: f32,
        /// 文本颜色。
        color: Color,
    },
    /// 绘制一幅已解码图像。
    ///
    /// 像素经 [`Scene::images`] 按下标间接引用，不内联在指令里：像素是
    /// 数 MB 级的大块内存，按 `Arc` 引用计数共享（构建、发送与光栅全程
    /// 零拷贝），内联则每次场景移动都会退化成整块拷贝。
    DrawImage {
        /// 目标矩形（图像左上角落点 + 绘制尺寸）。
        rect: Rect,
        /// [`Scene::images`] 中的图像下标。
        image: usize,
    },
}

/// 一幅已解码的图像：非预乘 RGBA8，行优先。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// 像素宽。
    pub width: u32,
    /// 像素高。
    pub height: u32,
    /// 非预乘 RGBA8 字节，长度应为 `width * height * 4`。
    pub data: Vec<u8>,
}

impl Image {
    /// 由像素数据构造图像。
    #[must_use]
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Self {
        Self {
            width,
            height,
            data,
        }
    }

    /// 读取 `(x, y)` 处的 RGBA 分量；越界返回 `None`。
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let index = (y as usize * self.width as usize + x as usize) * 4;
        self.data
            .get(index..index + 4)
            .map(|slice| [slice[0], slice[1], slice[2], slice[3]])
    }
}

/// 一帧的绘制指令列表。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    /// 按绘制顺序排列的指令。
    pub commands: Vec<Command>,
    /// 图像资源池（按 `Arc` 共享，构建/发送/光栅全程零像素拷贝）；
    /// [`Command::DrawImage`] 按下标引用。
    pub images: Vec<Arc<Image>>,
}

impl Scene {
    /// 生成整体平移 `(dx, dy)` 后的场景副本（M6 滚动路径）。
    ///
    /// 页面坐标系场景滚动复用时，origin 平移在发送阶段施加：字形与图片
    /// 像素按 `Arc`/`Vec` 引用计数共享，只有指令包装层重新分配——比整树
    /// 重新 emit（逐字形坐标重算 + 字体表重建 + 高亮相交判定）便宜一个
    /// 量级。图片池按下标引用，平移不改下标，`Arc` 逐项共享。
    #[must_use]
    pub fn translated(&self, dx: f32, dy: f32) -> Scene {
        let commands = self
            .commands
            .iter()
            .map(|command| match command {
                Command::FillRect { rect, color } => Command::FillRect {
                    rect: Rect {
                        x: rect.x + dx,
                        y: rect.y + dy,
                        ..*rect
                    },
                    color: *color,
                },
                Command::StrokeRect { rect, color, width } => Command::StrokeRect {
                    rect: Rect {
                        x: rect.x + dx,
                        y: rect.y + dy,
                        ..*rect
                    },
                    color: *color,
                    width: *width,
                },
                Command::FillRoundedRect { rect, radii, color } => Command::FillRoundedRect {
                    rect: Rect {
                        x: rect.x + dx,
                        y: rect.y + dy,
                        ..*rect
                    },
                    radii: *radii,
                    color: *color,
                },
                Command::StrokeRoundedRect {
                    rect,
                    radii,
                    color,
                    width,
                } => Command::StrokeRoundedRect {
                    rect: Rect {
                        x: rect.x + dx,
                        y: rect.y + dy,
                        ..*rect
                    },
                    radii: *radii,
                    color: *color,
                    width: *width,
                },
                Command::FillGradient {
                    rect,
                    radii,
                    start,
                    end,
                    stops,
                } => Command::FillGradient {
                    rect: Rect {
                        x: rect.x + dx,
                        y: rect.y + dy,
                        ..*rect
                    },
                    radii: *radii,
                    // 渐变端点是场景坐标：必须与矩形同步平移，否则滚动后渐变错位
                    start: [start[0] + dx, start[1] + dy],
                    end: [end[0] + dx, end[1] + dy],
                    stops: stops.clone(),
                },
                Command::DrawText {
                    glyphs,
                    baseline,
                    fonts,
                    size,
                    color,
                } => Command::DrawText {
                    glyphs: glyphs
                        .iter()
                        .map(|glyph| TextGlyph {
                            x: glyph.x + dx,
                            ..*glyph
                        })
                        .collect(),
                    baseline: baseline + dy,
                    fonts: fonts.clone(),
                    size: *size,
                    color: *color,
                },
                Command::DrawImage { rect, image } => Command::DrawImage {
                    rect: Rect {
                        x: rect.x + dx,
                        y: rect.y + dy,
                        ..*rect
                    },
                    image: *image,
                },
            })
            .collect();
        Scene {
            commands,
            images: self.images.clone(),
        }
    }
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

/// 字体 / 图像字节的Blob 包装。
///
/// vello 的 `Blob` 要求 `AsRef<[u8]>`，但 trait 对象胖指针无法直接满足
/// `Sized` 约束，故用这个 Sized 包装完成转换。字节按引用计数共享，不复制。
#[cfg(feature = "cpu")]
#[derive(Clone)]
struct FontBytes(Arc<[u8]>);

#[cfg(feature = "cpu")]
impl AsRef<[u8]> for FontBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
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
    /// 尺寸超出后端上限（vello_cpu 的渲染目标为 u16 像素；GPU 后端同形语义）。
    ///
    /// 与 [`DecodeError::SizeUnsupported`] 的区别：本变体描述渲染目标上限，
    /// 解码侧的资源防护预检在 [`decode`] 内完成。
    SizeExceedsLimit,
    /// 后端不支持场景中的指令。
    UnsupportedCommand,
}

/// [`Rasterizer`] 的 vello_cpu 实现（CPU 光栅）。
/// [`Rasterizer`] 的 vello_cpu 实现（CPU 光栅）。
///
/// `vello_cpu` 对未支持特性是 panic 而非返回错误；按渲染隔离规则，
/// 调用方须把本实现放进独立线程并以 `catch_unwind` 兜底。
///
/// 文本渲染的字体数据由 text 层整形结果直接提供（[`Command::DrawText::fonts`]）：
/// 按脚本回退后一条 run 可跨多个字体，字形归属在整形期已确定，光栅层按
/// `glyph.font` 分组用对应字体渲染。
#[cfg(feature = "cpu")]
#[derive(Debug, Default)]
pub struct VelloCpuRasterizer;

#[cfg(feature = "cpu")]
impl VelloCpuRasterizer {
    /// 创建 CPU 光栅后端。
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[cfg(feature = "cpu")]
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
        let mut resources = vello_cpu::Resources::new();
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
                Command::StrokeRect { rect, color, width } => {
                    if *width <= 0.0 {
                        continue;
                    }
                    context.set_paint(vello_cpu::peniko::Color::from_rgba8(
                        color.r, color.g, color.b, color.a,
                    ));
                    context.set_stroke(vello_cpu::kurbo::Stroke::new(f64::from(*width)));
                    context.stroke_rect(&vello_cpu::kurbo::Rect::new(
                        f64::from(rect.x),
                        f64::from(rect.y),
                        f64::from(rect.x) + f64::from(rect.width),
                        f64::from(rect.y) + f64::from(rect.height),
                    ));
                }
                Command::FillRoundedRect { rect, radii, color } => {
                    // 非法输入跳过而非 panic（对齐 draw_image 先例）
                    if let Some(path) = rounded_rect_path(*rect, *radii) {
                        context.set_paint(vello_cpu::peniko::Color::from_rgba8(
                            color.r, color.g, color.b, color.a,
                        ));
                        context.fill_path(&path);
                    }
                }
                Command::StrokeRoundedRect {
                    rect,
                    radii,
                    color,
                    width,
                } => {
                    if !width.is_finite() || *width <= 0.0 {
                        continue;
                    }
                    // CSS 边框带在 border-box 内侧：中心线路径整体内缩 width/2，
                    // 圆角半径减 width/2（负值钳 0）
                    let half = *width / 2.0;
                    let inner = Rect {
                        x: rect.x + half,
                        y: rect.y + half,
                        width: rect.width - *width,
                        height: rect.height - *width,
                    };
                    let radii = sanitize_radii(*radii).map(|r| (r - half).max(0.0));
                    if let Some(path) = rounded_rect_path(inner, radii) {
                        context.set_paint(vello_cpu::peniko::Color::from_rgba8(
                            color.r, color.g, color.b, color.a,
                        ));
                        context.set_stroke(vello_cpu::kurbo::Stroke::new(f64::from(*width)));
                        context.stroke_path(&path);
                    }
                }
                Command::FillGradient {
                    rect,
                    radii,
                    start,
                    end,
                    stops,
                } => {
                    let Some(path) = rounded_rect_path(*rect, *radii) else {
                        continue;
                    };
                    if !start.iter().all(|v| v.is_finite()) || !end.iter().all(|v| v.is_finite()) {
                        continue;
                    }
                    // offset 非有限的色标丢弃；一条不剩则无渐变可画
                    let peniko_stops: Vec<vello_cpu::peniko::ColorStop> = stops
                        .iter()
                        .filter(|stop| stop.offset.is_finite())
                        .map(|stop| {
                            vello_cpu::peniko::ColorStop::from((
                                stop.offset,
                                gradient_color(stop.color),
                            ))
                        })
                        .collect();
                    if peniko_stops.is_empty() {
                        continue;
                    }
                    let source = vello_cpu::peniko::ColorStops::from(peniko_stops.as_slice());
                    let gradient: vello_cpu::peniko::Gradient =
                        vello_cpu::peniko::Gradient::new_linear(
                            vello_cpu::kurbo::Point::new(f64::from(start[0]), f64::from(start[1])),
                            vello_cpu::kurbo::Point::new(f64::from(end[0]), f64::from(end[1])),
                        )
                        .with_stops(&source);
                    // 渐变端点与路径同处场景坐标系，默认恒等 paint transform
                    // 直接可用（draw_image 的 transform 只服务于图像像素空间映射，
                    // 其收尾已 reset，无状态残留）。
                    context.set_paint(gradient);
                    context.fill_path(&path);
                }
                Command::DrawText {
                    glyphs,
                    baseline,
                    fonts,
                    size,
                    color,
                } => {
                    context.set_paint(vello_cpu::peniko::Color::from_rgba8(
                        color.r, color.g, color.b, color.a,
                    ));
                    // 按字体下标分组（组内字形连续，通常只有一组）：回退后
                    // 一条 run 可跨多个字体，字形 id 只对所属字体有意义
                    let mut groups: Vec<(u32, Vec<&TextGlyph>)> = Vec::new();
                    for glyph in glyphs {
                        match groups.last_mut() {
                            Some((index, group)) if *index == glyph.font => {
                                group.push(glyph);
                            }
                            _ => groups.push((glyph.font, vec![glyph])),
                        }
                    }
                    for (font_index, group) in groups {
                        // 下标越界（调用方构造错误）时跳过该组而非 panic
                        let Some(font) = fonts.get(font_index as usize) else {
                            continue;
                        };
                        // 字体数据按引用计数共享给 vello 的 Blob
                        let bytes: Arc<dyn AsRef<[u8]> + Send + Sync> =
                            Arc::new(FontBytes(font.data().clone()));
                        let font_data = vello_cpu::peniko::FontData::new(
                            vello_cpu::peniko::Blob::new(bytes),
                            font.index(),
                        );
                        let glyphs = group.iter().map(|glyph| vello_cpu::Glyph {
                            id: glyph.id,
                            x: glyph.x,
                            y: baseline + glyph.y,
                        });
                        context
                            .glyph_run(&mut resources, &font_data)
                            .font_size(*size)
                            .fill_glyphs(glyphs);
                    }
                }
                Command::DrawImage { rect, image } => {
                    // 下标越界（资源表被改过）时跳过该指令而不是 panic：
                    // 渲染隔离虽能兜底，但跳过远好过整帧失败。
                    let Some(source) = scene.images.get(*image) else {
                        continue;
                    };
                    draw_image(&mut context, &mut resources, source, rect);
                }
            }
        }

        let mut target = vello_cpu::Pixmap::new(width, height);
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

/// 绘制一幅已解码图像到目标矩形。
///
/// vello_cpu 没有专门的 image 绘制入口（只有 `fill_path` / `glyph_run`），
/// 所以这里把图像作为 `Brush::Image` 填进矩形路径。缩放由 paint transform
/// 表达：源图像 1px 对应 `scale` 个目标 px。
///
/// 尺寸越界（转 u16 失败）或数据长度不符时**直接返回不绘制**——绘制路径
/// 无法返回错误，静默跳过优于让上游 panic（渲染隔离虽兜底，但会丢掉整帧）。
#[cfg(feature = "cpu")]
fn draw_image(
    context: &mut vello_cpu::RenderContext,
    resources: &mut vello_cpu::Resources,
    source: &Image,
    rect: &Rect,
) {
    // 图像尺寸须能放进 vello 的 u16 渲染目标
    let (Ok(width), Ok(height)) = (u16::try_from(source.width), u16::try_from(source.height))
    else {
        return;
    };
    let expected = (source.width as usize)
        .saturating_mul(source.height as usize)
        .saturating_mul(4);
    if source.data.len() < expected || source.width == 0 || source.height == 0 {
        return;
    }
    // 目标矩形宽高为非正时不绘制
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }

    // vello 的 Pixmap 存预乘 RGBA8，facade 契约是非预乘，故逐像素转换。
    // 每帧重建 Pixmap 是 CPU 光栅的固有成本（像素本就在内存里）。
    let mut pixmap = vello_cpu::Pixmap::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let index = (y as usize * width as usize + x as usize) * 4;
            // 非预乘 → 预乘：各颜色通道乘 alpha。
            // 用整数舍入（+128 >>8）与 vello 内部转换一致，避免浮点误差。
            let alpha = u16::from(source.data[index + 3]);
            let premultiply = |channel: u8| ((u16::from(channel) * alpha + 128) >> 8) as u8;
            pixmap.set_pixel(
                x,
                y,
                vello_cpu::color::PremulRgba8::from_u8_array([
                    premultiply(source.data[index]),
                    premultiply(source.data[index + 1]),
                    premultiply(source.data[index + 2]),
                    source.data[index + 3],
                ]),
            );
        }
    }
    // 注册进资源表拿句柄，再组装成 image paint
    let image_id = resources.register_image(Arc::new(pixmap));
    let image_source = vello_cpu::ImageSource::opaque_id(image_id);
    let brush = vello_cpu::peniko::ImageBrush {
        image: image_source,
        sampler: Default::default(),
    };
    context.set_paint(vello_cpu::PaintType::Image(brush));

    // 用 paint transform 表达缩放：源 1px → 目标 scale px。
    // 先缩放再平移，使图像左上角对齐 rect 左上角。
    let scale_x = f64::from(rect.width) / f64::from(source.width);
    let scale_y = f64::from(rect.height) / f64::from(source.height);
    let transform = vello_cpu::kurbo::Affine::scale_non_uniform(scale_x, scale_y).then_translate(
        vello_cpu::kurbo::Vec2::new(f64::from(rect.x), f64::from(rect.y)),
    );
    context.set_paint_transform(transform);

    // 几何用**场景坐标**（目标矩形），paint transform 只负责把图像的
    // 像素空间映射到同一位置——两者必须指向同一个矩形，否则图像会被
    // 采样到路径之外。
    // kurbo 的 Shape trait 提供 to_path(tolerance)，需引入该 trait。
    use vello_cpu::kurbo::Shape as _;
    let target_rect = vello_cpu::kurbo::Rect::new(
        f64::from(rect.x),
        f64::from(rect.y),
        f64::from(rect.x) + f64::from(rect.width),
        f64::from(rect.y) + f64::from(rect.height),
    );
    context.fill_path(&target_rect.to_path(0.1));
    context.reset_paint_transform();
}

/// 把圆角半径收敛为有限非负值：负值与非有限值一律按 0 处理（渲染为直角）。
///
/// 渲染后端对非法数值是 panic 而非返回错误，入口先收敛（对齐 `draw_image`
/// 的「跳过优于 panic」先例——半径语义上 0 总是安全的降级）。
#[cfg(feature = "cpu")]
fn sanitize_radii(radii: [f32; 4]) -> [f32; 4] {
    radii.map(|radius| {
        if radius.is_finite() {
            radius.max(0.0)
        } else {
            0.0
        }
    })
}

/// CSS Backgrounds §5.5（角重叠）：任一边上相邻两角半径之和超过边长时，
/// 全部四角半径按 `f = min(边长 / 半径和)`（只取超限边）等比缩减。
///
/// 输入须已过 [`sanitize_radii`]（有限非负）且边长为正——本函数是纯几何
/// 计算，不做输入防御。
#[cfg(feature = "cpu")]
fn scale_overlapping_radii(width: f32, height: f32, radii: [f32; 4]) -> [f32; 4] {
    // 四条边上的（相邻角半径和, 边长）
    let edges = [
        (radii[0] + radii[1], width),  // 上：左上 + 右上
        (radii[1] + radii[2], height), // 右：右上 + 右下
        (radii[2] + radii[3], width),  // 下：右下 + 左下
        (radii[3] + radii[0], height), // 左：左下 + 左上
    ];
    let mut factor = f32::INFINITY;
    for (sum, length) in edges {
        if sum > length {
            factor = factor.min(length / sum);
        }
    }
    if factor < 1.0 {
        radii.map(|radius| radius * factor)
    } else {
        radii
    }
}

/// 矩形是否可绘制：坐标有限且宽高为正。
#[cfg(feature = "cpu")]
fn rect_is_paintable(rect: &Rect) -> bool {
    rect.x.is_finite()
        && rect.y.is_finite()
        && rect.width.is_finite()
        && rect.height.is_finite()
        && rect.width > 0.0
        && rect.height > 0.0
}

/// 圆角矩形 → 光栅路径；矩形不可绘制时返回 `None`（调用方跳过该指令）。
///
/// 半径先经 [`sanitize_radii`] 收敛、再按 [`scale_overlapping_radii`]
/// 缩减（CSS Backgrounds §5.5）。已知偏差：kurbo 的 `RoundedRect::from_rect`
/// 会把每个半径再钳到最短边一半——不对称大半径（如 40×100 盒上单角 40px，
/// CSS 渲染四分之一圆）会被进一步压小，按降级语义接受。
#[cfg(feature = "cpu")]
fn rounded_rect_path(rect: Rect, radii: [f32; 4]) -> Option<vello_cpu::kurbo::BezPath> {
    if !rect_is_paintable(&rect) {
        return None;
    }
    let radii = scale_overlapping_radii(rect.width, rect.height, sanitize_radii(radii));
    let kurbo_rect = vello_cpu::kurbo::Rect::new(
        f64::from(rect.x),
        f64::from(rect.y),
        f64::from(rect.x) + f64::from(rect.width),
        f64::from(rect.y) + f64::from(rect.height),
    );
    let kurbo_radii = vello_cpu::kurbo::RoundedRectRadii::new(
        f64::from(radii[0]),
        f64::from(radii[1]),
        f64::from(radii[2]),
        f64::from(radii[3]),
    );
    use vello_cpu::kurbo::Shape as _;
    Some(vello_cpu::kurbo::RoundedRect::from_rect(kurbo_rect, kurbo_radii).to_path(0.1))
}

/// 非预乘 RGBA8 → peniko 渐变色标颜色（依赖类型不进入 pub 导出）。
#[cfg(feature = "cpu")]
fn gradient_color(color: Color) -> vello_cpu::peniko::color::DynamicColor {
    use vello_cpu::peniko::color::{AlphaColor, Srgb};
    AlphaColor::<Srgb>::from_rgba8(color.r, color.g, color.b, color.a).into()
}

// 测试需要 CPU 光栅后端与图片解码（`image`），二者同属 `cpu` feature。
#[cfg(all(test, feature = "cpu"))]
mod tests {
    use super::*;
    use nexty_text::ResolvedFont;

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
            images: Vec::new(),
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
            images: Vec::new(),
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
            images: Vec::new(),
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
            images: Vec::new(),
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

    /// 用 text 层整形一个字符，返回其字形与基线度量。
    fn shape_char(character: char) -> (Vec<TextGlyph>, Vec<ResolvedFont>, f32) {
        use nexty_text::TextShaper;
        let shaper = nexty_text::ParleyTextShaper::new();
        let style = nexty_text::TextStyle {
            families: vec!["serif".to_owned()],
            size: 32.0,
        };
        let text = character.to_string();
        let shaped = shaper.shape(&text, &style).expect("shape");
        let metrics = shaper.metrics(&style).expect("metrics");
        let glyphs = shaped
            .glyphs
            .iter()
            .map(|glyph| TextGlyph {
                id: glyph.id,
                x: glyph.x,
                y: glyph.y,
                font: glyph.font,
            })
            .collect();
        (glyphs, shaped.fonts, metrics.ascent)
    }

    #[test]
    fn draw_text_produces_opaque_pixels() {
        let (glyphs, fonts, ascent) = shape_char('A');
        let scene = Scene {
            commands: vec![Command::DrawText {
                glyphs,
                baseline: 40.0,
                fonts,
                size: 32.0,
                color: Color::opaque(0, 0, 0),
            }],
            images: Vec::new(),
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(48, 48))
            .expect("rasterize");

        // 字形区域内应有大量不透明像素（基线在 40，字形主体在其上方）
        let opaque = (0..48u32)
            .flat_map(|x| (0..ascent as u32 + 8).map(move |y| (x, y)))
            .filter(|(x, y)| pixmap.pixel(*x, *y).is_some_and(|p| p[3] > 200))
            .count();
        assert!(opaque > 20, "字形应产生不透明像素，实际 {opaque}");
    }

    #[test]
    fn draw_text_honors_color_and_draw_order() {
        let (glyphs, fonts, _) = shape_char('A');
        let scene = Scene {
            commands: vec![
                Command::FillRect {
                    rect: rect(0.0, 0.0, 48.0, 48.0),
                    color: Color::opaque(255, 255, 255),
                },
                Command::DrawText {
                    glyphs,
                    baseline: 40.0,
                    fonts,
                    size: 32.0,
                    color: Color::opaque(255, 0, 0),
                },
            ],
            images: Vec::new(),
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(48, 48))
            .expect("rasterize");

        // 存在偏红的字形像素（抗锯齿边缘允许混色）
        let has_red = (0..48u32).any(|x| {
            (0..48u32).any(|y| {
                pixmap
                    .pixel(x, y)
                    .is_some_and(|p| p[3] > 200 && p[0] > 150 && p[1] < 120)
            })
        });
        assert!(has_red, "白底上的红色字形应可辨");
    }

    #[test]
    fn stroke_rect_draws_outline_only() {
        let scene = Scene {
            commands: vec![Command::StrokeRect {
                rect: rect(2.0, 2.0, 10.0, 10.0),
                color: Color::opaque(255, 0, 0),
                width: 2.0,
            }],
            images: Vec::new(),
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(16, 16))
            .expect("rasterize");

        // 边框线上有红色
        assert_eq!(pixmap.pixel(2, 2), Some([255, 0, 0, 255]));
        assert_eq!(pixmap.pixel(11, 2), Some([255, 0, 0, 255]));
        assert_eq!(pixmap.pixel(11, 11), Some([255, 0, 0, 255]));
        // 中心保持透明（只描边不填心）
        assert_eq!(pixmap.pixel(7, 7), Some([0, 0, 0, 0]));
        assert_eq!(pixmap.pixel(6, 6), Some([0, 0, 0, 0]));
    }

    #[test]
    fn stroke_rect_zero_width_is_noop() {
        let scene = Scene {
            commands: vec![Command::StrokeRect {
                rect: rect(2.0, 2.0, 8.0, 8.0),
                color: Color::opaque(255, 0, 0),
                width: 0.0,
            }],
            images: Vec::new(),
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(16, 16))
            .expect("rasterize");
        assert!(pixmap.data.iter().all(|byte| *byte == 0));
    }

    /// 构造一幅纯色图像。
    fn solid_image(width: u32, height: u32, color: [u8; 4]) -> Image {
        Image::new(width, height, color.repeat((width * height) as usize))
    }

    /// 断言像素接近期望颜色（图像采样有±1 的插值舍入误差）。
    fn assert_pixel_near(pixmap: &Pixmap, x: u32, y: u32, expected: [u8; 4]) {
        let actual = pixmap.pixel(x, y).expect("pixel in bounds");
        for (index, (got, want)) in actual.iter().zip(expected).enumerate() {
            assert!(
                i32::from(*got).abs_diff(i32::from(want)) <= 1,
                "({x}, {y}) 通道{index}:实际 {got}，期望 {want}±1"
            );
        }
    }

    #[test]
    fn draw_image_places_pixels_at_target_rect() {
        let scene = Scene {
            commands: vec![Command::DrawImage {
                rect: rect(2.0, 3.0, 4.0, 5.0),
                image: 0,
            }],
            images: vec![Arc::new(solid_image(4, 5, [0, 128, 255, 255]))],
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(10, 12))
            .expect("rasterize");

        // 目标矩形内应是图像色
        assert_pixel_near(&pixmap, 2, 3, [0, 128, 255, 255]);
        assert_pixel_near(&pixmap, 5, 7, [0, 128, 255, 255]);
        // 矩形外保持透明
        assert_eq!(pixmap.pixel(1, 3), Some([0, 0, 0, 0]));
        assert_eq!(pixmap.pixel(6, 3), Some([0, 0, 0, 0]));
        assert_eq!(pixmap.pixel(2, 2), Some([0, 0, 0, 0]));
        assert_eq!(pixmap.pixel(2, 8), Some([0, 0, 0, 0]));
    }

    #[test]
    fn draw_image_scales_to_target_rect() {
        // 2x2 图像画到 4x4：像素应被放大（采样最近邻，仍是纯色）
        let scene = Scene {
            commands: vec![Command::DrawImage {
                rect: rect(0.0, 0.0, 4.0, 4.0),
                image: 0,
            }],
            images: vec![Arc::new(solid_image(2, 2, [255, 0, 0, 255]))],
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(6, 6))
            .expect("rasterize");
        // 整个 4x4 区域都被覆盖
        // 4x4 目标矩形覆盖 [0,4)²
        for y in 0..4u32 {
            for x in 0..4u32 {
                assert_pixel_near(&pixmap, x, y, [255, 0, 0, 255]);
            }
        }
        assert_eq!(pixmap.pixel(5, 5), Some([0, 0, 0, 0]));
    }

    #[test]
    fn draw_image_clips_at_canvas_edge() {
        // 目标矩形部分超出画布：越界部分被裁剪，不panic
        let scene = Scene {
            commands: vec![Command::DrawImage {
                rect: rect(6.0, 6.0, 8.0, 8.0),
                image: 0,
            }],
            images: vec![Arc::new(solid_image(8, 8, [0, 255, 0, 255]))],
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(10, 10))
            .expect("rasterize");
        assert_pixel_near(&pixmap, 7, 7, [0, 255, 0, 255]);
        assert_pixel_near(&pixmap, 9, 9, [0, 255, 0, 255]);
        assert_eq!(pixmap.pixel(5, 5), Some([0, 0, 0, 0]));
    }

    #[test]
    fn draw_image_preserves_transparency() {
        // 半透明图像：alpha 应体现在结果里
        let scene = Scene {
            commands: vec![Command::DrawImage {
                rect: rect(0.0, 0.0, 4.0, 4.0),
                image: 0,
            }],
            images: vec![Arc::new(solid_image(4, 4, [255, 255, 255, 128]))],
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(6, 6))
            .expect("rasterize");
        let pixel = pixmap.pixel(2, 2).expect("pixel");
        assert_eq!(pixel[3], 128, "alpha 保持");
        // 非预乘输出：白色半透明 → RGB 应接近 255
        assert!(pixel[0] > 240, "r = {}", pixel[0]);
    }

    #[test]
    fn draw_image_with_out_of_range_index_is_skipped() {
        // 资源表被改过导致下标越界：跳过该指令而不是 panic
        let scene = Scene {
            commands: vec![Command::DrawImage {
                rect: rect(0.0, 0.0, 4.0, 4.0),
                image: 7,
            }],
            images: vec![Arc::new(solid_image(4, 4, [255, 0, 0, 255]))],
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(6, 6))
            .expect("rasterize");
        assert!(
            pixmap.data.iter().all(|byte| *byte == 0),
            "越界下标应被跳过，画布保持全透明"
        );
    }

    #[test]
    fn draw_image_with_malformed_data_is_skipped() {
        // 数据长度不足：不绘制且不 panic
        let scene = Scene {
            commands: vec![Command::DrawImage {
                rect: rect(0.0, 0.0, 4.0, 4.0),
                image: 0,
            }],
            images: vec![Arc::new(Image::new(4, 4, vec![255, 0, 0]))],
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(6, 6))
            .expect("rasterize");
        assert!(pixmap.data.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn draw_image_composites_over_existing_pixels() {
        // 图像应按 src-over 叠在已有填充之上
        let scene = Scene {
            commands: vec![
                fill(0.0, 0.0, 6.0, 6.0, Color::opaque(0, 0, 255)),
                Command::DrawImage {
                    rect: rect(1.0, 1.0, 4.0, 4.0),
                    image: 0,
                },
            ],
            images: vec![Arc::new(solid_image(4, 4, [255, 0, 0, 255]))],
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(6, 6))
            .expect("rasterize");
        assert_pixel_near(&pixmap, 2, 2, [255, 0, 0, 255]);
        assert_eq!(pixmap.pixel(0, 0), Some([0, 0, 255, 255]), "蓝底仍可见");
    }

    #[test]
    fn image_pixel_accessor_handles_bounds() {
        let image = solid_image(2, 2, [1, 2, 3, 4]);
        assert_eq!(image.pixel(0, 0), Some([1, 2, 3, 4]));
        assert_eq!(image.pixel(1, 1), Some([1, 2, 3, 4]));
        assert_eq!(image.pixel(2, 0), None);
        assert_eq!(image.pixel(0, 2), None);
    }

    /// 生成一张指定颜色的 PNG 字节（用 image 编码，覆盖我们声明支持的格式）。
    fn encode_png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        let mut buffer = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            width,
            height,
            image::Rgba(color),
        ))
        .write_to(
            &mut std::io::Cursor::new(&mut buffer),
            image::ImageFormat::Png,
        )
        .expect("encode png");
        buffer
    }

    #[test]
    fn decode_png_yields_rgba8() {
        let bytes = encode_png(3, 2, [10, 20, 30, 255]);
        let image = decode(&bytes).expect("decode png");
        assert_eq!((image.width, image.height), (3, 2));
        assert_eq!(image.data.len(), 3 * 2 * 4);
        // 所有像素都是同一个颜色
        for y in 0..2u32 {
            for x in 0..3u32 {
                assert_eq!(image.pixel(x, y), Some([10, 20, 30, 255]));
            }
        }
    }

    #[test]
    fn decode_normalizes_rgb_and_grayscale_to_rgba() {
        // RGB（无 alpha）解码后 alpha 应为 255
        let mut buffer = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            2,
            2,
            image::Rgb([200, 100, 50]),
        ))
        .write_to(
            &mut std::io::Cursor::new(&mut buffer),
            image::ImageFormat::Png,
        )
        .expect("encode png");
        let image = decode(&buffer).expect("decode rgb png");
        assert_eq!(image.pixel(0, 0), Some([200, 100, 50, 255]));

        // 灰度图应复制到 RGB 三通道
        let mut buffer = Vec::new();
        image::DynamicImage::ImageLuma8(image::GrayImage::from_pixel(2, 2, image::Luma([128])))
            .write_to(
                &mut std::io::Cursor::new(&mut buffer),
                image::ImageFormat::Png,
            )
            .expect("encode png");
        let image = decode(&buffer).expect("decode luma png");
        assert_eq!(image.pixel(1, 1), Some([128, 128, 128, 255]));
    }

    #[test]
    fn decode_preserves_alpha() {
        let bytes = encode_png(1, 1, [1, 2, 3, 128]);
        let image = decode(&bytes).expect("decode");
        assert_eq!(image.pixel(0, 0), Some([1, 2, 3, 128]));
    }

    #[test]
    fn decode_detects_format_by_content_not_extension() {
        // 字节里没有文件名线索，格式靠魔数嗅探
        let png = encode_png(2, 2, [9, 9, 9, 255]);
        let mut jpeg = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(2, 2, image::Rgb([9, 9, 9])))
            .write_to(
                &mut std::io::Cursor::new(&mut jpeg),
                image::ImageFormat::Jpeg,
            )
            .expect("encode jpeg");
        // JPEG 魔噪点使像素不完全无损，只断言尺寸与类型
        assert_eq!(decode(&png).expect("png").width, 2);
        assert_eq!(decode(&jpeg).expect("jpeg").width, 2);
    }

    #[test]
    fn decode_rejects_non_image_bytes() {
        // HTML 文档不是图片
        let error = decode(b"<!DOCTYPE html><html><body>hi</body></html>").expect_err("not image");
        assert_eq!(error, DecodeError::InvalidImage);
        // 空字节
        assert_eq!(decode(&[]).expect_err("empty"), DecodeError::InvalidImage);
    }

    // ---- 解码预检：头部声明的巨图不进入像素解码（H3 防线） ----

    /// CRC-32（IEEE，PNG chunk 校验用）。
    fn crc32(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &byte in data {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    /// 把真实 1×1 PNG 的 IHDR 尺寸改写为声明值（重算 CRC，像素数据保持
    /// 1×1）——用于探测预检边界：预检通过者会进入像素解码并因数据不足报
    /// `InvalidImage`，预检不通过者在头部即报 `SizeUnsupported`。
    fn png_with_declared_size(width: u32, height: u32) -> Vec<u8> {
        let mut png = encode_png(1, 1, [0, 0, 255, 255]);
        // 签名 0..8，IHDR 长度 8..12、类型 12..16、数据 16..29、CRC 29..33
        png[16..20].copy_from_slice(&width.to_be_bytes());
        png[20..24].copy_from_slice(&height.to_be_bytes());
        let crc = crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        png
    }

    #[test]
    fn decode_rejects_huge_declared_dimensions_before_pixel_decode() {
        // 65535×65535：按旧实现会先分配约 17 GB 再检查
        let png = png_with_declared_size(65535, 65535);
        assert_eq!(
            decode(&png).expect_err("huge"),
            DecodeError::SizeUnsupported
        );
    }

    #[test]
    fn decode_preflight_boundary_checks() {
        // 边长上限两侧
        assert_eq!(
            decode(&png_with_declared_size(16385, 1)).expect_err("side over"),
            DecodeError::SizeUnsupported
        );
        assert_eq!(
            decode(&png_with_declared_size(1, 16385)).expect_err("side over"),
            DecodeError::SizeUnsupported
        );
        // 16384×1 在边长与面积预算内：预检通过，进入像素解码（数据不足 → InvalidImage）
        assert_eq!(
            decode(&png_with_declared_size(16384, 1)).expect_err("decodes"),
            DecodeError::InvalidImage
        );
        // 面积上限两侧：8192² = 2^26 恰好通过预检；8193² 超限
        assert_eq!(
            decode(&png_with_declared_size(8192, 8192)).expect_err("decodes"),
            DecodeError::InvalidImage
        );
        assert_eq!(
            decode(&png_with_declared_size(8193, 8192)).expect_err("area over"),
            DecodeError::SizeUnsupported
        );
    }

    #[test]
    fn decode_rejects_corrupt_image_data() {
        let mut bytes = encode_png(4, 4, [7, 7, 7, 255]);
        // 破坏压缩数据（保留魔数，使格式嗅探通过但解码失败）
        let tail = bytes.len();
        for byte in bytes.iter_mut().skip(tail / 2).take(16) {
            *byte ^= 0xff;
        }
        assert_eq!(
            decode(&bytes).expect_err("corrupt"),
            DecodeError::InvalidImage
        );
    }

    #[test]
    fn decoded_image_feeds_draw_image() {
        // 端到端：解码 → 资源池 → 光栅
        let bytes = encode_png(2, 2, [0, 200, 0, 255]);
        let image = decode(&bytes).expect("decode");
        let scene = Scene {
            commands: vec![Command::DrawImage {
                rect: rect(0.0, 0.0, 4.0, 4.0),
                image: 0,
            }],
            images: vec![Arc::new(image)],
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(6, 6))
            .expect("rasterize");
        assert_pixel_near(&pixmap, 1, 1, [0, 200, 0, 255]);
        assert_pixel_near(&pixmap, 3, 3, [0, 200, 0, 255]);
    }

    /// gif / webp 是 paint 显式开启的 feature，必须与 png/jpeg 一样有解码
    /// 测试——否则「声明支持」与「实际可用」可能脱节。
    #[test]
    fn decode_gif_yields_rgba() {
        let mut buffer = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            3,
            2,
            image::Rgba([12, 34, 56, 255]),
        ))
        .write_to(
            &mut std::io::Cursor::new(&mut buffer),
            image::ImageFormat::Gif,
        )
        .expect("encode gif");
        let decoded = decode(&buffer).expect("decode gif");
        assert_eq!((decoded.width, decoded.height), (3, 2));
        assert_eq!(decoded.pixel(0, 0), Some([12, 34, 56, 255]));
    }

    #[test]
    fn decode_webp_yields_rgba() {
        let mut buffer = Vec::new();
        // WebP 有损，断言尺寸与「不透明」而非精确颜色
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            4,
            4,
            image::Rgba([200, 100, 50, 255]),
        ))
        .write_to(
            &mut std::io::Cursor::new(&mut buffer),
            image::ImageFormat::WebP,
        )
        .expect("encode webp");
        let decoded = decode(&buffer).expect("decode webp");
        assert_eq!((decoded.width, decoded.height), (4, 4));
        let pixel = decoded.pixel(2, 2).expect("pixel");
        assert_eq!(pixel[3], 255, "alpha 应不透明");
        // 颜色在有损压缩误差范围内
        assert!(
            i32::from(pixel[0]).abs_diff(200) <= 12
                && i32::from(pixel[1]).abs_diff(100) <= 12
                && i32::from(pixel[2]).abs_diff(50) <= 12,
            "webp 颜色应在误差内，实际 {pixel:?}"
        );
    }

    /// 四种声明支持的格式都要能被嗅探解码——feature 与实现不得脱节。
    #[test]
    fn all_declared_formats_decode() {
        let source = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            2,
            2,
            image::Rgba([30, 60, 90, 255]),
        ));
        for format in [
            image::ImageFormat::Png,
            image::ImageFormat::Jpeg,
            image::ImageFormat::Gif,
            image::ImageFormat::WebP,
        ] {
            let mut buffer = Vec::new();
            source
                .write_to(&mut std::io::Cursor::new(&mut buffer), format)
                .expect("encode");
            let decoded =
                decode(&buffer).unwrap_or_else(|error| panic!("{format:?} 应可解码: {error:?}"));
            assert_eq!((decoded.width, decoded.height), (2, 2), "{format:?} 尺寸");
        }
    }

    /// translated 的坐标平移对全部指令生效，字形与图片像素共享不复制（M6）。
    #[test]
    fn translated_shifts_all_commands_and_shares_pixels() {
        let image = Arc::new(Image::new(
            2,
            2,
            vec![1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4, 1, 2, 3, 4],
        ));
        let scene = Scene {
            commands: vec![
                fill(1.0, 2.0, 3.0, 4.0, Color::opaque(255, 0, 0)),
                Command::StrokeRect {
                    rect: rect(5.0, 6.0, 7.0, 8.0),
                    color: Color::opaque(0, 255, 0),
                    width: 2.0,
                },
                Command::DrawText {
                    glyphs: vec![TextGlyph {
                        id: 9,
                        x: 10.0,
                        y: -2.0,
                        font: 0,
                    }],
                    baseline: 20.0,
                    fonts: Vec::new(),
                    size: 14.0,
                    color: Color::opaque(0, 0, 255),
                },
                Command::DrawImage {
                    rect: rect(1.0, 1.0, 2.0, 2.0),
                    image: 0,
                },
            ],
            images: vec![image],
        };
        let moved = scene.translated(3.0, -1.0);
        match &moved.commands[0] {
            Command::FillRect { rect, color } => {
                assert_eq!(
                    (rect.x, rect.y, rect.width, rect.height),
                    (4.0, 1.0, 3.0, 4.0)
                );
                assert_eq!(*color, Color::opaque(255, 0, 0));
            }
            other => panic!("first command changed: {other:?}"),
        }
        match &moved.commands[2] {
            Command::DrawText {
                glyphs, baseline, ..
            } => {
                assert_eq!(glyphs[0].x, 13.0);
                assert_eq!(glyphs[0].y, -2.0, "y 相对基线不平移");
                assert_eq!(*baseline, 19.0);
            }
            other => panic!("text command changed: {other:?}"),
        }
        match &moved.commands[3] {
            Command::DrawImage { rect, image: index } => {
                assert_eq!((rect.x, rect.y), (4.0, 0.0));
                assert_eq!(*index, 0);
            }
            other => panic!("image command changed: {other:?}"),
        }
        // 像素共享：平移副本与源场景的图片是同一份（Arc 引用计数）
        assert!(Arc::ptr_eq(&scene.images[0], &moved.images[0]));
        // 源场景不受影响（out-of-place）
        match &scene.commands[0] {
            Command::FillRect { rect, .. } => assert_eq!((rect.x, rect.y), (1.0, 2.0)),
            other => panic!("source changed: {other:?}"),
        }
    }

    /// 断言像素接近期望颜色（`tol` 为每通道容差；渐变插值与抖动用较宽容差）。
    fn assert_pixel_within(pixmap: &Pixmap, x: u32, y: u32, expected: [u8; 4], tol: u32) {
        let actual = pixmap.pixel(x, y).expect("pixel in bounds");
        for (index, (got, want)) in actual.iter().zip(expected).enumerate() {
            assert!(
                i32::from(*got).abs_diff(i32::from(want)) <= tol,
                "({x}, {y}) 通道{index}: 实际 {got}，期望 {want}±{tol}，像素 {actual:?}"
            );
        }
    }

    // ---- 圆角矩形填充（FillRoundedRect） ----

    #[test]
    fn fill_rounded_rect_cuts_all_four_corners() {
        // 48×48 盒（(8,8)–(56,56)）半径 12：角圆心 (20,20)/(44,20)/(44,44)/(20,44)
        let scene = Scene {
            commands: vec![Command::FillRoundedRect {
                rect: rect(8.0, 8.0, 48.0, 48.0),
                radii: [12.0; 4],
                color: Color::opaque(255, 0, 0),
            }],
            images: Vec::new(),
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(64, 64))
            .expect("rasterize");

        // 中心、直边中点与圆角内侧：完全不透明
        for (x, y) in [
            (32, 32),
            (32, 9),
            (9, 32),
            (55, 32),
            (32, 55),
            (24, 24),
            (40, 24),
            (40, 40),
            (24, 40),
        ] {
            assert_pixel_within(&pixmap, x, y, [255, 0, 0, 255], 8);
        }
        // 四角外侧（圆角截掉的区域）：透明
        for (x, y) in [(9, 9), (55, 9), (55, 55), (9, 55)] {
            let pixel = pixmap.pixel(x, y).expect("in bounds");
            assert!(pixel[3] <= 8, "({x}, {y}) 应在圆角外，实际 {pixel:?}");
        }
    }

    #[test]
    fn fill_rounded_rect_invalid_radii_render_sharp() {
        // 负值 / NaN / 无穷半径一律按 0 处理 → 直角矩形，四角全覆盖
        let scene = Scene {
            commands: vec![Command::FillRoundedRect {
                rect: rect(8.0, 8.0, 48.0, 48.0),
                radii: [-5.0, f32::NAN, f32::INFINITY, -0.5],
                color: Color::opaque(255, 0, 0),
            }],
            images: Vec::new(),
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(64, 64))
            .expect("rasterize");
        for (x, y) in [(9, 9), (55, 9), (55, 55), (9, 55), (32, 32)] {
            assert_pixel_within(&pixmap, x, y, [255, 0, 0, 255], 8);
        }
    }

    #[test]
    fn fill_rounded_rect_invalid_rect_is_skipped() {
        let cases = [
            Rect {
                x: f32::NAN,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            },
            Rect {
                x: 0.0,
                y: f32::NAN,
                width: 10.0,
                height: 10.0,
            },
            Rect {
                x: 0.0,
                y: 0.0,
                width: f32::NAN,
                height: 10.0,
            },
            Rect {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 10.0,
            },
            Rect {
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height: -3.0,
            },
            Rect {
                x: f32::INFINITY,
                y: 0.0,
                width: 10.0,
                height: 10.0,
            },
        ];
        for invalid in cases {
            let scene = Scene {
                commands: vec![Command::FillRoundedRect {
                    rect: invalid,
                    radii: [4.0; 4],
                    color: Color::opaque(255, 0, 0),
                }],
                images: Vec::new(),
            };
            let pixmap = VelloCpuRasterizer::new()
                .rasterize(&scene, size(16, 16))
                .expect("rasterize");
            assert!(
                pixmap.data.iter().all(|byte| *byte == 0),
                "非法矩形 {invalid:?} 应被跳过"
            );
        }
    }

    // ---- 圆角矩形描边（StrokeRoundedRect：border-box 内侧带） ----

    #[test]
    fn stroke_rounded_rect_band_geometry_and_hollow_center() {
        // border-box (8,8)–(56,56) 半径 12、带宽 4 → 中心线 (10,10)–(54,54)
        // 半径 10；直边带 y∈[8,12]，角带半径 ∈[8,12]（角圆心 (20,20)）
        let scene = Scene {
            commands: vec![Command::StrokeRoundedRect {
                rect: rect(8.0, 8.0, 48.0, 48.0),
                radii: [12.0; 4],
                color: Color::opaque(0, 0, 255),
                width: 4.0,
            }],
            images: Vec::new(),
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(64, 64))
            .expect("rasterize");

        // 直边带与弧带
        assert_pixel_within(&pixmap, 32, 9, [0, 0, 255, 255], 8);
        assert_pixel_within(&pixmap, 13, 13, [0, 0, 255, 255], 8);
        // 带外（上侧）、空心中心与角圆心
        for (x, y) in [(32, 7), (32, 32), (20, 20)] {
            let pixel = pixmap.pixel(x, y).expect("in bounds");
            assert!(pixel[3] == 0, "({x}, {y}) 应空心，实际 {pixel:?}");
        }
        // border-box 外无描边
        let pixel = pixmap.pixel(9, 9).expect("in bounds");
        assert!(pixel[3] <= 8, "border-box 外应无描边，实际 {pixel:?}");
    }

    #[test]
    fn stroke_rounded_rect_invalid_width_is_noop() {
        for width in [0.0f32, -2.0, f32::NAN, f32::INFINITY] {
            let scene = Scene {
                commands: vec![Command::StrokeRoundedRect {
                    rect: rect(8.0, 8.0, 48.0, 48.0),
                    radii: [12.0; 4],
                    color: Color::opaque(0, 0, 255),
                    width,
                }],
                images: Vec::new(),
            };
            let pixmap = VelloCpuRasterizer::new()
                .rasterize(&scene, size(64, 64))
                .expect("rasterize");
            assert!(
                pixmap.data.iter().all(|byte| *byte == 0),
                "线宽 {width} 应不绘制"
            );
        }
    }

    #[test]
    fn stroke_rounded_rect_degenerate_inner_rect_is_skipped() {
        // 带宽 ≥ 盒边长：中心线内缩后退化，无带可画 → 跳过而非 panic
        for width in [48.0f32, 60.0] {
            let scene = Scene {
                commands: vec![Command::StrokeRoundedRect {
                    rect: rect(8.0, 8.0, 48.0, 48.0),
                    radii: [12.0; 4],
                    color: Color::opaque(0, 0, 255),
                    width,
                }],
                images: Vec::new(),
            };
            let pixmap = VelloCpuRasterizer::new()
                .rasterize(&scene, size(64, 64))
                .expect("rasterize");
            assert!(
                pixmap.data.iter().all(|byte| *byte == 0),
                "带宽 {width} ≥ 边长应跳过"
            );
        }
    }

    // ---- 线性渐变填充（FillGradient） ----

    #[test]
    fn fill_gradient_endpoints_and_midpoint() {
        // 水平渐变（场景坐标端点 (0,8)→(64,8)），矩形 (0,8,64,16)
        let scene = Scene {
            commands: vec![Command::FillGradient {
                rect: rect(0.0, 8.0, 64.0, 16.0),
                radii: [0.0; 4],
                start: [0.0, 8.0],
                end: [64.0, 8.0],
                stops: vec![
                    GradientStop {
                        offset: 0.0,
                        color: Color::opaque(255, 0, 0),
                    },
                    GradientStop {
                        offset: 1.0,
                        color: Color::opaque(0, 0, 255),
                    },
                ],
            }],
            images: Vec::new(),
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(64, 32))
            .expect("rasterize");

        // t = x/64 的 sRGB 线性插值（光栅抖动容差 ±16）
        assert_pixel_within(&pixmap, 2, 16, [247, 0, 8, 255], 16);
        assert_pixel_within(&pixmap, 32, 16, [128, 0, 128, 255], 16);
        assert_pixel_within(&pixmap, 62, 16, [8, 0, 247, 255], 16);
        // 矩形外透明
        assert_eq!(pixmap.pixel(32, 4), Some([0, 0, 0, 0]));
        assert_eq!(pixmap.pixel(32, 28), Some([0, 0, 0, 0]));
    }

    #[test]
    fn fill_gradient_explicit_stop_offsets() {
        // 显式 offset 三色标：25% 处纯绿，绿→蓝插值中点在 t=0.625 → x=40
        let scene = Scene {
            commands: vec![Command::FillGradient {
                rect: rect(0.0, 0.0, 64.0, 8.0),
                radii: [0.0; 4],
                start: [0.0, 0.0],
                end: [64.0, 0.0],
                stops: vec![
                    GradientStop {
                        offset: 0.0,
                        color: Color::opaque(255, 0, 0),
                    },
                    GradientStop {
                        offset: 0.25,
                        color: Color::opaque(0, 255, 0),
                    },
                    GradientStop {
                        offset: 1.0,
                        color: Color::opaque(0, 0, 255),
                    },
                ],
            }],
            images: Vec::new(),
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(64, 16))
            .expect("rasterize");

        assert_pixel_within(&pixmap, 16, 4, [0, 255, 0, 255], 8);
        assert_pixel_within(&pixmap, 40, 4, [0, 128, 128, 255], 16);
        assert_pixel_within(&pixmap, 62, 4, [0, 8, 247, 255], 16);
    }

    #[test]
    fn fill_gradient_clipped_by_rounded_radii() {
        // 渐变 × 圆角：单条指令完成，角外被路径裁剪
        let scene = Scene {
            commands: vec![Command::FillGradient {
                rect: rect(8.0, 8.0, 48.0, 48.0),
                radii: [12.0; 4],
                start: [8.0, 0.0],
                end: [56.0, 0.0],
                stops: vec![
                    GradientStop {
                        offset: 0.0,
                        color: Color::opaque(255, 0, 0),
                    },
                    GradientStop {
                        offset: 1.0,
                        color: Color::opaque(0, 0, 255),
                    },
                ],
            }],
            images: Vec::new(),
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(64, 64))
            .expect("rasterize");

        // 中心与直边中点都在渐变中部（x=32 → t≈0.5）
        assert_pixel_within(&pixmap, 32, 32, [128, 0, 128, 255], 20);
        assert_pixel_within(&pixmap, 32, 9, [128, 0, 128, 255], 20);
        // 圆角外被裁剪
        for (x, y) in [(9, 9), (55, 9), (55, 55), (9, 55)] {
            let pixel = pixmap.pixel(x, y).expect("in bounds");
            assert!(
                pixel[3] <= 8,
                "({x}, {y}) 渐变应在圆角外被裁剪，实际 {pixel:?}"
            );
        }
    }

    #[test]
    fn fill_gradient_invalid_input_is_skipped() {
        let stops = || {
            vec![
                GradientStop {
                    offset: 0.0,
                    color: Color::opaque(255, 0, 0),
                },
                GradientStop {
                    offset: 1.0,
                    color: Color::opaque(0, 0, 255),
                },
            ]
        };
        let cases = [
            // 端点非有限
            (
                rect(0.0, 0.0, 64.0, 8.0),
                [f32::NAN, 0.0],
                [64.0, 0.0],
                stops(),
            ),
            (
                rect(0.0, 0.0, 64.0, 8.0),
                [0.0, 0.0],
                [f32::NEG_INFINITY, 0.0],
                stops(),
            ),
            // 色标为空
            (
                rect(0.0, 0.0, 64.0, 8.0),
                [0.0, 0.0],
                [64.0, 0.0],
                Vec::new(),
            ),
            // 矩形非法
            (
                Rect {
                    x: f32::NAN,
                    y: 0.0,
                    width: 64.0,
                    height: 8.0,
                },
                [0.0, 0.0],
                [64.0, 0.0],
                stops(),
            ),
        ];
        for (invalid_rect, start, end, stops) in cases {
            let scene = Scene {
                commands: vec![Command::FillGradient {
                    rect: invalid_rect,
                    radii: [0.0; 4],
                    start,
                    end,
                    stops,
                }],
                images: Vec::new(),
            };
            let pixmap = VelloCpuRasterizer::new()
                .rasterize(&scene, size(64, 16))
                .expect("rasterize");
            assert!(
                pixmap.data.iter().all(|byte| *byte == 0),
                "非法渐变输入应被跳过"
            );
        }
    }

    #[test]
    fn fill_gradient_drops_non_finite_offset_stops() {
        // 中间色标 offset 为 NaN：该色标被丢弃，红→蓝照常插值
        let scene = Scene {
            commands: vec![Command::FillGradient {
                rect: rect(0.0, 8.0, 64.0, 16.0),
                radii: [0.0; 4],
                start: [0.0, 8.0],
                end: [64.0, 8.0],
                stops: vec![
                    GradientStop {
                        offset: 0.0,
                        color: Color::opaque(255, 0, 0),
                    },
                    GradientStop {
                        offset: f32::NAN,
                        color: Color::opaque(0, 255, 0),
                    },
                    GradientStop {
                        offset: 1.0,
                        color: Color::opaque(0, 0, 255),
                    },
                ],
            }],
            images: Vec::new(),
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(64, 32))
            .expect("rasterize");
        assert_pixel_within(&pixmap, 2, 16, [247, 0, 8, 255], 16);
        assert_pixel_within(&pixmap, 32, 16, [128, 0, 128, 255], 16);
        assert_pixel_within(&pixmap, 62, 16, [8, 0, 247, 255], 16);
    }

    // ---- CSS Backgrounds §5.5 半径缩减（纯函数 + 像素级） ----

    #[test]
    fn sanitize_radii_clamps_negative_and_non_finite_to_zero() {
        assert_eq!(
            sanitize_radii([3.0, -2.0, f32::NAN, f32::INFINITY]),
            [3.0, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn scale_radii_noop_when_adjacent_sums_fit_edges() {
        assert_eq!(
            scale_overlapping_radii(100.0, 60.0, [40.0, 40.0, 0.0, 0.0]),
            [40.0, 40.0, 0.0, 0.0]
        );
    }

    #[test]
    fn scale_radii_applies_min_factor_across_all_edges() {
        // 100×60，radii [80,40,0,0]：上边和 120/100 → f≈0.833；左边和 80/60 →
        // f=0.75；取最小 f=0.75 施加到**全部**四角（CSS §5.5）
        assert_eq!(
            scale_overlapping_radii(100.0, 60.0, [80.0, 40.0, 0.0, 0.0]),
            [60.0, 30.0, 0.0, 0.0]
        );
    }

    #[test]
    fn scale_radii_single_oversized_corner_is_capped_to_edge() {
        // 32×32 单角 128：相邻和 128 > 32 → f = 0.25 → 半径缩到边长
        assert_eq!(
            scale_overlapping_radii(32.0, 32.0, [128.0, 0.0, 0.0, 0.0]),
            [32.0, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn scale_radii_zero_sums_need_no_scaling() {
        assert_eq!(scale_overlapping_radii(32.0, 32.0, [0.0; 4]), [0.0; 4]);
    }

    #[test]
    fn fill_rounded_rect_overlarge_radii_scale_down_per_css() {
        // 40×40 盒四角半径 30：相邻和 60 > 40 → f = 2/3 → 全部缩到 20，
        // 四角圆心重合于盒中心 (32,32)，形状恰为半径 20 的圆
        let scene = Scene {
            commands: vec![Command::FillRoundedRect {
                rect: rect(12.0, 12.0, 40.0, 40.0),
                radii: [30.0; 4],
                color: Color::opaque(255, 0, 0),
            }],
            images: Vec::new(),
        };
        let pixmap = VelloCpuRasterizer::new()
            .rasterize(&scene, size(64, 64))
            .expect("rasterize");

        // 缩减后（r=20，圆心 (32,32)）在形状内；若未缩减（r=30，圆心 (42,42)），
        // (20,20) 距圆心 31.1 在形状外——该点是缩减行为的判别点
        assert_pixel_within(&pixmap, 20, 20, [255, 0, 0, 255], 8);
        assert_pixel_within(&pixmap, 32, 32, [255, 0, 0, 255], 8);
        assert_pixel_within(&pixmap, 45, 32, [255, 0, 0, 255], 8);
        // 圆外：距 (32,32) 24.7 > 20
        let pixel = pixmap.pixel(14, 14).expect("in bounds");
        assert!(pixel[3] <= 8, "缩减后 (14,14) 应在形状外，实际 {pixel:?}");
    }

    // ---- M6 兼容：新指令进 Scene::translated 平移路径 ----

    #[test]
    fn translated_shifts_rounded_and_gradient_commands() {
        let red = Color::opaque(255, 0, 0);
        let blue = Color::opaque(0, 0, 255);
        let stops = vec![
            GradientStop {
                offset: 0.0,
                color: red,
            },
            GradientStop {
                offset: 1.0,
                color: blue,
            },
        ];
        let scene = Scene {
            commands: vec![
                Command::FillRoundedRect {
                    rect: rect(1.0, 2.0, 3.0, 4.0),
                    radii: [1.0, 2.0, 3.0, 4.0],
                    color: red,
                },
                Command::StrokeRoundedRect {
                    rect: rect(5.0, 6.0, 7.0, 8.0),
                    radii: [4.0; 4],
                    color: blue,
                    width: 2.0,
                },
                Command::FillGradient {
                    rect: rect(0.0, 0.0, 10.0, 10.0),
                    radii: [0.0; 4],
                    start: [10.0, 20.0],
                    end: [30.0, 40.0],
                    stops: stops.clone(),
                },
            ],
            images: Vec::new(),
        };
        let moved = scene.translated(3.0, -1.0);
        match &moved.commands[0] {
            Command::FillRoundedRect { rect, radii, color } => {
                assert_eq!(
                    (rect.x, rect.y, rect.width, rect.height),
                    (4.0, 1.0, 3.0, 4.0)
                );
                assert_eq!(*radii, [1.0, 2.0, 3.0, 4.0]);
                assert_eq!(*color, red);
            }
            other => panic!("first command changed: {other:?}"),
        }
        match &moved.commands[1] {
            Command::StrokeRoundedRect {
                rect,
                radii,
                color,
                width,
            } => {
                assert_eq!(
                    (rect.x, rect.y, rect.width, rect.height),
                    (8.0, 5.0, 7.0, 8.0)
                );
                assert_eq!(*radii, [4.0; 4]);
                assert_eq!((*color, *width), (blue, 2.0));
            }
            other => panic!("second command changed: {other:?}"),
        }
        match &moved.commands[2] {
            Command::FillGradient {
                rect,
                radii,
                start,
                end,
                stops: moved_stops,
            } => {
                assert_eq!((rect.x, rect.y), (3.0, -1.0));
                assert_eq!(*radii, [0.0; 4]);
                // 渐变端点是场景坐标：必须与矩形同步平移（漏移则滚动后错位）
                assert_eq!(*start, [13.0, 19.0]);
                assert_eq!(*end, [33.0, 39.0]);
                assert_eq!(*moved_stops, stops);
            }
            other => panic!("third command changed: {other:?}"),
        }
        // 源场景不受影响
        match &scene.commands[2] {
            Command::FillGradient { start, end, .. } => {
                assert_eq!((*start, *end), ([10.0, 20.0], [30.0, 40.0]));
            }
            other => panic!("source changed: {other:?}"),
        }
    }

    #[test]
    fn translated_new_commands_render_identically_at_offset() {
        // 整数平移后逐像素一致（亚像素相位不变），渐变端点随矩形同步平移
        let red = Color::opaque(255, 0, 0);
        let blue = Color::opaque(0, 0, 255);
        let scene = Scene {
            commands: vec![
                Command::FillRoundedRect {
                    rect: rect(2.0, 2.0, 20.0, 20.0),
                    radii: [6.0; 4],
                    color: red,
                },
                Command::StrokeRoundedRect {
                    rect: rect(2.0, 26.0, 20.0, 20.0),
                    radii: [4.0; 4],
                    color: blue,
                    width: 3.0,
                },
                Command::FillGradient {
                    rect: rect(2.0, 50.0, 20.0, 12.0),
                    radii: [3.0, 0.0, 3.0, 0.0],
                    start: [2.0, 50.0],
                    end: [22.0, 62.0],
                    stops: vec![
                        GradientStop {
                            offset: 0.0,
                            color: red,
                        },
                        GradientStop {
                            offset: 1.0,
                            color: blue,
                        },
                    ],
                },
            ],
            images: Vec::new(),
        };
        let direct = VelloCpuRasterizer::new()
            .rasterize(&scene, size(32, 72))
            .expect("rasterize");
        let moved = scene.translated(8.0, 4.0);
        let shifted = VelloCpuRasterizer::new()
            .rasterize(&moved, size(32, 72))
            .expect("rasterize");

        // 非空验证：三条指令各自应有像素落下（防止双方全透明导致空断言）
        assert_eq!(direct.pixel(12, 12), Some([255, 0, 0, 255]));
        assert_eq!(direct.pixel(12, 27), Some([0, 0, 255, 255]));
        assert_pixel_within(&direct, 12, 56, [128, 0, 128, 255], 16);

        // 平移场景在 (+8, +4) 处与原场景逐像素一致（渐变抖动允许 ±2）
        for y in 0..68u32 {
            for x in 0..24u32 {
                let want = direct.pixel(x, y).expect("in bounds");
                let got = shifted.pixel(x + 8, y + 4).expect("in bounds");
                for (index, (g, w)) in got.iter().zip(want).enumerate() {
                    assert!(
                        i32::from(*g).abs_diff(i32::from(w)) <= 2,
                        "({x}, {y}) 通道{index}: 平移后 {g}，原始 {w}"
                    );
                }
            }
        }
    }
}
