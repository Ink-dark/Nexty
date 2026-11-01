# goal.md — 批次 2 并行轮（已收口）

本轮任务定义与退出条件的唯一权威版本：
[docs/plans/2026-10-31-round-batch2-parallel.md](docs/plans/2026-10-31-round-batch2-parallel.md)。
执行报告：[docs/plans/2026-10-31-round-batch2-execution-report.md](docs/plans/2026-10-31-round-batch2-execution-report.md)。

分支：`feat/batch2-jsrt-scroll-images`（自 main @ 1415cec 切出，全程直接 push）。
交付纪律：分支直接 push、收口由代理开 PR（用户指令 2026-10-31）——已执行。

## 并行执行状态（终态）

| 泳道 | 任务 | 状态 | 证据 / 备注 |
| --- | --- | --- | --- |
| A (K5) | T0 选型侦察 + ADR | ✅ | rquickjs（QuickJS-NG 0.16.2）定案，28 包 / MSVC check 15.8s；!Send 维持现状；[ADR](docs/decisions/2026-10-31-jsrt.md) |
| A (K5) | T1 dom jsrt 模块 | ✅ | ScriptHost/JsrtGate/DomRegistry/任务泵；dom 0.1.5；65 测试 |
| A (K5) | T2 runner 适配 crate | ✅ | nexty-jsrt-rquickjs 0.1.0，绑定 v0 全清单，实测零 unsafe；24 测试；覆盖率 80.83% |
| A (K5) | T3 chrome 脚本接线 | ✅ | script feature + ScriptSession + K1→JS 成对派发 + 泵/脏位重排；chrome 0.1.34；6 条集成测试 |
| A (K5) | T4 WPT scripted + 登记 | ✅ | 6/6 全过（WPT 1860/1959）；dom 0.1.6 / jsrt-rquickjs 0.1.1 / html 0.1.6 |
| C (K6) | T0 接口先行 | ✅ | OverflowValue 双轴 / Fragment 滚动接缝 / PushClip·PopClip |
| C (K6) | T1 css overflow 完备化 | ✅ | 简写展开 + visible→auto；css 0.1.19；122 测试 |
| C (K6) | T2 layout 滚动几何 | ✅ | scroll feature + scrollable overflow rect；layout 0.1.30；161 测试 |
| C (K6) | T3 paint clip 指令 | ✅ | vello_cpu push_clip_path（无降级）；paint 0.1.10；63 测试 |
| C (K6) | T4 chrome 接入 | ✅ | clip 栈 + 绘制期偏移 + 容器滚动条 + 滚轮链；chrome 0.1.33；6 条集成测试 |
| C (K6) | T5 验收资产 | ✅ | /k6nested /k6hidden /k6bar 三页三处登记 |
| S (K13) | T1 paint decode | ✅ | ico 补齐（jpeg/gif/webp 原已在）；7 测试 + 10 fixture；偏差入模块文档 |
| S (K13) | T2 chrome favicon | ✅ | 编排 + 图标槽；chrome 测试 +6 |
| S (K13) | T3 验收资产 | ✅ | /k13 /k13icon /k13noicon 三页三处登记 |
| 收口 | app favicon 接线 | ✅ | 收口补线：主线程解析 URL + 会话缓存 + 后台抓取 + 跨页门控；chrome 0.1.35 |
| 收口 | workspace 门禁 | ✅ | check / test（688 passed, 0 failed）/ clippy(-D) / fmt 全绿 |
| 收口 | deny / about | ✅ | deny 四项 ok；about 重生成 + 注入（NOTICES 8 / MULTI 386） |
| 收口 | bench 冒烟 | ✅ | 四条机器无关断言全绿（runs=5 / emit=0 / shape 1.00x / metrics≤10） |
| 收口 | AGENTS 登记 | ✅ | 状态段 + Feature 映射 5 行 + Hard Rules unsafe 口径修订 + 选型表 |
| 收口 | push + PR | ✅ | 分支直接 push；PR 由代理开（gh） |

## 决策记录（轮内）

- unsafe 口径：kernel facade crate 维持 forbid；runner 适配 crate 允许
  unsafe FFI（用户指令 2026-10-31）——AGENTS Hard Rules 已修订；当前适配层
  实测零 unsafe。
- JSRT 引擎：rquickjs（QuickJS-NG），Boa 留作可插拔备选（[ADR](docs/decisions/2026-10-31-jsrt.md)）。
- `Document` !Send：维持现状（Send 数据面 + 主线程建 Document），重估触发
  条件写入 ADR；跨层共享统一 `Rc<RefCell<Document>>`。
- foster 落点分支按「last table 父节点是否为元素」（与 WPT 语料对齐，与规范
  步骤 5 文本分歧记偏差）。
