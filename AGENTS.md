# AGENTS.md

This file provides guidance to AI coding agents (Codex CLI / Claude Code / etc.) when working with code in this repository.

## Project: Nexty

MusKitty 的**不造轮子**分支。MusKitty 从零手写浏览器核心模块；Nexty 反过来——**能复用成熟 crate 就不自己实现**，只把精力留给真正没有现成轮子的部分。

行为 ground truth 仍是 WHATWG 规范与 WPT 测试套件。Chromium 源码仅作参考。

当前状态：仓库刚初始化，workspace 骨架为空（`members = []`），模块划分尚未确定。

## Build & Test Commands

> 注意：当前 `members` 为空，virtual workspace 下 `cargo check/test` 会报
> "The manifest is virtual, and the workspace has no members"。以下命令在
> `members` 里加入第一个 crate 后才可用。

workspace 根目录（`Ink-dark/Nexty`）：

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

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
├── Cargo.toml                          # members = []（空骨架，后续按需添加）
├── AGENTS.md                           # 硬约束指南（本文件）
├── README.md                           # 项目 README
├── .gitignore
├── crates/                             # workspace member 目录（当前为空）
└── docs/
    ├── decisions/                      # 架构决策记录（ADR）：依赖选型、自研决策
    └── plans/                          # 阶段计划文档
```

## Hard Rules

### Technical
- Rust stable，**仓库内代码严禁 unsafe**：本仓库自研代码一律不得出现 unsafe 块 / unsafe fn / unsafe impl，无例外、无 FFI 豁免。每个 crate 在 `lib.rs` 顶部用 `#![forbid(unsafe_code)]` 固化该约束。第三方 crate 内部的 unsafe 不由我们改写（`forbid` 也不作用于依赖），但选型时必须查其 unsafe 用量与边界
- **依赖类型不得外泄**：每个 crate 的公共 API 只暴露自身抽象类型。依赖 crate（html5ever / cssparser / taffy / …）的类型不得出现在任何 `pub` 导出中——含 pub fn 签名、pub struct/enum 的 pub 字段、pub trait 的方法签名、`pub use` re-export。跨界传递依赖类型时，包一层自己的类型再暴露
- **不造轮子优先**：已有成熟 crate 能覆盖的能力，直接依赖，不自己实现。参考优先级：**WHATWG 规范 > WPT 测试套件 > 成熟 crate > 自行实现**
- 引入依赖前先查：维护活跃度、许可证兼容性（Apache-2.0 / MIT）、unsafe 用量、依赖树体积、是否贴合 spec 行为
- 每个新依赖在 `docs/decisions/` 记一条 ADR，写清"为什么用它、为什么不是别的、边界在哪"
- 自研仅限两种情况：① 没有可用轮子；② 现有轮子行为不符合 spec。两者都必须在 ADR 里写明"现有 crate 为什么不够用"
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