# AGENTS.md

This file provides guidance to AI coding agents (Codex CLI / Claude Code / etc.) when working with code in this repository.

## Project: Nexty

MusKitty 的**不造轮子**分支。MusKitty 从零手写浏览器核心模块；Nexty 反过来——**能复用成熟 crate 就不自己实现**，只把精力留给真正没有现成轮子的部分。

行为 ground truth 仍是 WHATWG 规范与 WPT 测试套件。Chromium 源码仅作参考。

当前状态：8 个 facade crate 已在根 `Cargo.toml` 注册，外部依赖已接线，`cargo check --workspace` 零 warning；依赖门禁（`cargo deny check`）与第三方依赖清单（`cargo about`）已跑通。

已落地八层（全部骨架 crate 就位，管线端到端可跑）：

- `nexty-dom`（v0.1.2）——自研 arena DOM：节点数据层、树变更算法（pre-insert/insert/remove/replace/clone/normalize）与文档模式控制。
- `nexty-html`（v0.1.4）——html5ever 的 `TreeSink` 桥接到 arena DOM，提供 `parse_document` 与 `parse_fragment`（含片段上下文命名空间）；子资源清单收集（`collect_style_resources` 收集 `<link rel=stylesheet>` 与 `<style>`、`collect_image_resources` 收集 `<img>`，只收集不解析）。
- `nexty-css`（v0.1.8）——cssparser/selectors 封装 + 自研 cascade：属性值解析（hex/rgb()/hsl()/命名色、font-size/weight、font-family、盒模型 margin/padding/border/width/height、line-height、box-sizing/overflow/min-width/min-height/max-width/max-height、flex-grow/flex-basis）、选择器匹配（Selectors 4，`:is()`/`:where()`/`:has()`/`:nth-child(of)`，HTML 大小写规则）、样式表解析（CSS Syntax §5 错误恢复 + 简写展开：margin/padding/border 系、1–4 值顺时针、`border` 按 `||` 语法；`@media` 按视口条件求值，不匹配的规则不参与级联）、cascade（UA/author 双 origin 重要性桶 → 内联样式 → 特异度 → 源顺序，多表按源顺序合并级联，UA 样式表见 `html_ua_stylesheet`）与 computed style（继承/initial、`em`/`%`/绝对尺寸关键字表、`bolder`/`lighter` 映射表、CSS-wide 关键字）。`var()`、user origin、伪元素等见 `crates/nexty-css` 模块文档的偏差清单。
- `nexty-network`（v0.1.5）——reqwest blocking 实现 `NetworkFetcher`（`Send + Sync`，供并发抓取）：GET/HEAD、`Request.headers` 请求头逐条应用、错误映射（URL 解析/协议 → `InvalidUrl`、超时 → `Timeout`、其余 → `Transport`）；`resolve` 以文档 URL 为 base 解析相对子资源地址；回环离线测试覆盖全部错误路径与请求头到达。
- `nexty-text`（v0.1.4）——parley 实现 `TextShaper`：fontique 选字（CSS 字体族列表语义 + 回退）、harfrust 整形，CSS px 单位；`FontMetrics` 供 strut 计算，`FontResolver::resolve_font` 给光栅层提供字体字节。单行整形（white-space 折叠归 layout 层）。注意：parley 0.11 的整形后端是 harfrust 而非 ADR 所列 swash，swash 已移出依赖树。
- `nexty-paint`（v0.1.3）——vello_cpu 实现 `Rasterizer`：`FillRect`（src-over + 覆盖率抗锯齿）、`StrokeRect`（只描边）、`DrawText`（字形经 text 层字体解析，族列表缓存）、`DrawImage`（已解码 RGBA8 按矩形下发，1:1 整数缩放）；`decode` 提供最小图片解码路径（png 可选 feature，RGB/灰度归一为 RGBA8，不支持格式返回 `DecodeError` 而非 panic）；facade 输出非预乘 RGBA8。GPU 后端 `vello_hybrid` 依赖 wgpu surface 由 chrome 层提供后接入，`Rasterizer` trait 即双后端接缝。
- `nexty-layout`（v0.1.10）——盒级几何（block / flex / grid / absolute）由 `taffy` 接管（MIT，Servo/Blitz 采用；`taffy` 依赖常驻，feature 只切能力开关），**inline / 文本布局与 table 仍自研**。taffy 管线三步：`style_map`（`ComputedStyle → taffy::Style` 单向映射，覆盖 display/position/box-sizing/inset/尺寸/margin/padding/border/flex-*/grid-*/gap）、`tree_build`（按 CSS Display §3 生成盒树，`none` 跳过、`contents` 提升、`<img>` 以自然尺寸建叶，维护 `NodeId ↔ taffy Node` 双向映射；行内内容经 `inline` 模块收集后包装成**匿名块 run 叶节点**，measure function 跑自研断行 `inline::build_lines` 提供内在尺寸）、`block`（`compute_layout` 解算后读回 `taffy::Layout` 组装 `Fragment`，行盒并入块容器或承接为匿名块片段）。taffy 覆盖：块级流（margin 折叠 §8.3.1、宽度解析 §10.3.3、min/max 收束 §10.4/§10.7）、flex（含 wrap/shrink/完整对齐值）、grid（轨道/auto-flow）、绝对定位（inset）。自研保留：行内断行（strut §10.8、混合样式 run、空白折叠与贪心断行）、行内原子盒（`inline-block`/`inline-flex`/`inline-table` 按 shrink-to-fit §10.3.7 独立布局，内在尺寸测量见 `intrinsic.rs`）、替换元素尺寸解析。片段树输出（边框盒 + 盒模型 + 文本行）供 paint 消费，`Fragment` 契约冻结。盒级布局完备度约 35% → 约 70–75%（grid / 多行 flex / 绝对定位借 taffy 补齐）。**未实现**：table、float、`position: fixed` 视口锚定与「最近定位祖先」包含块搜索（taffy 相对树内父盒定位）、UAX#14 完整断行（偏差清单见 `crates/nexty-layout/src/block.rs` 与 `style_map.rs` 模块文档）。模块功能按 feature 拆分（`block`/`inline`/`flex`/`replaced`/`taffy-map`/`grid`/`absolute`，`table` 预留），见下方Feature 映射表。
- `nexty-chrome`（v0.1.15）——浏览器外壳：`pipeline`（HTML → 级联 → 布局 → 片段树 → paint 显示列表，含 CSS 边框转四条填充带、文本 run 绝对坐标与**行偏移累加**、行内/块级图片与行内原子盒的 `DrawImage`/递归下发、多表级联与子资源并发抓取编排、`hit_test`/`hit_test_ex`/`hover_kind` 悬停语义与 `link_target` 点击命中、`document_title` 提取）、`history`（会话历史栈：push/go_back/go_forward，新导航截断前进分支）、`render`（渲染隔离线程 + `catch_unwind` 兜底，panic 转 `RenderError::Panicked` 且线程存活可继续服务）、`ui`（自绘工具带状态机：后退/前进/刷新按钮（可用标志禁用态）、地址栏**编辑态**——光标/选区/`←→`/Home/End/Shift 扩选/Ctrl+A/Backspace/Delete、点击=聚焦+全选、超宽文本按光标横向滚动、`shortcut()` 纯映射 Chrome 快捷键）、`scrollbar`（滚动几何 + 拖拽状态机：thumb 抓取偏移拖动、轨道点击翻页、悬停配色）、`app`（winit 0.30 + wgpu 29 surface，Pixmap→纹理→全屏 blit 呈现，Rgba8Unorm 优先/BGRA 换色回退；页面滚动（滚轮/PageUp/PageDown/Home/End/方向键）+ 可拖滚动条、resize 重排、链接点击导航、悬停系统光标（链接手型/文本 I 形）、Ctrl+L/Ctrl+R/F5/Esc 与 Alt+方向键、`Arc<dyn NetworkFetcher>` 接驳缝 + 30s 超时 + User-Agent、窗口标题随 `<title>`；**无头环境不可自动化测试**，实机验证）。入口 `cargo run -p nexty-chrome --bin nexty`。

`nexty-html` 带 WPT tree-construction 比对 harness：语料钉在 `web-platform-tests/wpt` commit `5cd8e3fa`，当前 **1854/1959 通过**。剩余 105 条已逐条入基线，分两类：**88 条语料过时**——`processing-instructions.dat` 等期望产出 PI 节点，而现行 WHATWG §13.2.5 已无处理指令词法状态（`<?…>` 在 tag open state 走 bogus comment），html5ever 的产出与规范一致；**17 条非语料问题**——11 条 html5ever 树构建未跟进规范（`in select` 模式、`<selectedcontent>` 克隆、`<template>` 的 frameset-ok/form 指针语义），6 条需 JSRT 的 scripted 用例。分词器不换：MusKitty 分词器的处理指令状态实现的是规范已删除的特性，评估与否决理由见 [docs/decisions/2026-10-01-muskitty-tokenizer-rejected.md](docs/decisions/2026-10-01-muskitty-tokenizer-rejected.md)。比对方式与基线见 `crates/nexty-html/tests/tree_construction.rs`。

八层骨架均已有内部实现，管线 network→html→dom→css→layout→paint→chrome 端到端连通（`cargo run -p nexty-chrome --bin nexty` 起窗口，地址栏输入 URL 导航，支持滚动与滚动条拖拽、链接点击、悬停光标、地址栏编辑与快捷键、后退/前进/刷新）。后续深化方向：paint 指令集扩展（圆角/渐变）、vello_hybrid GPU 后端接入、layout 补齐 float 与 table（grid/定位/多行 flex 已由 taffy 落地）、**页面文本选择与复制**（需 text 层 cluster→字符映射与 layout 层 `TextRun` 携带原文，另轮立项）。分层与 crate 选型见 [docs/decisions/2026-10-01-crate-selection.md](docs/decisions/2026-10-01-crate-selection.md)。

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
| 绘制 / 光栅 | `vello_cpu`（已落地）+ `vello_hybrid`（待 chrome 接入） | Apache-2.0/MIT | 已定（双后端，API 同形不同名，facade 需适配层） |
| 窗口 / 输入 | `winit` + `wgpu` | Apache-2.0/MIT | 已定 |
| 网络 | `reqwest` | MIT/Apache-2.0 | 已定 |
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
- **许可证政策**：白名单 MIT / Apache-2.0 / BSD / ISC / MPL-2.0 / LGPL / Zlib / CC0-1.0 / Unicode-3.0 / CDLA-Permissive-2.0，禁 GPL / AGPL。后 4 个是已选 crate 栈（wgpu / icu4x / reqwest）无法回避的宽松、非 copyleft 传递依赖，逐条理由见 `deny.toml`。`cargo-deny` 做硬门禁（含传递依赖），`cargo-about` 生成的第三方依赖清单入库。引入或升级依赖前必须确认传递依赖未引入白名单外的许可证
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
| nexty-layout | `block` | `block` 块级流（taffy 解算 + Fragment 回填，隐含 `inline`） | ✅ |
| | `inline` | `inline` 行内流与匿名块（断行永久自研） | ✅ |
| | `flex` | flex 容器 blockify（tree_build）+ `Display::Flex` 映射（style_map），几何由 taffy flexbox 解算（隐含 `block`） | ✅ |
| | `replaced` | `block::is_image_element` 块级 `<img>` 替换盒 | ✅ |
| | `taffy-map` | `style_map` `ComputedStyle → taffy::Style` 映射层 + `tree_build` 盒树构建（隐含 `inline`） | ✅ |
| | `grid` | `style_map` grid-* 映射 + `Display::Grid`（taffy grid 算法解算；item 经块容器走法收集） | ✅ |
| | `table` | 预留：表格布局 | ❌ 未实现 |
| | `absolute` | `style_map` position/inset 映射（taffy 绝对定位解算，相对树内父盒） | ✅ |
| nexty-paint | `cpu` | `VelloCpuRasterizer` + `decode`（`vello_cpu`/`image`） | ✅ |
| | `gpu` | 预留：`vello_hybrid`，待 chrome 接入 wgpu surface | ❌ 未实现 |
| nexty-network | `fetch` | `ReqwestFetcher` / `NetworkFetcher` | ✅ |
| | `resolve` | `resolve` 相对 URL 解析（WHATWG basic URL） | ✅ |
| nexty-chrome | `pipeline` | `pipeline` 导航管线 | ✅ |
| | `resources` | `resources` 子资源抓取与解码编排 | ✅ |
| | `render` | `render` 渲染隔离线程 | ✅ |
| | `history` | `history` 会话历史栈 | ✅ |
| | `ui` | `ui` 自绘工具带状态机 | ✅ |
| | `scrollbar` | `scrollbar` 滚动几何与拖拽状态机 | ✅ |
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