//! Nexty 文本整形与字体 facade。
//!
//! 上游：`parley`（Apache-2.0/MIT）——内部用 `fontique` 做字体选择与回退、
//! `harfrust`（HarfBuzz 整形算法的 Rust 实现）做字形整形。选型 ADR 中列出的
//! `swash` 自 parley 0.11 起已不参与整形，本层不再依赖（决策见
//! `docs/decisions/2026-10-01-crate-selection.md`）。
//!
//! 行为 ground truth：CSS Text、CSS Fonts 与 Unicode 断行算法；字形定位遵循
//! OpenType 度量（units_per_em 归一到 CSS px，scale = 1）。
//!
//! 字体选择：族列表匹配由 parley/fontique 完成；主族缺字形的码点按脚本
//! 回退到系统字体（CJK 在 Windows 命中 YaHei 等）。[`ShapedText::fonts`]
//! 记录实际用到的字体、每个 [`Glyph::font`] 指明归属，光栅层按此渲染。
//!
//! Web 字体（@font-face）：[`FontRegistry`] 按会话注册字体字节与描述符
//! （CSS 声明族名 / weight / style），注册时经 fontique 的
//! `FontInfoOverride` 把 CSS 声明名注入字体集合——声明名与字体内部族名
//! 解耦（CSS Fonts §4）；族内 face 选择（width → style → weight）委托
//! fontique 的 CSS Fonts §5.2 匹配实现。注册面对整形与解析同时生效：
//! 整形器在每次整形前增量同步，[`FontResolver`] 在每次解析前同步。
//!
//! 本层只整形**单行**文本：white-space 折叠（含换行符处理）是 layout 层的
//! 职责，进入本层的文本应已折叠。
//!
//! 上游类型一律不得出现在本 crate 的 pub 导出中（AGENTS.md 硬规则）。
//!
//! **偏差清单（Web 字体简化档）**：同族名的系统字体与注册 face 会合并进
//! 同一 fontique 族参与匹配——CSS Fonts 要求 @font-face 覆盖同名系统
//! 字体，此处不保证覆盖顺序（同名共存罕见，接受）；多 face collection
//! （TTC）的全部 face 以同一描述符注册（face index 仍按 (blob id, face
//! index) 缓存路径流转）；`font-stretch` / `unicode-range` / 可变字体
//! 轴不支持；oblique 期望由上层映射为 italic；`font-display` 行为在
//! chrome 层编排。
//!
//! **Feature**：单行整形（`shaping`，含 `TextShaper` 与 parley 实现）与字体文件
//! 解析（`font-resolution`，含 `FontResolver` 与 [`FontRegistry`]）分别受
//! feature 控制，默认全开；二者共享 parley 依赖，故各自隐含 `dep:parley`。

#![forbid(unsafe_code)]

#[cfg(feature = "shaping")]
use std::cell::Cell;
#[cfg(feature = "shaping")]
use std::cell::RefCell;
#[cfg(feature = "shaping")]
use std::collections::HashMap;
#[cfg(feature = "font-resolution")]
use std::sync::Mutex;

#[cfg(feature = "shaping")]
use parley::StyleProperty;

/// 字体族与字号。
#[cfg(feature = "shaping")]
#[derive(Debug, Clone, PartialEq)]
pub struct TextStyle {
    /// 字体族列表，按 CSS `font-family` 优先顺序排列。
    ///
    /// 每一项是一个族名（如 `"Times New Roman"`）或 generic 族关键字
    /// （如 `"serif"` / `"sans-serif"` / `"monospace"`，大小写不敏感），
    /// 解析与回退由 parley/fontique 按 CSS Fonts 语义处理。
    pub families: Vec<String>,
    /// 字号，CSS px。
    pub size: f32,
}

/// 整形后的单个字形。
#[cfg(feature = "shaping")]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    /// 字体内部字形编号。
    pub id: u32,
    /// 相对行首的水平偏移，CSS px。
    pub x: f32,
    /// 相对基线的垂直偏移，CSS px（负值在基线上方）。
    pub y: f32,
    /// 步进宽度，CSS px。
    pub advance: f32,
    /// 所属字体在 [`ShapedText::fonts`] 中的下标（按脚本回退后一段文本
    /// 可能用到多个字体）。
    pub font: u32,
    /// 所属 cluster 在源文本中的起始字节偏移（UAX#14 断点与行内布局按它
    /// 把一次整形结果切到各不可拆单元上）。
    pub cluster: u32,
}

/// 一段文本的整形结果。
///
/// 坐标系：`x` 从行首起算，`y` 以基线为 0、向下为正；[`ShapedText::height`]
/// 是 parley 按字体度量算出的行高。
#[cfg(feature = "shaping")]
#[derive(Debug, Clone, PartialEq)]
pub struct ShapedText {
    /// 按绘制顺序排列的字形（`glyph.font` 指向 [`ShapedText::fonts`]）。
    pub glyphs: Vec<Glyph>,
    /// 本次整形实际用到的字体（parley/fontique 按脚本回退后可能多于一个）。
    pub fonts: Vec<ResolvedFont>,
    /// 行宽（含行尾空白），CSS px。
    pub width: f32,
    /// 行高，CSS px。
    pub height: f32,
}

/// 字体度量（CSS px；strut / 行盒计算用）。
#[cfg(feature = "shaping")]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontMetrics {
    /// ascent：基线到行内最高点的距离（CSS px，正值）。
    pub ascent: f32,
    /// descent：基线到行内最低点的距离（CSS px，正值）。
    pub descent: f32,
    /// 该字体的默认行高（ascent + descent + leading，CSS px）。
    pub line_height: f32,
}

/// 解析出的字体资源：字体文件字节与 collection 内 face 索引。
///
/// 供光栅层（paint）构造字形渲染所需的字体数据；字节为共享快照，
/// 解析结果可按族列表缓存。
#[cfg(feature = "font-resolution")]
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedFont {
    data: std::sync::Arc<[u8]>,
    index: u32,
    /// 字体实例标识（fontique blob id）。同一实例判等用它而非字节比较
    /// （字体文件可达数 MB）。
    id: u64,
}

#[cfg(feature = "font-resolution")]
impl ResolvedFont {
    /// 字体文件字节（TTC 时为整个 collection）。
    #[must_use]
    pub fn data(&self) -> &std::sync::Arc<[u8]> {
        &self.data
    }

    /// face 在 collection 中的索引（单字体文件为 0）。
    #[must_use]
    pub fn index(&self) -> u32 {
        self.index
    }

    /// 字体实例标识：同一来源字体的多次解析共享同一 id。
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }
}

#[cfg(feature = "font-resolution")]
impl std::fmt::Debug for ResolvedFont {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedFont")
            .field("bytes", &self.data.len())
            .field("index", &self.index)
            .field("id", &self.id)
            .finish()
    }
}

/// 字体样式（CSS `font-style` 的简化档：仅 normal / italic，无 oblique）。
///
/// 期望 oblique 的请求由上层映射为 [`FontStyle::Italic`]（偏差见模块文档）。
#[cfg(feature = "font-resolution")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontStyle {
    /// 直立体。
    Normal,
    /// 斜体。
    Italic,
}

/// @font-face 字体描述符（CSS Fonts §4 简化档）。
///
/// `family` 是 @font-face 规则声明的 CSS 族名，与字体文件内部的族名无关；
/// 注册后按该名参与 `font-family` 匹配（大小写不敏感，与 CSS 一致）。
/// `weight` 是 font-weight 描述符的单值（1–1000，超界值注册时 clamp）；
/// `style` 是 font-style 描述符。
#[cfg(feature = "font-resolution")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontFaceDescriptor {
    /// @font-face 规则的 font-family 描述符（CSS 声明名）。
    pub family: String,
    /// @font-face 规则的 font-weight 描述符（单值，1–1000）。
    pub weight: u16,
    /// @font-face 规则的 font-style 描述符。
    pub style: FontStyle,
}

/// [`FontRegistry::register`] 的失败原因。
#[cfg(feature = "font-resolution")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontRegistryError {
    /// 描述符的 family 名为空（或全空白）。
    EmptyFamily,
    /// 字体数据无法解析出任何 face（非字体文件或已损坏）。
    InvalidFontData,
}

/// 会话级 @font-face 字体注册表。
///
/// 持有注册的字体字节与描述符；消费方（[`FontResolver`] 与整形器
/// `ParleyTextShaper`）在使用前把新注册面增量注入各自的 fontique 字体
/// 集合——注册即生效，无需任何「激活」动作。注册只增不减（生命周期与
/// 会话一致）；`Clone` 是廉价的共享句柄（内部 `Arc`），`Send + Sync`。
#[cfg(feature = "font-resolution")]
#[derive(Clone, Default)]
pub struct FontRegistry {
    inner: std::sync::Arc<RegistryInner>,
}

#[cfg(feature = "font-resolution")]
#[derive(Default)]
struct RegistryInner {
    /// 按注册顺序排列的 face；长度即同步代次。
    faces: Mutex<Vec<RegisteredFace>>,
}

#[cfg(feature = "font-resolution")]
struct RegisteredFace {
    descriptor: FontFaceDescriptor,
    /// 字体字节共享快照：id 全局唯一，克隆到各消费方后 `(blob id,
    /// face index)` 缓存路径与系统字体共用一份逻辑。
    blob: parley::fontique::Blob<u8>,
}

#[cfg(feature = "font-resolution")]
impl std::fmt::Debug for FontRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self
            .inner
            .faces
            .lock()
            .map(|faces| faces.len())
            .unwrap_or(0);
        f.debug_struct("FontRegistry")
            .field("faces", &count)
            .finish()
    }
}

#[cfg(feature = "font-resolution")]
impl FontRegistry {
    /// 创建空注册表。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一份字体字节为指定描述符的 face。
    ///
    /// 字节先经 fontique 扫描校验（解析不出任何 face 即
    /// [`FontRegistryError::InvalidFontData`]，非法字节不进入注册表）；
    /// 合法面进入注册表，对已接入的解析器 / 整形器在下一次使用时生效。
    /// 多 face collection（TTC）的全部 face 都以该描述符注册（偏差见
    /// 模块文档）；face index 由 fontique 扫描得出并随解析结果流转，
    /// `(blob id, face index)` 缓存路径与系统字体共用。
    ///
    /// # Errors
    ///
    /// family 名为空返回 [`FontRegistryError::EmptyFamily`]；字节不是可
    /// 解析字体返回 [`FontRegistryError::InvalidFontData`]。
    pub fn register(
        &self,
        data: Vec<u8>,
        descriptor: FontFaceDescriptor,
    ) -> Result<(), FontRegistryError> {
        if descriptor.family.trim().is_empty() {
            return Err(FontRegistryError::EmptyFamily);
        }
        let mut descriptor = descriptor;
        descriptor.weight = descriptor.weight.clamp(1, 1000);
        let blob: parley::fontique::Blob<u8> = parley::fontique::Blob::from(data);
        // 校验用临时集合（不枚举系统字体）：扫描不出任何 face 即非法字节
        let mut probe = parley::fontique::Collection::new(parley::fontique::CollectionOptions {
            shared: false,
            system_fonts: false,
        });
        let registered =
            probe.register_fonts(blob.clone(), Some(registered_face_override(&descriptor)));
        if registered.is_empty() {
            return Err(FontRegistryError::InvalidFontData);
        }
        self.inner
            .faces
            .lock()
            .expect("font registry poisoned")
            .push(RegisteredFace { descriptor, blob });
        Ok(())
    }

    /// 把 `*applied` 之后新注册的 face 增量注入 `context` 的字体集合并
    /// 推进代次（幂等；无新注册时空操作）。仅 crate 内部使用。
    pub(crate) fn sync_into(&self, context: &mut parley::FontContext, applied: &mut usize) {
        let faces = self.inner.faces.lock().expect("font registry poisoned");
        if *applied >= faces.len() {
            return;
        }
        for face in &faces[*applied..] {
            context.collection.register_fonts(
                face.blob.clone(),
                Some(registered_face_override(&face.descriptor)),
            );
        }
        *applied = faces.len();
    }
}

/// 描述符 → fontique 注册元数据覆盖：CSS 声明名与 weight/style 经此
/// 写入 face 属性（fontique 为 @font-face 类 API 预留的入口）。
#[cfg(feature = "font-resolution")]
fn registered_face_override(
    descriptor: &FontFaceDescriptor,
) -> parley::fontique::FontInfoOverride<'_> {
    parley::fontique::FontInfoOverride {
        family_name: Some(&descriptor.family),
        width: None,
        style: Some(match descriptor.style {
            FontStyle::Normal => parley::fontique::FontStyle::Normal,
            FontStyle::Italic => parley::fontique::FontStyle::Italic,
        }),
        weight: Some(parley::fontique::FontWeight::new(f32::from(
            descriptor.weight,
        ))),
        axes: None,
    }
}

/// 文本整形后端。
///
/// 实现方负责把 [`TextStyle`] 映射到具体字体选择与整形引擎，
/// 只对外暴露本 crate 的类型。
#[cfg(feature = "shaping")]
pub trait TextShaper {
    /// 对一段文本整形。
    ///
    /// # Errors
    ///
    /// 字体缺失或整形失败时返回 [`TextError`]。
    fn shape(&self, text: &str, style: &TextStyle) -> Result<ShapedText, TextError>;

    /// 返回字体族列表对应字体的度量。
    ///
    /// # Errors
    ///
    /// 字体族列表为空时返回 [`TextError::FontUnavailable`]。
    fn metrics(&self, style: &TextStyle) -> Result<FontMetrics, TextError>;
}

/// 文本整形错误。
#[cfg(feature = "shaping")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextError {
    /// 请求的字体族列表为空，无法选择字体。
    ///
    /// 单个未知族名不会触发本错误：parley/fontique 会按 CSS Fonts 的回退
    /// 语义选择可用字体。
    FontUnavailable,
}

/// 字体解析器：把 CSS 字体族列表解析为字体文件字节。
///
/// 独立于整形器（无布局上下文），内部用 `Mutex` 提供可变性，
/// 可跨线程共享（`Send + Sync`）。可接入会话级 [`FontRegistry`]：
/// 族列表中的 @font-face 声明名命中注册 face，未命中的族走既有系统
/// 字体回退。
#[cfg(feature = "font-resolution")]
#[derive(Default)]
pub struct FontResolver {
    state: Mutex<ResolveState>,
    registry: Option<FontRegistry>,
}

#[cfg(feature = "font-resolution")]
#[derive(Default)]
struct ResolveState {
    /// fontique 字体上下文（系统字体 + 已注入的注册 face）。
    context: parley::FontContext,
    /// 已注入的注册 face 数（同步代次，见 [`FontRegistry::sync_into`]）。
    applied: usize,
}

#[cfg(feature = "font-resolution")]
impl std::fmt::Debug for FontResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontResolver")
            .field("with_registry", &self.registry.is_some())
            .finish()
    }
}

#[cfg(feature = "font-resolution")]
impl FontResolver {
    /// 创建解析器并发现系统字体。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建接入会话级字体注册表的解析器。
    ///
    /// `registry` 内部按 `Arc` 共享（克隆廉价）；此后每次解析前把新注册
    /// face 增量注入，注册即生效。
    #[must_use]
    pub fn with_registry(registry: &FontRegistry) -> Self {
        Self {
            registry: Some(registry.clone()),
            ..Self::default()
        }
    }

    /// 按字体族列表解析出可用字体的文件字节（期望样式默认 normal/400）。
    ///
    /// 族名 / generic 关键字的语义与 [`TextShaper::shape`] 一致；全部族名
    /// 都不可用时按 CSS Fonts 回退语义退到 sans-serif，仍无则返回 `None`。
    /// 空族列表返回 `None`。
    pub fn resolve_font(&self, families: &[String]) -> Option<ResolvedFont> {
        // CSS Fonts 的 normal 关键字 = 400
        self.resolve_font_with(families, 400, FontStyle::Normal)
    }

    /// [`FontResolver::resolve_font`] 的带期望样式扩展（@font-face 就近匹配）。
    ///
    /// `weight`（CSS 1–1000）与 `style` 作为期望值参与族内匹配：注册 face
    /// 与系统 face 同池对待，width → style → weight 逐级就近（CSS Fonts
    /// §5.2）；族列表按序前进，第一个有可用 face 的族生效。
    pub fn resolve_font_with(
        &self,
        families: &[String],
        weight: u16,
        style: FontStyle,
    ) -> Option<ResolvedFont> {
        if families.is_empty() {
            return None;
        }
        let mut state = self.state.lock().expect("font resolver poisoned");
        if let Some(registry) = &self.registry {
            // 解构拆字段借用：MutexGuard 的两次 DerefMut 会整体冲突
            let ResolveState { context, applied } = &mut *state;
            registry.sync_into(context, applied);
        }
        resolve_font_in(&mut state.context, families, weight, style)
    }
}

/// 族列表解析的共用实现（`FontResolver` 与整形器共用）。
///
/// 调用方负责先把会话级注册 face 注入 `context`（见
/// [`FontRegistry::sync_into`]）；族名 / generic 关键字的解析交给 parley
/// （`FontFamilyName`），查询按 CSS Fonts §5.2 逐族前进，全部未命中时
/// 回退 sans-serif。
#[cfg(feature = "font-resolution")]
fn resolve_font_in(
    context: &mut parley::FontContext,
    families: &[String],
    weight: u16,
    style: FontStyle,
) -> Option<ResolvedFont> {
    // 族名 / generic 关键字的解析与整形同源（parlance）；
    // QueryFamily 借用族名字符串，故持有 owned 副本保证生命周期
    enum FamilySpec {
        Named(String),
        Generic(parley::GenericFamily),
    }
    fn build_items(specs: &[FamilySpec]) -> Vec<parley::fontique::QueryFamily<'_>> {
        specs
            .iter()
            .map(|spec| match spec {
                FamilySpec::Named(name) => parley::fontique::QueryFamily::Named(name),
                FamilySpec::Generic(generic) => parley::fontique::QueryFamily::Generic(*generic),
            })
            .collect()
    }
    let specs: Vec<FamilySpec> = families
        .iter()
        .filter_map(|family| match parley::FontFamilyName::parse(family)? {
            parley::FontFamilyName::Named(name) => Some(FamilySpec::Named(name.into_owned())),
            parley::FontFamilyName::Generic(generic) => Some(FamilySpec::Generic(generic)),
        })
        .collect();

    // 拆字段借用：collection 与 source_cache 同时可变借用
    let parley::FontContext {
        collection,
        source_cache,
    } = context;
    let mut resolve = |specs: &[FamilySpec]| -> Option<ResolvedFont> {
        let items = build_items(specs);
        let mut query = collection.query(source_cache);
        query.set_families(items);
        // CSS Fonts §5.2 的族内匹配：width → style → weight 逐级就近。
        // width 恒取查询默认值（本层不支持 font-stretch，见模块偏差清单）。
        query.set_attributes(parley::fontique::Attributes::new(
            parley::fontique::FontWidth::default(),
            match style {
                FontStyle::Normal => parley::fontique::FontStyle::Normal,
                FontStyle::Italic => parley::fontique::FontStyle::Italic,
            },
            parley::fontique::FontWeight::new(f32::from(weight)),
        ));
        let mut found = None;
        query.matches_with(|font| {
            // Blob 是共享引用计数，字节取快照供跨线程使用；
            // blob id 作实例标识（判等免去字节比较）
            found = Some(ResolvedFont {
                data: std::sync::Arc::from(font.blob.as_ref()),
                index: font.index,
                id: font.blob.id(),
            });
            parley::fontique::QueryStatus::Stop
        });
        found
    };

    let mut resolved = resolve(&specs);
    if resolved.is_none() {
        // 回退：sans-serif（CSS Fonts 的 last-resort 语义简化）
        resolved = resolve(&[FamilySpec::Generic(parley::GenericFamily::SansSerif)]);
    }
    resolved
}

/// [`TextShaper`] 的 parley 实现。
///
/// 持有 fontique 字体集合（系统字体与已注入的注册 face）与 parley 布局
/// 上下文；上下文可复用，用 `RefCell` 提供内部可变性（`shape` 只需
/// `&self`）。
#[cfg(feature = "shaping")]
pub struct ParleyTextShaper {
    font_context: RefCell<parley::FontContext>,
    layout_context: RefCell<parley::LayoutContext>,
    /// 已解析字体实例缓存：`(blob id, face index) → ResolvedFont`。
    ///
    /// 同一字体的每次命中共享一份字节快照（`Arc` 引用计数）。没有它，
    /// 每次整形都会为触到的每个字体 run 复制整个字体文件——CJK 回退
    /// 字体（msyh.ttc）约 20 MB，按文本 chunk 数放大后布局期内存以 GB
    /// 计（T1 基准实测：2,000 段页面冲到 9 GB 提交内存后分配失败 abort）。
    font_cache: RefCell<HashMap<(u64, u32), ResolvedFont>>,
    /// 会话级 @font-face 注册表（可选）；整形 / 解析前增量注入 font_context。
    registry: Option<FontRegistry>,
    /// 注册表同步代次（已注入的注册 face 数）。
    registry_applied: Cell<usize>,
}

#[cfg(feature = "shaping")]
impl std::fmt::Debug for ParleyTextShaper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParleyTextShaper").finish()
    }
}

#[cfg(feature = "shaping")]
impl Default for ParleyTextShaper {
    fn default() -> Self {
        Self {
            font_context: RefCell::new(parley::FontContext::new()),
            layout_context: RefCell::new(parley::LayoutContext::new()),
            font_cache: RefCell::new(HashMap::new()),
            registry: None,
            registry_applied: Cell::new(0),
        }
    }
}

#[cfg(feature = "shaping")]
impl ParleyTextShaper {
    /// 创建整形器并发现系统字体。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建整形器并接入会话级 @font-face 字体注册表。
    ///
    /// 注册表中的 face 在此后每次整形 / 解析前增量注入（注册即生效，
    /// 无需重建整形器）；`registry` 内部按 `Arc` 共享，克隆廉价。
    #[must_use]
    pub fn with_registry(registry: &FontRegistry) -> Self {
        Self {
            registry: Some(registry.clone()),
            ..Self::default()
        }
    }

    /// 按字体族列表解析出可用字体的文件字节。
    ///
    /// 语义见 [`FontResolver::resolve_font`]；整形器接入的注册表（见
    /// [`ParleyTextShaper::with_registry`]）同样参与命中。
    pub fn resolve_font(&self, families: &[String]) -> Option<ResolvedFont> {
        if families.is_empty() {
            return None;
        }
        let mut font_context = self.font_context.borrow_mut();
        self.sync_registry(&mut font_context);
        resolve_font_in(&mut font_context, families, 400, FontStyle::Normal)
    }

    /// 把注册表的新 face 增量注入 `font_context`（未接入注册表时空操作）。
    fn sync_registry(&self, font_context: &mut parley::FontContext) {
        if let Some(registry) = &self.registry {
            let mut applied = self.registry_applied.get();
            registry.sync_into(font_context, &mut applied);
            self.registry_applied.set(applied);
        }
    }

    /// 按样式对文本做单行 parley 布局（不折行）。
    fn build_layout(
        &self,
        text: &str,
        style: &TextStyle,
    ) -> Result<parley::Layout<[u8; 4]>, TextError> {
        if style.families.is_empty() {
            return Err(TextError::FontUnavailable);
        }
        // CSS font-family 列表的解析（族名/引号/generic 关键字/回退顺序）
        // 交给 parley 的 FontFamily::Source
        let family_list = style.families.join(", ");

        let mut font_context = self.font_context.borrow_mut();
        // @font-face 注册面增量注入（注册即生效）
        self.sync_registry(&mut font_context);
        let mut layout_context = self.layout_context.borrow_mut();
        let mut builder = layout_context.ranged_builder(&mut font_context, text, 1.0, false);
        builder.push_default(StyleProperty::FontFamily(parley::FontFamily::Source(
            std::borrow::Cow::Borrowed(&family_list),
        )));
        builder.push_default(StyleProperty::FontSize(style.size));

        let mut layout = builder.build(text);
        // max_advance = None：单行整形，不做折行
        layout.break_all_lines(None);
        Ok(layout)
    }
}

#[cfg(feature = "shaping")]
impl TextShaper for ParleyTextShaper {
    fn shape(&self, text: &str, style: &TextStyle) -> Result<ShapedText, TextError> {
        if text.is_empty() {
            return Ok(ShapedText {
                glyphs: Vec::new(),
                fonts: Vec::new(),
                width: 0.0,
                height: 0.0,
            });
        }

        let layout = self.build_layout(text, style)?;

        // 按脚本回退后一个 layout 可能含多个字体 run：收集去重成字体表，
        // 每个字形带自己在表中的下标（供光栅层用正确字体渲染字形 id）
        let mut fonts: Vec<ResolvedFont> = Vec::new();
        let mut glyphs = Vec::new();
        for line in layout.lines() {
            let baseline = line.metrics().baseline;
            for item in line.items() {
                let parley::PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                    continue;
                };
                let run_font = glyph_run.run().font();
                let font_key = (run_font.data.id(), run_font.index);
                let resolved = {
                    let mut cache = self.font_cache.borrow_mut();
                    match cache.get(&font_key) {
                        Some(resolved) => resolved.clone(),
                        None => {
                            let resolved = ResolvedFont {
                                data: std::sync::Arc::from(run_font.data.as_ref()),
                                index: run_font.index,
                                id: font_key.0,
                            };
                            cache.insert(font_key, resolved.clone());
                            resolved
                        }
                    }
                };
                // 同一实例来自缓存（(blob id, face index) 唯一），按下标判等
                // 免去字节比较
                let font_index = fonts
                    .iter()
                    .position(|font| font.id == resolved.id && font.index == resolved.index)
                    .unwrap_or_else(|| {
                        fonts.push(resolved);
                        fonts.len() - 1
                    }) as u32;
                // LB9/LB10：CM/ZWJ 序列并入基字符的 cluster；cluster 起点取
                // parley 的 cluster text_range（源文本字节偏移）。当前 shaper
                // 只推单一样式，GlyphRun 覆盖整个 run，故按顺序 zip 即可。
                let cluster_starts: Vec<u32> = glyph_run
                    .run()
                    .visual_clusters()
                    .flat_map(|cluster| {
                        let start = cluster.text_range().start as u32;
                        cluster.glyphs().map(move |_| start)
                    })
                    .collect();
                for (index, glyph) in glyph_run.positioned_glyphs().enumerate() {
                    glyphs.push(Glyph {
                        id: glyph.id,
                        // positioned_glyphs 的 y 已含基线偏移，换回相对基线
                        x: glyph.x,
                        y: glyph.y - baseline,
                        advance: glyph.advance,
                        font: font_index,
                        cluster: cluster_starts.get(index).copied().unwrap_or(0),
                    });
                }
            }
        }

        let width = layout
            .width()
            .max(glyphs.last().map_or(0.0, |glyph| glyph.x + glyph.advance));

        Ok(ShapedText {
            glyphs,
            fonts,
            // parley 的行宽按断行测量语义**不含行尾空白**，而本层契约是含
            // （调用方按 ShapedText.width 计词间空白的步进，见 inline 模块的
            // space_after_previous）——取字形右缘与行宽的较大者。
            width,
            height: layout.height(),
        })
    }

    fn metrics(&self, style: &TextStyle) -> Result<FontMetrics, TextError> {
        // 用不换行空格探针取度量：NBSP 在几乎所有字体中都有字形，
        // parley 的行度量来自 run 字体的 ascent/descent/leading
        let layout = self.build_layout("\u{00A0}", style)?;
        let line = layout.lines().next().ok_or(TextError::FontUnavailable)?;
        let metrics = line.metrics();
        Ok(FontMetrics {
            ascent: metrics.ascent,
            descent: metrics.descent,
            line_height: metrics.line_height,
        })
    }
}

// 测试覆盖整形与字体解析（shaping 隐含 font-resolution）。
#[cfg(all(test, feature = "shaping"))]
mod tests {
    use super::*;

    fn shaper() -> ParleyTextShaper {
        ParleyTextShaper::new()
    }

    fn style(families: &[&str], size: f32) -> TextStyle {
        TextStyle {
            families: families.iter().map(|family| (*family).to_owned()).collect(),
            size,
        }
    }

    #[test]
    fn empty_text_yields_empty_output() {
        let shaped = shaper()
            .shape("", &style(&["serif"], 16.0))
            .expect("empty text shapes to empty");
        assert!(shaped.glyphs.is_empty());
        assert_eq!(shaped.width, 0.0);
    }

    #[test]
    fn empty_family_list_is_font_unavailable() {
        let error = shaper()
            .shape("hello", &style(&[], 16.0))
            .expect_err("no families");
        assert_eq!(error, TextError::FontUnavailable);
    }

    #[test]
    fn ascii_text_shapes_to_positive_metrics() {
        let shaped = shaper()
            .shape("Hello", &style(&["serif"], 16.0))
            .expect("shaping with system fonts");

        assert!(!shaped.glyphs.is_empty());
        assert!(shaped.width > 0.0);
        assert!(shaped.height > 0.0);
        // 每个字形有非负步进，x 单调不减
        let mut previous_x = 0.0;
        for glyph in &shaped.glyphs {
            assert!(glyph.advance >= 0.0);
            assert!(glyph.x >= previous_x - 1e-4);
            previous_x = glyph.x;
        }
        // 宽度与最后一个字形的右缘一致（容差内）
        let last = shaped.glyphs.last().expect("non-empty");
        assert!((shaped.width - (last.x + last.advance)).abs() < 1e-3);
    }

    #[test]
    fn glyphs_carry_source_byte_clusters() {
        let text = "a\u{301}b";
        let shaped = shaper()
            .shape(text, &style(&["serif"], 16.0))
            .expect("shaping with system fonts");

        assert!(!shaped.glyphs.is_empty());
        for glyph in &shaped.glyphs {
            let cluster = glyph.cluster as usize;
            assert!(cluster <= text.len());
            assert!(text.is_char_boundary(cluster));
        }
        assert_eq!(shaped.glyphs[0].cluster, 0);
        assert!(
            shaped
                .glyphs
                .windows(2)
                .all(|pair| pair[0].cluster <= pair[1].cluster),
            "LTR cluster values must be monotone: {:?}",
            shaped
                .glyphs
                .iter()
                .map(|glyph| glyph.cluster)
                .collect::<Vec<_>>()
        );
        // 组合附加符并入基字符 cluster：b 的起始字节是 3，不是 1。
        assert!(shaped.glyphs.iter().any(|glyph| glyph.cluster == 3));
        assert!(shaped.glyphs.iter().all(|glyph| glyph.cluster != 1));
    }

    #[test]
    fn advances_scale_with_font_size() {
        let shaper = shaper();
        let small = shaper
            .shape("iii", &style(&["serif"], 16.0))
            .expect("small");
        let large = shaper
            .shape("iii", &style(&["serif"], 32.0))
            .expect("large");

        assert_eq!(
            small.glyphs.len(),
            large.glyphs.len(),
            "同字体同文本字形数一致"
        );
        let small_advance: f32 = small.glyphs.iter().map(|glyph| glyph.advance).sum();
        let large_advance: f32 = large.glyphs.iter().map(|glyph| glyph.advance).sum();
        let ratio = large_advance / small_advance;
        assert!(
            (ratio - 2.0).abs() < 0.02,
            "advance 比例应约等于字号比例: {ratio}"
        );
    }

    #[test]
    fn unknown_family_falls_back_gracefully() {
        let shaper = shaper();
        // 未知族名按 CSS Fonts 语义回退到可用字体，不报错
        let shaped = shaper
            .shape("Hi", &style(&["no-such-family-xyz", "serif"], 16.0))
            .expect("fallback");
        assert!(!shaped.glyphs.is_empty());

        // 只有未知族名同样回退
        let shaped = shaper
            .shape("Hi", &style(&["no-such-family-xyz"], 16.0))
            .expect("fallback with single unknown family");
        assert!(!shaped.glyphs.is_empty());
    }

    #[test]
    fn generic_family_keywords_work() {
        let shaper = shaper();
        for family in ["sans-serif", "SANS-SERIF", "monospace", "system-ui"] {
            let shaped = shaper
                .shape("Hi", &style(&[family], 16.0))
                .unwrap_or_else(|error| panic!("{family} should shape: {error:?}"));
            assert!(!shaped.glyphs.is_empty(), "{family} produced glyphs");
        }
    }

    #[test]
    fn trailing_whitespace_is_included_in_width() {
        // parley 行宽裁剪行尾空白；ShapedText.width 契约是含行尾空白
        // （空格的步进供行内布局计词间距，见下一测试）
        let shaper = shaper();
        let style = style(&["serif"], 16.0);
        let space = shaper.shape(" ", &style).expect("shape space");
        assert!(space.width > 0.0, "行尾空白计入宽度：width={}", space.width);
        let word = shaper.shape("a", &style).expect("shape a");
        let trailing = shaper.shape("a ", &style).expect("shape a-space");
        assert!(
            trailing.width > word.width,
            "a 加行尾空格应更宽：{} vs {}",
            trailing.width,
            word.width
        );
    }

    #[test]
    fn inter_word_space_advances_position() {
        // 词间空白的宽度 = 后词起点与前词右缘之差（行内布局的空格语义）
        let shaper = shaper();
        let style = style(&["serif"], 16.0);
        let space = shaper.shape(" ", &style).expect("shape space");
        let text = shaper.shape("a b", &style).expect("shape a b");
        let b = text.glyphs.last().expect("b glyph");
        let a = text.glyphs.first().expect("a glyph");
        let gap = b.x - (a.x + a.advance);
        assert!(
            (gap - space.width).abs() < 1e-3,
            "词间空隙 {gap} 应等于空格宽 {}",
            space.width
        );
    }

    #[test]
    fn metrics_are_positive_and_scale_with_size() {
        let shaper = shaper();
        let small = shaper
            .metrics(&style(&["serif"], 16.0))
            .expect("metrics for serif");
        assert!(small.ascent > 0.0);
        assert!(small.descent > 0.0);
        assert!(small.line_height >= small.ascent + small.descent);

        let large = shaper
            .metrics(&style(&["serif"], 32.0))
            .expect("metrics at 2x size");
        let ratio = large.ascent / small.ascent;
        assert!((ratio - 2.0).abs() < 0.05, "ascent 应约随字号翻倍: {ratio}");
    }

    #[test]
    fn metrics_empty_family_list_is_font_unavailable() {
        let error = shaper()
            .metrics(&style(&[], 16.0))
            .expect_err("no families");
        assert_eq!(error, TextError::FontUnavailable);
    }

    #[test]
    fn resolve_font_returns_bytes_for_known_and_unknown_families() {
        let shaper = shaper();
        let resolved = shaper
            .resolve_font(&["serif".to_owned()])
            .expect("generic family resolves");
        assert!(!resolved.data().is_empty(), "字体字节非空");
        assert_eq!(resolved.index(), 0, "常规字体文件单 face");

        // 全部族名未知 → 回退仍能解析出字体
        let fallback = shaper
            .resolve_font(&["no-such-family-xyz".to_owned()])
            .expect("fallback resolves");
        assert!(!fallback.data().is_empty());
    }

    #[test]
    fn resolve_font_empty_family_list_is_none() {
        assert!(shaper().resolve_font(&[]).is_none());
    }

    /// 字体实例缓存：同一字体的多次整形共享同一份字节快照（`Arc` 引用
    /// 计数）。回归钉住——此前每次整形都复制整个字体文件，CJK 回退字体
    /// （约 20 MB）按文本 chunk 数放大后布局期内存以 GB 计。
    #[test]
    fn repeated_shapes_share_font_data() {
        let shaper = shaper();
        let mixed = style(&["serif", "sans-serif"], 14.0);
        let first = shaper.shape("中英混排 mixed text", &mixed).expect("shape");
        let second = shaper
            .shape("另一段 other text 混排", &mixed)
            .expect("shape");
        assert!(!first.fonts.is_empty(), "混排触发至少一个字体");
        for font in &first.fonts {
            let shared = second
                .fonts
                .iter()
                .find(|other| other.id == font.id)
                .expect("同一字体实例再次出现");
            assert!(
                std::sync::Arc::ptr_eq(font.data(), shared.data()),
                "同一字体实例的字节必须共享而非复制"
            );
        }
    }

    #[test]
    fn resolved_font_bytes_are_valid_font_data() {
        // skrifa（parley 的字体解析库依赖路径）能读取为合法 face
        let shaper = shaper();
        let resolved = shaper
            .resolve_font(&["serif".to_owned()])
            .expect("resolves");
        let face_count = skrifa::FontRef::from_index(resolved.data(), resolved.index())
            .map(|_| 1)
            .unwrap_or(0);
        assert_eq!(face_count, 1, "字节可被 skrifa 解析");
    }

    #[test]
    fn cjk_text_shapes_without_notdef_glyphs() {
        // fontique 回退应把缺字码点交给系统 CJK 字体（Windows YaHei 等），
        // 不得产出 .notdef（字形 id 0）豆腐块。
        // 环境门：系统字体对有任一测试字符无覆盖时（部分 CI runner 未装
        // 完整 CJK 字体），notdef 是唯一合法产出，跳过断言——覆盖按
        // cmap 逐字符独立核查（见 system_covers_cjk_probe），不走被测管线。
        let probe = "中文测试";
        if !system_covers_text(probe) {
            eprintln!("skipping notdef assertion: system fonts do not cover {probe:?}");
            return;
        }
        let shaper = shaper();
        let shaped = shaper
            .shape(probe, &style(&["sans-serif"], 16.0))
            .expect("CJK shaping");
        assert!(!shaped.glyphs.is_empty());
        assert!(!shaped.fonts.is_empty(), "字体表非空");
        let notdef = shaped.glyphs.iter().filter(|glyph| glyph.id == 0).count();
        assert_eq!(notdef, 0, "中文字形不得有 notdef 豆腐块");
        assert!(
            shaped
                .glyphs
                .iter()
                .all(|glyph| (glyph.font as usize) < shaped.fonts.len()),
            "font 下标均在字体表内"
        );
    }

    /// 系统字体（sans-serif 链 + Hani 脚本回退）是否覆盖 `text` 全部字符。
    ///
    /// 与被测管线无关的独立核查：直接向 fontique 查询字体族与回退族，
    /// 逐字符查 cmap（`QueryFont::charmap`）确认存在覆盖。任一字符无字体
    /// 覆盖时返回 false——此时整形产出 notdef 是环境的唯一合法行为
    /// （无 CJK 字体的 CI runner），notdef 断言按环境跳过；跳过不算通过
    /// 验证，有覆盖的系统（开发机 / 装了字体的 CI）上断言始终生效。
    fn system_covers_text(text: &str) -> bool {
        let mut context = parley::FontContext::new();
        let parley::FontContext {
            collection,
            source_cache,
        } = &mut context;
        let mut query = collection.query(source_cache);
        query.set_families([parley::fontique::QueryFamily::Generic(
            parley::GenericFamily::SansSerif,
        )]);
        query.set_fallbacks(parley::fontique::FallbackKey::new(
            parley::fontique::Script::from_str_unchecked("Hani"),
            None,
        ));
        // 逐个字符收集「已覆盖」标记，任一字体命中即算覆盖
        let mut covered = vec![false; text.chars().count()];
        query.matches_with(|font| {
            if let Some(charmap) = font.charmap() {
                for (index, character) in text.chars().enumerate() {
                    if !covered[index] && charmap.map(character).is_some() {
                        covered[index] = true;
                    }
                }
            }
            if covered.iter().all(|flag| *flag) {
                parley::fontique::QueryStatus::Stop
            } else {
                parley::fontique::QueryStatus::Continue
            }
        });
        covered.iter().all(|flag| *flag)
    }

    #[test]
    fn mixed_script_text_keeps_valid_font_indices() {
        // 与被测管线无关的属性（字形-字体归属一致性）无条件断言；
        // notdef 断言仅在有覆盖的系统上生效（环境门见上一个测试）
        let probe = "Ab中cd";
        let gate = system_covers_text(probe);
        let shaper = shaper();
        let shaped = shaper
            .shape(probe, &style(&["sans-serif"], 16.0))
            .expect("mixed-script shaping");
        assert_eq!(shaped.glyphs.len(), 5, "每个字符一个字形");
        assert!(
            shaped
                .glyphs
                .iter()
                .all(|glyph| (glyph.font as usize) < shaped.fonts.len()),
            "font 下标均在字体表内"
        );
        if gate {
            let notdef = shaped.glyphs.iter().filter(|glyph| glyph.id == 0).count();
            assert_eq!(notdef, 0, "混排文本不得有 notdef");
        }
    }

    // ===== @font-face 注册表（K4 Web 字体 T2）=====

    /// 读取 tests/data 下的测试字体字节（Space Mono，OFL，见目录 README）。
    fn test_font(name: &str) -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/data")
                .join(name),
        )
        .unwrap_or_else(|error| panic!("read test font {name}: {error}"))
    }

    fn face_descriptor(family: &str, weight: u16, style: FontStyle) -> FontFaceDescriptor {
        FontFaceDescriptor {
            family: family.to_owned(),
            weight,
            style,
        }
    }

    #[test]
    fn registry_rejects_invalid_font_data_and_empty_family() {
        let registry = FontRegistry::new();
        assert_eq!(
            registry.register(
                b"not a font".to_vec(),
                face_descriptor("NextyInvalid", 400, FontStyle::Normal)
            ),
            Err(FontRegistryError::InvalidFontData),
            "非字体字节必须被拒绝且不入注册表"
        );
        assert_eq!(
            registry.register(
                test_font("SpaceMono-Regular.ttf"),
                face_descriptor("   ", 400, FontStyle::Normal)
            ),
            Err(FontRegistryError::EmptyFamily),
            "空族名描述符必须被拒绝"
        );
    }

    #[test]
    fn registry_multi_weight_nearest_match() {
        // 同一 CSS 族名注册两个 face（声明名与字体内部族名解耦是
        // @font-face 的核心语义）：Regular 声明为 400、Bold 声明为 700。
        let registry = FontRegistry::new();
        let regular = test_font("SpaceMono-Regular.ttf");
        let bold = test_font("SpaceMono-Bold.ttf");
        registry
            .register(
                regular.clone(),
                face_descriptor("NextyWeightTest", 400, FontStyle::Normal),
            )
            .expect("register regular");
        registry
            .register(
                bold.clone(),
                face_descriptor("NextyWeightTest", 700, FontStyle::Normal),
            )
            .expect("register bold");
        let resolver = FontResolver::with_registry(&registry);
        let family = ["NextyWeightTest".to_owned()];

        // 期望 400 / 700 精确命中对应 face
        let resolved_400 = resolver
            .resolve_font_with(&family, 400, FontStyle::Normal)
            .expect("weight 400 resolves");
        assert_eq!(
            resolved_400.data().as_ref(),
            regular.as_slice(),
            "期望 400 命中 Regular face"
        );
        let resolved_700 = resolver
            .resolve_font_with(&family, 700, FontStyle::Normal)
            .expect("weight 700 resolves");
        assert_eq!(
            resolved_700.data().as_ref(),
            bold.as_slice(),
            "期望 700 命中 Bold face"
        );

        // CSS Fonts §5.2 就近匹配：期望 600 → ≥600 升序 → 700
        let resolved_600 = resolver
            .resolve_font_with(&family, 600, FontStyle::Normal)
            .expect("weight 600 resolves");
        assert_eq!(resolved_600.data().as_ref(), bold.as_slice());
        // 期望 500 → 400–500 区间先查 ≥500（无 500±），再 <500 降序 → 400
        let resolved_500 = resolver
            .resolve_font_with(&family, 500, FontStyle::Normal)
            .expect("weight 500 resolves");
        assert_eq!(resolved_500.data().as_ref(), regular.as_slice());
        // 期望 300 → <400 降序（无），再 >300 升序 → 400
        let resolved_300 = resolver
            .resolve_font_with(&family, 300, FontStyle::Normal)
            .expect("weight 300 resolves");
        assert_eq!(resolved_300.data().as_ref(), regular.as_slice());
    }

    #[test]
    fn registry_style_italic_preferred() {
        // 描述符驱动匹配：Regular 字节声明为 normal、Bold 字节声明为
        // italic，同族两 face 按 style 匹配（weight 同为 400）。
        let registry = FontRegistry::new();
        let regular = test_font("SpaceMono-Regular.ttf");
        let bold = test_font("SpaceMono-Bold.ttf");
        registry
            .register(
                regular.clone(),
                face_descriptor("NextyStyleTest", 400, FontStyle::Normal),
            )
            .expect("register normal face");
        registry
            .register(
                bold.clone(),
                face_descriptor("NextyStyleTest", 400, FontStyle::Italic),
            )
            .expect("register italic face");
        let resolver = FontResolver::with_registry(&registry);
        let family = ["NextyStyleTest".to_owned()];

        let italic = resolver
            .resolve_font_with(&family, 400, FontStyle::Italic)
            .expect("italic desired resolves");
        assert_eq!(
            italic.data().as_ref(),
            bold.as_slice(),
            "italic 请求命中 italic 声明 face"
        );
        let normal = resolver
            .resolve_font_with(&family, 400, FontStyle::Normal)
            .expect("normal desired resolves");
        assert_eq!(normal.data().as_ref(), regular.as_slice());

        // 族内只有 normal face 时，italic 请求退到 normal face（CSS Fonts §5.2）
        let only_normal = FontRegistry::new();
        only_normal
            .register(
                regular.clone(),
                face_descriptor("NextyItalicFallback", 400, FontStyle::Normal),
            )
            .expect("register");
        let resolved = FontResolver::with_registry(&only_normal)
            .resolve_font_with(&["NextyItalicFallback".to_owned()], 400, FontStyle::Italic)
            .expect("italic falls back to normal face");
        assert_eq!(resolved.data().as_ref(), regular.as_slice());
    }

    #[test]
    fn registry_miss_falls_back_to_system_fonts() {
        let registry = FontRegistry::new();
        registry
            .register(
                test_font("SpaceMono-Regular.ttf"),
                face_descriptor("NextyHitFamily", 400, FontStyle::Normal),
            )
            .expect("register");
        let resolver = FontResolver::with_registry(&registry);

        // 注册族命中注册字节（旧签名路径，默认 400/normal）
        let hit = resolver
            .resolve_font(&["NextyHitFamily".to_owned()])
            .expect("registered family resolves");
        assert_eq!(
            hit.data().as_ref(),
            test_font("SpaceMono-Regular.ttf").as_slice(),
            "注册族必须命中注册字节"
        );

        // 未注册族走既有系统字体回退；同一解析器两次调用结果一致
        let miss = resolver
            .resolve_font(&["nexty-no-such-family-xyz".to_owned()])
            .expect("system fallback");
        assert!(!miss.data().is_empty(), "回退字体字节非空");
        let miss_again = resolver
            .resolve_font(&["nexty-no-such-family-xyz".to_owned()])
            .expect("system fallback again");
        assert_eq!(miss.id(), miss_again.id(), "同一解析器的回退结果必须确定");
    }

    #[test]
    fn registered_family_shapes_with_registered_font() {
        let registry = FontRegistry::new();
        let regular = test_font("SpaceMono-Regular.ttf");
        registry
            .register(
                regular.clone(),
                face_descriptor("NextyShapeTest", 400, FontStyle::Normal),
            )
            .expect("register");
        let shaper = ParleyTextShaper::with_registry(&registry);

        let shaped = shaper
            .shape("Hello Web Font", &style(&["NextyShapeTest"], 16.0))
            .expect("shape with registered family");
        assert!(!shaped.glyphs.is_empty());
        assert_eq!(shaped.fonts.len(), 1, "单族单 face 文本只应命中一个字体");
        assert_eq!(
            shaped.fonts[0].data().as_ref(),
            regular.as_slice(),
            "整形产出的字体必须是注册字节（@font-face 命中）"
        );
        assert!(shaped.width > 0.0);
        assert!(
            shaped
                .glyphs
                .iter()
                .all(|glyph| (glyph.font as usize) < shaped.fonts.len()),
            "font 下标均在字体表内"
        );
        assert_eq!(
            shaped.glyphs.iter().filter(|glyph| glyph.id == 0).count(),
            0,
            "Latin 文本不得产出 notdef 豆腐块"
        );

        // 同一注册字体的第二次整形共享同一份字节快照——注册字体走与系统
        // 字体相同的 (blob id, face index) 缓存路径
        let again = shaper
            .shape("Second run", &style(&["NextyShapeTest"], 16.0))
            .expect("second shape");
        assert!(
            std::sync::Arc::ptr_eq(shaped.fonts[0].data(), again.fonts[0].data()),
            "注册字体的字节快照必须跨整形共享"
        );

        // 注册表跨消费方共享：解析器与整形器拿到同一 blob id
        let resolved = FontResolver::with_registry(&registry)
            .resolve_font(&["NextyShapeTest".to_owned()])
            .expect("resolver hit");
        assert_eq!(
            resolved.id(),
            shaped.fonts[0].id(),
            "注册字体 blob id 跨消费方一致"
        );

        // 未接入注册表的整形器不消费注册面（with_registry 是唯一接线点）
        let plain = ParleyTextShaper::new();
        let fallback = plain
            .shape("Hello", &style(&["NextyShapeTest"], 16.0))
            .expect("unregistered shaper falls back to system");
        assert_ne!(
            fallback.fonts[0].id(),
            shaped.fonts[0].id(),
            "未接线整形器不得命中注册字体"
        );
    }
}
