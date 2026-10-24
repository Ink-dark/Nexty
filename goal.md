# goal.md — 本轮任务：不可信输入边界加固（DOM 深度上限 / 数值卫生 / 解码与响应体预算 / 错误页转义）

日期：2026-10-24。前置：页面文本选择与复制轮已完成（layout 0.1.17 /
chrome 0.1.18），旧 goal 归档于
[docs/plans/2026-10-23-round-text-selection-copy.md](docs/plans/2026-10-23-round-text-selection-copy.md)。
本轮触发依据：[2026-10-24 全仓库性能与安全审查](docs/2026-10-24-perf-security-review.md)
（只读审查，未改代码）发现 4 项高严重性中有 3 项是「几十 KB 构造输入 →
进程级 DoS」，且全部**绕过**渲染隔离线程的 `catch_unwind` 兜底。
当前版本：css 0.1.11 / text 0.1.7 / layout 0.1.17 / paint 0.1.4 /
html 0.1.4 / dom 0.1.2 / network 0.1.5 / chrome 0.1.18。

## 本轮为什么做这些

审查报告 §8 的第 1 梯队。三个高风险共享同一性质——**入口不设防，失败模式
不可恢复**：

- **H1 深层 DOM 栈溢出**：管线五层（css `walk`、layout `build_box` ↔
  `collect_children_boxes` / `assemble_element` / `Builder::collect` /
  `collect_children`、html `collect_from`、dom `clone_node`、chrome
  `document_title`）对 DOM 深度无界递归。栈溢出在 Rust 中是 **abort 不是
  panic**，`catch_unwind` 兜不住——任意网页可远程击穿「渲染失败不拖垮主进程」
  的硬约束底线。
- **H2 `f32::INFINITY` 注入**：`<img width="999…9">`（40+ 位）经
  `attribute_px` 的 `parse::<f32>()` 得 `Ok(inf)` 且通过 `> 0.0` 过滤，
  inf/NaN 沿 taffy / 行内几何扩散污染整页坐标；CSS 值层（`font-size: 1e40px`
  类）同样无 clamp。整条管线没有任何 `is_finite()` 检查。
- **H3 OOM abort**：图片解码的尺寸校验发生在 `decode()` **之后**——几 KB 的
  PNG 声明 `65535×65535` 即先分配约 17 GB 再检查；网络响应体无字节上限，
  chrome 编排层无预算。分配失败是 abort，同样兜不住。
- 顺带 **M2 错误页标记注入**：导航失败的 URL / 错误串未转义直接拼进错误页
  HTML，可触发第三方子资源请求、泄露用户正在访问的 URL。

四项修复模式一致（入口 clamp + 降级语义），适合一轮收口。

### 为什么插队在 justify 之前

selection 轮的 roadmap 把「justify + CSS Text 排版细化」排为下一轮，本轮将其
顺延：justify 是视觉保真，H1–H3 是**远程可触发的进程死亡**，且违反 AGENTS.md
明文的渲染隔离硬约束——安全债优先于功能债。性能项（H4 整形缓存等）同理
顺延：它们影响帧率而非存活。

### 防线设计取舍（单点 vs 九点）

审查报告列了九条递归链。逐点加深度计数会随未来新增遍历继续漏防（本轮九条
正是这么长出来的）。本轮采用**单点主防线**：DOM 层维护节点深度，HTML 解析
入口截断超深内容，使树深度全局有界；下游各递归点无需各自计数（512 层 ×
每层约百字节栈帧 ≪ 默认 8 MB 栈预算）。`build_box` 加 `debug_assert` 作纵深
防御。取舍记录：截断意味着超深内容按「资源防护降级」语义消失（浏览器同样
对超深嵌套截断），记为已知偏差，不尝试按 CSS 语义保留。

## 任务清单与退出条件

### T1 nexty-dom + nexty-html：DOM 深度上限主防线（dom 0.1.2 → 0.1.3、html 0.1.4 → 0.1.5）

- dom：`Node` 私有字段 `depth: u32`（文档根 0）；`link_before` 挂接时
  `depth = parent_depth + 1`，detach 不清（游离节点 depth 无意义，重挂时覆盖）。
  免校验 `insert_node` 同经 `link_before`，维护不漏。`set_template_contents`
  时片段根 depth 置 0（模板内容是独立片段，深度独立起算）。新增
  `pub const MAX_TREE_DEPTH: usize = 512` 与 `Document::depth(NodeId) -> u32`
  只读访问器（常量与访问器均为自有类型，不涉依赖外泄）。
- html：`HtmlTreeSink` 的 `append` / `append_based_on_parent_node` 挂接路径上，
  若 `depth(parent) + 1 > MAX_TREE_DEPTH` 则丢弃该节点（不插入；其后续子内容
  挂到游离节点上，`finish` 后自然不可达）。截断只影响树形状，不触碰 html5ever
  状态机本身。
- layout 纵深防御：`build_box` / `collect_children_boxes` 入口
  `debug_assert!(depth <= MAX_TREE_DEPTH)`。
- 退出条件：
  - dom 单测：append / insert_before / replace / detach 后重挂，depth 正确
    （重挂到不同深度父节点会变）；template 内容片段独立从 0 起算。
  - html 测试：10 万层 `<div>` 嵌套的 `parse_document` 在有限时间内完成不崩，
    产出树中任意节点 depth ≤ 512，被截断内容不在树里；`parse_fragment` 不受
    影响（片段上下文深度另算或同样受限，写测试钉住选择的行为）。
  - WPT tree-construction 1854/1959 基线**不回归**（正常语料深度远小于 512）。

### T2 nexty-css + nexty-layout：数值卫生（css 0.1.11 → 0.1.12、layout 0.1.17 → 0.1.18）

- layout `attribute_px`：前缀解析对齐 HTML 尺寸属性规则（实现前核对
  WHATWG §2.4 的 dimension 解析，当前 `take_while` 接受 `'.'` 的行为按规范
  裁决）；解析结果必须 `is_finite()` 且 ≤ `MAX_CSS_LENGTH = 1e7` px，否则返回
  `None` 回落自然尺寸。
- css 值层：`f32` 长度/字号/字重类解析出口统一 clamp 到 `[-1e7, 1e7]` 并滤除
  非有限值（Chromium 传统上限同量级；超出按 UA clamp 记偏差）。
- layout 几何入口：`map_style` 产出的 taffy definite size、`build_lines` 的
  行内坐标，遇非有限值降级为 auto / 0——污染隔离在单元素，不向兄弟与整页扩散。
- 退出条件（failing-first，当前注入会扩散/溢出）：
  - `<img width="999…9（45 位）">`：块级与行内两条路径的几何均为有限值；
    同行其他元素坐标不被污染。
  - `font-size: 1e40px`、`margin-top: calc` 外的裸 `1e40px`、`width: 1e39em`
    等：computed 值有限、页面可布局。
  - 数值边界单测：恰好 1e7 通过、超限回落、NaN/inf 拒绝。

### T3 nexty-paint + nexty-chrome：解码与响应体预算（paint 0.1.4 → 0.1.5、chrome 0.1.18 → 0.1.19）

- paint `decode`：`ImageReader` 先 `into_dimensions()` 只读头部（不解码像素），
  校验边长 ≤ 16384 且 `w.checked_mul(h) ≤ 2^26`（约 256 MB RGBA 缓冲上限），
  超限返回 `SizeUnsupported`；通过后再 `decode()`。doc 注明「预检失败与
  不可渲染同错误变体」，`RasterError::SizeExceedsLimit` 的双后端语义描述同步。
- chrome `resources`：`fetch_ok` 增加按类别的字节预算常量（HTML/CSS 10 MB、
  图片 50 MB 量级，集中定义并写注释），`Response.body` 超预算按既有降级语义
  跳过该资源。预算放编排层，`NetworkFetcher` trait 与 network crate 本轮不动。
- 退出条件：
  - 测试内手工拼 PNG 字节（8 字节签名 + IHDR 声明 65535×65535 + IEND）：
    `decode` 返回 `SizeUnsupported` 且测试正常跑完（即未发生巨量分配）。
  - 声明 16385×1、1×16385、`2^26` 边界内外的尺寸各一条断言。
  - 本地回环服务器（沿用 network 测试模式）返回超预算 body → `fetch_ok`
    返回 `None`；预算内路径回归绿。
  - 既有 26 条 paint 测试 + gif/webp 解码测试不改仍绿。

### T4 nexty-chrome：错误页转义（与 T3 同版 bump）

- `app.rs` 错误页 `format!` 前，对 `url` 与错误串做 HTML 实体转义
  （`<` `>` `&` `"` `'`）；错误串收敛为错误类别 + 状态码，不再整段回显
  reqwest 原始错误（其内含目标 URL）。
- 退出条件：构造 load 失败且 URL 含 `<img src=//x/1>` 的用例——错误页 DOM
  中无 `img` 元素、无对第三方地址的子资源请求（`collect_image_resources`
  为空）、显示文本为字面转义后的 URL；正常 URL 的错误页显示不受影响。

### T5 门禁、文档与版本

- 全套门禁：`cargo check --workspace` 零 warning；`cargo test --workspace`
  全绿；`cargo clippy --workspace --all-targets -- -D warnings`；
  `cargo fmt --all -- --check`；`cargo deny check` 通过。
- feature 组合零 warning：各 crate `--no-default-features --features <单开>`
  逐一验证（本轮不新增 feature——深度/数值/预算是核心行为，不是可拆能力，
  Feature 映射表不动）。
- **全链加固回归测试（本轮验收旗舰）**：5000 层 `<div>` 嵌套 + 巨数属性 +
  `<img>` 指向巨图声明的组合页面，走 parse → cascade → layout → TextMap →
  `document_title` → `collect_*_resources` 全链，不崩、有限时间、几何有限。
- AGENTS.md 同步：八层状态行补「输入边界防护」（深度 512 截断 / CSS 长度
  clamp 1e7 / 解码面积 2^26 / 响应体按类别预算），已知偏差清单登记
  「超深内容截断」「超限长度 clamp」两条。
- 版本 bump：dom 0.1.3 / html 0.1.5 / css 0.1.12 / layout 0.1.18 /
  paint 0.1.5 / chrome 0.1.19；text / network 本轮无代码改动不动。
- 分任务 commit（`[dom]` / `[html]` / `[css]` / `[layout]` / `[paint]` /
  `[chrome]` / `[docs]`），退出条件全部满足后 push。

### T6 实机验收（无头不可测项，轻量）

按项目记忆的 Computer Use 流程与 `.workbuddy/t3-test` 资产：

- 深嵌套页面（本地 serve 5 万层 div）：打开不崩、可滚动、标题栏正常。
- 巨图声明页面（本地 serve 恶意 PNG）：不崩，显示破图/空白而非拖垮进程。
- 错误页注入样例（地址栏输入含 `<img ...>` 的失效 URL）：错误页无注入痕迹，
  URL 字面显示。
- 正常页面（t3-test 既有资产）回归目测：渲染、选择、滚动无变化。

## 非目标（本轮不做）

- **性能项全部顺延**：H4 整形/度量缓存、M1 `Arc<Image>`、M4–M7、M10–M12 与
  全部 L 项——留待下一轮「性能：文本整形缓存与渲染热路径」（审查 §8 第 2、4 梯队）。
- **M3 页面加载竞态**（generation counter）：正确性而非输入边界，并入下轮
  chrome 小项。
- **M8 TextMap 行遮蔽 / M9 行内子树块级子元素静默丢弃**：布局正确性独立轮次
  处理（各需先写 failing 测试钉行为）。
- **cargo-fuzz 基建**：Windows 环境需 nightly + libFuzzer，另立基础设施任务；
  本轮以构造输入回归测试（T1–T5 的 failing-first 用例 + 旗舰全链测试）兜底。
- **network trait 层流式读取 / Content-Length 预检**：编排层预算已挡住本轮
  风险面，trait 演进随 JSRT 轮再议。
- **M12 `remap_glyph` 无界索引**等低项、float / table / justify 等既有
  路线图缺口。

## 方向评估（为什么是这轮）

| 候选 | 价值 | 成本 | 结论 |
| --- | --- | --- | --- |
| **不可信输入边界加固（本轮）** | 消除 3 条远程进程级 DoS 路径 + 1 条注入；修复模式统一（入口 clamp + 降级），一轮收口 | 中：dom/html 深度机制 + 三层 clamp/预算 + 测试；单点防线改动集中 | ✅ 本轮 |
| H4 整形缓存等性能轮 | 全量重排开销回落一个档次 | 中：TextChunk 缓存 + chrome 热路径 | 下一轮 |
| justify + CSS Text 细化 | 视觉保真 | 小–中 | 顺延（原 roadmap 下一轮让位） |
| M8 + M9 布局正确性 | 选区/内容可见性 | 中：需 failing-first 行为钉住 | 后续轮 |

## 验证流（AGENTS.md Verification Flow）

每个子任务先写 failing 测试看它红，再实现到绿（H1 的深嵌套、H2 的巨数注入、
H3 的巨图声明在当前 HEAD 上均应红——以崩溃/几何无限/分配失败的形式）；
每层 `cargo check` 零 warning → `cargo test` 全绿 → `fmt` / `clippy` → commit；
WPT 基线（1854/1959）与既有全部测试不改仍绿是硬前提。T5 旗舰全链测试是本轮
完成的判据性用例。退出条件全部满足后 push。
