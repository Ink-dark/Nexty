# goal.md — 本轮任务：落地 nexty-text，补全 nexty-network 与 nexty-paint

日期：2026-10-01。前置：nexty-dom / nexty-html / nexty-css 已落地。本轮做「有现成
轮子」的三层：text（主任务）、network、paint——三者骨架 trait 已定，本轮补内部
实现。layout（自研）与 chrome（integration，依赖本轮三层）留待后续轮。

## 任务清单与退出条件

### T1 nexty-network：ReqwestNetworkFetcher
- 任务：用 reqwest（blocking）实现既有 `NetworkFetcher` trait。URL 解析失败 →
  `InvalidUrl`；超时 → `Timeout`；其余传输错误 → `Transport`。响应头、状态码、
  响应体逐项透传；重定向跟随交给 reqwest 默认行为。
- 退出条件：离线回环测试（std TcpListener 起本地 HTTP 服务）覆盖
  success（GET/HEAD、状态码、响应头、响应体）、`InvalidUrl`、`Timeout` 三条路径；
  `cargo check` 零 warning。

### T2 nexty-text：parley 后端实现 TextShaper
- 任务：`TextStyle` 扩为字体族列表（命名族 + CSS generic 族关键字），实现
  `ParleyTextShaper`：fontique 选字体（按族列表顺序，generic 关键字映射），
  parley 布局出字形（id/位置/advance），单位 CSS px（scale = 1）。
- 退出条件：单测覆盖——空文本零输出、非空文本字形非空且 advance/width 为正、
  字号缩放比例正确、字体族列表与 generic 族不 panic（回退路径）、族列表为空
  → `FontUnavailable`。上游类型不进入 pub 导出。

### T3 nexty-paint：VelloCpuRasterizer
- 任务：用 vello_cpu 实现 `Rasterizer`（本轮指令集仅 FillRect）。CPU 后端本轮
  落地；GPU 后端（vello_hybrid）依赖 wgpu device/surface，由 chrome 层提供，
  **本轮从 nexty-paint 摘除 vello_hybrid 依赖**并在模块文档记录（ADR 已注明
  GPU 侧后续评估），Rasterizer trait 即双后端接缝。
- 退出条件：单测——FillRect 像素级验证（矩形内颜色、矩形外空白）、零尺寸
  → `EmptySize`、多矩形叠加顺序正确。

### T4 门禁与收尾
- `cargo check --workspace` 零 warning；`cargo test --workspace` 全绿；
  clippy `-D warnings` 零告警；`cargo fmt --all -- --check` 通过。
- 本轮摘除 vello_hybrid 依赖 → 依赖树变化 → `cargo deny check` +
  `cargo about generate` 重跑入库。
- 三个 crate 版本 0.1.0 → 显式 0.1.1（骨架 → 落地 +1 patch，对齐先例）。
- AGENTS.md「当前状态」更新（已落地六层）。
- 分层 commit（network / text / paint 各一笔），最终 push。

## 非目标（本轮不做）
- 渲染隔离线程与 `catch_unwind` 兜底（chrome 层持有，见选型 ADR）
- vello_hybrid GPU 后端（等 chrome 提供 wgpu surface）
- 文本多行/换行/bidi（white-space 折叠是 layout 层职责；本轮 shape 单行）
- @font-face 与自定义字体加载（fontique 系统字体集合）
- 网络并发/流式、HTTPS 证书定制

## 验证流（AGENTS.md Verification Flow）
每层：cargo check 零 warning → cargo test 全绿 → fmt/clippy → commit。
WPT 语义比对以规范引用单测替代（network/text/paint 无对应 WPT 语料钉版任务），
架构师复核后允许 push。
