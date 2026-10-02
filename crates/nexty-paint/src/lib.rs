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

use std::collections::HashMap;
use std::sync::Arc;

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
    /// 绘制一行字形。
    ///
    /// 字形必须已经过 text 层整形（字形 id 对应 [`Command::DrawText::families`]
    /// 解析出的字体）；`glyphs[i].y` 相对基线，`baseline` 是基线的绝对 y。
    DrawText {
        /// 字形序列（x 为绝对坐标，y 相对基线）。
        glyphs: Vec<TextGlyph>,
        /// 基线的绝对 y，px。
        baseline: f32,
        /// 字体族列表（CSS font-family 语义），经 text 层解析为字体数据。
        families: Vec<String>,
        /// 字号，px。
        size: f32,
        /// 文本颜色。
        color: Color,
    },
    /// 绘制一幅已解码图像。
    ///
    /// 像素经 [`Scene::images`] 按下标间接引用，不内联在指令里：`Scene`
    /// 每帧都要整体 clone（渲染线程按值接收），内联会让每次 clone 退化成
    /// 大块内存拷贝。
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
    /// 图像资源池；[`Command::DrawImage`] 按下标引用。
    pub images: Vec<Image>,
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
#[derive(Clone)]
struct FontBytes(Arc<[u8]>);

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
    /// 尺寸超出后端上限（vello_cpu 的渲染目标为 u16 像素）。
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
/// 文本渲染的字体解析经 text 层的 [`nexty_text::FontResolver`] 完成，
/// 按族列表缓存解析结果。
#[derive(Debug, Default)]
pub struct VelloCpuRasterizer {
    font_cache: std::sync::Mutex<HashMap<String, Option<std::sync::Arc<nexty_text::ResolvedFont>>>>,
}

impl VelloCpuRasterizer {
    /// 创建 CPU 光栅后端。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 解析并缓存字体（族列表 → 字体数据）。
    fn font_for(&self, families: &[String]) -> Option<std::sync::Arc<nexty_text::ResolvedFont>> {
        let key = families.join(", ");
        let mut cache = self.font_cache.lock().expect("font cache poisoned");
        if let Some(cached) = cache.get(&key) {
            return cached.clone();
        }
        let resolved = nexty_text::FontResolver::new()
            .resolve_font(families)
            .map(Arc::from);
        cache.insert(key, resolved.clone());
        resolved
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
                Command::DrawText {
                    glyphs,
                    baseline,
                    families,
                    size,
                    color,
                } => {
                    let Some(font) = self.font_for(families) else {
                        continue;
                    };
                    context.set_paint(vello_cpu::peniko::Color::from_rgba8(
                        color.r, color.g, color.b, color.a,
                    ));
                    // 字体数据按引用计数共享给 vello 的 Blob
                    let bytes: Arc<dyn AsRef<[u8]> + Send + Sync> =
                        Arc::new(FontBytes(font.data().clone()));
                    let font_data = vello_cpu::peniko::FontData::new(
                        vello_cpu::peniko::Blob::new(bytes),
                        font.index(),
                    );
                    let glyphs = glyphs.iter().map(|glyph| vello_cpu::Glyph {
                        id: glyph.id,
                        x: glyph.x,
                        y: baseline + glyph.y,
                    });
                    context
                        .glyph_run(&mut resources, &font_data)
                        .font_size(*size)
                        .fill_glyphs(glyphs);
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
    fn shape_char(character: char) -> (Vec<TextGlyph>, f32) {
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
            })
            .collect();
        (glyphs, metrics.ascent)
    }

    #[test]
    fn draw_text_produces_opaque_pixels() {
        let (glyphs, ascent) = shape_char('A');
        let scene = Scene {
            commands: vec![Command::DrawText {
                glyphs,
                baseline: 40.0,
                families: vec!["serif".to_owned()],
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
        let (glyphs, _) = shape_char('A');
        let scene = Scene {
            commands: vec![
                Command::FillRect {
                    rect: rect(0.0, 0.0, 48.0, 48.0),
                    color: Color::opaque(255, 255, 255),
                },
                Command::DrawText {
                    glyphs,
                    baseline: 40.0,
                    families: vec!["serif".to_owned()],
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
            images: vec![solid_image(4, 5, [0, 128, 255, 255])],
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
            images: vec![solid_image(2, 2, [255, 0, 0, 255])],
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
            images: vec![solid_image(8, 8, [0, 255, 0, 255])],
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
            images: vec![solid_image(4, 4, [255, 255, 255, 128])],
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
            images: vec![solid_image(4, 4, [255, 0, 0, 255])],
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
            images: vec![Image::new(4, 4, vec![255, 0, 0])],
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
            images: vec![solid_image(4, 4, [255, 0, 0, 255])],
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
}
