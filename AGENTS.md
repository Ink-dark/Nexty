# AGENTS.md

This file provides guidance to AI coding agents (Codex CLI / Claude Code / etc.) when working with code in this repository.

## Project: Nexty

MusKitty 的**不造轮子**分支。MusKitty 从零手写浏览器核心模块；Nexty 反过来——**能复用成熟 crate 就不自己实现**，只把精力留给真正没有现成轮子的部分。

行为 ground truth 仍是 WHATWG 规范与 WPT 测试套件。Chromium 源码仅作参考。

当前状态：8 个 facade crate 已在根 `Cargo.toml` 注册，外部依赖已接线，`cargo check --workspace` 零 warning；依赖门禁（`cargo deny check`）与第三方依赖清单（`cargo about`）已跑通。

已落地两层：

- `nexty-dom`（v0.1.1）——自研 arena DOM：节点数据层、树变更算法（pre-insert/insert/remove/replace/clone/normalize）与文档模式控制。
- `nexty-html`（v0.1.1）——html5ever 的 `TreeSink` 桥接到 arena DOM，提供 `parse_document` 与 `parse_fragment`（含片段上下文命名空间）。

`nexty-html` 带 WPT tree-construction 比对 harness：语料钉在 `web-platform-tests/wpt` commit `5cd8e3fa`，当前 **1854/1959 通过**。剩余 105 条已逐条入基线，分两类：**88 条语料过时**——`processing-instructions.dat` 等期望产出 PI 节点，而现行 WHATWG §13.2.5 已无处理指令词法状态（`<?…>` 在 tag open state 走 bogus comment），html5ever 的产出与规范一致；**17 条非语料问题**——11 条 html5ever 树构建未跟进规范（`in select` 模式、`<selectedcontent>` 克隆、`<template>` 的 frameset-ok/form 指针语义），6 条需 JSRT 的 scripted 用例。分词器不换：MusKitty 分词器的处理指令状态实现的是规范已删除的特性，评估与否决理由见 [docs/decisions/2026-10-01-muskitty-tokenizer-rejected.md](docs/decisions/2026-10-01-muskitty-tokenizer-rejected.md)。比对方式与基线见 `crates/nexty-html/tests/tree_construction.rs`。

其余层（css / layout / text / paint / network / chrome）的内部实现尚未落地。分层与 crate 选型见 [docs/decisions/2026-10-01-crate-selection.md](docs/decisions/2026-10-01-crate-selection.md)。

## Build & Test Commands

workspace 根目录（`Ink-dark/Nexty`）：

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

# 依赖门禁与许可证清单（引入/升级任何依赖后必跑）
cargo deny check                         # 白名单见根目录 deny.toml，扫传递依赖
cargo about generate about.hbs -o docs/dependencies.html   # 重新生成第三方依赖清单

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
│   ├── nexty-layout/                   #   自研盒级布局
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
| 盒级布局 | 自研 | — | 已定 |
| 文本整形 / 字体 | `parley` + `swash` + `fontique` | Apache-2.0/MIT | 已定 |
| 绘制 / 光栅 | `vello_cpu` + `vello_hybrid` | Apache-2.0/MIT | 已定（双后端，API 同形不同名，facade 需适配层） |
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