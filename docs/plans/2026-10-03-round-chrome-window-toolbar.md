# goal.md — 本轮任务：Chrome 窗口层补全；网络接驳准备

日期：2026-10-03。前置：上轮 T1–T8 完成（子资源加载、低成本 CSS、@media、
inline-block/单行 flex），文档与子资源已在 app 后台线程经 `ReqwestFetcher`
真实抓取。旧 goal 归档于
[docs/plans/2026-10-02-round-subresources-css-flex.md](docs/plans/2026-10-02-round-subresources-css-flex.md)。

## 本轮为什么做这些

网络能拉回页面，但窗口层没有配套能力：页面高于视口就看不到下方内容
（无滚动）、链接点不了（无命中测试）、没有后退/前进/刷新、加载中无反馈、
多行文本还画在同一个 baseline 上（下探时发现的渲染 bug）。这些是「真实
网页能逛起来」的直接门槛。网络层 trait 与 reqwest 实现已就位，缺的是
接驳缝：可替换 fetcher、请求头（User-Agent）、超时、加载状态出口。

## 任务清单与退出条件

### T0 nexty-chrome：多行文本 baseline 重叠（渲染 bug）
- `pipeline::emit` 逐行累加行偏移：文本 baseline、行内图片与行内原子盒
  的 y 都要加「前面行高之和」；`LineFragment` 坐标保持行内局部系不变。
- 退出条件：先写 failing test（折行段落的各 DrawText baseline 严格递增）
  看它 fail → 修 → pass；单行内容回归不破坏。

### T1 nexty-chrome/ui：地址栏工具带（样式 + 状态机）
- 布局改为：左侧 后退/前进/刷新 三按钮（等宽方块，字形 DrawText，整形
  失败降级不画字形），右侧输入框（现有样式迁移）；输入区不再占满整条。
- `UiAction` 扩展工具命令（back/forward/reload）；按钮按可用标志禁用
  灰显，禁用点击无动作。
- 加载状态：`set_loading(bool)`，加载中在地址栏下缘画强调色进度条
  （静态，无动画）。
- 退出条件：单测——按钮命中区产生对应命令；禁用态无命令；loading 条按
  状态绘制；输入框新几何下聚焦/输入/提交回归不破坏。

### T2 nexty-chrome：会话历史栈
- 新增 `History`：push（新导航清空前进栈）、go_back/go_forward（返回
  URL 并交换栈位）、current、can_back/can_forward。
- app 接线：导航入栈；T1 按钮触发后退/前进/刷新；后退/前进把地址栏
  可用标志与滚动位置归零。
- 退出条件：History 单测覆盖 push/back/forward/新导航清前进栈/空栈边界；
  app 接线实机验收（无头不可测，模块文档注明）。

### T3 nexty-chrome/app：页面滚动与滚动条
- 滚动状态：scroll_y ∈ [0, max]，max = 内容高 + BAR_HEIGHT − 视口高
  （下限 0）；新页面加载归零，resize 重排后保留并重新 clamp。
- 输入：滚轮（LineDelta/PixelDelta 均支持）；地址栏未聚焦时
  PageUp/PageDown/Home/End/↑/↓。
- 合成：每帧按 `origin_y = BAR_HEIGHT − scroll_y` 重建页面显示列表
  （复用 `build_scene_at`）；页面溢出时右侧绘制滚动条轨道 + thumb
  （纯几何函数，thumb 最小高度下限）。
- resize 触发按当前视口宽重排（复用 set_page）。
- 退出条件：max/clamp 与 thumb 几何单测（零溢出、半溢出、超长文档、
  thumb 下限）；实机验收滚轮/按键/滚动条指示。

### T4 nexty-chrome/pipeline：链接命中测试与点击导航
- `hit_test(root, point) -> Option<NodeId>`：块级/原子盒 border_box、
  行盒文本 run（字形 x 范围 + 行纵向范围）、图片 run 递归命中，取最深。
- `link_target(page, node) -> Option<String>`：DOM parent 链找最近
  `<a href>` 祖先，返回 href 原文；相对 href 由调用方经
  `nexty_network::resolve` 归一。
- app：地址栏区域外的点击换算页面坐标（含 scroll）→ hit test →
  link_target → resolve → 导航。
- 退出条件：单测——命中行内链接文本、块级链接、图片链接、行内原子盒；
  点空白返回 None；`<a>` 无 href 不产生目标；相对 href 解析为绝对 URL。

### T5 nexty-network + chrome：网络接驳准备
- `Request` 增加 `headers: Vec<(String, String)>`（默认空；reqwest 实现逐
  条应用到请求；回环测试验证到达）。
- app 侧：fetcher 字段改为 `Arc<dyn NetworkFetcher>`（接驳缝，网络层可
  整体替换）；启动用 `with_timeout(30s)`；导航请求带 User-Agent。
- 加载反馈接线：navigate → loading=true，PageLoaded → loading=false；
  `document_title` helper → 窗口标题；错误页加内联样式（可读排版）。
- 退出条件：network 回环单测——请求头到达服务端、既有错误路径回归；
  document_title 单测（有/无 title）；实机验收真实站点（UA/超时/错误页/
  标题）。

### T6 门禁与收尾
- cargo check --workspace 零 warning；cargo test --workspace 全绿；
  clippy -D warnings 零告警；cargo fmt 通过。
- AGENTS.md「当前状态」与 chrome 模块偏差清单同步（无头不可测项注明
  实机验收）。
- 分任务 commit，最终 push。

## 非目标（本轮不做）
- 多标签页、下载、右键菜单、文本选择、表单交互
- 滚动条拖拽（本轮滚动条仅指示不可拖）、平滑滚动/滚动动画
- vello_hybrid GPU 光栅、paint 圆角/渐变/阴影
- JS 运行时、cookie/缓存层、HTTP/2 以上特性调优
- 触摸/手势输入、DPI 缩放策略（仍按物理像素 1:1）

## 验证流（AGENTS.md Verification Flow）
每层：cargo check 零 warning → cargo test 全绿 → fmt/clippy → commit。
app 胶水（滚轮/键盘路由/窗口标题/实机网络）无头不可测：纯逻辑
（UI 状态机、History、滚动几何、命中测试、title 提取）进单测，其余列为
实机验收待办并在模块文档注明。
