# goal.md — 本轮任务：性能轮（大页面基准 / H4 整形与断行缓存 / M6 resize 合帧与滚动场景缓存 / M1 图片零拷贝 / M7 工具带整形缓存 / M5 级联热路径 / M10 克隆链）

日期：2026-10-25。前置：CSS Text 排版细化 + 行内正确性轮已完成（css 0.1.13 /
layout 0.1.24），justify / text-indent / 字距词距 / tab-size / pre-wrap 悬挂 /
M8 / M9 全部落地，旧 goal 归档于
[docs/plans/2026-10-24-round-css-text-inline-correctness.md](docs/plans/2026-10-24-round-css-text-inline-correctness.md)。
本轮是 roadmap 排定的下一项，任务集来自
[2026-10-24 性能与安全审查](docs/2026-10-24-perf-security-review.md)：H4（断行
热路径最大放大器）+ M1 / M5 / M6 / M7 / M10（§8 建议顺序的第 2–5 梯队里全部
纯性能项）。AGENTS.md 明确要求「动手前先做大页面基准确认热点归属」——T1 的
基准与基线文档是全轮的闸门，不是附庸。
当前版本：css 0.1.13 / text 0.1.7 / paint 0.1.5 / layout 0.1.24 /
chrome 0.1.20 / html 0.1.5 / dom 0.1.3 / network 0.1.5。

## 本轮为什么做这些

- **H4 是断行热路径的最大放大器**：taffy 对每个行内 run 典型发起 2–3 次
  measure（MaxContent / MinContent / 定宽），`build_lines`
  （[inline/mod.rs:831](crates/nexty-layout/src/inline/mod.rs)）每次调用都重建
  `shaped_items`——整形、cluster 映射、断点计算全部与宽度无关却按宽度档位
  成倍重跑；`metrics()`（[inline/mod.rs:1001](crates/nexty-layout/src/inline/mod.rs)）
  以固定探针文本跑完整 parley 布局，同一行 N 个 span 就 N 次。resize 拖动时
  对全文档文本按每档宽度重复，这是「万级元素页面每次 resize 数秒」的主因。
- **M6 让滚动 / resize 的每个事件都走全管线**：每个 `Resized` 事件全量
  重级联 + 重排 + 重整形（[app.rs:646](crates/nexty-chrome/src/app.rs)）；滚动
  只是 origin 变化却整树重建显示列表（每 run glyphs 逐个 map+collect、
  `fonts.clone()`，[pipeline.rs:297](crates/nexty-chrome/src/pipeline.rs)）；
  `pad_rows` 每帧整屏拷贝。这是体感（拖不动、滚不动）的直接来源。
- **M1 每帧图片像素深拷贝两次**：`scene.images.push(source.clone())`
  （[pipeline.rs:542](crates/nexty-chrome/src/pipeline.rs)）+ `.send((scene.clone(), …))`
  （[render.rs:92](crates/nexty-chrome/src/render.rs)）——1920×1080 图一帧约
  16.6 MB ×2，多图 60fps 下分配带宽 GB/s 量级。
- **M5 级联 O(元素 × 规则) 线性扫描**：无规则分桶、每命中声明 `clone()`
  （[cascade.rs:377,443](crates/nexty-css/src/cascade.rs)）、内联样式每次级联
  重新解析（[cascade.rs:452](crates/nexty-css/src/cascade.rs)）、`namespace_url`
  每层祖先一次 String 分配（[selector.rs:291](crates/nexty-css/src/selector.rs)）。
  首屏与 resize recascade 的主要 CPU 去向之一。
- **M7 工具带每帧重复整形**：按钮字形是常量却每帧 shape
  （[ui.rs:473](crates/nexty-chrome/src/ui.rs)），URL 全串 + caret 前缀每帧
  reshape（[ui.rs:535,620](crates/nexty-chrome/src/ui.rs)）。
- **M10 克隆链**：`root_style` 深克隆只借不存
  （[tree_build.rs:327,366](crates/nexty-layout/src/tree_build.rs)）、`lines_at`
  缓存命中仍全量深拷贝（[tree_build.rs:135](crates/nexty-layout/src/tree_build.rs)）、
  `ensure_style` 每字符对 ComputedStyle 全字段 `PartialEq`
  （[inline/mod.rs:269](crates/nexty-layout/src/inline/mod.rs)）。

### 本轮的硬约束：行为零变更

性能轮不改任何可观察行为（唯一例外是速度与帧率）。全部既有测试——含
conformance 全量、WPT 基线 1854/1959、选区 / CSS Text 轮新增测试——**不改
仍绿**是每个任务的硬前提。任何优化在 T1 基准上无实测收益即回退，并在归档
记录里写明（基准是仲裁者，不是摆设）。

## 任务清单与退出条件

### T1 nexty-chrome：大页面基准与基线文档（0.1.21）
- 新增 `benches/pipeline.rs`（`[[bench]] harness = false`，
  `required-features = ["pipeline"]`），**不引入 criterion 等新依赖**——
  std::time 计时 + 预热 + N 次取中位数即可满足「确认热点归属」的精度。
  为避免 `cargo test` 拖慢：无 `BENCH_MODE=1` 环境变量时打印 skip 并立即
  退出（`cargo clippy --all-targets` 仍会编译它，零 warning 门禁照常生效）。
- 基准内建一个 `CountingShaper`（包装 `&dyn TextShaper`，`AtomicUsize`
  计数 `shape` / `metrics` 调用）——放大倍数用调用次数表达，不依赖机器。
- 语料三页（确定性生成、入库为 bench 代码，不依赖网络）：**文本密集页**
  （约 2,000 段中英混排 + 行内样式嵌套）、**规则密集页**（约 5,000 条规则 ×
  万级元素）、**图片页**（多张生成 RGBA 图）。场景：首屏全管线（parse /
  cascade / layout / scene 各阶段分别计时）、resize 风暴（宽度 ±1 连续 20 次
  重排）、滚动风暴（同布局 50 个滚动偏移的 emit+rasterize）。
- 产出 `docs/2026-10-25-perf-baseline.md`：机器信息、运行命令、各场景各阶段
  基线数字 + CountingShaper 放大计数 + **热点归属结论**（哪个阶段占比多少，
  M5 的规则分桶值不值得做按数据定）。
- 退出条件：`BENCH_MODE=1 cargo bench -p nexty-chrome --bench pipeline`
  可重复跑通且数字稳定（同机两次各阶段中位数波动 < 10%）；基线文档入库；
  `cargo test -p nexty-chrome` 不执行 bench 正文（skip 路径有测试钉住）。

### T2 nexty-layout：H4——宽度无关计算出 measure 循环（0.1.25）
- `ShapedText` 提升为 run 节点级缓存（按 chunk 身份，首次 `build_lines`
  填充，跨宽度档位复用）；`cluster_map` / `break_opportunities` /
  `split_spans` 只依赖 `(text, BreakOptions)`，随 shaping 一并缓存；`metrics`
  改为 `build_lines` 开头按 TextStyle 预取一次存表（探针文本固定，样式相同
  结果必相同）。只有贪心放置按宽度重跑。
- CSS Text 轮的放置期修正（justify / 字距 / tab / 悬挂）全部在放置侧，
  必须不受缓存影响——逐项测试钉住「缓存前后行为逐字节一致」。
- 退出条件：T1 基准文本密集页上，CountingShaper 显示单次全量布局的
  `shape` 调用数 ≤ 文本 chunk 数（基线记录放大倍数并对比）、`metrics`
  调用数 ≤ 不同 TextStyle 数；resize 风暴场景 layout 阶段时间较基线下降
  （幅度记入基线文档「优化后」栏）；layout 全部既有测试不改仍绿。

### T3 nexty-chrome：M1——图片与场景零拷贝（0.1.22）
- `Scene.images` / `Page.images` 改 `Arc<Image>`；`RenderThread::render`
  收 `Scene` **所有权**（app 侧构建后 move，消除 `.send((scene.clone(), …))`
  的第二份整树克隆）；改动限制在 chrome crate 边界内（paint 层
  `DrawImage` 入口同步适配，paint 0.1.5 → 0.1.6）。
- 退出条件：显示列表构建与发送路径不再出现图片像素拷贝——测试用
  `Arc::ptr_eq` 断言 scene 与 page 里的图是同一份；渲染线程收所有权
  （类型签名即证）；既有图片渲染测试不改仍绿。

### T4 nexty-chrome：M6——resize 合帧与滚动场景缓存（0.1.23）
- **resize 合帧**：`Resized` 只置 dirty 标志 + `request_redraw`，重级联 /
  重排合并到下一次 `RedrawRequested` 执行一次（winit 拖动边缘连发事件时
  管线每帧至多跑一遍）。
- **滚动不重建显示列表**：显示列表按（页面代次 / 布局结果 / 高亮 / 视口宽）
  缓存为**页面坐标系**版本，origin 平移下放到光栅 / 发送阶段；滚动、滚动条
  拖拽、选区高亮更新复用缓存场景，仅导航 / resize / 选区变化使缓存失效。
  `pad_rows` staging 缓冲按宽高复用，不每帧重建。
- 退出条件：无头可测部分（场景缓存 API：同代次场景复用、失效条件、页面
  坐标 + origin 平移的正确性——高亮 / 文本坐标与现行为逐字节一致）有单测；
  T1 滚动风暴场景 emit 次数降为 0（计数断言进 bench 输出）、resize 风暴
  场景管线执行次数从「每事件一次」降为「每帧一次」（bench 计数）；事件
  合帧与滚动体感留 T9 实机。

### T5 nexty-chrome：M7——工具带整形缓存（0.1.24）
- 按钮字形（常量字符 ×3）整形结果缓存在 UI 状态里一次；URL 整形按
  （文本, 样式）缓存，URL 未变不 reshape；caret / 选区前缀宽度改为对已
  整形 glyph 的 x 前缀累加（O(前缀) 不再 O(全文) reshape）；`families`
  String 不再每帧新建。
- 退出条件：ui 层加 CountingShaper 式计数测试——URL 不变的 N 次 redraw
  触发 0 次整形；长 URL 输入时每次按键整形调用量有上界断言；既有 UI /
  快捷键 / 地址栏编辑测试不改仍绿。

### T6 nexty-css：M5——级联热路径（0.1.14）
- 按审查性价比排序：① `matched` 收 `(CascadeKey, &Declaration)` 引用，
  声明生命周期覆盖 `resolve_all` 调用，消除每命中 `clone()`；② 内联样式
  解析结果随元素缓存（resize 后 recascade 直接复用，不重解析 `style` 属性
  字符串）；③ `namespace_url` 四类标准命名空间返回 `&'static str`（仅
  `Other` 分配）；④ **规则分桶（tag / class / id 三桶，跳过不可能命中的
  规则）仅在 T1 基线显示规则匹配占比确实居前时做**——数据不支撑就记入
  基线文档不做。
- 语义零变更：桶化后命中规则仍按 origin → importance → specificity →
  源顺序仲裁，既有级联 / 选择器全量测试是安全网。
- 退出条件：规则密集页基准 cascade 阶段较基线下降（数字记档）；css 全部
  既有测试不改仍绿；若做了分桶，追加「桶化前后 computed style 逐元素一致」
  的对照测试。

### T7 nexty-layout：M10——克隆链 + M12 顺带（0.1.26）
- `root_style` 改为按 `ctx.styles` 借用（仅缺样式时才构造 initial）；
  `block.rs` 组装处不再二次深克隆；`lines_at` 缓存命中路径改 take / 引用
  语义（它已是缓存最后消费者）；`ensure_style` 删掉 ComputedStyle 全字段
  比较条件（样式按 NodeId 查表，来源节点相等即样式相等）。
- 顺带 M12：`remap_glyph` 对 `glyph.font` 改 `.get()` 缺失跳过（审查 §M12，
  trait 契约破坏不再 panic，与「畸形输入不 panic」基准对齐）。
- 退出条件：layout 既有测试不改仍绿；文本密集页 resize 场景数字记档；
  M12 加一条「越界 font 下标不 panic、字形跳过」单测。

### T8 门禁、文档与版本
- `cargo check --workspace` 零 warning；`cargo test --workspace` 全绿；
  `cargo clippy --workspace --all-targets -- -D warnings`；
  `cargo fmt --all -- --check`；`cargo deny check` 通过（本轮零新依赖）。
- feature 组合零 warning复验（本轮不动 feature 面，不得回归；bench 的
  `required-features` 按 Feature Gating 约定第 7 条登记）。
- 基线文档补「优化后」列 + 各任务回填实测数字；无收益回退项如实记录。
- AGENTS.md 同步：性能轮条目落地情况、审查报告 H4/M1/M5/M6/M7/M10/M12
  状态更新、roadmap 改为 paint 扩展（text-decoration / 圆角 / 渐变 / 背景传播）
  其后 float / table。
- 版本号随逻辑提交 +1：css 0.1.13 → 0.1.14、layout 0.1.24 → 0.1.26（T2/T7
  两次）、paint 0.1.5 → 0.1.6、chrome 0.1.20 → 0.1.24（T1–T5 各一次）；
  text / html / dom / network 无改动不动。
- 分任务 commit（`[chrome]` / `[layout]` / `[css]` / `[paint]` / `[docs]`），
  退出条件全部满足后 push。

### T9 实机验收（体感与无头不可测项）
- Computer Use 流程：文本密集基准页本地起服务 + 实打开；验收动作：拖
  窗口边缘 resize（不再逐事件全管线卡顿）、长页滚轮连滚与滚动条拖拽
  （滚动跟手）、多图页面滚动、长 URL 地址栏逐键输入（无迟滞）、10k 元素
  页面首屏可接受；与改动前同页对比（可 `git stash` 临时对照）。
- 截图 / 简短体感记录入基线文档附录。

## 非目标（本轮不做）

- M3 加载竞态 / M4 wgpu uncaptured error handler——正确性 / 健壮性项，
  与性能混做会模糊「行为零变更」判据，留独立小轮。
- M11 原子盒布局记忆化（与 H4 同域但独立，基准若显示其占比大再立项）；
  页面纹理缓存 / 增量光栅（M6 的终态方案，本轮只到「场景缓存 + origin
  平移」）。
- 布局增量更新（脏路径传播）、样式共享（style sharing）、并行光栅、
  GPU 后端（vello_hybrid）——结构性大项，各自独立轮。
- 任何新外部依赖（criterion / profilers 不入库；如需火焰图用本机工具，
  结论记档即可）。

## 方向评估（为什么是这轮）

| 候选 | 价值 | 成本 | 结论 |
| --- | --- | --- | --- |
| **性能轮 H4 + M1/M5/M6/M7/M10（本轮）** | roadmap 排定；审查 §8 第 2–5 梯队全部纯性能项一次清完；resize / 滚动体感直接改善 | 中：基准先行 + 五个局部优化，无行为变更 | ✅ 本轮 |
| paint 扩展（text-decoration / 圆角 / 渐变）+ 背景传播 | 视觉保真 | 小–中 | 下一轮 |
| M3 / M4 chrome 正确性小轮 | 加载竞态 / GPU 健壮性 | 小 | 性能轮后插入 |
| float / table | 老页面 / 邮件类兼容 | 大 | 后续 |
| vello_hybrid GPU 后端 | 性能上限 | 中：surface 接入与适配层 | 后续（CPU 路径先榨干） |

## 验证流（AGENTS.md Verification Flow）

T1 先行且必须完成（基线是后续所有任务的对照与仲裁）。T2–T7 每个任务：
先跑基准拿到「改前」数字 → 改动 → 既有全量测试不改仍绿（行为零变更的硬
判据）→ 再跑基准拿「改后」数字回填基线文档 → 无收益即回退并记录 →
commit。WPT 基线 1854/1959 与 conformance 全量每任务都不改仍绿。性能断言
用调用计数与执行次数表达（机器无关），时间数字仅作趋势记录。退出条件全部
满足后 push。
