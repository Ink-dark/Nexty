# goal.md — 批次 2 并行轮（执行中）

本轮任务定义与退出条件的唯一权威版本：
[docs/plans/2026-10-31-round-batch2-parallel.md](docs/plans/2026-10-31-round-batch2-parallel.md)。
本文件是根目录工作副本：并行执行状态跟踪 + 收口记录，随轮推进更新。

分支：`feat/batch2-jsrt-scroll-images`（自 main @ 1415cec 切出，已 push）。
交付纪律：分支直接 push、收口由代理开 PR（用户指令 2026-10-31）。

## 并行执行状态

| 泳道 | 任务 | 状态 | 证据 / 备注 |
| --- | --- | --- | --- |
| A (K5) | T0 选型侦察 + ADR | 🔄 进行中 | Boa vs rquickjs(QuickJS-ng) 侦察；含 Document !Send 决策 |
| A (K5) | T1 dom jsrt 模块 | ❌ 未开始 | 等 T0 定案 |
| A (K5) | T2 runner 适配 crate | ❌ 未开始 | 等 T1 接缝 |
| A (K5) | T3 chrome 脚本接线 | ❌ 未开始 | |
| A (K5) | T4 WPT scripted 6 条 + 登记 | ❌ 未开始 | |
| C (K6) | T0 接口先行 commit | 🔄 进行中 | clip 指令 / 滚动元数据 / overflow 双轴类型 |
| C (K6) | T1 css overflow 完备化 | 🔄 进行中 | css → 0.1.19 |
| C (K6) | T2 layout 滚动几何 | ❌ 未开始 | layout → 0.1.30 |
| C (K6) | T3 paint clip 指令 | ❌ 未开始 | vello_cpu 能力探针 |
| C (K6) | T4 chrome clip 栈/滚动条/滚轮链 | ❌ 未开始 | |
| C (K6) | T5 验收资产 + 登记 | ❌ 未开始 | |
| S (K13) | T1 paint decode jpeg/gif/webp | 🔄 进行中 | paint → 0.1.9 |
| S (K13) | T2 chrome favicon | ❌ 未开始 | |
| S (K13) | T3 验收资产 | ❌ 未开始 | |
| 收口 | workspace 门禁 + deny/about + bench 冒烟 | ❌ 未开始 | |
| 收口 | AGENTS/路线图登记 + 执行报告 | ❌ 未开始 | |
| 收口 | push + PR | ❌ 未开始 | |

## 决策记录（轮内）

- unsafe 口径：kernel facade crate 维持 forbid；runner 适配 crate 允许
  unsafe FFI（用户指令 2026-10-31，AGENTS 措辞随 T4 修订）。
- JSRT 引擎与 `Document` !Send 架构：K5 T0 定案，见
  `docs/decisions/2026-10-31-jsrt.md`。
