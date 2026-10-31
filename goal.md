# goal.md — 批次 1 并行轮（执行中）

本轮任务定义与退出条件的唯一权威版本：
[docs/plans/2026-10-31-round-batch1-parallel.md](docs/plans/2026-10-31-round-batch1-parallel.md)。
本文件是根目录工作副本：并行执行状态跟踪 + 收口记录，随轮推进更新。

## 并行执行状态

| 泳道 | 任务 | 状态 | 证据 / 备注 |
| --- | --- | --- | --- |
| A (K1) | T1 dom event 模块 | ✅ 已完成 | 46 测试全绿（新增 12）；Event/EventTarget/传播/数据面；dom 0.1.4；微任务队列落为文档级缝隙（偏差已记） |
| A (K1) | T2 EventLoop + 生命周期类型 | ✅ 已完成 | FIFO/不重入（类型结构性保证）/生命周期字段单测；与 T1 同线 |
| A (K1) | T3 chrome 接线 | ✅ 已完成 | dispatch-then-default 全接线 + DCL/load 生命周期；93 测试全绿；！Send 以「Send 数据面+主线程建 Document」解决（两次解析成本记 K5 决策窗口）；chrome 0.1.26 |
| B (K2) | T0 vello_cpu 侦察 | ✅ 已完成 | vello_cpu 0.2.0 三项能力原生支持（fill/stroke_path+kurbo 圆角、peniko::Gradient 渐变）；自算项：重叠半径缩减、边框描边内缩、CSS 角度→渐变端点；报告 .workbuddy/t0-k2-recon/T0-report.md |
| B (K2) | T1 css 新属性解析 | ✅ 已完成 | 101 测试全绿（新增 15）；8 值类型+简写展开+ComputedStyle 6 字段；css 0.1.16；UA 表未加链接下划线（未决项交 T3） |
| B (K2) | T2 paint 指令 | ✅ 已完成 | FillRoundedRect/StrokeRoundedRect/FillGradient + GradientStop 自有类型；translated 补臂（渐变端点同步平移）；CSS §5.5 半径缩减 + 描边内缩在 facade；paint 0.1.8 |
| B (K2) | T3 pipeline 显示列表 | ✅ 已完成 | 子代理主体完成，被挂起中断在收尾；编排者补两处：①渐变四正向改精确轴向量（f32 90° 噪声）②选区高亮测试载体迁移（body 背景已传播到画布，白底挪入包裹 div）；107 测试全绿；chrome 0.1.27；对照页 .workbuddy/t-k2/k2-visual.html |
| D (K3) | T1–T4 network | ✅ 已完成 | url 2.5.8 直依赖（resolve 不再拉 reqwest）+ encoding_rs 0.8.42 解码入口 + 重定向最终 URL/显式 policy + cache feature（CachedFetcher，回环测试矩阵）；network 0.1.6；API 清单在报告 |
| D (K3) | T5 chrome 导航接线 | ✅ 已完成 | CachedFetcher 栈 + 四层解码（GBK 不再进错误页）+ final_url base + 地址栏规范化；115 测试全绿；chrome 0.1.28；ADR addressbar-url；编排者补 chrome 卫生（clippy 2 类 + fmt，K2 残留） |
| E (K4) | T1 css @font-face | ✅ 已完成 | @font-face at-rule 解析（复用 @media 基建）；14 单测；FontFaceRule/Source/Style/Display 导出；css 0.1.17；嵌套 @font-face 记偏差 |
| E (K4) | T2 text 字体注册表 | ✅ 已完成 | FontRegistry + resolve_font_with + with_registry 接线；5 个注册表测试；SpaceMono Regular/Bold + OFL 入库 tests/data/；text 0.1.9；偏差已记模块文档 |
| E (K4) | T3 chrome 字体编排 | ✅ 已完成 | FontOrchestrator collect/fetch/register 会话去重 + with_registry 接线 + 就绪恰一轮重排；121 测试全绿、11 组 feature 零 warning；chrome 0.1.29 |
| B (K2) | UA 表补 `a{text-decoration:underline}` | ✅ 已完成 | ua.rs 追加规则 + 镜像单测；css 0.1.18；:link/:visited 近似偏差已记；cursor/颜色属伪类轮 |
| 收口 | 批次门禁 + 分 crate 提交 | 执行中（AGENTS chrome 段已补；workspace 门禁进行） | |

chrome 接线串行链：K1 T3 → K3 T5 → K4 T3；K2 T3 与 K1 T3 分文件并行。

## 前置状态记录

- float 轮主体已合入：`952d723`（css float/clear，0.1.15）、`9821385`
  （layout float_layout，0.1.28）、`6d62b99`（float wrap 几何）。
- float 轮收尾 WIP 未提交（crates/nexty-layout、AGENTS.md、Cargo.lock、
  docs/decisions/2026-10-01-crate-selection.md、docs/plans/2026-10-31-round-float.md）
  ——归 float 轮所有者，本轮不触碰、提交时按文件精确 git add 规避。
- 起点版本：css 0.1.15 / dom 0.1.3 / network 0.1.5 / text 0.1.8 /
  paint 0.1.7 / chrome 0.1.25 / layout 0.1.29（WIP）。
- 预期落点：dom 0.1.4、network 0.1.6、text 0.1.9、paint 0.1.8、
  css 0.1.17（K2 T1→0.1.16、K4 T1→0.1.17）、chrome ≥ 0.1.26。
