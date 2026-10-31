//! 导航管线：HTML 字符串 → 级联 → 布局 → 片段树 → paint 显示列表。
//!
//! 行为分工：解析归 nexty-html，级联归 nexty-css（UA + author 双 origin），
//! 布局归 nexty-layout，本模块负责把片段树转译为 [`nexty_paint::Scene`]：
//! 背景与渐变（css-images-3 渐变线端点在 emit 前换算）、CSS 边框（四边等宽
//! 有圆角时一条圆角描边，否则四条填充带）、行内文本 run 与装饰带
//! （坐标换算到绝对坐标系）、行内/块级图片（`DrawImage`，像素从页面图片池
//! 按节点索引）。匿名片段不产生背景与边框；背景绘制次序遵循
//! z-index 基础定序（普通流先、定位元素后，定位兄弟按整数排序）。
//!
//! 画布底色（CSS 2.2 §14.2）：html/body 背景传播到画布——html 携带背景
//! 用 html，否则用 body，都无回落白（[`canvas_background_commands`]）；
//! 传播是搬移语义，被传播的元素不再自绘背景。
//!
//! 已知偏差：z-index 为基础定序档，不做完整 stacking context——定位元素
//! 统一画在普通流之上（负 z-index 不回落普通流之下），命中测试也不镜像
//! 定位层序（K7 补）；border-radius 百分比按边框盒对应轴折算后取两轴
//! 较小分量（paint 只有圆形角，椭圆 `/` 语法 css 层已整条拒收）；四边
//! border-width 不等时圆角边框降级直角填充带；text-decoration 按文本 run
//! 自身元素的 computed style 绘制（规范的祖先「传播」未实现，css 层偏差；
//! 跨行内元素边界按 run 分段断线）、非 solid 不画、度量用经验值
//! （0.06em 厚 / underline 基线下 0.08em，无字体 underline 数据）；
//! 片段树不做滚动裁剪。

use std::collections::HashMap;
use std::sync::Arc;

use nexty_css::{
    BackgroundImageValue, BorderRadiusValue, ComputedStyle, GradientColorStop, MediaViewport,
    PositionValue, Rgba, Stylesheet, TextDecorationLineValue, TextDecorationStyleValue,
    ZIndexValue, html_ua_stylesheet,
};
use nexty_dom::{Document, Event, EventLoop, NodeId, Task, TaskContext};
use nexty_html::ParseOptions;
use nexty_layout::{Fragment, Rect as LayoutRect, TextRun};
use nexty_paint::{Color, Command, GradientStop, Image, Rect as PaintRect, Scene, Size, TextGlyph};
use nexty_text::TextShaper;

/// 一个已解析、已级联的页面。
pub struct Page {
    /// arena 文档。
    pub document: Document,
    /// 各元素的 computed style。
    pub styles: HashMap<NodeId, ComputedStyle>,
    /// 作者样式表（视口变化后重级联用；UA 样式表固定不存）。
    stylesheets: Vec<Stylesheet>,
    /// 内联样式解析缓存（M5 ②）：resize 后 recascade 按 style 属性原文
    /// 校验复用，不重解析字符串。
    inline_style_cache: nexty_css::InlineStyleCache,
    /// 已解码图片（`<img>` 节点 → RGBA 图像），构建 Scene 时进入图片池。
    /// 按 `Arc` 共享：构建/发送/光栅全程零像素拷贝（M1）。
    pub images: HashMap<NodeId, Arc<Image>>,
}

/// 用已解析文档与作者样式表构建页面（UA 样式表固定为 nexty-css 的 HTML 默认）。
///
/// `stylesheets` 按级联优先级排列（CSS Cascade：源顺序靠后者胜出）——
/// inline `<style>` 与外链样式表按文档出现顺序交错。
///
/// 级联在此先做一次（无视口，视口单位保持未折算）；布局前由调用方以
/// 实际视口走 [`recascade`]（后台线程拿不到窗口尺寸）。
#[must_use]
pub fn build_page(
    document: Document,
    stylesheets: Vec<Stylesheet>,
    images: HashMap<NodeId, Arc<Image>>,
) -> Page {
    let sheet_refs: Vec<&Stylesheet> = stylesheets.iter().collect();
    let mut inline_style_cache = nexty_css::InlineStyleCache::new();
    let styles = nexty_css::compute_document_styles_in_with_cache(
        &document,
        &sheet_refs,
        Some(html_ua_stylesheet()),
        None,
        &mut inline_style_cache,
    );
    Page {
        document,
        styles,
        stylesheets,
        inline_style_cache,
        images,
    }
}

/// 按给定视口重新级联：视口单位（vw/vh/vmin/vmax）在 computed 阶段
/// 折算为绝对 px。resize 后布局前必须调用，否则视口尺寸滞后。
pub fn recascade(page: &mut Page, viewport: MediaViewport) {
    let sheet_refs: Vec<&Stylesheet> = page.stylesheets.iter().collect();
    page.styles = nexty_css::compute_document_styles_in_with_cache(
        &page.document,
        &sheet_refs,
        Some(html_ua_stylesheet()),
        Some(viewport),
        &mut page.inline_style_cache,
    );
}

/// 解析并级联一份页面（单张作者样式表，无图片——内部页/空白页用）。
#[must_use]
pub fn load_page(html: &str, author_css: &str) -> Page {
    let document = nexty_html::parse_document(html, ParseOptions::default());
    let sheet = Stylesheet::parse(author_css);
    build_page(document, vec![sheet], HashMap::new())
}

/// 页面待派发的生命周期事件信号集（K1 T3）。
///
/// 信号在后台线程于真实完成点捕获（HTML 解析返回 / 首轮子资源编排
/// settle——含失败/超时/预算降级路径），随页面载荷送回主线程；主线程经
/// [`Page::post_lifecycle_tasks`] post 进 EventLoop 再排空派发。后台线程
/// 只投信号，不得跨线程触碰 [`Document`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PendingLifecycle {
    /// HTML 解析完成 → `DOMContentLoaded`（bubbles=true、cancelable=false，
    /// 文档根上派发）。
    pub dom_content_loaded: bool,
    /// 首轮子资源编排 settle（含失败/超时/预算降级）→ `load`（bubbles=false）。
    pub load: bool,
}

impl PendingLifecycle {
    /// 两个信号都未到。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.dom_content_loaded && !self.load
    }
}

impl Page {
    /// 导航/解析完成点的**单处** EventLoop 钩子（K1 T3）：把信号了的生命
    /// 周期事件按 `DOMContentLoaded` → `load` 顺序 post 进 EventLoop
    /// （FIFO 保序）；任务执行时在文档根上派发（根无祖先链 → 仅 target
    /// 阶段，恰好一次）。
    ///
    /// 本方法不存状态：同一信号重复 post 会重复派发——每页只 post 一次
    /// 由调用方（页面安装点）保证。
    pub fn post_lifecycle_tasks(&mut self, event_loop: &mut EventLoop, pending: PendingLifecycle) {
        if pending.dom_content_loaded {
            event_loop.post_task(Task::new(|ctx: &mut TaskContext| {
                let document = ctx.document();
                let root = document.root();
                let mut event = Event::dom_content_loaded();
                document.dispatch(root, &mut event);
            }));
        }
        if pending.load {
            event_loop.post_task(Task::new(|ctx: &mut TaskContext| {
                let document = ctx.document();
                let root = document.root();
                let mut event = Event::load();
                document.dispatch(root, &mut event);
            }));
        }
    }
}

/// 对页面做布局，返回根元素片段。
#[must_use]
pub fn layout_page(page: &Page, shaper: &dyn TextShaper, viewport_width: f32) -> Option<Fragment> {
    // 布局层只要自然尺寸（CSS px = 像素 1:1）
    let image_sizes: HashMap<NodeId, (f32, f32)> = page
        .images
        .iter()
        .map(|(node, image)| (*node, (image.width as f32, image.height as f32)))
        .collect();
    nexty_layout::layout_document(
        &page.document,
        shaper,
        &page.styles,
        &image_sizes,
        viewport_width,
    )
}

/// 选区高亮色（半透明蓝，src-over 叠在页面背景之上）。
const SELECTION_COLOR: Color = Color {
    r: 0,
    g: 100,
    b: 255,
    a: 64,
};

/// 把片段树转译为显示列表，原点在视口左上角。
#[must_use]
pub fn build_scene(page: &Page, root: &Fragment) -> Scene {
    build_scene_at(page, root, 0.0, 0.0)
}

/// 把片段树转译为显示列表，并整体平移到 `(origin_x, origin_y)`。
///
/// 用于给页面内容留出顶部 UI（如地址栏）占用的区域。图片按绘制顺序进入
/// `Scene.images` 池，`DrawImage` 按下标引用（同一节点多图共享一份像素）。
#[must_use]
pub fn build_scene_at(page: &Page, root: &Fragment, origin_x: f32, origin_y: f32) -> Scene {
    build_scene_at_with_highlights(page, root, origin_x, origin_y, &[])
}

/// 同 [`build_scene_at`]，并在每个行盒的文字之前插入选区高亮矩形。
///
/// `highlights` 是 `TextMap::rects` 的输出（相对根片段边框盒）。高亮按行
/// 发射：只在该行自身字形与矩形相交时绘制，故位于页面背景/边框之上、
/// 文字与图片之下；嵌套行内原子盒的矩形不会在父行重复绘制。
#[must_use]
pub fn build_scene_at_with_highlights(
    page: &Page,
    root: &Fragment,
    origin_x: f32,
    origin_y: f32,
    highlights: &[LayoutRect],
) -> Scene {
    let mut scene = Scene::default();
    let mut pool: HashMap<NodeId, usize> = HashMap::new();
    // §14.2 传播源（html 优先、body 回落）：emit 内被传播的元素不再自绘
    // 背景（搬移语义）；全树只查一次，递归透传。
    let canvas_source = canvas_background_source(page);
    // 高亮矩形相对根片段；先平移到场景坐标，递归 emit 里不再叠加 origin
    //（emit 的 origin 是父链累积位置，不是场景原点）。
    let highlights: Vec<LayoutRect> = highlights
        .iter()
        .map(|rect| LayoutRect {
            x: rect.x + origin_x,
            y: rect.y + origin_y,
            ..*rect
        })
        .collect();
    emit(
        page,
        root,
        origin_x,
        origin_y,
        &mut scene,
        &mut pool,
        &highlights,
        canvas_source,
    );
    scene
}

/// 文本装饰带的经验度量（em 单位；无字体 underline 度量可用——T0 报告
/// F11，记偏差）：厚度一律 `0.06em`；underline 带顶在基线下 `0.08em`；
/// overline 近似 em 盒顶（基线上方 `0.88em` ≈ 0.8em ascent + 0.08em）；
/// line-through 穿过 x 高中部近似（基线上方 `0.30em`）。
const DECORATION_THICKNESS_EM: f32 = 0.06;
const UNDERLINE_OFFSET_EM: f32 = 0.08;
const OVERLINE_OFFSET_EM: f32 = 0.88;
const LINE_THROUGH_OFFSET_EM: f32 = 0.30;

/// 递归 emit 的固定形态参数较多；八参均在用，重签名不值当（针对性抑制）。
#[allow(clippy::too_many_arguments)]
fn emit(
    page: &Page,
    fragment: &Fragment,
    origin_x: f32,
    origin_y: f32,
    scene: &mut Scene,
    pool: &mut HashMap<NodeId, usize>,
    highlights: &[LayoutRect],
    canvas_source: Option<NodeId>,
) {
    let x = origin_x + fragment.border_box.x;
    let y = origin_y + fragment.border_box.y;
    let width = fragment.border_box.width;
    let height = fragment.border_box.height;
    let style = &fragment.style;
    // 四角圆角解析为 px（CSS Backgrounds §5.5；全 0 保持直角旧路径）
    let radii = resolve_corner_radii(style, width, height);
    let rounded = radii != [0.0; 4];
    // §14.2 传播是搬移：被传播的元素不再自绘背景（颜色与渐变）
    let background_propagated = !fragment.anonymous && Some(fragment.node) == canvas_source;

    if !fragment.anonymous {
        // 背景色（透明跳过；有圆角走 FillRoundedRect，全 0 保持 FillRect）
        if style.background_color.a > 0 && !background_propagated {
            let rect = PaintRect {
                x,
                y,
                width,
                height,
            };
            let color = to_color(style.background_color);
            if rounded {
                scene
                    .commands
                    .push(Command::FillRoundedRect { rect, radii, color });
            } else {
                scene.commands.push(Command::FillRect { rect, color });
            }
        }
        // 渐变背景（CSS Images §3.4，画在背景色之上）：渐变线端点按
        // css-images-3 公式在 emit 前换算，radii 随 border_radius——单条
        // 指令完成渐变 × 圆角。
        if !background_propagated
            && let BackgroundImageValue::LinearGradient(gradient) = &style.background_image
        {
            let (start, end) = linear_gradient_endpoints(gradient.angle, x, y, width, height);
            let stops = gradient_stops(&gradient.stops);
            if !stops.is_empty() {
                scene.commands.push(Command::FillGradient {
                    rect: PaintRect {
                        x,
                        y,
                        width,
                        height,
                    },
                    radii,
                    start,
                    end,
                    stops,
                });
            }
        }
        // CSS 边框：画在边框盒内侧。四边宽度相等且有圆角 → 一条圆角描边
        //（facade 内缩 `width/2`，带恰落在 [0, width]，颜色取 top——四边
        // 异色记偏差）；否则四条填充带精确表达（宽 0 跳过；宽不等时圆角
        // 降级直角——偏差）。
        let border = fragment.border;
        let colors = &style.border_color;
        let uniform_border =
            border.top == border.bottom && border.left == border.right && border.top == border.left;
        if border.top > 0.0 && rounded && uniform_border {
            scene.commands.push(Command::StrokeRoundedRect {
                rect: PaintRect {
                    x,
                    y,
                    width,
                    height,
                },
                radii,
                color: to_color(colors.top),
                width: border.top,
            });
        } else {
            if border.top > 0.0 {
                push_strip(scene, x, y, width, border.top, colors.top);
            }
            if border.bottom > 0.0 {
                push_strip(
                    scene,
                    x,
                    y + height - border.bottom,
                    width,
                    border.bottom,
                    colors.bottom,
                );
            }
            if border.left > 0.0 {
                push_strip(
                    scene,
                    x,
                    y + border.top,
                    border.left,
                    height - border.top - border.bottom,
                    colors.left,
                );
            }
            if border.right > 0.0 {
                push_strip(
                    scene,
                    x + width - border.right,
                    y + border.top,
                    border.right,
                    height - border.top - border.bottom,
                    colors.right,
                );
            }
        }
    }

    // 内容盒原点：子片段与文本行的参照
    let content_x = x + fragment.border.left + fragment.padding.left;
    let content_y = y + fragment.border.top + fragment.padding.top;

    // 块级替换元素（`<img>` display:block）：内容盒即图片矩形
    if is_image_node(&page.document, fragment.node) {
        draw_image_at(
            page,
            fragment.node,
            PaintRect {
                x: content_x,
                y: content_y,
                width: width
                    - fragment.border.left
                    - fragment.border.right
                    - fragment.padding.left
                    - fragment.padding.right,
                height: height
                    - fragment.border.top
                    - fragment.border.bottom
                    - fragment.padding.top
                    - fragment.padding.bottom,
            },
            scene,
            pool,
        );
    }

    // 行盒纵向堆叠：LineFragment 内坐标是行内局部系（baseline、图片/原子盒
    // 的 y 相对行顶），绘制前累加前面各行高
    let mut line_offset = 0.0_f32;
    for line in &fragment.lines {
        let line_y = content_y + line_offset;
        let baseline = line_y + line.baseline;
        // 选区高亮：垫在本行文字之下；只画与本行自身字形相交的矩形，
        // 避免嵌套行内原子盒的矩形在父行重复叠加。
        for highlight in highlights {
            let highlight_x = highlight.x;
            let highlight_y = highlight.y;
            if (highlight_y - line_y).abs() > 0.5 {
                continue;
            }
            let overlaps = line.runs.iter().flat_map(|run| &run.glyphs).any(|glyph| {
                let glyph_x = content_x + glyph.x;
                highlight_x < glyph_x + glyph.advance && highlight_x + highlight.width > glyph_x
            });
            if overlaps {
                scene.commands.push(Command::FillRect {
                    rect: PaintRect {
                        x: highlight_x,
                        y: highlight_y,
                        width: highlight.width,
                        height: highlight.height,
                    },
                    color: SELECTION_COLOR,
                });
            }
        }
        for run in &line.runs {
            if run.glyphs.is_empty() {
                continue;
            }
            // 文本装饰带（CSS Text Decor 3）：按 run 自身元素的 computed
            // style（装饰不继承、规范的祖先「传播」未实现——css 层偏差；
            // 跨行内元素边界按 run 分段断线），垫在本 run 文字之下。
            push_text_decoration(page, run, content_x, baseline, scene);
            scene.commands.push(Command::DrawText {
                glyphs: run
                    .glyphs
                    .iter()
                    .map(|glyph| TextGlyph {
                        id: glyph.id,
                        x: content_x + glyph.x,
                        y: glyph.y,
                        font: glyph.font,
                    })
                    .collect(),
                baseline,
                fonts: run.fonts.clone(),
                size: run.style.size,
                color: to_color(run.color),
            });
        }
        // 行内图片：底边对齐基线（布局层已折算 y）
        for image in &line.images {
            draw_image_at(
                page,
                image.node,
                PaintRect {
                    x: content_x + image.x,
                    y: line_y + image.y,
                    width: image.width,
                    height: image.height,
                },
                scene,
                pool,
            );
        }
        // 行内原子盒（inline-block 等）：完整子片段树，x/y 已相对行顶定位，
        // 按子片段递归（原点 = 本行顶）；定位原子盒后画并按 z-index 排序
        //（基础定序，同块级子盒）。
        let (normal_atomic, positioned_atomic) = split_positioned(&line.boxes);
        for atomic in normal_atomic {
            emit(
                page,
                atomic,
                content_x,
                line_y,
                scene,
                pool,
                highlights,
                canvas_source,
            );
        }
        for atomic in positioned_atomic {
            emit(
                page,
                atomic,
                content_x,
                line_y,
                scene,
                pool,
                highlights,
                canvas_source,
            );
        }
        line_offset += line.height;
    }

    // z-index 基础定序（CSS Position 3 §5 简化档）：普通流子片段先按文档
    // 序 emit，定位子片段（position ≠ static）后按 z-index 整数升序 emit
    //（auto 视作 0、相等保持文档序；完整 stacking context 留 K7）。
    let (normal, positioned) = split_positioned(&fragment.children);
    for child in normal {
        emit(
            page,
            child,
            content_x,
            content_y,
            scene,
            pool,
            highlights,
            canvas_source,
        );
    }
    for child in positioned {
        emit(
            page,
            child,
            content_x,
            content_y,
            scene,
            pool,
            highlights,
            canvas_source,
        );
    }
}

/// 把子片段分拣为（普通流，定位）：普通流保持文档序；定位元素（position
/// ≠ static）按 `z-index` 整数升序**稳定**排序（auto 视作 0，相等保持
/// 文档序）。
fn split_positioned(children: &[Fragment]) -> (Vec<&Fragment>, Vec<&Fragment>) {
    let mut normal = Vec::new();
    let mut positioned: Vec<(&Fragment, i32)> = Vec::new();
    for child in children {
        if child.style.position == PositionValue::Static {
            normal.push(child);
        } else {
            positioned.push((child, z_index_key(&child.style)));
        }
    }
    positioned.sort_by_key(|&(_, z)| z);
    let positioned = positioned
        .into_iter()
        .map(|(fragment, _)| fragment)
        .collect();
    (normal, positioned)
}

/// `z-index` 的定序键：`auto` 与 `0` 同序（基础定序档，无 stacking context）。
fn z_index_key(style: &ComputedStyle) -> i32 {
    match style.z_index {
        ZIndexValue::Auto => 0,
        ZIndexValue::Integer(z) => z,
    }
}

/// 为一个文本 run 下发装饰带（`FillRect`，垫在文字下）。
///
/// 装饰属性取 run 自身元素的 computed style：非 solid 不画（偏差）；颜色
/// 透明不画；带顶相对基线按 [`UNDERLINE_OFFSET_EM`] 等经验度量；带 x 范围
/// 为该 run 字形的实际横向覆盖（与命中测试同一口径）。度量为经验值、无
/// 字体 underline 数据（T0 报告 F11，记偏差）。
fn push_text_decoration(
    page: &Page,
    run: &TextRun,
    content_x: f32,
    baseline: f32,
    scene: &mut Scene,
) {
    let Some(style) = page.styles.get(&run.node) else {
        return;
    };
    if style.text_decoration_style != TextDecorationStyleValue::Solid {
        return;
    }
    let offset_em = match style.text_decoration_line {
        TextDecorationLineValue::None => return,
        TextDecorationLineValue::Underline => UNDERLINE_OFFSET_EM,
        TextDecorationLineValue::Overline => -OVERLINE_OFFSET_EM,
        TextDecorationLineValue::LineThrough => -LINE_THROUGH_OFFSET_EM,
    };
    if style.text_decoration_color.a == 0 {
        return;
    }
    let size = run.style.size;
    let thickness = size * DECORATION_THICKNESS_EM;
    if !(thickness.is_finite() && thickness > 0.0) {
        return;
    }
    let Some(first) = run.glyphs.first() else {
        return;
    };
    let Some(last) = run.glyphs.last() else {
        return;
    };
    let start = content_x + first.x;
    let width = (last.x + last.advance) - first.x;
    if !(width.is_finite() && width > 0.0) {
        return;
    }
    scene.commands.push(Command::FillRect {
        rect: PaintRect {
            x: start,
            y: baseline + size * offset_em,
            width,
            height: thickness,
        },
        color: to_color(style.text_decoration_color),
    });
}

/// 把 computed 的四角圆角解析为 px 半径 `[左上, 右上, 右下, 左下]`。
///
/// - `Length(px)` 直取；`Percent` 按边框盒对应轴折算（水平÷宽、垂直÷高）后
///   取两轴**较小**分量——paint 只有圆形角，取较小分量保证角不出盒
///   （50% 在任意盒上退化为药丸半高；椭圆角记偏差）；
/// - `Viewport` 仅在调用方未以视口 recascade 时残留，按 0 降级（直角）；
/// - 负值与非有限按 0 处理（渲染为直角，与 paint 层防御一致）。
fn resolve_corner_radii(style: &ComputedStyle, width: f32, height: f32) -> [f32; 4] {
    fn one(value: BorderRadiusValue, width: f32, height: f32) -> f32 {
        let radius = match value {
            BorderRadiusValue::Length(px) => px,
            BorderRadiusValue::Percent(p) => (p * width).min(p * height),
            BorderRadiusValue::Viewport(..) => 0.0,
        };
        if radius.is_finite() && radius > 0.0 {
            radius
        } else {
            0.0
        }
    }
    let corners = &style.border_radius;
    [
        one(corners.top_left, width, height),
        one(corners.top_right, width, height),
        one(corners.bottom_right, width, height),
        one(corners.bottom_left, width, height),
    ]
}

/// css-images-3 线性渐变线端点：角度自北顺时针（0 = to top），渐变线过盒
/// 中心、长度 `|W·sinα| + |H·cosα|`（CSS Images 3 §3.3.4 图式），两端关于
/// 中心对称。返回 `(start, end)` 场景坐标；`angle_deg` 非有限按 180
///（to bottom）降级。
fn linear_gradient_endpoints(
    angle_deg: f32,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
) -> ([f32; 2], [f32; 2]) {
    let angle = if angle_deg.is_finite() {
        angle_deg
    } else {
        180.0
    };
    let center_x = x + width / 2.0;
    let center_y = y + height / 2.0;
    // 渐变方向单位向量：0° 向上（y 轴向下为正的屏幕坐标系）。四正向
    // （rem_euclid 后恰为 0/90/180/270）走精确轴向量与精确线长——f32
    // 三角函数在 90° 带 ~4e-6 噪声，最常用方向的渐变线必须严格水平
    // /垂直，端点才确定。
    let (dx, dy, length) = match angle.rem_euclid(360.0) {
        0.0 => (0.0, -1.0, height.abs()),
        90.0 => (1.0, 0.0, width.abs()),
        180.0 => (0.0, 1.0, height.abs()),
        270.0 => (-1.0, 0.0, width.abs()),
        _ => {
            let alpha = angle.to_radians();
            let (sin, cos) = alpha.sin_cos();
            (sin, -cos, (width * sin).abs() + (height * cos).abs())
        }
    };
    (
        [center_x - dx * length / 2.0, center_y - dy * length / 2.0],
        [center_x + dx * length / 2.0, center_y + dy * length / 2.0],
    )
}

/// css 色标 → paint 色标（offset 透传，颜色经非预乘 RGBA 转换）。
fn gradient_stops(stops: &[GradientColorStop]) -> Vec<GradientStop> {
    stops
        .iter()
        .map(|stop| GradientStop {
            offset: stop.position,
            color: to_color(stop.color),
        })
        .collect()
}

/// 元素是否携带可见背景（颜色不透明度 > 0 或渐变）——§14.2 传播判定用。
fn has_background(style: &ComputedStyle) -> bool {
    style.background_color.a > 0
        || matches!(
            style.background_image,
            BackgroundImageValue::LinearGradient(_)
        )
}

/// 按 HTML 命名空间的标签名找文档中首个元素（html5ever 强制存在 html/body）。
fn find_html_element(page: &Page, name: &str) -> Option<NodeId> {
    use nexty_dom::{Namespace, NodeKind};
    fn walk(page: &Page, node: NodeId, name: &str) -> Option<NodeId> {
        if let Some(NodeKind::Element(data)) = page.document.node(node)
            && data.namespace == Namespace::Html
            && data.name == name
        {
            return Some(node);
        }
        page.document
            .children(node)
            .find_map(|child| walk(page, child, name))
    }
    walk(page, page.document.root(), name)
}

/// CSS 2.2 §14.2 的画布背景传播源：html 携带背景用 html，否则 body，
/// 都无返回 `None`（画布回落白）。
#[must_use]
pub fn canvas_background_source(page: &Page) -> Option<NodeId> {
    if let Some(html) = find_html_element(page, "html")
        && page.styles.get(&html).is_some_and(has_background)
    {
        return Some(html);
    }
    if let Some(body) = find_html_element(page, "body")
        && page.styles.get(&body).is_some_and(has_background)
    {
        return Some(body);
    }
    None
}

/// 画布底色指令（CSS 2.2 §14.2 背景传播），供呈现层插到场景**底部**。
///
/// - html/body 均无背景 → 一条白色 `FillRect`（画布必须不透明：透明像素
///   直接 blit 给 surface 在部分后端表现为黑屏/花屏）；
/// - 传播源只有纯色 → 一条该色 `FillRect`；
/// - 传播源带渐变 → 先垫一条底色（传播色不透明时用它，否则白——保证
///   渐变透明色标不露出未初始化画布），再一条铺满视口的 `FillGradient`
///   （渐变线按**视口矩形**解析：传播到画布的背景按画布尺寸取值）。
///
/// 被传播元素的自身背景由 emit 抑制（搬移语义，见 [`emit`]）。
#[must_use]
pub fn canvas_background_commands(
    page: &Page,
    viewport_width: f32,
    viewport_height: f32,
) -> Vec<Command> {
    let rect = PaintRect {
        x: 0.0,
        y: 0.0,
        width: viewport_width,
        height: viewport_height,
    };
    let white = Color::opaque(0xff, 0xff, 0xff);
    let Some(source) = canvas_background_source(page) else {
        return vec![Command::FillRect { rect, color: white }];
    };
    // 传播源必有 computed style（级联覆盖全部元素）
    let style = &page.styles[&source];
    let base = if style.background_color.a > 0 {
        to_color(style.background_color)
    } else {
        white
    };
    let mut commands = vec![Command::FillRect { rect, color: base }];
    if let BackgroundImageValue::LinearGradient(gradient) = &style.background_image {
        let (start, end) =
            linear_gradient_endpoints(gradient.angle, 0.0, 0.0, viewport_width, viewport_height);
        let stops = gradient_stops(&gradient.stops);
        if !stops.is_empty() {
            commands.push(Command::FillGradient {
                rect,
                radii: [0.0; 4],
                start,
                end,
                stops,
            });
        }
    }
    commands
}

/// `node` 是否为 HTML 命名空间的 `<img>`（块级替换路径的判定）。
fn is_image_node(document: &Document, node: NodeId) -> bool {
    use nexty_dom::{Namespace, NodeKind};
    matches!(
        document.node(node),
        Some(NodeKind::Element(data))
            if data.namespace == Namespace::Html && data.name == "img"
    )
}

/// 命中的内容类型（决定悬停光标等交互反馈）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitKind {
    /// 命中的是盒本身（未深入行内内容或块级子盒）。
    Fragment,
    /// 命中了行内文本 run。
    TextRun,
    /// 命中了图片（行内替换元素）。
    Image,
}

/// 命中结果：最深片段节点 + 命中类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    /// 命中的节点。
    pub node: NodeId,
    /// 命中类型。
    pub kind: HitKind,
}

/// 悬停光标语义：链接 → 手型，文本 → I 形，其余 → 默认箭头。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoverKind {
    /// 默认箭头。
    Default,
    /// I 形（文本）。
    Text,
    /// 手型（链接）。
    Pointer,
}

/// 计算页面点 `(x, y)` 的悬停光标语义（坐标与 [`hit_test_ex`] 同系）。
///
/// 链接触发优先于文本：命中节点的祖先链上存在带 `href` 的 `<a>` 即为链接。
#[must_use]
pub fn hover_kind(page: &Page, root: &Fragment, x: f32, y: f32) -> HoverKind {
    let Some(hit) = hit_test_ex(root, x, y) else {
        return HoverKind::Default;
    };
    if link_target(page, hit.node).is_some() {
        return HoverKind::Pointer;
    }
    match hit.kind {
        HitKind::TextRun => HoverKind::Text,
        HitKind::Fragment | HitKind::Image => HoverKind::Default,
    }
}

/// 命中测试：返回视口点 `(x, y)` 命中的最深片段节点与命中类型。
///
/// 坐标以 `root` 的边框盒原点为参照（与 [`build_scene_at`] 的原点参数
/// 一致：窗口坐标减去原点即得）。命中顺序镜像绘制顺序：行内内容
/// （文本 run / 图片 / 行内原子盒）优先于块级子盒；点在盒内但未命中
/// 更深层时返回本盒节点。
#[must_use]
pub fn hit_test_ex(root: &Fragment, x: f32, y: f32) -> Option<Hit> {
    hit_fragment(root, 0.0, 0.0, x, y)
}

/// 命中测试：只取命中的节点（[`hit_test_ex`] 的薄封装）。
#[must_use]
pub fn hit_test(root: &Fragment, x: f32, y: f32) -> Option<NodeId> {
    hit_test_ex(root, x, y).map(|hit| hit.node)
}

/// `hit_test_ex` 的递归体：`(ox, oy)` 为父链累积的本片段边框盒原点。
fn hit_fragment(fragment: &Fragment, ox: f32, oy: f32, x: f32, y: f32) -> Option<Hit> {
    let bx = ox + fragment.border_box.x;
    let by = oy + fragment.border_box.y;
    if x < bx || x > bx + fragment.border_box.width || y < by || y > by + fragment.border_box.height
    {
        return None;
    }
    let content_x = bx + fragment.border.left + fragment.padding.left;
    let content_y = by + fragment.border.top + fragment.padding.top;

    // 行内内容按行堆叠（与 emit 的行偏移累加一致）
    let mut line_offset = 0.0_f32;
    for line in &fragment.lines {
        let line_y = content_y + line_offset;
        if y >= line_y && y <= line_y + line.height {
            // 文本 run：字形 x 范围（glyph.x 已相对本片段内容盒）
            for run in &line.runs {
                let Some(first) = run.glyphs.first() else {
                    continue;
                };
                let Some(last) = run.glyphs.last() else {
                    continue;
                };
                let run_start = content_x + first.x;
                let run_end = content_x + last.x + last.advance;
                if x >= run_start && x <= run_end {
                    return Some(Hit {
                        node: run.node,
                        kind: HitKind::TextRun,
                    });
                }
            }
            for image in &line.images {
                let top = line_y + image.y;
                if x >= content_x + image.x
                    && x <= content_x + image.x + image.width
                    && y >= top
                    && y <= top + image.height
                {
                    return Some(Hit {
                        node: image.node,
                        kind: HitKind::Image,
                    });
                }
            }
            for atomic in &line.boxes {
                if let Some(hit) = hit_fragment(atomic, content_x, line_y, x, y) {
                    return Some(hit);
                }
            }
        }
        line_offset += line.height;
    }

    for child in &fragment.children {
        if let Some(hit) = hit_fragment(child, content_x, content_y, x, y) {
            return Some(hit);
        }
    }
    Some(Hit {
        node: fragment.node,
        kind: HitKind::Fragment,
    })
}

/// `node` 向上找最近的 `<a href>` 祖先（含自身），返回 href 原文。
///
/// 返回的是文档里的原始属性值；相对地址由调用方以文档 URL 为 base
/// 归一（`nexty_network::resolve`）。无 `<a>` 祖先或 href 为空返回 `None`。
pub fn link_target(page: &Page, node: NodeId) -> Option<String> {
    use nexty_dom::{Namespace, NodeKind};
    let mut current = Some(node);
    while let Some(id) = current {
        if let Some(NodeKind::Element(data)) = page.document.node(id)
            && data.namespace == Namespace::Html
            && data.name == "a"
        {
            let href = data
                .attributes
                .iter()
                .find(|attr| attr.name == "href")
                .map(|attr| attr.value.trim().to_owned())
                .unwrap_or_default();
            if href.is_empty() {
                return None;
            }
            return Some(href);
        }
        current = page.document.parent(id);
    }
    None
}

/// 提取文档 `<title>` 的文本（HTML 命名空间的 title 元素，首个非空者）。
///
/// 供窗口标题显示；缺失或空白返回 `None`。SVG 的 `<title>` 不在此列。
pub fn document_title(document: &Document) -> Option<String> {
    use nexty_dom::{Namespace, NodeKind};
    fn walk(document: &Document, node: NodeId) -> Option<String> {
        // Document/文本等非元素节点没有 title，但子树仍需下钻
        if let Some(NodeKind::Element(data)) = document.node(node)
            && data.namespace == Namespace::Html
            && data.name == "title"
        {
            let mut text = String::new();
            for child in document.children(node) {
                if let Some(NodeKind::Text(content)) = document.node(child) {
                    text.push_str(content);
                }
            }
            let trimmed = text.trim();
            return (!trimmed.is_empty()).then(|| trimmed.to_owned());
        }
        document
            .children(node)
            .find_map(|child| walk(document, child))
    }
    walk(document, document.root())
}

/// 在指定矩形下发一张图片：像素进池（按节点去重），指令按下标引用。
fn draw_image_at(
    page: &Page,
    node: NodeId,
    rect: PaintRect,
    scene: &mut Scene,
    pool: &mut HashMap<NodeId, usize>,
) {
    // 未解码成功的图片没有像素（抓取失败降级）：矩形跳过，不影响其余内容
    let Some(source) = page.images.get(&node) else {
        return;
    };
    let index = match pool.get(&node) {
        Some(index) => *index,
        None => {
            let index = scene.images.len();
            // Arc 引用计数共享像素：同一节点多图与跨帧重建均零拷贝（M1）
            scene.images.push(Arc::clone(source));
            pool.insert(node, index);
            index
        }
    };
    scene
        .commands
        .push(Command::DrawImage { rect, image: index });
}

fn push_strip(scene: &mut Scene, x: f32, y: f32, width: f32, height: f32, color: Rgba) {
    if color.a > 0 {
        scene.commands.push(Command::FillRect {
            rect: PaintRect {
                x,
                y,
                width,
                height,
            },
            color: to_color(color),
        });
    }
}

fn to_color(rgba: Rgba) -> Color {
    Color {
        r: rgba.r,
        g: rgba.g,
        b: rgba.b,
        a: rgba.a,
    }
}

/// 视口尺寸别名（paint 的 `Size`）。
pub type Viewport = Size;

/// resize 合帧器（M6）：连发的 `Resized` 事件合并到下一次重绘。
///
/// winit 拖动窗口边缘时 `Resized` 连发，逐事件全管线重排会把管线跑成
/// 每事件一次；合帧后事件只置脏标志，重排在下一次 `RedrawRequested`
/// 里至多执行一次（app 与基准共用本状态机，保证「每帧一次」语义同源）。
#[derive(Debug, Default)]
pub struct ResizeCoalescer {
    dirty: bool,
}

impl ResizeCoalescer {
    /// 一个 `Resized` 事件到达（只置脏，不做任何重排工作）。
    pub fn on_resized(&mut self) {
        self.dirty = true;
    }

    /// 取走脏标志：返回真时调用方执行一次重排（每帧至多一次）。
    pub fn take(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
}

/// 页面坐标系显示列表缓存（M6）。
///
/// 场景按（页面代次、视口宽、高亮集合）缓存为 **页面坐标系**版本
/// （origin 全零）：滚动、滚动条拖拽只是光栅前的一次
/// [`Scene::translated`] origin 平移，不再重建显示列表。导航与 resize
/// 推进页面代次（布局结果变化）、选区变化改变高亮集合——两者都使缓存
/// 失效重建。
#[derive(Default)]
pub struct SceneCache {
    cached: Option<CachedScene>,
    /// 累计 emit（全片段树重建）次数；基准与测试用它断言滚动路径零 emit。
    emits: usize,
    /// 上一次 [`SceneCache::scene_for`] 是否命中。
    last_hit: bool,
}

struct CachedScene {
    generation: u64,
    viewport_width: f32,
    highlights: Vec<LayoutRect>,
    scene: Scene,
}

impl SceneCache {
    /// 空缓存。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 取页面坐标系场景：键（代次、视口宽、高亮）命中直接复用，未命中
    /// 走一次全片段树 emit 并入库。
    pub fn scene_for(
        &mut self,
        generation: u64,
        page: &Page,
        root: &Fragment,
        viewport_width: f32,
        highlights: &[LayoutRect],
    ) -> &Scene {
        let hit = self.cached.as_ref().is_some_and(|cached| {
            cached.generation == generation
                && cached.viewport_width == viewport_width
                && cached.highlights == highlights
        });
        self.last_hit = hit;
        if !hit {
            self.emits += 1;
            let scene = build_scene_at_with_highlights(page, root, 0.0, 0.0, highlights);
            self.cached = Some(CachedScene {
                generation,
                viewport_width,
                highlights: highlights.to_vec(),
                scene,
            });
        }
        &self.cached.as_ref().expect("cache populated on miss").scene
    }

    /// 上一次 [`SceneCache::scene_for`] 是否命中缓存（未走 emit）。
    #[must_use]
    pub fn last_was_hit(&self) -> bool {
        self.last_hit
    }

    /// 累计 emit 次数。
    #[must_use]
    pub fn emits(&self) -> usize {
        self.emits
    }

    /// emit 计数清零（基准分场景统计）。
    pub fn reset_emits(&mut self) {
        self.emits = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexty_css::SizeValue;
    use nexty_dom::NodeKind;

    /// recascade 按视口折算视口单位：50vw @800×600 → 400px、10vh → 60px。
    #[test]
    fn recascade_resolves_viewport_units() {
        let document = nexty_html::parse_document("<div></div>", ParseOptions::default());
        let sheet = Stylesheet::parse("div { width: 50vw; height: 10vh }");
        let mut page = build_page(document, vec![sheet], HashMap::new());

        // 构建期无视口：视口单位保持未折算
        let div = *page
            .styles
            .keys()
            .find(|&&id| {
                matches!(
                    page.document.node(id),
                    Some(NodeKind::Element(data)) if data.name == "div"
                )
            })
            .expect("div element");
        assert!(matches!(page.styles[&div].width, SizeValue::Viewport(..)));

        recascade(
            &mut page,
            MediaViewport {
                width: 800.0,
                height: 600.0,
            },
        );
        assert_eq!(page.styles[&div].width, SizeValue::Length(400.0), "50vw");
        assert_eq!(page.styles[&div].height, SizeValue::Length(60.0), "10vh");
    }

    /// 布局一段文本页供缓存测试用。
    fn text_page() -> (Page, Fragment) {
        let page = load_page(
            "<body><p>hello 缓存 scene</p><p>second line of text</p></body>",
            "",
        );
        let shaper = nexty_text::ParleyTextShaper::new();
        let root = layout_page(&page, &shaper, 400.0).expect("root");
        (page, root)
    }

    /// 同键重复请求复用缓存（零 emit），键变化（代次/视口宽/高亮）失效重建。
    #[test]
    fn scene_cache_reuses_on_same_key_and_invalidates_on_key_change() {
        let (page, root) = text_page();
        let mut cache = SceneCache::new();

        let _ = cache.scene_for(7, &page, &root, 400.0, &[]);
        assert!(!cache.last_was_hit(), "首次构建必 miss");
        assert_eq!(cache.emits(), 1);

        let _ = cache.scene_for(7, &page, &root, 400.0, &[]);
        assert!(cache.last_was_hit(), "同键命中");
        assert_eq!(cache.emits(), 1, "命中不 emit");

        // 页面代次推进（导航 / resize 重排）→ 失效
        let _ = cache.scene_for(8, &page, &root, 400.0, &[]);
        assert!(!cache.last_was_hit());
        assert_eq!(cache.emits(), 2);

        // 视口宽变化 → 失效
        let _ = cache.scene_for(8, &page, &root, 500.0, &[]);
        assert!(!cache.last_was_hit());

        // 高亮集合变化（选区）→ 失效
        let highlights = [LayoutRect {
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 20.0,
        }];
        let _ = cache.scene_for(8, &page, &root, 500.0, &highlights);
        assert!(!cache.last_was_hit());
        let _ = cache.scene_for(8, &page, &root, 500.0, &highlights);
        assert!(cache.last_was_hit(), "相同高亮集合命中");
        assert_eq!(cache.emits(), 4);
    }

    /// 缓存场景是页面坐标系（origin 全零）：平移到视口后几何与直接以
    /// 该 origin 构建的场景逐字节一致（M6 的正确性判据）。
    #[test]
    fn cached_page_scene_translated_matches_direct_build() {
        let (page, root) = text_page();
        let mut cache = SceneCache::new();
        let page_scene = cache.scene_for(1, &page, &root, 400.0, &[]);
        let origin_y = 48.0 - 120.0; // BAR_HEIGHT - scroll 的样例
        let translated = page_scene.translated(0.0, origin_y);
        let direct = build_scene_at_with_highlights(&page, &root, 0.0, origin_y, &[]);
        assert_eq!(format!("{translated:?}"), format!("{direct:?}"));
    }

    /// 合帧器：连发事件折叠为一次 take。
    #[test]
    fn resize_coalescer_collapses_bursts() {
        let mut coalescer = ResizeCoalescer::default();
        assert!(!coalescer.take(), "初始无脏标志");
        coalescer.on_resized();
        coalescer.on_resized();
        coalescer.on_resized();
        assert!(coalescer.take(), "三次事件折叠为一次");
        assert!(!coalescer.take(), "take 后标志复位");
        coalescer.on_resized();
        assert!(coalescer.take());
    }

    // ---- K2 T3：圆角 / 渐变 / 装饰带 / z-index / 背景传播 ----

    use nexty_css::{Corners, ViewportUnit};

    /// 渐变线端点：css-images-3 公式 `|W·sinα| + |H·cosα|`，四正向端点
    /// 方向、对角 45° 线长与非有限角度降级。
    #[test]
    fn linear_gradient_endpoints_follow_css_images_formula() {
        // 0°（to top）：线长 = H，起点在下、终点在上
        let (start, end) = linear_gradient_endpoints(0.0, 10.0, 20.0, 100.0, 50.0);
        assert_eq!(start, [60.0, 70.0]);
        assert_eq!(end, [60.0, 20.0]);
        // 90°（to right）：线长 = W，起点在左、终点在右
        let (start, end) = linear_gradient_endpoints(90.0, 10.0, 20.0, 100.0, 50.0);
        assert_eq!(start, [10.0, 45.0]);
        assert_eq!(end, [110.0, 45.0]);
        // 180°（to bottom，缺省角）：起点在上、终点在下
        let (start, end) = linear_gradient_endpoints(180.0, 10.0, 20.0, 100.0, 50.0);
        assert_eq!(start, [60.0, 20.0]);
        assert_eq!(end, [60.0, 70.0]);
        // 270°（to left）：起点在右、终点在左
        let (start, end) = linear_gradient_endpoints(270.0, 10.0, 20.0, 100.0, 50.0);
        assert_eq!(start, [110.0, 45.0]);
        assert_eq!(end, [10.0, 45.0]);
        // 45°：L = W·sin45 + H·cos45；方形盒端点落在对角上
        let (start, end) = linear_gradient_endpoints(45.0, 0.0, 0.0, 100.0, 100.0);
        assert!((start[0] - 0.0).abs() < 1e-4 && (start[1] - 100.0).abs() < 1e-4);
        assert!((end[0] - 100.0).abs() < 1e-4 && (end[1] - 0.0).abs() < 1e-4);
        // 非有限角度降级 180（to bottom）
        let (start, end) = linear_gradient_endpoints(f32::NAN, 0.0, 0.0, 100.0, 50.0);
        assert_eq!(start, [50.0, 0.0]);
        assert_eq!(end, [50.0, 50.0]);
    }

    /// 圆角解析：Length 直取、Percent 两轴取小、Viewport 残留降级 0、
    /// 负值与非有限钳 0。
    #[test]
    fn corner_radii_resolve_length_percent_and_viewport_fallback() {
        let mut style = ComputedStyle::initial();
        style.border_radius = Corners::splat(BorderRadiusValue::Length(10.0));
        assert_eq!(resolve_corner_radii(&style, 800.0, 200.0), [10.0; 4]);

        // 50% @800×200：水平 400、垂直 100 → 取较小分量（药丸半高）
        style.border_radius = Corners::splat(BorderRadiusValue::Percent(0.5));
        assert_eq!(resolve_corner_radii(&style, 800.0, 200.0), [100.0; 4]);

        // 视口单位残留（未 recascade）→ 0 降级直角
        style.border_radius = Corners::splat(BorderRadiusValue::Viewport(2.0, ViewportUnit::Vw));
        assert_eq!(resolve_corner_radii(&style, 800.0, 200.0), [0.0; 4]);

        style.border_radius = Corners::splat(BorderRadiusValue::Length(f32::NAN));
        assert_eq!(resolve_corner_radii(&style, 800.0, 200.0), [0.0; 4]);
        style.border_radius = Corners::splat(BorderRadiusValue::Length(-5.0));
        assert_eq!(resolve_corner_radii(&style, 800.0, 200.0), [0.0; 4]);
    }

    /// 布局一页并构建场景的测试辅助。
    fn scene_of(html: &str) -> Scene {
        let page = load_page(html, "");
        let shaper = nexty_text::ParleyTextShaper::new();
        let root = layout_page(&page, &shaper, 400.0).expect("root");
        build_scene(&page, &root)
    }

    /// 渐变背景发 FillGradient：to right 端点从左到右、色标映射正确、
    /// 渐变线过盒中心。
    #[test]
    fn gradient_background_emits_fill_gradient_with_stops() {
        let scene = scene_of(
            "<body><div style=\"background-image: linear-gradient(to right, red, blue); height: 40px\"></div></body>",
        );
        let gradients: Vec<_> = scene
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::FillGradient {
                    rect,
                    start,
                    end,
                    stops,
                    ..
                } => Some((*rect, *start, *end, stops.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(gradients.len(), 1, "恰一条 FillGradient");
        let (rect, start, end, stops) = &gradients[0];
        assert!(start[0] < end[0], "to right 起点在左终点在右");
        assert_eq!(start[1], end[1], "to right 渐变线水平");
        assert_eq!(stops.len(), 2);
        assert_eq!(stops[0].offset, 0.0);
        assert_eq!(stops[0].color, Color::opaque(255, 0, 0));
        assert_eq!(stops[1].offset, 1.0);
        assert_eq!(stops[1].color, Color::opaque(0, 0, 255));
        let center_x = rect.x + rect.width / 2.0;
        assert_eq!((start[0] + end[0]) / 2.0, center_x, "渐变线过盒中心");
    }

    /// 渐变 × 圆角单指令：radii 随 border_radius，不另发圆角纯色背景。
    #[test]
    fn gradient_with_radius_is_single_command() {
        let scene = scene_of(
            "<body><div style=\"background-image: linear-gradient(45deg, red, blue); border-radius: 12px; height: 40px\"></div></body>",
        );
        assert!(
            !scene
                .commands
                .iter()
                .any(|command| matches!(command, Command::FillRoundedRect { .. })),
            "背景色透明时不发纯色圆角指令"
        );
        let gradients: Vec<_> = scene
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::FillGradient { radii, .. } => Some(*radii),
                _ => None,
            })
            .collect();
        assert_eq!(gradients, vec![[12.0; 4]], "渐变 radii 随 border_radius");
    }

    /// 有圆角背景走 FillRoundedRect；全 0 半径保持 FillRect 旧路径。
    #[test]
    fn rounded_background_uses_fill_rounded_rect_zero_radius_keeps_fill_rect() {
        let scene = scene_of(
            "<body>\
             <div style=\"background-color: red; border-radius: 10px; height: 30px\"></div>\
             <div style=\"background-color: blue; height: 30px\"></div>\
             </body>",
        );
        let rounded: Vec<_> = scene
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::FillRoundedRect { radii, color, .. } => Some((*radii, *color)),
                _ => None,
            })
            .collect();
        assert_eq!(rounded, vec![([10.0; 4], Color::opaque(255, 0, 0))]);
        assert!(
            scene.commands.iter().any(|command| matches!(
                command,
                Command::FillRect { color, .. } if *color == Color::opaque(0, 0, 255)
            )),
            "无圆角 div 保持 FillRect 旧路径"
        );
    }

    /// 四边等宽 + 圆角 → 一条圆角描边；宽不等维持四条填充带（直角降级）。
    #[test]
    fn uniform_borders_use_stroke_rounded_rect_unequal_keep_strips() {
        let scene = scene_of(
            "<body>\
             <div style=\"border: 4px solid black; border-radius: 8px; height: 30px\"></div>\
             <div style=\"border-style: solid; border-width: 2px 4px; border-radius: 8px; height: 30px\"></div>\
             </body>",
        );
        let strokes: Vec<_> = scene
            .commands
            .iter()
            .filter_map(|command| match command {
                Command::StrokeRoundedRect { radii, width, .. } => Some((*radii, *width)),
                _ => None,
            })
            .collect();
        assert_eq!(strokes, vec![([8.0; 4], 4.0)], "等宽圆角边框一条描边");
        let black_strips = scene
            .commands
            .iter()
            .filter(|command| {
                matches!(
                    command,
                    Command::FillRect { color, .. } if *color == Color::opaque(0, 0, 0)
                )
            })
            .count();
        assert_eq!(black_strips, 4, "宽不等时四条直角填充带");
    }

    /// 场景中某色首个 `FillRect` 的下标。
    fn fill_index(scene: &Scene, color: Color) -> Option<usize> {
        scene.commands.iter().position(
            |command| matches!(command, Command::FillRect { color: c, .. } if *c == color),
        )
    }

    /// z-index 基础定序：定位元素画在普通流之后、定位兄弟按整数升序。
    #[test]
    fn z_index_orders_positioned_after_static_sorted_by_integer() {
        let scene = scene_of(
            "<body style=\"margin: 0\">\
             <div style=\"background-color: red; height: 20px\"></div>\
             <div style=\"position: absolute; z-index: 2; background-color: green; height: 20px\"></div>\
             <div style=\"position: absolute; z-index: 1; background-color: blue; height: 20px\"></div>\
             <div style=\"position: absolute; z-index: -1; background-color: yellow; height: 20px\"></div>\
             <div style=\"background-color: white; height: 20px\"></div>\
             </body>",
        );
        let red = fill_index(&scene, Color::opaque(255, 0, 0)).expect("普通流红");
        let white = fill_index(&scene, Color::opaque(255, 255, 255)).expect("普通流白");
        let green = fill_index(&scene, Color::opaque(0, 128, 0)).expect("z=2 绿");
        let blue = fill_index(&scene, Color::opaque(0, 0, 255)).expect("z=1 蓝");
        let yellow = fill_index(&scene, Color::opaque(255, 255, 0)).expect("z=-1 黄");
        assert!(red < yellow && white < yellow, "定位元素画在普通流之后");
        assert!(yellow < blue && blue < green, "定位兄弟按 z-index 升序");
    }

    /// 选区高亮插入次序回归：高亮在背景之上、文字之下。
    #[test]
    fn selection_highlight_stays_between_background_and_text() {
        let page = load_page(
            "<body style=\"margin: 0\"><div style=\"background-color: rgb(0, 0, 255); min-height: 100px\"><p style=\"margin: 0\">hello</p></div></body>",
            "",
        );
        let shaper = nexty_text::ParleyTextShaper::new();
        let root = layout_page(&page, &shaper, 400.0).expect("root");
        let highlights = [LayoutRect {
            x: 0.0,
            y: 0.0,
            width: 400.0,
            height: 300.0,
        }];
        let scene = build_scene_at_with_highlights(&page, &root, 0.0, 0.0, &highlights);
        let background = fill_index(&scene, Color::opaque(0, 0, 255)).expect("div 背景");
        let highlight = scene
            .commands
            .iter()
            .position(|command| matches!(command, Command::FillRect { color, .. } if *color == SELECTION_COLOR))
            .expect("选区高亮");
        let text = scene
            .commands
            .iter()
            .position(|command| matches!(command, Command::DrawText { .. }))
            .expect("文本命令");
        assert!(background < highlight, "高亮在背景之上");
        assert!(highlight < text, "高亮在文字之下");
    }

    /// 画布背景传播源：html 优先、body 回落、都无回落白。
    #[test]
    fn canvas_background_prefers_html_then_body_falls_back_to_white() {
        let page = load_page(
            "<html style=\"background-color: blue\"><body style=\"background-color: green\"></body></html>",
            "",
        );
        let canvas = canvas_background_commands(&page, 400.0, 300.0);
        assert!(
            matches!(
                &canvas[..],
                [Command::FillRect { color, .. }] if *color == Color::opaque(0, 0, 255)
            ),
            "html 背景优先传播"
        );

        let page = load_page("<body style=\"background-color: green\"></body>", "");
        let canvas = canvas_background_commands(&page, 400.0, 300.0);
        assert!(
            matches!(
                &canvas[..],
                [Command::FillRect { color, .. }] if *color == Color::opaque(0, 128, 0)
            ),
            "html 无背景时 body 回落"
        );

        let page = load_page("<body></body>", "");
        let canvas = canvas_background_commands(&page, 400.0, 300.0);
        assert!(
            matches!(
                &canvas[..],
                [Command::FillRect { color, .. }] if *color == Color::opaque(255, 255, 255)
            ),
            "都无背景回落白"
        );
    }

    /// 渐变背景传播：垫底色 + 铺满视口的 FillGradient（渐变线按视口矩形）。
    #[test]
    fn canvas_gradient_propagates_as_viewport_fill_gradient() {
        let page = load_page(
            "<body style=\"background-image: linear-gradient(to right, red, blue)\"></body>",
            "",
        );
        let canvas = canvas_background_commands(&page, 400.0, 300.0);
        assert_eq!(canvas.len(), 2, "垫底色 + 渐变两条指令");
        assert!(
            matches!(
                &canvas[0],
                Command::FillRect { color, .. } if *color == Color::opaque(255, 255, 255)
            ),
            "传播色透明时垫白"
        );
        let Command::FillGradient {
            rect,
            start,
            end,
            stops,
            ..
        } = &canvas[1]
        else {
            panic!("第二条应是渐变");
        };
        assert_eq!(rect.width, 400.0);
        assert_eq!(rect.height, 300.0);
        assert!(start[0] < end[0]);
        assert_eq!(stops.len(), 2);
    }

    /// 传播是搬移：body 背景被传播后场景中不再自绘；html 携带背景时
    /// body 背景不被传播、仍自绘。
    #[test]
    fn propagated_background_is_suppressed_on_fragment() {
        let page = load_page(
            "<body style=\"background-color: green\"><p style=\"margin: 0\">x</p></body>",
            "",
        );
        let shaper = nexty_text::ParleyTextShaper::new();
        let root = layout_page(&page, &shaper, 400.0).expect("root");
        let scene = build_scene(&page, &root);
        assert!(
            scene.commands.iter().all(|command| !matches!(
                command,
                Command::FillRect { color, .. } if *color == Color::opaque(0, 128, 0)
            )),
            "被传播的 body 背景不再自绘"
        );

        let page = load_page(
            "<html style=\"background-color: blue\"><body style=\"background-color: green\"><p style=\"margin: 0\">x</p></body></html>",
            "",
        );
        let root = layout_page(&page, &shaper, 400.0).expect("root");
        let scene = build_scene(&page, &root);
        assert!(
            scene.commands.iter().any(|command| matches!(
                command,
                Command::FillRect { color, .. } if *color == Color::opaque(0, 128, 0)
            )),
            "body 背景未被传播时仍自绘"
        );
        assert!(
            scene.commands.iter().all(|command| !matches!(
                command,
                Command::FillRect { color, .. } if *color == Color::opaque(0, 0, 255)
            )),
            "html 背景被传播后不再自绘"
        );
    }

    /// 场景中某色首个非黑装饰带（带高 = 0.06em）的下标与几何。
    fn decoration_band(scene: &Scene) -> Option<PaintRect> {
        scene.commands.iter().find_map(|command| match command {
            Command::FillRect { rect, color }
                if *color == Color::opaque(0, 0, 0) && (rect.height - 0.96).abs() < 1e-3 =>
            {
                Some(*rect)
            }
            _ => None,
        })
    }

    /// 装饰带几何：厚度 0.06em、underline 带顶 = 基线 + 0.08em、x 覆盖
    /// run 字形横向范围；overline / line-through 按经验偏移。
    #[test]
    fn decoration_band_geometry_follows_run_baseline() {
        let scene = scene_of(
            "<body style=\"margin: 0\"><p style=\"margin: 0; text-decoration: underline\">hello</p></body>",
        );
        let band = decoration_band(&scene).expect("underline 装饰带");
        assert!(
            (band.height - 16.0 * 0.06).abs() < 1e-3,
            "厚度 0.06em @16px"
        );
        let baseline = scene
            .commands
            .iter()
            .find_map(|command| match command {
                Command::DrawText { baseline, .. } => Some(*baseline),
                _ => None,
            })
            .expect("文本命令");
        assert!(
            (band.y - (baseline + 16.0 * 0.08)).abs() < 1e-3,
            "带顶 = 基线 + 0.08em"
        );
        // 装饰带垫在文字下：带下标早于本 run 的 DrawText
        let band_index = scene
            .commands
            .iter()
            .position(|command| matches!(command, Command::FillRect { rect, .. } if *rect == band))
            .expect("带下标");
        let text_index = scene
            .commands
            .iter()
            .position(|command| matches!(command, Command::DrawText { .. }))
            .expect("文本下标");
        assert!(band_index < text_index, "装饰带垫在文字下");

        for (decoration, offset_em) in [("overline", -0.88), ("line-through", -0.30)] {
            let scene = scene_of(&format!(
                "<body style=\"margin: 0\"><p style=\"margin: 0; text-decoration: {decoration}\">hello</p></body>"
            ));
            let band = decoration_band(&scene).unwrap_or_else(|| panic!("{decoration} 装饰带"));
            let baseline = scene
                .commands
                .iter()
                .find_map(|command| match command {
                    Command::DrawText { baseline, .. } => Some(*baseline),
                    _ => None,
                })
                .expect("文本命令");
            assert!(
                (band.y - (baseline + 16.0 * offset_em)).abs() < 1e-3,
                "{decoration} 带顶 = 基线 {offset_em}em"
            );
        }
    }

    /// 非 solid（wavy）与透明装饰色不画（渲染仅 solid——偏差）。
    #[test]
    fn decoration_skipped_for_non_solid_and_transparent() {
        let scene = scene_of(
            "<body style=\"margin: 0\"><p style=\"margin: 0; text-decoration: underline wavy\">hello</p></body>",
        );
        assert!(decoration_band(&scene).is_none(), "wavy 不按 solid 画");
        let scene = scene_of(
            "<body style=\"margin: 0\"><p style=\"margin: 0; text-decoration: underline; text-decoration-color: transparent\">hello</p></body>",
        );
        assert!(decoration_band(&scene).is_none(), "透明装饰色不画");
    }

    /// M6 回归（新指令）：缓存场景经 `translated` 与直接以该 origin 构建
    /// 的场景逐字节一致（渐变端点 / 圆角 / 装饰带全部随平移）。
    #[test]
    fn translated_matches_direct_build_with_new_commands() {
        let page = load_page(
            "<body style=\"margin: 0\">\
             <div style=\"background-image: linear-gradient(45deg, red, blue); border-radius: 12px; border: 3px solid black; height: 40px\">\
             <p style=\"margin: 0; text-decoration: underline\">hello</p>\
             </div></body>",
            "",
        );
        let shaper = nexty_text::ParleyTextShaper::new();
        let root = layout_page(&page, &shaper, 400.0).expect("root");
        let mut cache = SceneCache::new();
        let page_scene = cache.scene_for(1, &page, &root, 400.0, &[]);
        let translated = page_scene.translated(0.0, 48.0);
        let direct = build_scene_at_with_highlights(&page, &root, 0.0, 48.0, &[]);
        assert_eq!(format!("{translated:?}"), format!("{direct:?}"));
    }
}
