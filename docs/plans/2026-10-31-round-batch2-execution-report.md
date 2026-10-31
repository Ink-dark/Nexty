# 执行报告 — 批次 2 并行轮（K5 JSRT ∥ K6 容器滚动裁剪 ∥ K13 图片格式小件）

日期：2026-11-01。分支 `feat/batch2-jsrt-scroll-images`（自 main @ 1415cec）。
任务定义：[2026-10-31-round-batch2-parallel.md](2026-10-31-round-batch2-parallel.md)。
执行方式：编排者 + 并行子代理集群（泳道 = 子代理，成果分片提交，全程
分支直接 push）。

## 一、任务总览与版本落点

| 轮次 | 泳道 | 结果 | 版本 |
| --- | --- | --- | --- |
| K5 JSRT | A | 引擎定案 + 接缝/适配/接线/WPT scripted 全链 | dom 0.1.4→**0.1.6** · **nexty-jsrt-rquickjs 0.1.0→0.1.1**（新增）· html 0.1.5→**0.1.6** · chrome→**0.1.35**（含收口接线） |
| K6 容器滚动 | C | 接口先行 → css/layout/paint/chrome 四层落地 | css 0.1.18→**0.1.19** · layout 0.1.29→**0.1.30** · paint 0.1.8→**0.1.10** |
| K13 图片格式 | S | ico 补齐 + favicon 编排/图标槽/接线 | paint（随 0.1.10）· chrome（随 0.1.35） |

## 二、K5：JSRT 选型与最小绑定

- **选型 ADR**（[docs/decisions/2026-10-31-jsrt.md](../decisions/2026-10-31-jsrt.md)）：
  rquickjs 0.14（QuickJS-NG 0.16.2）——28 包 vs Boa 171 包、MSVC check
  15.8s vs 51.6s（探针实证）、维护节奏与生态先例占优；Boa 留作可插拔
  备选。`Document` !Send 维持现状（两引擎 Context/Runtime 均 !Send；
  两次解析实测 21–98ms，重估触发条件写入 ADR）。
- **unsafe 口径落地**：kernel facade crate 维持 `#![forbid(unsafe_code)]`；
  runner 适配 crate 允许 unsafe FFI（用户口径，AGENTS Hard Rules 已修订）
  ——当前适配层**实测零 unsafe**（rquickjs-core safe wrapper 足够）。
- **dom `jsrt` 模块**：`ScriptHost` 接缝 trait、`JsrtGate` 共享句柄（重入报
  `Reentrant`）、`DomRegistry` 绑定注册表 v0（树变更/textContent/属性/
  classList/getElementById/getElementsByTagName/documentElement/
  addEventListener/dispatchEvent；querySelector 与 getComputedStyle 经注入
  钩子——dom 不反向依赖 css）、`JsrtRuntime` 任务泵（微任务检查点 → 宏任务
  → 到期定时器，嵌套钳制 5 层/5ms）。
- **html `parse_document_scripted`**：script 元素弹出点同步执行回调，
  `document.write` 内容前插回输入流（嵌套 script 递归触发）；foster 落点
  按「last table 父节点是否为元素」分支（与语料对齐，与规范步骤 5 文本
  分歧记偏差）。**WPT 1860/1959，scripted 6/6 全过**（adoption01、ark、
  foster01×2、webkit01×2）。
- **chrome `script` feature**：ScriptSession 装配 + 注入钩子（nexty-css）+
  `install_and_run`（inline/src defer 档文档序、缺席降级）；输入/生命周期
  事件「K1 表 → JS 表」成对派发；事件间隙泵 + 脏位重排；外链脚本 10 MB
  预算；`Page.document` 共享 `Rc<RefCell<Document>>`。

## 三、K6：容器滚动与裁剪

- **接口先行**：css OverflowValue 四值模型、layout Fragment 滚动接缝字段、
  paint PushClip/PopClip 指令类型（编译绿后并行实现）。
- **css**：overflow 简写展开双轴 + visible→auto 转换简化档。
- **layout**：feature `scroll`——overflow≠visible 容器识别、scrollable
  overflow rect（简化档）、滚动偏移不参与布局解算。
- **paint**：vello_cpu `push_clip_path`/`pop_clip_path`（探针确认嵌套相交
  栈可用，无降级语义）；`Scene::translated` 同步平移 clip 矩形；空弹
  panic 由 facade 深度防御。
- **chrome**：`Page.scroll_offsets`（NodeId 键，重排保留 + clamp）、emit
  容器 clip + 偏移绘制期施加、`hit_test_scrolled`、`scroll_chain_at`/
  `clamp_scroll_offsets`、scrollbar 容器级推广（`for_container` 同构几何
  复用视口状态机 + `wheel_chain_target` 链式决策）、滚轮由内向外链式消费。
  **视口滚动 emit=0 不变量保持**（四条 bench 断言全绿）；容器滚动重建场景
  （代次失效）记偏差。

## 四、K13：图片格式 + favicon

- **paint decode**：jpeg/gif/webp feature 原已启用（78caf96，M1 轮），
  本轮补 `ico`（image 内置，零新外部依赖）；预检（MAX_DIMENSION /
  MAX_PIXEL_AREA）对新格式生效（20000x1 / 16384x5000 超限 fixture 走
  SizeUnsupported）；偏差：GIF 取首帧、webp 动画不支持、EXIF 方向不应用。
- **chrome favicon**：`collect_favicon_url`（chrome 内 DOM 遍历
  link[rel~=icon]，回落 /favicon.ico）、`fetch_favicon`
  （`MAX_FAVICON_BODY_BYTES` 1 MiB）、`FaviconOrchestrator`（会话缓存 +
  去重，负结果不入缓存）；ui 地址栏左 16×16 图标槽（DrawImage 路径，文本
  右移让位）。
- **收口补线**（编排者）：app 层接线——主线程解析 URL + 会话缓存查询
  （Document !Send，DOM 遍历只在主线程），未命中起后台抓取线程，就绪经
  `FaviconReady` 回主线程入缓存并上图标槽；跨页门控（`page_base` 比对）
  防串页；page_loaded 入口清旧图标、错误页与 about:blank 清槽。chrome
  0.1.35。

## 五、门禁状态（收口实测）

| 门禁 | 结果 |
| --- | --- |
| cargo check --workspace | 绿（零 warning） |
| cargo test --workspace | **688 passed / 0 failed**（含 WPT harness、container_scroll 6、script 6） |
| cargo clippy --workspace --all-targets -D warnings | 绿 |
| cargo fmt --all -- --check | 绿 |
| 逐 crate feature 矩阵 | dom 4 组 / chrome 9 组 / paint·css·layout 各组零 warning |
| cargo deny check | advisories / bans / licenses / sources 全 ok（rquickjs 树入白名单，deny.toml 零改动） |
| cargo about + 注入 | 清单重生成；NOTICES 8 项 / MULTI 386 项注入成功 |
| bench 冒烟（2 帧滚动） | 四条机器无关断言全绿：runs=5 / emit=0 / shape 1.00x / metrics≤10 |

## 六、实机验收清单（架构师执行，判据全部入库 `tools/acceptance`）

启动：`node tools/acceptance/server.cjs`（8472）+ `cargo run -p nexty-chrome --bin nexty`。

- **K5**：`/k5script` 四条判据（README K5-1…K5-4）——inline 文档序执行、
  click → JS 监听器改 DOM → 重排、setTimeout 泵驱动、无脚本错误页。
- **K6**：`/k6nested` `/k6hidden` `/k6bar` 八条判据（K6-1…K6-8）——嵌套
  独立滚动、滚轮内→外冒泡、容器滚动条拖拽、hidden 裁剪、resize 保留。
- **K13**：`/k13` 四格式渲染；`/k13icon` `/k13noicon` favicon 两态
  （link 优先 / 回落；`/stats` 计数断言）。
- **回归**：批次 1 清单（K1/K2/K3/K4）不回退；302 落点修复持续有效。

## 七、已知偏差 / 后续（按 crate 模块文档汇总）

- **K5**：无 async 脚本档（defer/async 同档）；`type`/`integrity`/`nomodule`
  不检查；Rust 监听器与 JS 监听器两张表成对派发；脚本错误只进日志
  （unhandled rejection 未上报宿主）；定时器只在事件间隙驱动（无
  `ControlFlow::WaitUntil` 唤醒源）；绑定面无
  createTextNode/createDocumentFragment/queueMicrotask/fetch/localStorage/
  canvas/worker/ESM；事件无 trusted/composed；getComputedStyle 快照语义。
- **K6**：容器滚动重建场景（偏移烘进显示列表）；横向滚动未建模；容器
  滚动条为绘制近似（无 scrollbar-gutter）；选区命中不感知容器滚动（绘制
  已跟随）；hidden 不可编程滚动（无脚本 scrollTop 通道，K5 后补）。
- **K13**：GIF 动图取首帧；ICO 不选条目；EXIF 方向不应用。
- **html**：foster 落点分支与规范步骤 5 文本分歧（按语料对齐）；
  `scripted-tests` feature 门控（默认测试不拉引擎执行）。
- **路线图位置**：K9（视口裁剪/增量光栅）可开工——clip scope 与容器滚动
  已就绪；批次 3 = K7 ∥ K9。
