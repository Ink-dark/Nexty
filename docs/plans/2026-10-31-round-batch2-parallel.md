# goal.md — 本轮任务：批次 2 并行轮（K5 JSRT 选型与最小绑定 ∥ K6 容器滚动与裁剪 ∥ K13 图片格式小件）

日期：2026-10-31。前置：批次 1 已合并（PR #10，main @ 1415cec）。本轮按
[kernel-roadmap](2026-10-31-kernel-roadmap.md) 批次 2 执行：三条泳道并行，
对应轮次卡片 K5 / K6 / K13；X 补全批次不进本轮。执行分支
`feat/batch2-jsrt-scroll-images`（自 main 切出）。每条泳道独立任务线、
独立 commit；整轮退出条件见文末批次收口。

**本轮交付纪律（用户指令，2026-10-31）**：分支直接 push、最后由代理用
gh CLI 开 PR，不再等人工 push；实机验收项留给架构师，不阻塞 PR（验收
清单写入 PR 描述与执行报告）。

**unsafe 口径（用户指令，2026-10-31）**：JSRT 是插拔层——引擎 FFI 的
unsafe 只允许出现在 runner 适配侧（独立适配 crate，非 facade 层）；
kernel 各 facade crate 维持 `#![forbid(unsafe_code)]` 不变。AGENTS.md
Hard Rules 措辞随本轮登记修订为「禁 unsafe 限定 kernel facade crate；
runner 适配 crate 允许 unsafe 并须在 ADR 记录边界」。

## 并行结构与启动条件

| 泳道 | 轮次 | crate 足迹 | 启动条件 | chrome 触碰清单 |
| --- | --- | --- | --- | --- |
| A | K5 JSRT | nexty-dom（event/jsrt 模块）、**新 runner 适配 crate**、nexty-chrome | **立即可启**（T0 选型侦察先行，时间盒内定案） | app 新文件 `app/script.rs`（装配 + 任务泵）+ pipeline 解析完成点单处钩子 |
| C | K6 容器滚动 | nexty-css、nexty-layout、nexty-paint、nexty-chrome | **立即可启**（T0 接口先行 commit） | pipeline clip 栈（场景构建段）+ scrollbar 容器化 + app/events wheel 链式消费 |
| 小件 | K13 图片格式 | nexty-paint（decode 独立模块）、nexty-chrome（resources/ui） | **立即可启** | resources favicon 资源类型 + ui 地址栏图标槽 |

交叉面纪律（§7）：
- paint：K6（clip 指令，scene 指令区）与 K13（decode 模块）分模块，互不
  触碰对方文件；
- chrome：三方分文件——K5 `app/script.rs`（新文件）+ pipeline 导航完成点
  单处钩子；K6 pipeline 场景构建段 + `scrollbar.rs` + `app/events.rs`
  wheel 路径；K13 `resources.rs` + `ui.rs`；
- pipeline.rs 允许 K5（导航完成点钩子）与 K6（场景构建段）分区域提交，
  先后顺序按收口合并时序，不做同段双改；
- 同一 crate 同一时刻只允许一条任务线在改；版本 bump 以逐任务提交为准。

## 泳道 A（K5）：JSRT 选型与最小绑定轮（dom 0.1.4 → 0.1.5；新增 runner crate 0.1.0）

### T0 选型侦察与 ADR（时间盒：侦察完成即定案，不现场造轮子）
- 候选：Boa（纯 Rust）vs QuickJS / quickjs-ng（rquickjs 绑定）vs 其他；
  维度：许可证白名单（含传递依赖过 deny）、unsafe 用量与边界（按本轮
  unsafe 口径：适配 crate 允许）、test262 通过率、维护活跃度、MSVC
  构建成本、依赖树体积、与自研 arena DOM 的绑定成本（引擎不要求与
  DOM GC 生命周期深度耦合者优先，K5 范围 out 已排除）。
- **`Document` !Send 架构决策**：批次 1 遗留决策窗口——注册表外置
  （ListenerRegistry 等迁出 Document，Document 转 Send）vs 维持现状
  （Send 数据面 + 主线程建 Document，接受两次解析/构建成本）。在本
  任务定案并写入 ADR。
- 架构口径：接缝 trait 与绑定注册表在 nexty-dom（feature `jsrt`），
  kernel 侧零 unsafe、引擎类型不入 pub API；引擎适配为独立 workspace
  member crate（非 facade 层），允许 unsafe FFI 并在 ADR 记录边界。
- 产出：`docs/decisions/2026-10-31-jsrt.md`（含「为什么不是别的」）；
  deny.toml / about 更新随实现任务走收口门禁。
- **退出条件**：ADR 定稿（引擎 + !Send 决策 + unsafe 边界三项全有结论）。

### T1 nexty-dom：`jsrt` 模块——runner 接缝 + 绑定注册表（dom → 0.1.5）
- feature `jsrt`（隐含 `event`）：`ScriptHost`/runner 接缝 trait——脚本
  编译执行入口、事件派发进引擎的回调面、任务泵入口；引擎值类型以自身
  抽象包装，不外泄（Hard Rules）。
- 绑定注册表：DOM 侧操作以 registry 提供——createElement / appendChild
  / remove、textContent、getAttribute / setAttribute、classList、
  getElementById、addEventListener / dispatchEvent；querySelector 与
  getComputedStyle 经注入钩子（Box<dyn Fn>，chrome 装配 css 选择器与
  级联只读快照）——**dom 不反向依赖 css**（层隔离）。
- 事件循环合流：填充 K1 EventLoop 预留微任务位（微任务先于下一个宏任务
  清空）；setTimeout / setInterval（ID 管理、clearTimeout/clearInterval、
  嵌套钳制简化档）。
- **退出条件**：单测——微任务/宏任务顺序、定时器顺序与清除、派发中
  post 不重入回归；逐 feature 编译零 warning；dom 0.1.5。

### T2 runner 适配 crate：引擎接入 + 绑定 v0
- workspace 新 member `crates/nexty-jsrt-<engine>`（命名随 T0 定案）：
  实现 T1 接缝；绑定面 v0 按卡片清单；inline script 同步执行，`src`
  脚本 defer 档（DCL 前执行完；async 记偏差）。
- **退出条件**：端到端单测——脚本经事件驱动改 DOM → 重排触发；引擎
  eval 错误上抛为错误类别不 panic；适配 crate 覆盖率 ≥ 80%；deny /
  about（新依赖）通过。

### T3 nexty-chrome：脚本编排接线
- pipeline：解析产物 `<script>` 收集（解析完成点单处钩子）；DCL 时机
  保持 K1 语义（defer 脚本执行完才派发 DOMContentLoaded）。
- app：`app/script.rs` 新文件——ScriptHost 装配（引擎实例、注入选择器 /
  级联快照钩子）、EventLoop 任务泵（微任务 / 定时器在事件间隙清空）、
  DOM 变更后触发重排。
- **退出条件**：集成测试——click 监听器改 textContent → 重排；定时器
  改样式 → 重排；既有 chrome 回归全绿；chrome 随行为 bump。

### T4 WPT scripted 6 条 + 登记
- tree-construction 剩余 6 条 scripted 用例（见
  `crates/nexty-html/tests/tree_construction.rs` 基线，WPT commit
  `5cd8e3fa`）经 runner 跑通并入 harness。
- 登记：Feature 映射表加 dom `jsrt` 行 + runner crate；AGENTS.md
  dom/chrome 状态段、Hard Rules unsafe 口径修订；偏差清单（fetch /
  localStorage / canvas / worker / ESM / async 脚本 / getComputedStyle
  快照语义 / GC 与 DOM 生命周期深度耦合未做）。
- **退出条件**：WPT 6 条通过；登记完整。

## 泳道 C（K6）：容器滚动与裁剪轮（css / layout / paint / chrome 各 +1 patch）

### T0 接口先行 commit（§7.2，跨 4 crate，编译绿、实现留后）
- paint：clip 指令类型（PushClip / PopClip 或等价嵌套裁剪矩形栈接口）；
- layout：盒树 / Fragment 滚动容器元数据（scrollable 标志、scrollable
  overflow rect 字段、滚动偏移不参与布局解算的接缝）；
- css：overflow 双轴值模型类型（解析可并入 T1）。

### T1 nexty-css：overflow 解析完备化（css → 0.1.19）
- overflow 简写展开为 overflow-x / overflow-y（visible / hidden / scroll
  / auto）；computed 双轴字段；visible→auto 的可见性转换语义按
  CSS Overflow §2 简化档并记偏差。
- **退出条件**：解析 / 简写展开 / computed 单测；零 warning。

### T2 nexty-layout：滚动容器建树与几何（layout → 0.1.30）
- overflow ≠ visible 容器识别（UA + 作者级联）；scrollable overflow
  rect（CSS Overflow §3 简化档：内容 + 后代边界盒）；滚动偏移独立于
  布局解算（绘制期平移施加）。
- **退出条件**：嵌套滚动几何单测（容器内溢出、嵌套容器、偏移不改变
  布局结果）。

### T3 nexty-paint：clip 指令（paint → 0.1.9 或 0.1.10，与 K13 时序为准）
- 侦察 vello_cpu clip / layer 能力（探针结论写任务记录；缺口定降级
  语义记偏差）；PushClip / PopClip 实现；`Scene::translated` 兼容
  （clip 矩形随平移）；hidden = 裁剪不滚动。
- **退出条件**：像素级单测（clip 内外采样、嵌套 clip、平移后 clip
  正确）。

### T4 nexty-chrome：pipeline clip 栈 + 容器滚动条 + 滚轮链式消费（chrome 随行为 bump）
- pipeline：场景构建按滚动容器嵌套 PushClip + 内容平移；Page 持容器
  滚动偏移（NodeId 键），重排后保留并 clamp（scrollTop 语义）。
- scrollbar：从视口级推广容器级（per-container 几何 + 命中 + 拖拽状态
  机）；容器滚动条拖拽不影响视口滚动。
- app/events：滚轮由内向外链式消费——命中测试定位最内可滚动容器，
  消费则停，到边界冒泡祖先 / 视口。
- **退出条件**：集成测试（容器滚动、边界冒泡、偏移保留、双容器独立
  滚动）；bench 回归——视口滚动 emit=0 不变、四条机器无关断言全绿
  （容器滚动允许场景重建，记偏差）。

### T5 验收资产 + 登记
- `tools/acceptance` 新页：嵌套滚动（内外双层 overflow:auto）、hidden
  裁剪、容器滚动条拖拽；index.html / README / server.cjs 三处登记。
- 登记：Feature 映射表（css overflow 轴并入 `values`、layout 滚动几何
  模块、paint `clip`、chrome 并入既有 feature）+ AGENTS.md 状态段 +
  偏差清单（scroll-snap / sticky / 自定义滚动条伪元素 / scroll
  anchoring / scrollbar-gutter 等）。

## 泳道 S（K13）：图片格式小件轮（paint decode 扩展 + chrome favicon）

### T1 nexty-paint decode：jpeg / gif / webp
- image crate 开对应解码 feature（GIF 取首帧，动图推进记偏差——远期）；
  解码预检（into_dimensions → MAX_DIMENSION / MAX_PIXEL_AREA）对新格式
  生效；测试图入库 `tests/data/`（含超限头部 fixture 走
  SizeUnsupported 路径）。
- **退出条件**：各格式解码 + 渲染单测；预检超限路径单测；未知格式
  仍 DecodeError；`cargo check -p nexty-paint` 零 warning。

### T2 nexty-chrome：favicon（resources + ui）
- 导航时 favicon 抓取：`<link rel=icon>` 优先（resources 收集扩展），
  回落 `/favicon.ico`；响应体预算独立常量化（小于图片档 50 MB）；
  解码失败静默降级不阻塞页面；会话去重。
- ui：地址栏左侧图标槽（16×16 绘制，走既有 DrawImage 路径）。
- **退出条件**：单测（link 优先 / 回落 / 失败降级 / 去重）；chrome
  随行为 bump。

### T3 验收资产
- formats 页（png / jpeg / gif / webp 并排 + 判据）、favicon 页（有
  link / 无 link 两态；server.cjs 提供 `/favicon.ico` 与请求计数）；
  index / README 登记。

## 批次收口（整轮退出条件）

1. workspace 门禁：check / test / clippy(-D warnings) / fmt 全绿；
   逐 crate 逐 feature 开关编译零 warning。
2. deny check 通过；about 生成 + 注入（.workbuddy 脚本）幂等重跑。
3. bench：本地冒烟（BENCH_MODE，2 帧滚动风暴）四条机器无关断言全绿；
   push 后 CI workflow（bench.yml）手动触发并记录。
4. 版本落点：dom 0.1.5 · css 0.1.19 · layout 0.1.30 · paint 0.1.9+
   · chrome 0.1.31+ · runner crate 0.1.0（新增）——以逐任务实际 bump
   为准，执行报告核对。
5. AGENTS.md 状态段 / Feature 映射表 / 偏差清单登记完整；路线图偏差
   归属迁移；Hard Rules unsafe 口径修订落地。
6. 执行报告 `docs/plans/2026-10-31-round-batch2-execution-report.md`：
   任务总览、证据锚点、实机验收清单（架构师执行项）、偏差与决策点。
7. **push + PR**：分支直接 push（用户指令，不等人工）；PR 描述按批次 1
   格式（概述 / 版本落点 / 验证 / 已知偏差 + 实机验收清单）。
