# goal.md — 本轮任务：批次 1 并行轮（K1 事件模型 ∥ K2 绘制扩展 ∥ K3 导航网络 ∥ K4 Web 字体）

日期：2026-10-31。前置：性能轮已归档（[2026-10-25-round-perf.md](2026-10-25-round-perf.md)）；
float 轮进行中（[2026-10-31-round-float.md](2026-10-31-round-float.md)）。
本轮按 [kernel-roadmap](2026-10-31-kernel-roadmap.md) 批次 1 执行：四条泳道
并行，对应轮次卡片 K1–K4；X 补全批次不进本轮。每条泳道独立任务线、独立
commit；整轮退出条件见文末批次收口。

## 并行结构与启动条件

| 泳道 | 轮次 | crate 足迹 | 启动条件 | chrome 触碰清单 |
| --- | --- | --- | --- | --- |
| A | K1 事件模型 | nexty-dom、nexty-chrome | **立即可启**（与 float 无 crate 冲突） | app 新事件接线文件 + pipeline 导航完成点单处钩子 |
| B | K2 绘制扩展 | nexty-css、nexty-paint、nexty-chrome | float 轮 T1（css float/clear）合入后 | pipeline 显示列表构建文件 |
| D | K3 导航网络 | nexty-network、nexty-chrome | T1–T4 立即可启；**T5 等 K1 T3 合入**（app.rs 交叉面） | app 导航编排 + resources 缓存接入 |
| E | K4 Web 字体 | nexty-css、nexty-text、nexty-chrome | css 任务在 **K2 T1 合入后**串行（同 crate）；text 任务立即可启；**T3 等 K3 T5 合入**（resources） | resources 字体资源类型 |

chrome 接线串行链（app.rs / resources.rs 交叉面，按 §7 纪律错峰）：
**K1 T3 → K3 T5 → K4 T3**；K2 T3 与 K1 T3 分文件并行（事件钩子 vs 显示
列表构建）。同一 crate 同时只允许一条任务线在改；红线冲突先在本文档对
齐，不改红线。

## 泳道 A（K1）：事件模型轮（dom 0.1.3 → 0.1.4）

### T1 nexty-dom：`event` 模块——Event / EventTarget / 传播算法
- `Event`：type、bubbles、cancelable、eventPhase（none / capturing /
  at_target / bubbling）、defaultPrevented、timeStamp；
  `stopPropagation` / `stopImmediatePropagation` / `preventDefault`
  （DOM §2.9 简化档：无 shadow、无 trusted 语义）。
- `EventTarget`：监听器按（type, capture）存于 Document 侧容器（arena
  节点不挂堆结构）；监听器用自身 trait 类型，不外泄依赖类型（Hard
  Rules）；add / remove / dispatch。
- 传播：capture 沿祖先链下行 → target → bubble 上行；target 上 capture
  与非 capture 监听器都触发、按注册顺序；bubbles=false 停在 target。
- 事件数据面：UIEvent / MouseEvent / KeyboardEvent / WheelEvent /
  FocusEvent（FocusEvent 本轮仅类型面，无派发点——K10 补）。
- **退出条件**：单测钉住——①三阶段顺序与注册顺序（含 target 双捕获
  语义）；②stopPropagation 与 stopImmediatePropagation 差异；
  ③preventDefault：cancelable=false 时无效、取消后 defaultPrevented=
  true；④bubbles=false 不冒泡；⑤removeEventListener 后不再触发。
  `cargo check -p nexty-dom` 零 warning；逐 feature
  `--no-default-features` 编译零 warning；dom 0.1.4。

### T2 nexty-dom：EventLoop + 生命周期事件定义（并入 0.1.4）
- `EventLoop`：FIFO 任务队列（自身 FnOnce 包装类型）+ `post_task`；
  「当前任务跑完才取下一个」——派发中 post 排队不重入；微任务队列只留
  接口空位（K5 填充）。
- 生命周期事件类型：`DOMContentLoaded`（bubbles=true，document 上派发）、
  `load`（bubbles=false，document 上派发）；派发时机在 T3 接线。
- **退出条件**：单测——FIFO 顺序；派发中 post_task 排队不重入；两个
  生命周期事件的字段与 bubbles 正确。

### T3 nexty-chrome：交互事件接线 + 生命周期派发
- hit_test_ex 结果 → 构造 MouseEvent；点击序列 mousedown → mouseup →
  click（释放且未拖动才 click，沿用现语义）；wheel → WheelEvent；键盘
  → KeyboardEvent。
- 默认行为改造：**先派发后默认**——链接导航、滚轮滚动、键盘滚动与快捷
  键全部改走「派发 → 未取消 → 默认动作」，行为对外不变。
- 生命周期派发：HTML 解析完成 → DOMContentLoaded；首轮子资源编排 settle
  （含失败 / 超时 / 预算降级）→ load；经 EventLoop post_task 派发。
- 触碰面约束：事件接线集中在 app 新模块文件（如 `app/events.rs`）；
  pipeline 仅导航 / 解析完成点插 EventLoop 钩子（单处）。
- **退出条件**：集成测试——①click 派发且默认导航仍发生；②注入测试
  监听器 preventDefault 后默认导航被跳过（证明「先派发后默认」顺序）；
  ③DOMContentLoaded 在解析后、load 在子资源 settle 后各恰一次（计数
  钩子）；④既有回归（链接 / 滚轮 / 键盘 / 悬停 / 选区 / 复制）全绿；
  实机冒烟行为与此前一致。chrome 随行为 commit bump。

### T4 收尾登记
- Feature 映射表加 nexty-dom `event` 行；AGENTS.md dom 状态段补事件系统；
  偏差清单登记（无 shadow / composed、无 trusted、FocusEvent 无派发点、
  无 pointer capture / 合成事件 / IME）。
- **退出条件**：登记完成并入批次收口门禁；逐任务独立 commit。

## 泳道 B（K2）：绘制指令集扩展轮（paint 0.1.7 → 0.1.8；css 0.1.15 → 0.1.16）

### T0 侦察（先于 T2，结论写进任务记录）
vello_cpu 能力探针：圆角矩形填充 / 描边、线性渐变填充的支持面。不支持
的部分当场定降级语义并记偏差（如圆角边框描边降级直角），不留悬案。

### T1 nexty-css：新属性解析（css → 0.1.16，float T1 的 0.1.15 之上）
- text-decoration-line（none | underline | overline | line-through）+
  简写 text-decoration（可带 color / style 成分，解析存储、渲染仅
  solid——记偏差）；不继承。
- z-index（auto | integer）；不继承。
- border-radius（1–4 值基础档；`/` 椭圆语法记偏差）；不继承。
- background-image: linear-gradient 基础档（deg 与 to left / right /
  top / bottom、色标 `color [%]?` 列表）；background 简写展开兼容渐变
  值；不继承。
- **退出条件**：解析单测（合法 / 非法 / 简写展开 / 继承性）；computed
  style 字段齐；零 warning；版本 bump 记入 commit。

### T2 nexty-paint：新绘制指令（paint → 0.1.8）
- 圆角矩形填充、线性渐变填充（按 T0 探针结论选 vello_cpu 路径或自算）。
- 文本装饰线指令（underline / overline / line-through 矩形带：位置与
  厚度按字体 metrics 的 underline 度量，缺省经验值 offset 0.08em /
  thickness 0.06em——记偏差）。
- M6 兼容：新指令进 `Scene::translated` 平移路径与 SceneCache（场景缓存
  指令枚举扩展）。
- **退出条件**：像素级单测——圆角四角 alpha 采样、渐变端点 / 中点色
  断言、装饰线 y 位置与厚度；`cargo check -p nexty-paint` 零 warning；
  paint 0.1.8。

### T3 nexty-chrome / pipeline：显示列表构建
- 背景传播（CSS 2.2 §14.2）：body 背景 → canvas，html 未指定时用 body
  的；场景底色 / 底渐变。
- z-index 基础定序：定位元素绘制在普通流之上、定位兄弟间按 z-index 整数
  排序（完整附录 E 留 K7，记偏差）。
- text-decoration：按 computed style + 文本 run 行几何逐行画装饰带
  （跨行分段；行内元素边界断线语义简化——记偏差）。
- border-radius：块盒背景换圆角路径；边框按 T0 探针结论（支持则圆角
  描边，否则维持直角边框带并记偏差）。
- **退出条件**：视觉对照页（.workbuddy：圆角卡片 / 渐变横幅 / 下划线
  链接 / z-index 叠放 / body 背景传播五组）实机逐条核对；M6 场景缓存
  回归绿（新指令经平移正确）；bench 三场景无 >5% 回退。

## 泳道 D（K3）：导航与网络完备轮（network 0.1.5 → 0.1.6）

### T1 nexty-network：`url` 直依赖 + resolve 收口（network → 0.1.6）
- 现状：resolve 经 reqwest 拿 `Url` 类型（`resolve = ["dep:reqwest"]`）。
  改为 `dep:url` 直依赖，resolve 特性不再拉 reqwest；`pub fn resolve`
  签名不变（String 进出，无依赖类型外泄）。
- 增 `Url` 最小封装（parse / join / as_str，自身类型）供 T5 与后续轮；
  ADR 一条（直接依赖 `url`）。
- **退出条件**：resolve 既有回环测试不变绿；
  `cargo check -p nexty-network --no-default-features --features resolve`
  编译通过且依赖树无 reqwest（cargo tree 断言）；deny / about 更新记入
  commit。

### T2 字符编码探测（encoding_rs，network + chrome）
- 解码入口（network 层，自身类型封装）：HTTP header Content-Type
  charset > BOM（UTF-8 / UTF-16LE / BE）> `<meta charset>` / http-equiv
  预扫（前 1024 字节）> 默认 UTF-8。
- chrome：主文档从严格 `String::from_utf8`（app.rs）改走探测解码；样式
  表维持简化档（按文档解码结果或 UTF-8，lossy 兜底）——记偏差。
- encoding_rs ADR + deny / about。
- **退出条件**：探测单测（四层优先级 × 构造用例矩阵）；GBK 测试页实机
  正确渲染（此前直接进错误页）；非法字节路径不 panic（错误页兜底）。

### T3 nexty-network：重定向语义显式化（并入 0.1.6）
- 显式 policy（跟随 ≤10、防环）；`Response` 增最终 URL（reqwest
  `Response::url()` 经自身类型透出）；文档 base = 最终 URL（子资源相对
  地址随重定向落点解析）。
- **退出条件**：本地测试服务器 301 / 302 用例最终 URL 正确；重定向后
  相对子资源按落点解析（探针页）；防环上限单测。

### T4 nexty-network：HTTP 缓存（feature `cache`，并入 0.1.6）
- 内存级：Cache-Control max-age / no-store / no-cache（no-cache = 强制
  再验证）、ETag → If-None-Match、Last-Modified → If-Modified-Since、
  304 复用缓存体；Vary 含除 User-Agent 外字段 → 不缓存；无新鲜度且无
  validator → 不缓存；容量上限常量化 + LRU。
- **退出条件**：回环测试（复用既有测试服务器基建）五例——命中 / 过期
  再验证 304 / no-store 不存 / Vary 不缓存 / 首次未命中；缓存键含
  method + url。

### T5 nexty-chrome：导航编排接线（等 K1 T3 合入后启动）
- 地址栏输入 → Url 解析（无 scheme 补 `http://`，记决策）；文档 / 样式
  表 / 图片抓取接入缓存；主文档解码走 T2。
- **退出条件**：实机——①GBK 页渲染正确；②302 测试页落点正确且相对
  链接基于最终 URL；③同 URL 二次导航命中缓存（计数断言）；④导航 /
  刷新 / 前进后退行为回归全绿。

### T6（可选，时间盒）：Cookie 最小面
内存会话级接受 / 回发，无持久化。时间不够整段拆独立轮，**不阻塞本轮
退出**。

## 泳道 E（K4）：Web 字体轮（text 0.1.8 → 0.1.9；css 0.1.16 → 0.1.17）

### T1 nexty-css：@font-face 解析（css → 0.1.17，K2 T1 之上串行）
- at-rule 解析进 stylesheets 模块（@media 基建复用）：font-family
  （必需）、src（url()[format()] / local() 有序列表）、font-weight
  （单值）、font-style（normal | italic）；font-display 解析存储、行为
  恒「就绪后重排一次」swap 简化（记偏差）；unicode-range / font-stretch
  记偏差。
- Stylesheet 携带 @font-face 规则列表；必需描述符缺失 / 非法 → 丢整条
  规则。
- **退出条件**：解析单测（多 src 回退序、缺描述符丢弃、非法 src 跳过
  该源）；css 版本 bump。

### T2 nexty-text：字体注册表 + FontResolver 扩展（text → 0.1.9）
- 会话级 `FontRegistry`：注册字体字节 + 描述符 → face（ttc face index
  复用现有 (blob id, face index) 缓存路径——侦察确认）；`resolve_font`
  扩展：@font-face 家族名命中 → 提供注册字节；同族多 face 按期望
  weight 就近匹配、italic 优先（CSS Fonts §5.2 简化档，记偏差）。
- 测试字体：入库一份 OFL / Apache 许可小字体 + 随附 license 文件（非
  cargo 依赖，不入 deny；license 合规记录进 T4 ADR）。
- **退出条件**：注册表单测（同名多 weight 就近匹配、未命中回退系统
  字体）；整形集成单测——shape 产出注册字体字形且 `ShapedText.fonts`
  归属正确（非 tofu）；text 0.1.9。

### T3 nexty-chrome：字体子资源编排（等 K3 T5 合入后启动）
- @font-face src 收集 → 抓取（新 `MAX_FONT_BODY_BYTES = 20 MB` 预算
  常量，超预算降级跳过——与既有预算语义一致）；404 / 失败 → 回退系统
  字体；字体就绪 → 失效世代号触发一次重排 + 重绘（chunk 分析缓存与
  SceneCache 缓存键一并接入）。
- **退出条件**：实机测试页（自托管 ttf）——①字形与系统回退可区分；
  ②404 字体页正常渲染（回退路径）；③就绪后恰一轮重排（shape 调用计数
  断言）；④重复导航共享已注册字体（不重复抓取——缓存命中断言）。

### T4 收尾
ADR（@font-face 方案 + woff / woff2 不支持决策 + 测试字体 license）；
AGENTS.md text / css / resources 状态段与偏差清单登记。

## 批次收口（整轮退出条件）

1. 四条泳道全部任务的退出条件逐条满足（T6 除外，其为可选项）。
2. 门禁全绿：`cargo check / test / clippy / fmt --workspace` 零 warning；
   `cargo deny check` 通过；`cargo about generate` 两份产出 + 注入脚本
   （`collect_notices.py` / `inject.py`）跑通——新依赖 `url` /
   `encoding_rs` 入清单与 NOTICE。
3. bench 回归：`BENCH_MODE=1` 三场景对比 perf baseline，任一阶段回退
   >5% 需在任务记录中说明原因或回退改动。
4. 版本纪律：每个行为 commit 随手 +1（预期落点：dom 0.1.4、network
   0.1.6、text 0.1.9、paint 0.1.8、css 0.1.17〔float T1 的 0.1.15 之上
   连续两 bump〕、chrome ≥ 0.1.26；若 float 实际版本号不同则顺延）。
5. AGENTS.md 登记：dom `event` 与 network `cache` 进 Feature 映射表；
   dom / css / text / network / chrome 各状态段与偏差清单更新。
6. 本 goal.md 归档 + push；等用户验收（架构师跑语义比对与实机验收）。

## 明确不做（偏差清单，进对应 crate 模块文档）

- JS 监听器消费与微任务执行（K5）；pointer capture / 合成事件 / IME；
  FocusEvent 派发点（K10 焦点模型）。
- woff / woff2 字体格式、unicode-range、font-stretch、font-display 全
  语义。
- Cookie 持久化（T6 仅内存会话级）；Service Worker；磁盘缓存。
- 径向 / 锥形渐变、多重背景、box-shadow、background-attachment；完整
  stacking context（K7）。
- 样式表独立编码探测（按文档编码或 UTF-8，lossy 兜底）。
- POST 表单提交（K3 后小轮）、文件上传。

## 归档（2026-10-31 执行完毕）

执行方式：Agent 集群——波次 1 五路并行（K1 dom / K3 network / K4 text /
K2 css / K2 侦察）+ 波次 2 按依赖链派发（K2 paint / K4 T1 css / chrome
串行链 K1 T3 → K2 T3 → K3 T5 → K4 T3），chrome 触碰任务全程串行无冲突。

- **结果**：12 项任务全部完成（T6 Cookie 可选项按时间盒跳过）；执行详情
  与证据锚点见 [execution report](2026-10-31-round-batch1-execution-report.md)。
- **版本落点**：dom 0.1.4 / css 0.1.18 / network 0.1.6 / text 0.1.9 /
  paint 0.1.8 / chrome 0.1.31（含验收修复）。
- **门禁**：workspace check / clippy(-D warnings) / fmt / test 全绿；
  deny / about 通过；bench 本机 text-heavy 无回退 + **CI runner 首次基线**
  （50 帧全量，四条机器无关断言全绿，滚动 translate 2.4ms / rasterize
  2472ms——K9 门槛以后的判定口径）。
- **验收修复**（用户实机验收发现）：302 链接基准缺陷——会话历史条目与
  地址栏落点后替换为最终 URL（`History::replace_current`，chrome 0.1.31）。
- **计划外事件**：K2 T3 子代理被框架挂起中断在收尾，编排者补齐（渐变四
  正向精确轴向量 / 选区测试载体迁移 / clippy 与 fmt 收口）；K1 衍生架构
  决策（Document !Send → Send 数据面 + 主线程建 Document）留 K5 窗口评估。
- 根目录 goal.md（状态跟踪工作副本）随归档移除；后续轮次从
  [kernel-roadmap](2026-10-31-kernel-roadmap.md) 批次 2 启动
  （K5 JSRT ∥ K6 容器滚动 ∥ K13 小件）。
