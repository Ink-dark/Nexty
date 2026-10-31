# 内核路线图：float 轮之后 → 完整浏览器内核

日期：2026-10-31。前置：性能轮已归档
（[2026-10-25-round-perf.md](2026-10-25-round-perf.md)）；float 轮进行中
（[2026-10-31-round-float.md](2026-10-31-round-float.md)）。
本文是规划文档，不是某一轮的 goal.md——每轮启动时从对应卡片裁剪出该轮
goal.md，任务与退出条件以卡片为准。分层依据见
[crate 选型 ADR](../decisions/2026-10-01-crate-selection.md)。

## 0. 规划约束（用户指令，2026-10-31）

1. **新功能优先**：优先排尚没有做过的能力——事件、脚本、表单、容器滚动、
   table、position:fixed、Web 字体、bidi、视口裁剪、GPU 后端；CSS 属性
   补全（var()、伪元素、rem/ex/ch）这类收尾工作统一放 X 批次殿后，不占
   主线轮次。
2. **并行拆分**：并行单位 = 泳道 = crate 主战场。同一泳道内轮次串行
   （同一 crate 不允许两轮同时改），不同泳道默认并行。chrome 是多泳道
   共用的热点 crate，例外规则见 §7。
3. 轮次粒度对齐既有惯例：一轮 = 一个 goal.md = 一个主题 + 可验证退出
   条件 + 版本 bump + 逐任务 commit（AGENTS.md 验证流不变）。
4. 新依赖一律过 cargo deny + cargo about 注入 + ADR；轮次卡片只列候选，
   选型在轮内侦察后定。

## 1. 「完整内核」基准（DoD，验收口径）

以「静态页面完整渲染 + 页面可编程 + 用户可交互 + 导航可信 + 性能达标 +
工程达标」为完成口径。六项全部满足即视为内核完整；超出部分进 §6 远期
评估，要扩基准先改本表。

| # | 基准项 | 达成判据 | 覆盖轮次 |
| --- | --- | --- | --- |
| 1 | 内容可渲染 | block / inline / flex / grid / float / absolute / fixed / table 全布局；背景 / 边框 / 圆角 / 渐变 / 文本装饰 / z-index 层叠；Web 字体；bidi | float 轮 + K2 / K4 / K7 / K8 / K11 |
| 2 | 页面可编程 | DOM 事件模型（捕获 / 冒泡 / 取消 / 默认行为）、JSRT 跑脚本、基线 DOM 绑定、定时器 / 微任务、DOMContentLoaded / load | K1 / K5 |
| 3 | 用户可交互 | 视口 + 容器两级滚动；链接 / 悬停 / 焦点；文本选区复制（已有）；表单控件基线 + GET 提交 | K6 / K10 |
| 4 | 导航可信 | WHATWG URL、字符编码探测、重定向、HTTP 缓存、Cookie（可选）、会话历史（已有）、错误页 | K3 |
| 5 | 性能达标 | 滚动光栅 O(视口)（bench 门槛见 K9）、GPU 后端可启用 | K9 / K12 |
| 6 | 工程达标 | 覆盖率 ≥ 80%、check / test / clippy / fmt / deny / about 全绿、新依赖有 ADR、WPT 子集入 CI | X2 + 各轮 |

## 2. 泳道与并行结构

| 泳道 | 主战场 crate | 轮次（泳道内串行） |
| --- | --- | --- |
| A 脚本与事件 | nexty-dom（event / jsrt 模块）、chrome app 事件接线 | K1 → K5 |
| B 绘制与性能 | nexty-paint、chrome pipeline / render | K2 → K9 → K12 |
| C 布局新能力 | nexty-layout | K6 → K7 → K8（K10 / K11 借道，分模块） |
| D 导航与网络 | nexty-network、chrome app / resources | K3 |
| E 文本与字体 | nexty-text、chrome resources | K4 → K11 |
| 小件 | 各层独立模块 | K13（任意批次插队） |
| X 补全（殿后） | nexty-css 为主 | X1 → X2 → X3 |

跨泳道硬依赖：

- K5 ← K1（事件模型先行，绑定才有 addEventListener 可接）。
- K9 ← K6（clip scope / 裁剪语义先行）。
- K10 ← K1（事件与焦点先行）。
- K11 ← K4（text 泳道串行；无技术依赖）。
- K12 ← K9（CPU 路径先收敛才有对照；非硬依赖，可对调）。
- 生命周期事件：K1 定义（DOMContentLoaded / load），K3（资源编排完成
  信号）与 K5（脚本侧监听）接线。

依赖图：

```
K1 事件模型 ──→ K5 JSRT 绑定 ──→ (K10 表单的事件面)
K2 绘制扩展 ─────────────────→ (K7 z-index 完整档)
K6 容器滚动/裁剪 ──→ K9 滚动性能 ──→ K12 GPU 后端
K4 Web 字体 ──→ K11 bidi
K3 导航与网络（独立）
C 泳道串行：K6 → K7 → K8 →（K10 ∥ K11，layout 分模块）
```

## 3. 建议批次（同一批内并行；可按人力增减）

| 批次 | 并行轮次 | 冲突注记 |
| --- | --- | --- |
| 1 | K1 ∥ K2 ∥ K3 ∥ K4 | K1 限定 chrome app 新事件模块；K2 限定 pipeline；K3 动 app 导航编排 + resources；K4 动 resources + css + text。K3 / K4 在 chrome resources 弱重叠 → K4 半轮错峰，或 K4 先做 css / text 侧、resources 接线挪批次 2。 |
| 2 | K5 ∥ K6 ∥ K13 | K13 只动 paint decode 独立模块与 network，与 K6 的 paint clip 部分分模块。 |
| 3 | K7 ∥ K9 | K7 主体在 layout（fixed / 包含块），K9 主体在 paint / chrome render；K7 的 z-index 定序若动 paint scene 构建，与 K9 光栅侧分模块 + 接口先行。 |
| 4 | K8 ∥ K12 ∥ X1 | K8 在 layout table 新模块；K12 在 paint gpu + chrome render / app；X1 在 css——三者无同 crate 冲突。 |
| 5 | K10 ∥ K11 | layout 借道冲突面：K10 只动 replaced（控件替换盒），K11 只动 inline——分模块并行，tree_build 接缝先落接口 commit。 |
| 6 | X2 ∥ X3 + 全仓 DoD 核对 | 收口批。 |

单执行线（无并行）时的串行优先序（按新功能价值 + 依赖）：

K1 → K2 → K5 → K3 → K6 → K9 → K4 → K7 → K8 → K10 → K12 → K11
→ X1 → X2 → X3

## 4. 轮次卡片

### K1 事件模型轮

- **目标**：DOM 事件系统落地——EventTarget / Event / 传播 / 默认行为 +
  chrome 交互事件派发接线。JSRT 未接前，默认行为（链接导航、滚动、焦点）
  由原生实现消费事件。
- **范围 in**：Event / EventTarget（DOM §2.9 简化档：无 shadow、无信任
  边界）；捕获 / 冒泡 / target 三阶段、stopPropagation / preventDefault
  / stopImmediatePropagation；UIEvent / MouseEvent / KeyboardEvent /
  WheelEvent / FocusEvent 数据面；EventLoop 抽象（任务队列，微任务位
  留 K5）；DOMContentLoaded / load 派发时机定义（资源编排完成处）；
  chrome：hit_test 结果 → 事件派发，链接导航 / 滚轮 / 键盘改走事件，
  行为回归不变。
- **范围 out**：JS 监听器（K5）、pointer capture、合成事件、IME。
- **crate 足迹**：nexty-dom（新 event 模块 + feature `event`）、
  nexty-chrome（app 事件接线，新文件）。
- **任务**：T1 event 模块（Event / EventTarget / 传播算法 + 单测）；
  T2 事件类型数据面；T3 EventLoop + 生命周期事件定义；T4 chrome 接线 +
  实机冒烟；T5 登记（feature 映射表 + 偏差清单）。
- **退出条件**：传播 / 取消语义单测；chrome 冒烟（点击链接、滚轮、键盘
  全走事件派发后行为不变）；既有测试回归绿。
- **规模**：中。**前置**：无。**并行**：批次 1。

### K2 绘制指令集扩展轮

- **目标**：paint 指令集补齐常用视觉能力 + 基础 z-index 定序（即原定
  「下一轮候选」，被 float 轮让位后回到批次 1）。
- **范围 in**：text-decoration（underline / overline / line-through +
  color / style 基础档，链接默认下划线）；border-radius（圆角矩形填充
  / 描边）；线性渐变背景（linear-gradient 基础档：角度 + 色标）；背景
  传播（body / html → canvas，CSS 2.1 §14.2）；z-index 基础定序（定位
  兄弟间整数排序，完整层叠上下文留 K7）。
- **范围 out**：径向 / 锥形渐变、多重背景、background-attachment、
  box-shadow、filter、完整层叠上下文。
- **crate 足迹**：nexty-paint（指令 + vello_cpu 映射）、nexty-css（属性
  解析）、nexty-chrome / pipeline（emit）。
- **侦察点**：vello_cpu 对圆角 / 渐变的支持面——先探针确认，缺口评估
  自研光栅路径或降级语义。
- **退出条件**：指令单测 + 视觉对照页实机核对要点；overflow 不裁剪
  语义不变；bench 无回退。
- **规模**：中。**前置**：无。**并行**：批次 1。

### K3 导航与网络完备轮

- **目标**：导航语义与网络栈补齐——URL 标准化、字符编码、重定向、
  HTTP 缓存。
- **范围 in**：`url` crate 接入（地址栏 / 相对解析 / 链接命中统一走
  WHATWG URL）；encoding_rs 字符编码探测（BOM → charset 声明 → 默认
  UTF-8，替换现有「假定 UTF-8」路径）；HTTP 重定向（3xx + 防环上限）；
  HTTP 缓存（内存级简化档：Cache-Control max-age / no-store、
  ETag / Last-Modified 条件请求）；错误页补 charset 相关类别。
- **范围 out**：Cookie（可选任务 T5，时间不够拆独立轮）、Service
  Worker、磁盘缓存、HTTP/2 推送。
- **crate 足迹**：nexty-network（cache / encoding 模块 + feature）、
  nexty-chrome（app 导航编排、resources）。
- **新依赖**：`url`（已在候选清单）、`encoding_rs`（MIT / Apache-2.0）
  → 各一条 ADR + deny / about 更新。
- **退出条件**：重定向 / 缓存命中 / 条件请求单测（本地测试服务器）；
  GBK 等非 UTF-8 测试页正确渲染；deny / about 通过。
- **规模**：中。**前置**：无。**并行**：批次 1。

### K4 Web 字体轮（@font-face）

- **目标**：作者字体从网络加载并参与选字回退。
- **范围 in**：@font-face 解析（src url() / local()，font-family /
  font-weight / font-style 描述符基础档）；字体子资源抓取（chrome
  resources 新类型，复用响应体预算）；nexty-text FontResolver 扩展：
  会话级字体注册表、按描述符参与 fontique 选字、加载失败回退系统字体；
  「字体就绪后重排一次」的简化加载语义。
- **范围 out**：unicode-range 子集、可变字体轴、FontFaceSet、
  font-display 全语义。
- **crate 足迹**：nexty-css（parser 新 at-rule）、nexty-text
  （FontResolver + 注册表）、nexty-chrome / resources。
- **侦察点**：woff / woff2 解码（harfrust 只吃 ttf / otf；不支持则进
  偏差清单）。
- **退出条件**：自托管 ttf 测试页字形与系统回退可区分；就绪触发重排
  一次（单测 + 实机）；失败回退路径单测；deny / about（若引入新 crate）。
- **规模**：中。**前置**：无。**并行**：批次 1（resources 部分与 K3
  错峰，见批次表）。

### K5 JSRT 选型与最小绑定轮

- **目标**：内核可编程——选定 JS 引擎并落地最小 DOM 绑定与事件循环。
- **选型**：Boa（纯 Rust）vs QuickJS（rquickjs 绑定）vs 其他——按许可
  证白名单、unsafe 用量、test262 通过率、维护度、依赖树体积出 ADR；
  时间盒内定案，不现场造轮子。
- **范围 in**：JS runner 接缝 trait（可插拔；JS runner 与绑定同处
  nexty-dom——AGENTS 约束）；事件循环（微任务队列 + setTimeout /
  setInterval，与 K1 EventLoop 合流）；绑定面 v0：window / document /
  element 包装、textContent、getAttribute / setAttribute、classList、
  createElement / appendChild / remove、getElementById / querySelector
  （复用 nexty-css 选择器）、addEventListener / dispatchEvent、
  getComputedStyle（只读快照）；WPT scripted 6 条用例。
- **范围 out**：fetch / localStorage / canvas / worker、GC 与 DOM 生命
  周期深度耦合优化、ESM 加载。
- **crate 足迹**：nexty-dom（jsrt 模块 + feature `jsrt`）、workspace
  新依赖。
- **退出条件**：示例页脚本经事件驱动 DOM 变更并触发重排（单测 + 实机）；
  微任务 / 定时器顺序单测；WPT 6 条通过；ADR；新模块覆盖率 ≥ 80%。
- **规模**：大。**前置**：K1。**并行**：批次 2。

### K6 容器滚动与裁剪轮

- **目标**：元素级滚动——overflow: scroll / auto 容器成为可滚动子树，
  配套 clip 作用域。
- **范围 in**：overflow 解析完备化（visible / hidden / scroll / auto，
  x / y 简写）；可滚动溢出几何（scrollable overflow rect）；布局侧
  滚动容器建树 + 滚动偏移参与绘制（场景树 clip scope：嵌套裁剪矩形栈）；
  hidden = 裁剪不滚动；重排后滚动位置保留（scrollTop 保持）；chrome：
  容器级滚动条（scrollbar 模块从视口级推广）、滚轮由内向外链式消费。
- **范围 out**：scroll-snap、sticky、自定义滚动条伪元素、scroll
  anchoring 完整语义。
- **crate 足迹**：nexty-css、nexty-layout（建树 + 几何）、nexty-paint
  （clip 指令）、nexty-chrome（pipeline clip 栈、scrollbar、app 滚动
  派发）。
- **关键风险**：clip scope 数据结构跨 4 crate——接口先行 commit。
- **退出条件**：嵌套滚动几何单测；clip 栈单测；实机冒烟（内层滚动不
  影响视口滚动、hidden 裁剪、容器滚动条拖拽）；bench 回归。
- **规模**：大。**前置**：无（K9 依赖它）。**并行**：批次 2。

### K7 定位补全轮

- **目标**：position:fixed 视口锚定 + 「最近定位祖先」包含块搜索 +
  z-index 完整层叠（简化档）。
- **范围 in**：最近定位祖先搜索（absolute 的包含块 = 最近 positioned
  祖先的 padding box；tree_build 把 fixed / absolute 节点挂到正确包含
  块宿主，taffy 树结构调整）；fixed = 包含块取初始包含块（视口），
  resize / 滚动语义；层叠上下文简化档（定位元素 + z-index 整数排序、
  同上下文绘制顺序 CSS 2.1 附录 E 简化，完整 stacking context 树记
  偏差）。
- **范围 out**：sticky、完整 stacking context 语义（opacity /
  mix-blend 等）。
- **crate 足迹**：nexty-layout（tree_build / absolute 扩展）、
  nexty-chrome / pipeline（视口锚定）、（必要时）nexty-paint 定序。
- **退出条件**：fixed 元素滚动不动（实机）；absolute 相对最近定位祖先
  （探针测试）；z-index 排序单测；float 轮回归绿。
- **规模**：中。**前置**：float 轮完成。**并行**：批次 3。

### K8 table 布局轮

- **目标**：CSS 表格布局落地（无成熟轮子：taffy 不含 table——自研例外
  ①，补 ADR；feature `table` 已预留）。
- **范围 in**：匿名表格盒生成（display: table / table-row / table-cell
  及匿名包装，CSS 2.2 §17.2.1）；固定布局算法 §17.5.2.1；自动布局
  §17.5.2.2 简化档（min / max 内容宽 + 均摊，完整约束求解记偏差）；
  rowspan / colspan；border 模型（separate 起步或 collapsed 基础档，
  侦察后定）；caption。
- **范围 out**：完整自动布局最优解、border-collapse 全语义、
  table-header-group 打印重复。
- **crate 足迹**：nexty-layout（新 table 模块 + feature `table`）、
  nexty-css（display 取值扩展）。
- **退出条件**：自建对照语料（固定布局三例、跨行跨列、匿名盒生成）+
  WPT css/CSS2 tables 子集抽样全绿；bench 回归。
- **规模**：大。**前置**：C 泳道排它（K6 / K7 之后）。**并行**：批次 4。

### K9 滚动性能大项（视口裁剪 / 增量光栅）

- **目标**：滚动光栅 O(视口)——消除 perf baseline 遗留瓶颈 #1
  （text / rules 页滚动风暴 rasterize 10–16 s/帧，全页无裁剪）。
- **范围 in**：片段树 → 场景构建期视口裁剪（复用 K6 clip scope）；
  场景分块缓存（按块缓存光栅结果，滚动只光栅新进条带，缓存上限常量化
  + LRU 淘汰）；脏区 / 增量呈现（Pixmap 局部上传，staging 复用已有）；
  页面纹理缓存 vs 分块的方案决策（按内存上限）。
- **范围 out**：GPU 合成（K12）、will-change。
- **crate 足迹**：nexty-paint、nexty-chrome / render + pipeline。
- **退出条件（bench 门槛）**：①首帧整视口冷光栅 ≤ 500 ms；②稳态滚动
  （分块命中 + 新进条带）rasterize 中位 ≤ 33 ms/帧；③与总页高无关
  （页高 ×2 语料，稳态数字变化 ≤ 10%）；emit / 呈现回归绿；实机滚动
  跟手。门槛绝对值随机器浮动，以 bench 复测记录为准。
- **规模**：大。**前置**：K6。**并行**：批次 3。

### K10 表单基线轮

- **目标**：表单可用——控件渲染 + 焦点输入 + GET 提交。
- **范围 in**：控件替换盒渲染（input text / checkbox / radio / button、
  button、select 单选简化档、textarea——自绘最小档，UA stylesheet 扩展）；
  焦点模型（Tab 遍历、focus 环、点击聚焦）；输入交互（文本插入 / 删除 /
  选区，复用地址栏编辑状态机思路；checkbox 切换走 K1 事件）；GET 提交
  （form 解析 → URL 序列化 → 导航）；label 关联点击。
- **范围 out**：POST（K3 完成后可小轮追加）、文件上传、select 多选、
  constraint validation、datalist。
- **crate 足迹**：nexty-html（语义已有）、nexty-layout（replaced 扩展）、
  nexty-chrome（app 键盘输入路由到焦点控件、pipeline）。
- **侦察点**：IME 中文输入（winit IME 事件接线）——若阻塞，先记偏差
  不硬卡退出条件。
- **退出条件**：测试表单（输入 + 勾选 + 提交 → GET 导航回显）实机走通；
  焦点遍历单测；控件渲染指令单测。
- **规模**：中–大。**前置**：K1。**并行**：批次 5。

### K11 bidi / RTL 轮

- **目标**：双向文本与方向属性。
- **范围 in**：direction / unicode-bidi 属性解析与级联；parley bidi
  能力侦察（fontique / harfrust 运行方向与重排支持面）——缺则接
  `unicode-bidi`（已在候选清单）做 run 方向分段；inline 布局按方向 run
  排布（RTL 行内顺序、中性字符解析简化档）；text-align 的 start / end。
- **范围 out**：完整 UAX#9 嵌套级别求解边缘（偏差清单）、竖排 writing
  modes。
- **crate 足迹**：nexty-css、nexty-text、nexty-layout / inline。
- **退出条件**：阿拉伯语 / 希伯来语自建语料渲染方向正确（与浏览器对照
  截图核对要点）；UAX#14 conformance 回归绿。
- **规模**：中。**前置**：K4（text 泳道串行）。**并行**：批次 5。

### K12 GPU 后端接入轮

- **目标**：vello_hybrid 双后端落地——Rasterizer trait 第二实现接入
  wgpu surface。
- **范围 in**：gpu feature 实现 VelloHybridRasterizer（双后端同形不同
  名，facade 适配层——选型 ADR 已注明）；chrome render / app 提供 wgpu
  surface 接缝（Rgba8Unorm 已有）；后端选择开关（feature / 环境变量，
  CPU 回退路径保留）；panic 兜底语义在 GPU 路径验证（catch_unwind 与
  wgpu 内部线程边界侦察）。
- **范围 out**：合成器级优化、部分呈现、MSAA。
- **crate 足迹**：nexty-paint（gpu feature）、nexty-chrome / render +
  app。
- **退出条件**：同一测试页 CPU / GPU 输出像素对照（容差断言）；双后端
  bench 数字入 perf baseline 文档；CPU 回退开关有效；渲染隔离语义
  （panic 上报）在 GPU 路径复验。
- **规模**：中–大。**前置**：K9（非硬依赖，可对调）。**并行**：批次 4。

### K13 小件轮（任意批次插队）

- **目标**：零散新能力清账。
- **范围 in**：图片格式扩展（image crate 开 jpeg / gif / webp feature；
  解码预检与预算天然复用；GIF 动图取首帧）；favicon（导航时抓取 + 地址
  栏绘制）。
- **crate 足迹**：nexty-paint（decode，独立模块）、nexty-network /
  nexty-chrome（favicon：resources + ui）。
- **退出条件**：各格式测试图解码 + 渲染单测；favicon 实机显示；预检
  对新格式生效（SizeUnsupported 路径）。
- **规模**：小。**并行**：与当批 paint 轮分模块。

## 5. X 批次：补全与收口（按用户指令殿后）

### X1 CSS 补全轮

var() / 自定义属性（级联 + 继承 + 循环检测）；rem / ex / ch 单位（root
font-size / 字体度量）；:hover / :focus / :active 动态伪类（restyle
触发路径）；::before / ::after 生成内容（树构建期匿名内容，跨 css +
layout）。规模：中。

### X2 合规收口轮

WPT html 剩余 17 条策略（11 条 html5ever 未跟进项：上游 issue / 本地
patch 决策记录进 ADR；6 条 scripted 已随 K5 收口）；dom / events、css
子集抽样入 CI（固定 wpt commit `5cd8e3fa`、通过率门槛表）；全仓覆盖率
≥ 80% 核对；deny / about 重跑。规模：中。

### X3 排版细节收口轮

text-align-last / text-justify、vertical-align 非 baseline、多行
baseline 组、float 逻辑值 inline-start / inline-end。规模：小–中。

各 crate 模块文档既有偏差清单在对应轮次启动时逐条评估，不因本路线图
自动消失。

## 6. 远期评估（明确不在本基准内）

SVG、iframe、GIF / 视频动图推进、Web Animations / transitions、
contenteditable、无障碍树、多进程 / 沙箱、WebGL / WebGPU、Storage /
Service Worker、多标签 UI。这些不是 §1 DoD 的组成部分；要纳入先改 §1。

## 7. 执行纪律

1. 泳道内串行、泳道间并行；chrome 例外：同时进行的两轮必须分属 chrome
   不同模块文件（app / pipeline / render / resources / selection / ui /
   scrollbar / history），并在两轮 goal.md 写明各自触碰清单。
2. 跨 crate 接口（K6 clip scope、K5 runner trait、K12 Rasterizer 第二
   实现）先落接口 commit，再并行实现。
3. 批次收口统一跑 cargo check / test / clippy / fmt --workspace +
   cargo deny check + cargo about 注入流程。
4. 每轮按 AGENTS 版本纪律随手 bump；行为变更逐条更新对应 crate 模块
   文档偏差清单；本路线图的偏差归属随轮次落地迁移。
5. 全部批次完成 → §1 DoD 逐项验收 → 更新 AGENTS.md 状态段 → 归档本文档。
