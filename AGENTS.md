# AGENTS.md

This file provides guidance to AI coding agents (Codex CLI / Claude Code / etc.) when working with code in this repository.

## Project: Nexty

MusKitty 的**不造轮子**分支。MusKitty 从零手写浏览器核心模块；Nexty 反过来——**能复用成熟 crate 就不自己实现**，只把精力留给真正没有现成轮子的部分。

行为 ground truth 仍是 WHATWG 规范与 WPT 测试套件。Chromium 源码仅作参考。

当前状态：8 个 facade crate 已在根 `Cargo.toml` 注册，外部依赖已接线，`cargo check --workspace` 零 warning；依赖门禁（`cargo deny check`）与第三方依赖清单（`cargo about`）已跑通。

已落地八层（全部骨架 crate 就位，管线端到端可跑）：

- `nexty-dom`（v0.1.4）——自研 arena DOM：节点数据层、树变更算法（pre-insert/insert/remove/replace/clone/normalize）与文档模式控制。**事件模型（K1）**：`event` 模块（feature `event`）——`Event`/`EventTarget`（监听器存 Document 侧 `ListenerRegistry`，句柄身份）、捕获/target/冒泡三阶段传播（target 上捕获与非捕获监听器按注册顺序混触发）、`stopPropagation`/`stopImmediatePropagation`/`preventDefault`（DOM §2.9 简化档：无 shadow/trusted/passive/once），数据面 UIEvent/MouseEvent/KeyboardEvent/WheelEvent/FocusEvent（FocusEvent 仅类型面，K10 补派发点），`EventLoop` FIFO 任务队列（派发中 post 不重入，由 `TaskContext` 类型结构性保证），生命周期事件类型 `DOMContentLoaded`/`load`。**输入边界防护**：节点携带树深度（`link_before` 单点维护、重挂重算、模板内容片段独立起算），`MAX_TREE_DEPTH = 512` 与 `Document::depth` 供下游防线使用。
- `nexty-html`（v0.1.5）——html5ever 的 `TreeSink` 桥接到 arena DOM，提供 `parse_document` 与 `parse_fragment`（含片段上下文命名空间）；子资源清单收集（`collect_style_resources` 收集 `<link rel=stylesheet>` 与 `<style>`、`collect_image_resources` 收集 `<img>`，只收集不解析）。**输入边界防护（深度主防线）**：`TreeSink::append` / `append_before_sibling` 挂接前检查父节点深度，超过 `MAX_TREE_DEPTH` 即丢弃该节点——超深内容在解析入口截断（已知偏差「超深内容截断」：按「资源防护降级」语义消失，不按 CSS 语义保留），树深度全局有界，下游递归遍历无需各自设防。注意 html5ever 对每个起始标签都在 open elements 栈上线性扫描，解析耗时随深度平方增长（上游固有特性，留给性能轮）。
- `nexty-css`（v0.1.18）——cssparser/selectors 封装 + 自研 cascade：属性值解析（hex/rgb()/hsl()/命名色、font-size/weight、font-family、盒模型 margin/padding/border/width/height、line-height、box-sizing/overflow/min-width/min-height/max-width/max-height、flex-grow/flex-basis、white-space/overflow-wrap/word-break、letter-spacing/word-spacing/text-indent/tab-size）、选择器匹配（Selectors 4，`:is()`/`:where()`/`:has()`/`:nth-child(of)`，HTML 大小写规则；**规则桶索引（M5）**：按选择器必要条件 id/class/tag 分桶，只跳过必要条件已失败的选择器，语义由「桶化 vs 全量扫描」逐元素对照测试钉住）、样式表解析（CSS Syntax §5 错误恢复 + 简写展开：margin/padding/border 系、1–4 值顺时针、`border` 与 `font` 按 `||`/`||`+必选语法（`font` 缺省成分重置 initial，`small-caps`/宽度成分接受后丢弃、系统字体关键字无效）；`@media` 按视口条件求值，不匹配的规则不参与级联）、cascade（UA/author 双 origin 重要性桶 → 内联样式 → 特异度 → 源顺序，多表按源顺序合并级联，UA 样式表见 `html_ua_stylesheet`；**热路径零克隆（M5）**：matched 持声明引用、`InlineStyleCache` 按 style 属性原文缓存解析结果跨 recascade 复用——`compute_document_styles_in_with_cache`）与 computed style（继承/initial、`em`/`%`/绝对尺寸关键字表、`bolder`/`lighter` 映射表、CSS-wide 关键字、视口单位 `vw`/`vh`/`vmin`/`vmax` 在盒级长度属性上按传入视口折算——`compute_document_styles_in`/`cascade_in` 带视口，无视口时原样保留由消费方降级；letter-spacing/word-spacing/text-indent/tab-size 四属性均继承，em 按自身字号折算，text-indent 百分比保留给 layout）。**输入边界防护（数值卫生）**：`MAX_CSS_LENGTH = 1e7`（`MAX_CSS_LENGTH` 公开常量），解析出口滤除 `inf`/`NaN`（整条声明无效）并 clamp 有限巨数，计算出口（字号乘积 / 行高折算 / 视口折算）再钳一次——非有限值不进布局（已知偏差「超限长度 clamp」：超出 1e7 的有限值按 UA clamp 处理）。**绘制扩展属性（K2）**：text-decoration 系（line/style/color + 简写）、z-index、border-radius（1–4 值，`/` 椭圆语法偏差）、linear-gradient（角度 + 色标解析期归一 0–1 非递减）、background 简写兼容渐变值；UA 样式表补 `a { text-decoration: underline }`。**@font-face（K4）**：at-rule 解析（family 必需/src url()+format()/local() 有序、weight 数字或 normal/bold、style normal|italic、display 存储恒 swap 简化），`Stylesheet::font_faces()` 导出。新增偏差（text-decoration 单关键字/仅 solid 渲染/无 thickness、椭圆语法、多重背景、url() 背景图、woff2、unicode-range、`:link`/`:visited` 以 `a` 近似）见 `crates/nexty-css` 模块文档；`var()`、user origin、伪元素、`rem`/`ex`/`ch` 等见 `crates/nexty-css` 模块文档的偏差清单。
- `nexty-network`（v0.1.6）——reqwest blocking 实现 `NetworkFetcher`（`Send + Sync`，供并发抓取）：GET/HEAD、`Request.headers` 请求头逐条应用、错误映射（URL 解析/协议 → `InvalidUrl`、超时 → `Timeout`、其余 → `Transport`）；`resolve` 以文档 URL 为 base 解析相对子资源地址（**`url` crate 直依赖（K3），resolve 特性不再借 reqwest 的 `Url` 类型**）；**字符编码探测（K3，feature `decode`）**：`decode_document` 四层优先级——HTTP Content-Type charset > BOM（UTF-8/UTF-16LE/BE）> meta charset 预扫（前 1024 字节，WHATWG §13.2.3.5 简化档）> 默认 UTF-8；**重定向（K3）**：显式 policy（≤10 跳、防环，`MAX_REDIRECTS`），`Response.final_url` 透出最终落点（子资源 base 语义）；**HTTP 缓存（K3，feature `cache`）**：`CachedFetcher` 装饰器——Cache-Control max-age/no-store/no-cache（再验证）、ETag→If-None-Match、Last-Modified→If-Modified-Since、304 复用、Vary 非 UA 字段不缓存、容量 LRU + 单条体积上限；回环离线测试覆盖全部错误路径、请求头到达、重定向、缓存与解码矩阵。
- `nexty-text`（v0.1.9）——parley 实现 `TextShaper`：fontique 选字（CSS 字体族列表语义 + 按脚本回退——主族缺字形的码点交给系统字体，CJK 在 Windows 命中 YaHei 等）、harfrust 整形，CSS px 单位；**Web 字体注册表（K4）**：`FontRegistry`（会话级 @font-face，`register(data, FontFaceDescriptor)` 注册即生效）+ `FontResolver::with_registry`/`resolve_font_with`（weight/style 精确化）+ `ParleyTextShaper::with_registry` 接线——匹配全权委托 fontique（CSS Fonts §5.2 完整实现），`(blob id, face index)` 缓存路径原样复用；测试字体 SpaceMono（OFL）入库 `tests/data/`；偏差（同名系统字体合并序、woff2、unicode-range）见模块文档；`ShapedText.fonts` 记录本次整形实际用到的字体、`Glyph.font` 标注每个字形归属（供光栅层用正确字体渲染，修复回退字形被主字体渲染的豆腐块）、`Glyph.cluster` 标注该字形所属 cluster 的源文本字节偏移（LB9/LB10 组合序列并入基字符）；`FontMetrics` 供 strut 计算，`FontResolver::resolve_font` 给光栅层提供字体字节（实例以 blob id 判等）。**性能（T1 基准发现的阻塞缺陷）**：`ParleyTextShaper` 按（blob id, face index）缓存 `ResolvedFont`——同一字体一份字节快照全进程共享（原先每次整形复制整个字体文件，CJK 回退字体 ≈20 MB/chunk 曾把 2,000 段页面的布局期内存推到 9 GB 后 abort）。单行整形（white-space 折叠归 layout 层）。注意：parley 0.11 的整形后端是 harfrust 而非 ADR 所列 swash，swash 已移出依赖树。
- `nexty-paint`（v0.1.8）——vello_cpu 实现 `Rasterizer`：`FillRect`（src-over + 覆盖率抗锯齿）、`StrokeRect`（只描边）、**圆角与渐变指令（K2）**：`FillRoundedRect`/`StrokeRoundedRect`（kurbo `RoundedRect::to_path`；CSS Backgrounds §5.5 重叠半径比例缩减与描边中心线内缩 `width/2` 在 facade）、`FillGradient`（`peniko::Gradient`，sRGB/premultiplied alpha/Pad 外推与 CSS 对齐；radii 全 0 即直角，单指令覆盖渐变×圆角组合；`Scene::translated` 同步平移端点——渐变坐标平移遗漏则滚动错位，单测钉住）、`DrawText`（字体数据由 text 层整形结果直接下发 `fonts`，按 `glyph.font` 分组用对应字体渲染字形——回退后一条 run 可跨多字体）、`DrawImage`（已解码 RGBA8 按矩形下发，1:1 整数缩放；**像素按 `Arc<Image>` 共享（M1）**——`Scene.images` 持引用计数，构建/发送/光栅全程零像素拷贝）；`Scene::translated`（M6）生成整体平移的场景副本（字形与图片像素引用计数共享，滚动路径的 origin 平移下放到发送阶段）；`decode` 提供最小图片解码路径（png 可选 feature，RGB/灰度归一为 RGBA8，不支持格式返回 `DecodeError` 而非 panic），**输入边界防护（解码预检）**：先 `into_dimensions()` 只读头部校验尺寸（边长 ≤ 16384 且 `宽 × 高` ≤ 2^26 ≈ 256 MB RGBA8，`MAX_DIMENSION` / `MAX_PIXEL_AREA` 公开常量），预检不通过返回 `SizeUnsupported` 且不做任何像素分配——分配失败是 abort，panic 兜底兜不住；facade 输出非预乘 RGBA8。GPU 后端 `vello_hybrid` 依赖 wgpu surface 由 chrome 层提供后接入，`Rasterizer` trait 即双后端接缝。
- `nexty-layout`（v0.1.29）——盒级几何（block / flex / grid / absolute / float）由 `taffy` 接管（MIT，Servo/Blitz 采用；`taffy` 依赖常驻，feature 只切能力开关），**inline / 文本布局与 table 仍自研**。taffy 管线三步：`style_map`（`ComputedStyle → taffy::Style` 单向映射，覆盖 display/position/box-sizing/inset/尺寸/margin/padding/border/flex-*/grid-*/gap；映射前做数值卫生兜底——非有限长度/百分比降级 auto/0，污染隔离在单元素）、`tree_build`（按 CSS Display §3 生成盒树，`none` 跳过、`contents` 提升、`<img>` 以自然尺寸建叶，维护 `NodeId ↔ taffy Node` 双向映射；`build_box` / `collect_children_boxes` 入口对 DOM 深度做 `debug_assert`（纵深防御，主防线在 html 解析入口截断）；行内内容经 `inline` 模块收集后包装成**匿名块 run 叶节点**（行内子树中的块级子元素按 CSS 2.2 §9.2.1.1 把 run 拆成多段、各自成匿名块，块级子盒按普通块布局——不再丢弃），measure function 手动构造 `LayoutOutput`——跑自研断行 `inline::build_lines` 提供内在尺寸并上报**首行 baseline**，使 flex/grid `align-items: baseline` 生效；`margins_can_collapse_through` 语义与 `compute_leaf_layout` 对齐：仅实测高度 0 可穿透）、`block`（`compute_layout` 解算后读回 `taffy::Layout` 组装 `Fragment`，行盒并入块容器或承接为匿名块片段；`text-align` 的 center/right 在行盒并入时施加——整行平移，溢出行防负偏移）。taffy 覆盖：块级流（margin 折叠 §8.3.1、宽度解析 §10.3.3、min/max 收束 §10.4/§10.7）、flex（含 wrap/shrink/完整对齐值）、grid（轨道/auto-flow）、绝对定位（inset）、**float（`float_layout` feature：浮动放置 / `clear` / BFC 槽位避让 / 容器高度贡献，CSS 2.2 §9.5）**。**浮动环绕修正 pass（layout 0.1.28–0.1.29）**：taffy 的浮动实现只对有子内容可下钻的盒子收缩可用宽，measure 叶（run 叶）拿不到浮动区间——首轮解算后按 taffy 已放置的浮动矩形逐行收缩重排 run 叶行盒（`build_lines` 的 `LineConstraints.floats`），行数变化时回写叶定高再解算一轮（浮动尺寸与文本无关，一次迭代固定点）；浮动矩形沿祖先 BFC 传播到后代 run 叶（跨 `<p>` 等块边界），遇新建 BFC（float / overflow 非 visible / absolute / flow-root）截断。偏差：行按行顶 y 取可用区间（跨入更窄区间不细分）、浮动只按文档序判定「前置」。自研保留：行内断行（strut §10.8、混合样式 run、**`white-space` 六种取值**、`<br>`/保留换行硬断、UAX#14 贪心断行）、**CSS Text 放置期修正**（letter-spacing / word-spacing 按簇施加——行末簇字距不计行宽；`text-align: justify` 的 `auto` 近似——软折行非末行把自由空间均匀分配到折叠空格与同块簇间断点（行尾裁剪按字形 x 计量，末字形与空格间的字距间隙计入可见宽，保证各行右缘一致）；`text-indent` 首行缩进——每容器一次、百分比按行盒包含块（块容器自身内容盒宽）折算〔与 Chromium 一致，Edge 实机对照确认〕、不进 min/max-content；`tab-size` 保留制表推进到 `tab-size`×空格宽的下一整数倍；pre-wrap 行尾空格悬挂——不占行宽可溢出、字节区间保留供复制/选区；全部为放置期修正、不改整形结果、不产生/消灭断点；偏差清单见 `inline/mod.rs` 模块文档：跨行内元素边界不加距、justify 不做 inter-word 收缩与 `text-justify` 分档、pre-wrap 空格不设空格机会、`each-line`/`hanging` 未实现）、行内原子盒（`inline-block`/`inline-flex`/`inline-table` 按 shrink-to-fit §10.3.7 独立布局，内在尺寸测量在 `tree_build::InlineRunNode::measure`）、替换元素尺寸解析（`attribute_px` 按 WHATWG §2.4.4.2 非负整数规则解析——只收数字、`40.5` → 40，结果须有限且 ≤ `MAX_CSS_LENGTH`，否则回落自然尺寸）、`build_lines` 包含块宽度 NaN 时按 0 降级（MaxContent 的 +inf 是合法「不折行」语义，保留）。UAX#14（Unicode 18.0.0）断点 / glyph cluster 映射 / min-content / span 切分 API（`break_opportunities` / `cluster_map` / `min_content_width` / `split_spans`，逐条比对官方 19,346 条 `LineBreakTest`，见 `tests/inline_conformance.rs`）**已接入 `build_lines`**：文本项只整形一次，按 `cluster_map → break_opportunities → split_spans` 切段后贪心放置；`word-break` / `overflow-wrap` 由 computed style 映射为 `BreakOptions`（`break-word` 的紧急断点不参与 min-content）。**性能（H4/M10/M12，layout 0.1.25–0.1.26）**：逐 chunk 宽度无关分析缓存（shaping / 簇映射 / 断点切分按 chunk 缓存于 run 节点，跨 measure 宽度档位复用，行为由片段树指纹测试钉住逐字节一致）、布局级 `MetricsCache`（同一 TextStyle 整次布局只整形一次 NBSP 探针——基线实测 text-heavy 64,818 次 → 10 次）、`root_style` 按 `ctx.styles` 借用（Cow）、run 叶行盒缓存命中 take 语义、`ensure_style` 按 NodeId 身份免全字段比较；`remap_glyph` 对越界字体下标跳过不 panic（M12，畸形整形器输出单测钉住）。conformance 代码按三分离拆为 `inline/{cluster_map,break_ops,span_split}.rs` 与 `inline/uax14/{classes,data,rules}.rs`。**文本选区**：`selection` 模块（`TextMap`）以「文本节点 + 源字节偏移」为端点，提供 glyph cluster 对齐的命中（不切 grapheme；命中/吸附按「y 距离 → x 距离 → 深度」选行并向嵌套原子盒内部行下钻——内部行不被外层行遮蔽）、逐行选区矩形、UAX#29 选词与复制文本重建（软折行不插换行、`<br>`/块边界插 `\n`、`pre` 保留空白）；`Collector` 保留文本节点身份与逐字符源映射，`TextRun` 携带 `text` / `source_map` / 实际渲染字节区间，`LineFragment` 记录行首软/硬断行。片段树输出（边框盒 + 盒模型 + 文本行 + run 字体表 + 源映射）供 paint 与选区消费。盒级布局完备度约 35% → 约 75–80%（grid / 多行 flex / 绝对定位借 taffy 补齐，CSS Text 放置期修正落地）。**未实现**：table、`position: fixed` 视口锚定与「最近定位祖先」包含块搜索（taffy 相对树内父盒定位）、`text-align-last`/`text-justify`、多行 baseline 组、`vertical-align` 非 baseline、bidi/RTL（偏差清单见 `crates/nexty-layout/src/block.rs`、`inline/mod.rs`、`selection.rs` 与 `style_map.rs` 模块文档）。**栈预算**：512 层布局递归（taffy 解算 + 片段装配）实测需 5–6 MB 栈（约 10 KB/层），布局只跑在主线程——Windows PE 主线程栈默认仅 1 MiB，512 层页面曾直接栈溢出拖垮进程（实机验收抓到），已由 `nexty-chrome/build.rs` 把主线程栈预留提到 16 MiB（`/STACK`，MSVC）；chrome 测试的全链加固回归用例把该约束钉在显式 32 MB 线程里。模块功能按 feature 拆分（`block`/`inline`/`flex`/`replaced`/`taffy-map`/`grid`/`absolute`/`selection`，`table` 预留），见下方Feature 映射表。
- `nexty-chrome`（v0.1.30）——浏览器外壳：`pipeline`（HTML → 级联 → 布局 → 片段树 → paint 显示列表，含 CSS 边框转四条填充带、**圆角/渐变/装饰带 emit（K2）：圆角解析（百分比按边框盒轴）、四边等宽边框走圆角描边否则直角降级、渐变端点按 css-images-3 公式（四正向精确轴向）+ 色标映射、text-decoration 经验度量带（0.08em/0.06em）、z-index 基础定序两遍收集（定位层后置、整数排序）、画布背景传播 `canvas_background_commands`（CSS 2.2 §14.2：html 优先 body 回落回落白）**、文本 run 绝对坐标与**行偏移累加**（run 携带字体表，`recascade` 以实际视口重级联——视口单位在 computed 阶段折算、resize 后取新值；`Page` 保留作者样式表供重级联并持有 `InlineStyleCache`（M5：recascade 复用内联样式解析）、**页面坐标系场景缓存 `SceneCache` 与 resize 合帧器 `ResizeCoalescer`（M6）**——滚动/滚动条拖拽只做 `Scene::translated` origin 平移零 emit，导航/重排/选区变化推进代次或高亮键失效重建，连发 Resized 折叠到下一次 RedrawRequested 每帧至多一次重排；**图片像素零拷贝（M1）**：`Page.images`/`Scene.images` 均为 `Arc<Image>`，`RenderThread::render` 按所有权收 `Scene` 不再整树克隆）、`bench`（性能基准共享支撑：确定性语料生成 + `CountingShaper` 整形调用计数 + BENCH_MODE 门控，基准正文 `benches/pipeline.rs` 跑在显式 32 MB 栈线程，基线见 [docs/2026-10-25-perf-baseline.md](docs/2026-10-25-perf-baseline.md)；**内置机器无关回归断言**——resize 合帧 runs=事件/每帧比、滚动 emit=0（M6）、单次布局 shape≈1×chunks（H4）、metrics≤10，破坏即 bench 非零退出；**CI 手动触发入口 `.github/workflows/bench.yml`**（workflow_dispatch，`BENCH_SCROLL_FRAMES` 控滚动风暴帧数/0 跳过，数字入 Job Summary 与 artifact））、行内/块级图片与行内原子盒的 `DrawImage`/递归下发、多表级联与子资源并发抓取编排、`hit_test`/`hit_test_ex`/`hover_kind` 悬停语义与 `link_target` 点击命中、`document_title` 提取、**选区高亮按行插在页面背景与文字之间**）、`selection`（页面文本选区状态机：拖拽、Shift+点击、双击选词、Ctrl+A 全选；端点按源位置保存，resize 后重新解析；`Ctrl+C` 经 `arboard` 写系统剪贴板，失败不 panic）、`history`（会话历史栈：push/go_back/go_forward，新导航截断前进分支）、`render`（渲染隔离线程 + `catch_unwind` 兜底，panic 转 `RenderError::Panicked` 且线程存活可继续服务；**异步呈现（T9 实机抓到并修复）**：主线程 request 光栅作业即返回，线程完成后经 EventLoopProxy 发 FrameReady 唤醒上屏、积压结果只 present 最后一份——同步阻塞会让巨页帧（数十秒）把窗口拖入系统「无响应」，winit 0.30 把 WM_PAINT 直接映射为 RedrawRequested，配合 DWM 失效重发形成无限重绘风暴）、`ui`（自绘工具带状态机：后退/前进/刷新按钮（可用标志禁用态）、地址栏**编辑态**——光标/选区/`←→`/Home/End/Shift 扩选/Ctrl+A/Backspace/Delete、点击=聚焦+全选、超宽文本按光标横向滚动、`shortcut()` 纯映射 Chrome 快捷键含 Ctrl+A/C；**整形缓存（M7）**：按钮字形一次整形、URL 按（文本, 字号）缓存未变不 reshape、光标/选区前缀宽度按已整形字形 cluster 前缀累加零整形，计数测试钉住「URL 不变 redraw 0 次整形、每键 ≤1 次」）、`scrollbar`（滚动几何 + 拖拽状态机：thumb 抓取偏移拖动、轨道点击翻页、悬停配色）、`app`（winit 0.30 + wgpu 29 surface，Pixmap→纹理→全屏 blit 呈现，Rgba8Unorm 优先/BGRA 换色回退；页面滚动（滚轮/PageUp/PageDown/Home/End/方向键）+ 可拖滚动条、resize 合帧重排、页面文本拖拽/双击/Shift+点击选择与 Ctrl+A/C、链接在释放且未拖动时导航、点击页面让地址栏失焦、导航清空选区、悬停系统光标（链接手型/文本 I 形）、Ctrl+L/Ctrl+R/F5/Esc 与 Alt+方向键、`Arc<dyn NetworkFetcher>` 接驳缝 + 30s 超时 + User-Agent、窗口标题随 `<title>`；pad_rows 行对齐 staging 缓冲按（宽, 高, 步幅）复用（M6）；**输入边界防护**：主文档响应体预算 10 MB、错误串收敛为「错误类别 + 状态码」（不回显 reqwest 原始错误——其内含目标 URL）、错误页 HTML 对 URL 与错误文本做实体转义（防标记注入与第三方子资源请求）；**无头环境不可自动化测试**，实机验证；**DOM 事件接线（K1）**：交互事件全部「先派发后默认」（mousedown/mouseup/click、滚轮、键盘/快捷键），`app::events` 纯函数接线层 + `EventLoop` 主线程排空，生命周期 DCL/load 经管线钩子派发，页面传输改 Send 数据面（后台只传字节，Document 主线程构建——!Send 约束）、**导航编排（K3）**：CachedFetcher 缓存栈、主文档四层解码、子资源 base=final_url、地址栏规范化（无 scheme 补 http://，ADR [docs/decisions/2026-10-31-addressbar-url.md](docs/decisions/2026-10-31-addressbar-url.md)）、**字体编排（K4）**：`FontOrchestrator` collect/fetch/register 会话去重、shaper `with_registry` 接线、字体就绪世代失效恰一轮重排）、`resources`（子资源编排 + **响应体字节预算**：文档/样式表 10 MB、图片 50 MB、字体 20 MB（K4 `MAX_FONT_BODY_BYTES`），`MAX_DOCUMENT_BODY_BYTES` 等公开常量，超预算按降级语义跳过——巨响应的分配失败是 abort，panic 兕底兜不住；解码侧像素面积另由 paint 预检控制；同 URL 多节点共享一份像素；`fetch_document` 主文档编排入口（抓取→预算→四层解码→规划解析→子资源）、字体 collect/fetch 会话去重）。入口 `cargo run -p nexty-chrome --bin nexty`。

`nexty-html` 带 WPT tree-construction 比对 harness：语料钉在 `web-platform-tests/wpt` commit `5cd8e3fa`，当前 **1854/1959 通过**。剩余 105 条已逐条入基线，分两类：**88 条语料过时**——`processing-instructions.dat` 等期望产出 PI 节点，而现行 WHATWG §13.2.5 已无处理指令词法状态（`<?…>` 在 tag open state 走 bogus comment），html5ever 的产出与规范一致；**17 条非语料问题**——11 条 html5ever 树构建未跟进规范（`in select` 模式、`<selectedcontent>` 克隆、`<template>` 的 frameset-ok/form 指针语义），6 条需 JSRT 的 scripted 用例。分词器不换：MusKitty 分词器的处理指令状态实现的是规范已删除的特性，评估与否决理由见 [docs/decisions/2026-10-01-muskitty-tokenizer-rejected.md](docs/decisions/2026-10-01-muskitty-tokenizer-rejected.md)。比对方式与基线见 `crates/nexty-html/tests/tree_construction.rs`。

八层骨架均已有内部实现，管线 network→html→dom→css→layout→paint→chrome 端到端连通（`cargo run -p nexty-chrome --bin nexty` 起窗口，地址栏输入 URL 导航，支持滚动与滚动条拖拽、链接点击、悬停光标、地址栏编辑与快捷键、后退/前进/刷新、页面文本选择与复制）。**批次 1 已落地（K1 事件模型 / K2 绘制扩展 / K3 导航网络 / K4 Web 字体，见 [docs/plans/2026-10-31-round-batch1-parallel.md](docs/plans/2026-10-31-round-batch1-parallel.md)）**。后续深化方向：M3/M4 chrome 正确性小轮（加载竞态 / wgpu error handler）、layout 补齐 table（整轮；grid/定位/多行 flex/float 已由 taffy 落地）、容器滚动与视口裁剪/增量光栅（K6/K9）、vello_hybrid GPU 后端接入。**性能轮已落地**（H4 整形与断行缓存、M1 图片零拷贝、M5 级联桶索引、M6 resize 合帧与滚动场景缓存、M7 工具带整形缓存、M10/M12 克隆链与畸形输入；基线与实测数字见 [docs/2026-10-25-perf-baseline.md](docs/2026-10-25-perf-baseline.md)，遗留瓶颈：滚动光栅化需视口裁剪/增量光栅，后续大项）。CSS Text 排版细化（justify / text-indent / letter-spacing / word-spacing / tab-size / pre-wrap 悬挂）已落地。分层与 crate 选型见 [docs/decisions/2026-10-01-crate-selection.md](docs/decisions/2026-10-01-crate-selection.md)。

## Build & Test Commands

workspace 根目录（`Ink-dark/Nexty`）：

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

# 依赖门禁与许可证清单（引入/升级任何依赖后必跑）
cargo deny check                         # 白名单见根目录 deny.toml，扫传递依赖
cargo about generate --format json -o .workbuddy/about.json# 供注入脚本读取
cargo about generate about.hbs -o docs/dependencies.html   # 重新生成第三方依赖清单
# 上面两步之后必须再跑注入，补cargo-about 不提供的两个区块：
#   NOTICE/归属声明清单、多重许可（SPDX 含 AND）人工核对表。
# 这两块无法写在 .hbs 里——cargo-about 的模板上下文只有
# overview/licenses/crates，没有 notices，也没有「同一 crate 跨几个 license 组」。
# 脚本在 .workbuddy/（本机代理数据，不入库），流程可重复执行且幂等：
python .workbuddy/collect_notices.py && python .workbuddy/inject.py

# 单个 crate 目录下（例如 crates/nexty-xxx/）
cargo check                             # 检查该 crate（必须零 warning）
cargo test                              # 运行该 crate 全部测试
cargo test --lib                        # 只跑 lib tests
cargo test --tests                      # 只跑 integration tests
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
```

新增 crate 时在根 `Cargo.toml` 的 `members` 里登记，crate 之间用 `path = "../nexty-xxx"` 互引。

## Architecture

```
Nexty/                                  # 主仓库 (Ink-dark/Nexty)，workspace 协调中心
├── Cargo.toml                          # workspace 成员：8 个 facade crate
├── AGENTS.md                           # 硬约束指南（本文件）
├── deny.toml                           # cargo-deny 依赖门禁（许可证白名单 + advisory 放行清单）
├── about.hbs                           # cargo-about 依赖清单模板
├── README.md                           # 项目 README
├── .gitignore
├── crates/                             # workspace member：一层一个 facade crate
│   ├── nexty-html/                     #   html5ever 封装 → 自有 DOM 的 TreeSink
│   ├── nexty-dom/                      #   自研 arena DOM；后续 JSRT 绑定与 JS runner 同处
│   ├── nexty-css/                      #   cssparser + selectors 封装 + 自研 cascade
│   ├── nexty-layout/                   #   盒级布局（taffy 接管盒级几何 + 自研 inline）
│   ├── nexty-text/                     #   parley + swash + fontique 封装
│   ├── nexty-paint/                    #   vello_cpu + vello_hybrid 双后端封装（隔离渲染线程）
│   ├── nexty-network/                  #   reqwest 封装（NetworkFetcher trait）
│   └── nexty-chrome/                   #   winit 窗口 + 地址栏 + 导航
└── docs/
    ├── decisions/                      # 架构决策记录（ADR）：依赖选型、自研决策
    ├── plans/                          # 阶段计划文档
    └── dependencies.html               # cargo-about 生成的第三方依赖清单
```

## Crate 选型

提问筛选结果（决策记录与逐层理由：[docs/decisions/2026-10-01-crate-selection.md](docs/decisions/2026-10-01-crate-selection.md)）。

| 层 | 选型 | 许可证 | 状态 |
| --- | --- | --- | --- |
| HTML 解析 | `html5ever` | MIT/Apache-2.0 | 已定 |
| DOM | 自研 arena DOM | — | 已定（与 JSRT 绑定同处） |
| CSS 解析 / 选择器 | `cssparser` + `selectors` | MPL-2.0 | 已定 |
| Cascade | 自研 | — | 已定 |
| 盒级布局 | `taffy`（block/flex/grid/absolute）+ 自研 inline/table | MIT | 已定（双轨：taffy 接管盒级几何，自研保留 inline/文本布局与 table，见选型 ADR 修正记录） |
| 文本整形 / 字体 | `parley`（内置 fontique 选字 + harfrust 整形） | Apache-2.0/MIT | 已定（swash 自 parley 0.11 起不参与整形，已移出依赖树） |
| 字形簇 / 断行 | `unicode-segmentation` + Unicode 18.0.0 UCD 数据文件 | MIT/Apache-2.0 | 已定（UAX#14 自研，理由见 [ADR](docs/decisions/2026-10-17-line-breaking.md)） |
| 绘制 / 光栅 | `vello_cpu`（已落地）+ `vello_hybrid`（待 chrome 接入） | Apache-2.0/MIT | 已定（双后端，API 同形不同名，facade 需适配层） |
| 窗口 / 输入 | `winit` + `wgpu` | Apache-2.0/MIT | 已定 |
| 网络 | `reqwest` | MIT/Apache-2.0 | 已定 |
| 剪贴板 | `arboard`（仅文本，关 `image-data`） | MIT/Apache-2.0（Windows 后端传递 BSL-1.0） | 已定（理由见 [ADR](docs/decisions/2026-10-23-clipboard.md)） |
| URL | `url` | MIT/Apache-2.0 | 候选，用到再引 |
| bidi / 复杂脚本 | `unicode-bidi` + 按需 `icu` | MIT/Apache-2.0 | 候选，用到再引 |
| 日志 / 错误 | `tracing` + `thiserror` | MIT/Apache-2.0 | 候选，用到再引 |
| JS 运行时 | 可插拔 JSRT 层 | — | 暂缓选型 |

外部依赖只允许出现在对应层的 facade crate 内；对外能力先定 trait（`NetworkFetcher` / `TextShaper` / `Rasterizer` 等），具体后端作为可替换实现。依赖类型一律不得进入 pub 导出（见 Hard Rules）。

渲染后端两个实现是**同形不同名**的 API：`vello_cpu` 用 `RenderContext`，`vello_hybrid` 用 `Scene`（`Resources` / `Pixmap` / `GlyphRunBuilder` / `RenderSettings` 同名同形但非同一类型），facade 需要一层适配。两者对未支持特性都是 panic 而非返回错误，故渲染走隔离线程 + `catch_unwind`。

## Hard Rules

### Technical
- **Feature 拆分（硬约束）**：每个 facade crate 的模块功能必须以 Cargo feature 拆分，`mod` 声明与对应 `pub use` 一律挂`#[cfg(feature = "…")]`，**不得**出现「声明了 feature 但代码不与之联动」的空壳。具体映射见下方[Feature 映射表](#feature-映射表)，新增模块时同步登记该表
- Rust stable，**仓库内代码严禁 unsafe**：本仓库自研代码一律不得出现 unsafe 块 / unsafe fn / unsafe impl，无例外、无 FFI 豁免。每个 crate 在 `lib.rs` 顶部用 `#![forbid(unsafe_code)]` 固化该约束。第三方 crate 内部的 unsafe 不由我们改写（`forbid` 也不作用于依赖），但选型时必须查其 unsafe 用量与边界
- **依赖类型不得外泄**：每个 crate 的公共 API 只暴露自身抽象类型。依赖 crate（html5ever / cssparser / selectors / parley / vello_cpu / vello_hybrid / reqwest …）的类型不得出现在任何 `pub` 导出中——含 pub fn 签名、pub struct/enum 的 pub 字段、pub trait 的方法签名、`pub use` re-export。跨界传递依赖类型时，包一层自己的类型再暴露
- **不造轮子优先**：已有成熟 crate 能覆盖的能力，直接依赖，不自己实现。参考优先级：**WHATWG 规范 > WPT 测试套件 > 成熟 crate > 自行实现**。**例外**：DOM / Cascade / 盒级布局三层已决策自研，理由与边界见选型 ADR，不得据此把自研扩大到其他层
- **许可证政策**：白名单 MIT / Apache-2.0 / BSD / ISC / MPL-2.0 / LGPL / Zlib / CC0-1.0 / Unicode-3.0 / CDLA-Permissive-2.0 / BSL-1.0，禁 GPL / AGPL。后 5 个是已选 crate 栈（wgpu / icu4x / reqwest / arboard）无法回避的宽松、非 copyleft 传递依赖，逐条理由见 `deny.toml`。`cargo-deny` 做硬门禁（含传递依赖），`cargo-about` 生成的第三方依赖清单入库。引入或升级依赖前必须确认传递依赖未引入白名单外的许可证
- **隔离**：每层一个 facade crate，外部 crate 只允许出现在该层 crate 内；对外能力先定 trait（`NetworkFetcher` / `TextShaper` / `Rasterizer` 等），具体后端作为可替换实现
- **渲染隔离**：渲染在独立线程执行，panic 由 `catch_unwind` 兜住并上报，不得让渲染失败拖垮主进程。因此 workspace 与各 crate 一律不得设置 `panic = "abort"`（会禁用 unwind，使兜底失效）。渲染后端对未支持特性是 panic 而非返回错误，此风险按已知限制对待
- **JSRT 可插拔**：JS 运行时作为可插拔层，选型暂缓。DOM 与 JS runner 必须同处（可变树、可共享引用，为绑定预留），JS 运行时类型不得渗入 DOM 公共 API
- 引入依赖前先查：维护活跃度、许可证（见上）、unsafe 用量、依赖树体积、是否贴合 spec 行为
- 每个新依赖在 `docs/decisions/` 记一条 ADR，写清"为什么用它、为什么不是别的、边界在哪"
- 自研仅限三种情况：① 没有可用轮子；② 现有轮子行为不符合 spec；③ 选型 ADR 已明确决策自研（DOM / Cascade / 盒级布局）。任一情况都必须在 ADR 里写明"现有 crate 为什么不够用"与自研边界
- 自研代码保持最小：只实现 spec 要求、轮子缺失的那部分，不顺手重写周边
- 每个模块独立 crate，测试覆盖率 ≥ 80%
- 公共 API 必须有 doc comment，引用规范条款

### Behavior
1. **Read before write** — 动手前先读规范对应章节，再读候选 crate 的源码与文档。不确定就问，不猜
2. **Think before code** — 先说清楚选择和取舍（用哪个 crate / 为什么自研）。真不懂就停
3. **Simplicity** — 最少代码解决问题。抵抗过早抽象。硬编码直到有真实理由需要配置
4. **Surgical changes** — diff 必须和任务一样小。不顺手改别的文件
5. **Verification** — 每个子任务先定义 success criterion。修 bug：先写 failing test → 看它 fail → 修 → 看它 pass
6. **Goal-driven** — ❌ "写个 tokenizer" ✅ "复用 html5ever 的 tokenizer，按 WHATWG §13.2.5 校验状态切换行为，附单元测试"
7. **Debugging** — 炸了先查，别猜。读完整报错。复现后再改，一次只改一处
8. **Self-check** — 提防：Kitchen Sink / Wrong Abstraction / Optimistic Path / Runaway Refactor

### Commit Discipline
- 每个子任务 + cargo check/test + cargo fmt 通过后立即 commit
- Message 格式：`[module] what + why`，例：`[tokenizer] adopt html5ever tokenizer, matches WHATWG §13.2.5.1`
- 必须 `git add <specific files>`，禁止 `git commit -a`
- 禁止 `git rebase -i` 压缩已完成的 commit
- WPT 语义比对通过后才允许 commit（架构师执行比对）

### Versioning Discipline
- 改逻辑（修 bug / 加特性 / 换行为）必须在提交里把该 crate 的 `Cargo.toml` `version` 随手 +1（patch），不允许"改了代码却发版版本号不变"
- `path =` 依赖在同一工作区内由 path 覆盖版本号，仅作安全校验，故 patch 升级不会破坏依赖方 `^X.Y.Z` 要求——无需连带改其他 crate 的依赖段（除非涉及 semver-incompatible 的大版本/次版本变更）
- 发版动作（打 tag / push / 发布 crates.io）另行人工执行，bump 版本号只改 `Cargo.toml`

### Verification Flow
1. 你写完 → `cargo check` 零 warning
2. `cargo test` 全绿
3. 架构师跑语义比对（WPT 输出 vs 你的实现）
4. 比对通过 → `git add <files>` + commit
5. 比对不通过 → 根据差异修，回到步骤 1
6. 你不许自行宣布"完成"

### Feature Gating

**约定（全 crate 统一，不得逐 crate 另立）**

1. **feature 名 = 能力名 = 模块名**：一个 feature 对应一个模块或一项独立能力，命名用 kebab-case（`parse-fragment` / `font-resolution` / `ua-stylesheet`）。
2. **`default` = 全部已实现能力**：下游 crate 依赖默认即可获得完整功能栈，无需逐个列举 feature。未实现的能力可声明 feature 占位，但**不进 default**，并在注释里写明落地任务。
3. **依赖上拉用feature 依赖表达**（`flex = ["block"]`），不用编译期 `cfg` 兜底。真实的算法依赖才写：块级流按 CSS 2.1 §9.2.1.1 需匿名块故 `block = ["inline"]`；flex 容器参与块级流且 flex-basis 需内在尺寸故 `flex = ["block"]`。
4. **可选依赖用 `dep:` 语法**：某 feature 才需要的 crate 写 `optional = true` + `cpu = ["dep:vello_cpu"]`。**例外**：nexty-layout 的 `taffy` **保持常驻**——feature 只切能力开关，不切依赖树。
5. **模块整体抑制用条件 allow**：某模块的内部项仅被上层 feature 消费时，用 `#![cfg_attr(not(feature = "…"), allow(dead_code))]` 按模块整体抑制，禁止逐项 `#[allow]`。
6. **部分 feature 构建必须能编译**：改动后至少验证「默认」+「逐个能力关闭」两种配置零warning（`cargo check -p <crate> --no-default-features --features <...>`）。
7. **二进制目标用 `required-features`**：依赖某 feature 才存在的入口时，`[[bin]]` 须声明 `required-features`，否则无头构建会失败。

**Feature 映射表**

新增/删除模块时**必须**同步更新本表，这是硬约束。

| crate | feature | 对应模块 / 能力 | 在 default |
| --- | --- | --- | --- |
| nexty-dom | `node` | `node` 节点数据层 | ✅ |
| | `tree` | `tree` 树变更算法（`Document`/`Children`） | ✅ |
| | `event` | `event` 事件模型：`Event`/`EventTarget`/传播/数据面/`EventLoop`（隐含 `tree`） | ✅ |
| nexty-html | `parse-document` | `parse_document` + html5ever `TreeSink` 桥接 | ✅ |
| | `parse-fragment` | `parse_fragment` / `FragmentContext`（§13.4） | ✅ |
| | `resources` | `collect_style_resources` / `collect_image_resources` | ✅ |
| nexty-css | `values` | `value` 属性值模型与逐属性解析 | ✅ |
| | `selectors` | `selector` 选择器解析与 arena DOM 匹配 | ✅ |
| | `stylesheets` | `parser` 样式表与声明块解析 | ✅ |
| | `cascade` | `cascade` 自研级联与 computed style | ✅ |
| | `ua-stylesheet` | `ua` UA 默认样式表 | ✅ |
| nexty-text | `shaping` | `TextShaper` / `ParleyTextShaper`（隐含 `font-resolution`） | ✅ |
| | `font-resolution` | `FontResolver` / `ResolvedFont` | ✅ |
| nexty-layout | `block` | `block` 块级流（taffy 解算 + Fragment 回填 + 浮动环绕修正 pass，隐含 `inline`；taffy `float_layout` 常开） | ✅ |
| | `inline` | `inline` 行内流与匿名块（断行永久自研） | ✅ |
| | `flex` | flex 容器 blockify（tree_build）+ `Display::Flex` 映射（style_map），几何由 taffy flexbox 解算（隐含 `block`） | ✅ |
| | `replaced` | `block::is_image_element` 块级 `<img>` 替换盒 | ✅ |
| | `taffy-map` | `style_map` `ComputedStyle → taffy::Style` 映射层 + `tree_build` 盒树构建（隐含 `inline`） | ✅ |
| | `grid` | `style_map` grid-* 映射 + `Display::Grid`（taffy grid 算法解算；item 经块容器走法收集） | ✅ |
| | `table` | 预留：表格布局 | ❌ 未实现 |
| | `absolute` | `style_map` position/inset 映射（taffy 绝对定位解算，相对树内父盒） | ✅ |
| | `selection` | `selection` 文本选区几何：`TextMap` 命中、逐行矩形、UAX#29 选词与复制文本重建（隐含 `block`） | ✅ |
| nexty-paint | `cpu` | `VelloCpuRasterizer` + `decode`（`vello_cpu`/`image`） | ✅ |
| | `gpu` | 预留：`vello_hybrid`，待 chrome 接入 wgpu surface | ❌ 未实现 |
| nexty-network | `fetch` | `ReqwestFetcher` / `NetworkFetcher` | ✅ |
| | `resolve` | `resolve` 相对 URL 解析（WHATWG basic URL，`url` crate 直依赖） | ✅ |
| | `decode` | `decode_document` 四层字符编码探测（`encoding_rs`） | ✅ |
| | `cache` | `CachedFetcher` 内存级 HTTP 缓存（隐含 `fetch`） | ✅ |
| nexty-chrome | `pipeline` | `pipeline` 导航管线 + `bench` 基准共享支撑（语料/计数器，正文在 `benches/pipeline.rs`，`required-features` 门控） | ✅ |
| | `resources` | `resources` 子资源抓取与解码编排 | ✅ |
| | `render` | `render` 渲染隔离线程 | ✅ |
| | `history` | `history` 会话历史栈 | ✅ |
| | `ui` | `ui` 自绘工具带状态机 | ✅ |
| | `scrollbar` | `scrollbar` 滚动几何与拖拽状态机 | ✅ |
| | `selection` | `selection` 选区状态机、高亮与剪贴板复制（隐含 `pipeline` + `arboard`） | ✅ |
| | `window` | `app` winit 事件循环 + wgpu 呈现，隐含其余全部模块 | ✅ |

### Goal-Driven Execution (本轮任务)
- 每轮任务有显式 `goal.md`，列明任务清单与每个任务的退出条件
- 任务完成的判据是退出条件全部满足，不是 agent 自行宣布
- 退出条件未满足时继续迭代，不要提前停下
- 退出条件全部满足后立即 commit + push，然后退出本轮，等待用户下一轮指令

## Style Conventions
- 别用 newtype 包裹，除非需要 orphan rule
- 别为未来需求加参数。真有需求时再加
- 别自己写 interner / 解析器 / 数据结构——需要时先找标准库或成熟 crate
- 需要多次 `clone()` 时先写出来，等 profiling 证明热路径以后再去掉