# Nexty 性能与安全审查报告

- 日期：2026-10-24
- 基线：HEAD `0e76900`（工作区除本地工具产物外干净）
- 性质：**只读审查**，未改动任何源码；行号均以当前工作树为准
- 范围：全部 8 个 facade crate（约 24,900 行 `.rs`）+ 根配置（Cargo.toml / deny.toml）
- 重点：**性能与安全**（区别于 2026-10-02 以 Hard Rules 合规为主的[上一轮审计](2026-10-02-code-audit.md)）

## 1. 审查方法

1. **机械检查**：生产代码（排除 `#[cfg(test)]` 模块）panic 位点统计、数值 `as` 转换位点统计、递归遍历函数排查；根配置与依赖门禁审读。
2. **并行深审**：两个审查通道分别通读 nexty-layout 与 nexty-chrome 全部源文件；nexty-css / nexty-dom / nexty-html / nexty-text / nexty-paint / nexty-network 由主审通读关键路径（级联热路径、树算法、TreeSink 桥接、整形/解码/抓取入口）。
3. **交叉复核**：Mimosa 密封深度扫描（静态、不执行目标代码），结果见 §3。
4. **门禁实跑**：clippy / test / deny 取证。

## 2. 总体结论

**门禁与代码卫生层面持续健康**：clippy `-D warnings` 零警告、363/363 测试通过、cargo-deny 全绿；生产代码 panic 位点（排除测试）全库仅约 20 处，且多为「树构建器契约保证」型 expect；`#![forbid(unsafe_code)]` 无一违例。2026-10-02 审计的两项中风险（主线程 draw 路径 expect、wgpu 行对齐）均已正确修复并配了回归测试。

本轮发现的风险集中在两个主题上：

1. **对不可信输入的边界设防缺失（安全）**：整条管线对 DOM 深度、数值范围（`f32::INFINITY`/NaN）、网络响应大小、图片解码像素量**均无上限检查**。四条独立的几十 KB 级构造输入路径可以把进程带进不可恢复状态——其中**栈溢出与分配失败是 abort 而非 unwind，渲染线程的 `catch_unwind` 兜底完全覆盖不到**（见 H1/H3）。另有一条错误页标记注入路径（M2）。
2. **热路径的重复计算与深拷贝（性能）**：与宽度无关的文本计算（整形/cluster/断点/字体度量）随 taffy 的多次 measure 成倍重跑（H4）；图片像素每帧深拷贝两次（M1）；级联对每元素全表线性扫描（M5）；resize/滚动每个事件全量重级联+重排+重光栅（M6）。

无「秘密数据外泄」类发现：当前无 JS、无 cookie、无本地存储，网络面只有 GET/HEAD 子资源抓取。

## 3. 门禁与扫描状态

| 项 | 结果 |
| --- | --- |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过（exit 0） |
| `cargo test --workspace` | 363 passed / 0 failed |
| `cargo deny check` | advisories / bans / licenses / sources 全 ok |
| `#![forbid(unsafe_code)]` | 8/8 crate，违例 0 |
| 生产代码 unwrap/expect/panic/unreachable | 全库约 20 处（集中在 inline/mod.rs 9、html 桥接 4、block.rs 3、render.rs/app.rs 各 2），绝大多数为内部不变量型 |
| Mimosa 深度扫描 | `scan-2026-10-24T12-12-08.823Z-20aa0cdc84d2`，seal `sha256:e39dbcf9…b3a4b8`，426 个依赖包扫描，finding 0，advisory 匹配 0（1 个 unknown 包） |

Mimosa 结论纪律：finding 为零**不构成**「项目无风险」的结论；本轮人工审查发现的 H1–H3 即为工具未覆盖的、依赖业务逻辑推理的路径。依赖侧唯一已知放行仍是 `RUSTSEC-2026-0192`（ttf-parser unmaintained，非漏洞，理由在 [deny.toml](../deny.toml)）。

## 4. 高严重性（建议优先处理）

### H1 · 深层 DOM 无递归深度保护——任意网页可触发栈溢出 abort

**位置**（互为印证的九条递归链）：

- [cascade.rs:484](../crates/nexty-css/src/cascade.rs#L484) `walk`（级联逐元素递归）
- [tree_build.rs:355](../crates/nexty-layout/src/tree_build.rs#L355) `build_box` ↔ [tree_build.rs:390](../crates/nexty-layout/src/tree_build.rs#L390) `collect_children_boxes` 互递归（建盒树）
- [block.rs:244](../crates/nexty-layout/src/block.rs#L244) `assemble_element`（Fragment 组装）
- [selection.rs:542](../crates/nexty-layout/src/selection.rs#L542) `Builder::collect`（TextMap 构建）
- [inline/mod.rs:456](../crates/nexty-layout/src/inline/mod.rs#L456) `collect_children`（行内收集）
- [lib.rs:228](../crates/nexty-html/src/lib.rs#L228) `collect_from` 与 [lib.rs:309](../crates/nexty-html/src/lib.rs#L309) `collect_images_from`（子资源收集）
- [tree.rs:276](../crates/nexty-dom/src/tree.rs#L276) `clone_node`（template/克隆路径）
- [pipeline.rs:504](../crates/nexty-chrome/src/pipeline.rs#L504) `document_title` 的 `walk`

**问题**：html5ever 对嵌套深度无限制（迭代建树不溢出），nexty-dom 也不设上限，但后续五层的树遍历全部递归。数万层 `<div>` 嵌套（几十 KB HTML）即可在级联/建盒/组装任一环打爆线程栈。

**影响**：栈溢出在 Rust 中是 **abort 而非 panic**，`catch_unwind` 兜不住，整个进程死亡——这是任意网页可远程触发的 DoS，直接击穿「渲染失败不拖垮主进程」的硬约束底线。

**建议**：统一机制而非逐点修补——在 `Context`/遍历入口携带深度计数，超过上限（浏览器常用 512–1000）时将该子树降级为空盒/跳过（沿用现有「建节点失败降级丢弃」语义），保证进程存活。优先覆盖 cascade `walk` 与 layout `build_box`（最先触达的两环）；`clone_node` 可单独加深度参数。长期可把 `walk` 类遍历改为显式栈迭代（dom 的 `normalize`/`collect_text_descendants` 已是迭代式范本，[tree.rs:556](../crates/nexty-dom/src/tree.rs#L556)）。

### H2 · `f32::INFINITY` 可从尺寸属性注入，inf/NaN 无任何防护污染整页几何

**位置**：[inline/mod.rs:133-145](../crates/nexty-layout/src/inline/mod.rs#L133-L145) `attribute_px`；传播点 [tree_build.rs:370-375](../crates/nexty-layout/src/tree_build.rs#L370-L375)、[inline/mod.rs:920-932](../crates/nexty-layout/src/inline/mod.rs#L920-L932)

**问题**：

```rust
digits.parse::<f32>().ok().filter(|value| *value > 0.0)
```

40 位以上的数字（`<img width="9999…">`）解析为 `Ok(f32::INFINITY)`，`inf > 0.0` 通过过滤。块级路径把 `length(inf)` 写进 taffy definite size；行内路径 `cursor += inf` → 同行第二个原子盒 `y = inf - inf = NaN`，经 measure 回灌 taffy（`inf × flex_shrink` 等）向全树扩散，最终片段树/绘制坐标整体污染。整条管线（含 ComputedStyle 的 font-size/line-height/margin）没有任何 `is_finite()` 检查。

**建议**：`attribute_px` 解析后 clamp 到合理上限（如 1e7 px）并拒绝非有限值；在 `map_style`/`build_lines` 入口对关键尺寸做 `is_finite` 卫生检查，非有限值降级为 auto/0，把污染隔离在单元素。附带修正：HTML 尺寸属性只取无符号整数前缀，当前 `take_while` 接受 `'.'`（`width="1.5"` 得 1.5px），应只取 ASCII 数字。

### H3 · 网络响应体与解码像素量无上限——恶意页面可 OOM abort

**位置**：[network/lib.rs:192](../crates/nexty-network/src/lib.rs#L192)（`response.bytes()` 全量进内存）；[paint/lib.rs:53-66](../crates/nexty-paint/src/lib.rs#L53-L66)（**尺寸校验在解码之后**）；[resources.rs:160-166](../crates/nexty-chrome/src/resources.rs#L160-L166)（编排层无预算）

**问题**：三层叠加——

1. `fetch` 对响应体无字节上限，`bytes()` 一次性收全量（30s 超时内按带宽可积累 GB 级）；
2. `decode` 先 `reader.decode()` 再检查 `MAX_DIMENSION`：一个几 KB 的 PNG 声明 `65535×65535`，解码阶段就试图分配约 17 GB（RGBA），**在尺寸检查执行前内存已经爆掉**；
3. chrome 编排层对 HTML/CSS/图片均无预算，并发 8 路抓取（[resources.rs:21](../crates/nexty-chrome/src/resources.rs#L21)，已限流，这点是对的）会叠加放大。

Rust 分配失败是 abort 而非 unwind，`catch_unwind` 兜不住。

**建议**：分两级修——

- **paint `decode`（关键一步，改动小）**：用 `image::ImageReader::into_dimensions()` 先读头部（不解码像素）校验尺寸，再解码；上限同时改为**面积约束**（如 `w*h ≤ 2^26` 像素 ≈ 256 MB 缓冲）而非仅边长，65535 边长上限本身就允许 17 GB 分配。
- **编排层**：`resources.rs` 对响应体设字节预算（HTML/CSS ~10MB、图片 ~50MB 量级），超限按现有降级语义跳过该资源并绘制占位。`NetworkFetcher` 加 `Content-Length` 预检或流式读取属更大改动，可后置。

### H4 · 与宽度无关的文本计算随 measure 成倍重跑——断行热路径最大性能放大器

**位置**：[inline/mod.rs:631-646](../crates/nexty-layout/src/inline/mod.rs#L631-L646)（每次 `build_lines` 重建 shaped_items）；[inline/mod.rs:974](../crates/nexty-layout/src/inline/mod.rs#L974) + [inline/mod.rs:773-781](../crates/nexty-layout/src/inline/mod.rs#L773-L781)（`metrics` 每片段每行一次）；根因 [nexty-text/lib.rs:339](../crates/nexty-text/src/lib.rs#L339) / [lib.rs:414-417](../crates/nexty-text/src/lib.rs#L414-L417)（`shape`/`metrics` 每次完整 parley 布局，无缓存）

**问题**：taffy 对每个 run 叶典型发起 2–3 次 measure（MaxContent / MinContent / 定宽），`lines_cache` 只缓存行结果，整形、cluster 映射、断点计算、字体度量探针全部重做；`metrics()` 更是以固定探针文本跑完整 `build_layout`，同一行 N 个 span 就 N 次。resize 拖动时对全文档文本按每档宽度成倍重复。

**建议**：把 `ShapedText` 提升为 `TextChunk` 的惰性字段（首次 `build_lines` 填充）；`cluster_map`/`break_opportunities`/`split_spans` 只依赖 `(text, options)` 与宽度无关，一并缓存；`metrics` 在 `build_lines` 开头按 `TextStyle` 预取一次存表（探针文本固定，样式相同结果必相同），或在 nexty-text 内按 TextStyle 缓存。只有贪心放置按宽度重跑。

## 5. 中严重性

### M1 · 每帧图片像素深拷贝两次

**位置**：[pipeline.rs:542](../crates/nexty-chrome/src/pipeline.rs#L542)（`scene.images.push(source.clone())`）；[render.rs:92](../crates/nexty-chrome/src/render.rs#L92)（`.send((scene.clone(), …))`）

`Image.data: Vec<u8>` 无共享句柄：每张图从 `page.images` clone 进 scene，再随整个 `scene.clone()` 拷第二份发渲染线程。1920×1080 图一帧两拷贝约 16.6 MB，多图页面 60fps 下分配带宽 GB/s 量级。**建议**：`Scene.images`/`Page.images` 改 `Arc<Image>`；`RenderThread::render` 收 `Scene` 所有权（app 侧构建后 `move`），消除第二次整树克隆。改动都在 crate 边界内。

### M2 · 错误页把未转义的 URL / 错误串直接拼进 HTML（标记注入）

**位置**：[app.rs:507-512](../crates/nexty-chrome/src/app.rs#L507-L512)

`url` 与 reqwest 错误串未转义直接 `format!` 进错误页 HTML。URL 含 `<img src=//attacker/ping>` 时（地址栏输入或链接 href 均可），错误页渲染即向第三方发起请求，**泄露用户正在访问的 URL**；还能注入样式。无 JS 引擎暂不升级为脚本执行，但这是「不可信输入拼进标记」的教科书路径。**建议**：`<`/`>`/`&`/`"` 实体转义后再插入；错误串只保留类别，不回显原始 URL 片段。

### M3 · 页面加载竞态：慢的旧导航覆盖新页面

**位置**：[app.rs:489-502](../crates/nexty-chrome/src/app.rs#L489-L502)（`page_loaded` 无来源校验）；[app.rs:633-636](../crates/nexty-chrome/src/app.rs#L633-L636)（事件直通）

导航 A（慢站点）→ 导航/后退到 B（快）：B 先回显，A 后到把页面覆盖回 A，而地址栏与历史栈停在 B。每次 `load` 还新 spawn 一个 fetch 线程，快速连点会并发多个在途加载。**建议**：单调递增的加载代次（generation counter）随 `PageLoaded` 回传，过期即丢弃；顺带用常驻 worker 替代每次 spawn（同时消掉 [app.rs:485](../crates/nexty-chrome/src/app.rs#L485) 的 spawn `expect`——线程资源耗尽时它崩的是主进程）。

### M4 · wgpu validation panic 残余面：无 uncaptured error handler，尺寸超限即崩

**位置**：[app.rs:944-955](../crates/nexty-chrome/src/app.rs#L944-L955)（configure）、[app.rs:1011-1027](../crates/nexty-chrome/src/app.rs#L1011-L1027)（resize）、[app.rs:1036-1056](../crates/nexty-chrome/src/app.rs#L1036-L1056)（create_texture）

旧审计 M2（bytes_per_row 256 对齐）已修，但未注册 `on_uncaptured_error`，任何 wgpu validation 错误仍按默认行为 panic 主线程。残余触发面：`max_texture_dimension_2d` 通常 8192，多屏拼接/高 DPI 最大化时 `inner_size` 物理像素超限，configure 与 create_texture 双双 panic。**建议**：`Gpu::new` 记录 `device.limits()`，尺寸用 `min(size, max_texture_dimension_2d)` clamp；注册 uncaptured error handler 把错误降级为日志 + 跳帧，与渲染隔离哲学对齐。

### M5 · 级联热路径：全规则线性扫描 + 每命中声明 clone + 内联样式重复解析

**位置**：[cascade.rs:328-446](../crates/nexty-css/src/cascade.rs#L328-L446)（`collect_origin_declarations` / `cascade_in`）；[selector.rs:243](../crates/nexty-css/src/selector.rs#L243)（每次匹配新建 `SelectorCaches`）；[selector.rs:291-313](../crates/nexty-css/src/selector.rs#L291-L313)（`namespace_url` 每调用一次 String 分配）；[cascade.rs:429-431](../crates/nexty-css/src/cascade.rs#L429-L431)（`parse_inline_style` 每次级联重新解析）

**问题**：复杂度 O(元素数 × 规则数 × 匹配成本)，无规则分桶（浏览器标准做法是按 id/class/tag 建规则哈希桶，跳过绝不可能命中的规则）；每条命中声明 `declaration.clone()`；`style` 属性字符串每次级联（含 resize 后的 recascade）重新解析；选择器匹配每次调用重建 nth 缓存，后代组合器上溯时每层祖先都可能触发 `namespace_url` 的 `.to_owned()` 分配。数千规则样式表 × 数万元素页面下，这是首屏与 resize 的主要 CPU 去向之一。**建议**（按性价比排序）：① `matched` 收 `(CascadeKey, &Declaration)` 引用而非 clone（声明生命周期覆盖 `resolve_all` 调用）；② 内联样式解析结果随元素缓存（recascade 直接复用）；③ `namespace_url` 返回 `&'static str`/`Cow`（四类命名空间 URL 是编译期常量，只有 `Other` 需要 to_owned）；④ 规则分桶（最右侧简单选择器的 tag/class/id 三桶）在样式表超大时才有收益，可后置。

### M6 · resize/滚动每事件全量重建管线

**位置**：[app.rs:645-656](../crates/nexty-chrome/src/app.rs#L645-L656)（每个 `Resized` 事件全量重级联+重排+重整形）；[app.rs:553-569](../crates/nexty-chrome/src/app.rs#L553-L569)（滚动只是 origin 变化却整树重建显示列表）；[app.rs:1062-1069](../crates/nexty-chrome/src/app.rs#L1062-L1069) + [app.rs:1219-1237](../crates/nexty-chrome/src/app.rs#L1219-L1237)（`pad_rows` 每帧整屏拷贝）

拖拽窗口边缘时 winit 连发 Resized，每次触发整页重级联（UA+author 全部样式表）、整树重布局、全部文本重新整形；滚轮一格/滚动条拖拽/选区拖拽的每个 mousemove 都走整树 emit（每 run 的 glyphs 逐个 map+collect 新 Vec、`run.fonts.clone()`）→ 全屏 CPU 光栅 → 宽度非 64 倍数时 `pad_rows` 再整屏拷贝。**建议**：① resize 置 dirty 标志，合并到下一次 `RedrawRequested` 执行一次；② 布局未变时缓存显示列表、滚动只改场景级 y 偏移；③ `pad_rows` 的 staging 按宽高缓存复用。终态（页面纹理缓存 + 只光栅新暴露条带）可后置。

### M7 · UI 工具带每帧重复整形

**位置**：[ui.rs:457-501](../crates/nexty-chrome/src/ui.rs#L457-L501)（按钮字形是常量却每帧 shape）；[ui.rs:616-621](../crates/nexty-chrome/src/ui.rs#L616-L621)（光标/选区定位对 URL 前缀整串 reshape，O(全文)）；[ui.rs:450-453](../crates/nexty-chrome/src/ui.rs#L450-L453)（每帧新建 families String）

每次 redraw：3 个按钮字形 + URL 全串 + caret 前缀 + 选区 2 次前缀整形；超长 URL 时每次按键约 4 次全串整形。**建议**：按钮字形整形结果缓存为字段；URL 整形按 `(url, style)` 缓存；前缀宽度直接用已整形 glyph 的 x 前缀累加，免再 reshape。

### M8 · TextMap 行遮蔽：inline-block 内的文本点不中

**位置**：[selection.rs:146-152](../crates/nexty-layout/src/selection.rs#L146-L152)（`hit` 取第一个 y 命中的行）；[selection.rs:533-543](../crates/nexty-layout/src/selection.rs#L533-L543)（`collect` 先 push 父行、后递归原子盒行）

父行的 `top..top+height` 完全覆盖其内部 inline-block 的行，且父行在数组中靠前，`position` 永远先命中它。点击 inline-block 内文本会命中外层文本或跳到相邻行——命中测试对原子盒内容系统性失效。**建议**：`collect` 时把原子盒的行插在父行之前（先深后浅），或 `hit` 收集全部 y 命中行后取更深层者。

### M9 · 块级子元素嵌在行内子树中被静默丢弃——内容从页面上消失

**位置**：[inline/mod.rs:453-470](../crates/nexty-layout/src/inline/mod.rs#L453-L470)

`collect_children` 的 `_ => {}` 分支注释称「由块级分组处理」，但进入嵌套行内子树后没有任何人会再处理：`<div>a <span><div>CONTENT</div></span> b</div>` 中 CONTENT 整棵子树不产生任何盒。CSS 规范行为是拆分 inline flow 生成匿名块，此项也不在 AGENTS.md 已知未实现清单内（清单里是 float/table/justify 等，不含 inline 拆分）。内容丢失比几何偏差严重。**建议**：至少降级为原子盒（独立格式化上下文布局）保证内容可见；或终止当前 run 并把块盒挂回块容器层。

### M10 · ComputedStyle / 行数据克隆链：每元素每次布局 2–4 次深克隆

**位置**：[tree_build.rs:343-356](../crates/nexty-layout/src/tree_build.rs#L343-L356)（`root_style` 深克隆后仅以 `&style` 借出，从不存储）；[block.rs:169-173](../crates/nexty-layout/src/block.rs#L169-L173)（组装再克隆一次）；[tree_build.rs:133-140](../crates/nexty-layout/src/tree_build.rs#L133-L140)（`lines_at` **命中**缓存仍 `lines.clone()` 全量深拷贝，miss 路径再克隆一份入缓存）；[inline/mod.rs:189-200](../crates/nexty-layout/src/inline/mod.rs#L189-L200)（`ensure_style` 每字符对 ComputedStyle 做全字段 `PartialEq`，含 `Vec<String>` 比较）

`ComputedStyle` 含 font_family 与两个 grid 模板 Vec，每次克隆 ≥3 次堆分配；万级元素页面每次 resize 数万次分配。**建议**：`build_box` 改为 `ctx.styles.get(&node)` 借用 + 仅兜底时构造 initial；长期方案 `HashMap<NodeId, Arc<ComputedStyle>>` 让克隆变引用计数；`lines_at` 组装是缓存最后消费者，改 take 语义避免命中路径再克隆；`ensure_style` 的第三个条件（样式全量比较）可直接删除——样式是按 NodeId 查表的纯函数，`text_style_node` 相等即样式相等。

### M11 · 原子盒布局无记忆化，嵌套 inline-block 组合放大

**位置**：[tree_build.rs:141-159](../crates/nexty-layout/src/tree_build.rs#L141-L159)（每个新的 `(available_width, min_content)` 组合对所有原子盒重排）；[block.rs:109-134](../crates/nexty-layout/src/block.rs#L109-L134)

定宽（`width: 200px`）原子盒的布局结果与可用宽无关也照样重排；嵌套 inline-block 每层 × 每宽度全子树重建（taffy 树每次新建），深度 d 近似 3^d 放大——恶意页面深嵌套可把单次布局变成组合爆炸（性能 DoS 面）。**建议**：定宽原子盒只布局一次；按 `(node, available_width)` 记忆化；嵌套深度上限与 H1 同一机制。

### M12 · `remap_glyph` 对 `glyph.font` 无界索引——trait 契约破坏即 panic

**位置**：[inline/mod.rs:997-1012](../crates/nexty-layout/src/inline/mod.rs#L997-L1012)

```rust
let source = &source_fonts[glyph.font as usize];
```

`TextShaper` 是 pub trait、`layout_document` 接受任意 `&dyn TextShaper`，实现返回越界 `glyph.font` 时布局层直接 panic（渲染线程能兜住但整页渲染失败）。当前仓库内实现满足契约，属防御缺失。**建议**：改 `source_fonts.get(...)`，缺失时跳过该字形，与「畸形输入不 panic」基准一致。

## 6. 低严重性 / 改进项

| # | 位置 | 问题与建议 |
| --- | --- | --- |
| L1 | [text/lib.rs:305-306](../crates/nexty-text/src/lib.rs#L305-L306) | `ParleyTextShaper::resolve_font` 每次 `FontResolver::new()`（= 全量系统字体发现）。生产管线目前已无人调用（paint 侧 `font_for` 已随重构消失），降级为 API 陷阱；建议移除该转发或改为共享一个 `FontResolver` 字段 |
| L2 | [text/lib.rs:324](../crates/nexty-text/src/lib.rs#L324) | `shape` 每次调用 `style.families.join(", ")` 分配；可在 `TextStyle` 构造时预拼或缓存 |
| L3 | [html/lib.rs:393-424](../crates/nexty-html/src/lib.rs#L393-L424) | `create_element` 每属性两次 `to_string()`（name + value）；百万属性页面的一次性解析成本，可先度量再决定是否引入 interner |
| L4 | [html/lib.rs:385](../crates/nexty-html/src/lib.rs#L385) 等 | TreeSink 回调内 4 处契约型 panic（`elem_name called on a non-element node` 等）。html5ever 状态机保证下理论不可达，但上游曾有 fuzz 历史；建议引入 cargo-fuzz 挸语料（见 §8） |
| L5 | [tree.rs:169-173](../crates/nexty-dom/src/tree.rs#L169-L173) | `insert_node` 每次插入分配一个单元素 `Vec`（fragment 检查用）；解析热路径上每节点一次小分配，可用 `SmallVec` 语义手写为栈上数组分支 |
| L6 | [tree.rs:276-300](../crates/nexty-dom/src/tree.rs#L276-L300) | `clone_node` 递归 + 每层 `collect::<Vec<_>>()`；深子树栈溢出（并入 H1 处理） |
| L7 | [selection.rs:302-348](../crates/nexty-layout/src/selection.rs#L302-L348) | `TextMap::text` 的 `first_collapses` 被后续行首段污染（每行首段都 AND 进来），首行折叠模式 + 后续 `pre` 开头行时复制文本带多余前导空格；改为只在第一个贡献段赋值一次 |
| L8 | [selection.rs:146-173](../crates/nexty-layout/src/selection.rs#L146-L173), [436-481](../crates/nexty-layout/src/selection.rs#L436-L481) | `hit`/`nearest`/`resolved` 全线性扫描，鼠标拖选每次 mousemove 全量扫；行按 `top` 有序可 `partition_point` 二分，`resolved` 可按 text_node 建索引 |
| L9 | [span_split.rs:25-58](../crates/nexty-layout/src/inline/span_split.rs#L25-L58) | `min_content_width` 双重平方复杂度（pub API，当前仅测试调用）；glyph 归属与分段求和都可改单遍游标 |
| L10 | [cluster_map.rs:31-47](../crates/nexty-layout/src/inline/cluster_map.rs#L31-L47), [span_split.rs:71-83](../crates/nexty-layout/src/inline/span_split.rs#L71-L83) | BTreeSet 逐元素插入（每 grapheme 一次节点分配）；改 `Vec` + `sort_unstable` + `dedup`（数据近乎有序，接近 O(n)），查询 `partition_point` |
| L11 | [tree_build.rs:56-60](../crates/nexty-layout/src/tree_build.rs#L56-L60) | `NodeId↔TaffyNode` 双 HashMap：NodeId 是稠密 arena 下标，可用 `Vec<Option<TaffyNode>>` 直接下标，measure 分发免哈希 |
| L12 | [inline/mod.rs:802-815](../crates/nexty-layout/src/inline/mod.rs#L802-L815) | `prepare_span` 计算 `glyph_range` 后丢弃、`place_span_at` 重算；返回四元组传递 |
| L13 | [app.rs:1071-1088](../crates/nexty-chrome/src/app.rs#L1071-L1088) | `present` 每帧 `create_view`/`create_sampler`/`create_bind_group`；sampler 与 bind group 布局在纹理未变时可缓存 |
| L14 | [selection.rs:29-32](../crates/nexty-chrome/src/selection.rs#L29-L32) | `Ctrl+C` 大文本在主线程同步写剪贴板，数 MB 可阻塞事件循环数百毫秒；超大文本截断或移后台线程 |
| L15 | [app.rs:426](../crates/nexty-chrome/src/app.rs#L426) | URL scheme 判断大小写敏感（`HTTP://` 报 InvalidUrl）；RFC 3986 scheme 不区分大小写，解析后按小写比较 |
| L16 | [app.rs:302](../crates/nexty-chrome/src/app.rs#L302), [pipeline.rs:487](../crates/nexty-chrome/src/pipeline.rs#L487) | 每次鼠标移动全树 hit_test（递归，剪枝后约 O(depth)）+ `link_target` 每次命中分配 String；后者可返回 `Option<&str>` |
| L17 | workspace 根 | `[profile.dev]` 未设 `overflow-checks`；release 默认无溢出检查（wrap 而非 panic）。数值卫生靠 H2 的 clamp 落地后此项自然缓解；如需 debug/release 行为一致可显式开 overflow-checks |

## 7. 2026-10-02 旧审计跟进

| 旧编号 | 内容 | 现状 |
| --- | --- | --- |
| M1 | ui.rs draw 路径 `shape().expect()` 主线程 panic | ✅ **已修复**：全部改为降级（按钮不画字形、地址栏不画文本），配 `FailingShaper` 回归测试（[ui.rs:1094-1135](../crates/nexty-chrome/src/ui.rs#L1094-L1135)） |
| M2 | `bytes_per_row` 不满足 256 对齐 | ✅ **已修复**：`pad_rows` 对齐补零 + `upload_aligned`，9 种宽度测试覆盖（[app.rs:1062-1069](../crates/nexty-chrome/src/app.rs#L1062-L1069)）；残余同类风险转为本报告 M4 |
| L1 | network 版本纪律违例 | ✅ 已修（现 0.1.5，历次 bump 正常） |
| L2 | skrifa 应降级 dev-dep | ✅ 已修（Cargo.toml 注释明确说明） |
| L3 | dom 免校验写入路径 pub | 未收敛（`insert_node`/`remove_node` 仍 pub）；外部消费方仅 html 桥接，维持低风险观察 |
| L5 | gif/webp 解码无测试 | ✅ 已修（`decode_gif_yields_rgba` / `decode_webp_yields_rgba`） |
| L7 | 字体解析重复全量发现 | ⚠ 半遗留：paint 侧 `font_for` 已随重构消失；text 侧 `resolve_font` 转发仍在（本报告 L1，生产无人调用） |
| H1 | 未跟踪目录 | 仍在：`.mimosa/`、`.workbuddy/`、`.zcodeignore` 及 crates 下多个 `.mimosa/` 建议入 `.gitignore` |

## 8. 建议处理顺序

1. **H1 + H2 + H3（安全三件套，一次迭代清完）**：深度上限 + `is_finite` clamp + decode 头部预检/面积上限。三者都是几十 KB 构造输入即可远程触发的进程级 DoS，且修复模式一致（入口 clamp + 降级），建议合并为一轮「不可信输入边界加固」，每项先写 failing 测试（深嵌套 HTML、`width="999…"`、巨图声明）再修。
2. **H4（整形缓存）**：当前最大性能赢面，把与宽度无关的计算提升为文本块级缓存，全量重排开销回落一个档次。
3. **M1 + M2 + M3（chrome 三项）**：`Arc<Image>` + 错误页转义 + 加载代次，改动都不大，收益直接（帧率、隐私、正确性）。
4. **M4 + M6 + M7（渲染/事件循环）**：wgpu 错误降级、resize 合帧、UI 整形缓存——滚动与 resize 的体感改善。
5. **M5 + M10（css/layout 克隆链）**：引用化 + 缓存化，属结构性优化，随下次触碰对应模块时顺带做。
6. **M8 + M9（选区/行内正确性）**：先各补一个 failing 测试钉住行为再修。
7. **M11 + M12 + 低项**：按迭代顺带处理。
8. **流程建议**：引入 `cargo-fuzz` 对 `parse_document` / `parse_stylesheets` / `decode` 三个入口做持续 fuzzing——L4 的 TreeSink 契约 panic 面与本轮全部「构造输入」类发现（H1/H2/H3）都能被 fuzz 语料库长期看住。

## 9. 局限性

- chrome 窗口/GPU 路径无法无头自动化验证，M1/M4/M6 的帧率影响为静态推理，未实测帧数据。
- 性能结论未做 profiling（无火焰图），严重性排序基于代码路径推理与调用频率估算；H4/M5/M6 建议修复前先以大页面基准确认热点归属。
- Mimosa 扫描为静态证据边界（`static_only_no_runtime_execution`），不覆盖运行时行为。
- nexty-css 的 value.rs（3141 行）按函数清单 + 抽样通读，未逐行；其解析全部基于 cssparser token 流迭代器（非手写字节扫描），panic 面经机械检查确认极小（生产 1 处）。
