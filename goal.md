# goal.md — 本轮任务：Chrome 外壳交互补全（滚动条拖拽 / 地址栏编辑态 / 快捷键 / 悬停光标）

日期：2026-10-08。前置：上轮 T0–T6 完成（多行 baseline、工具带、历史栈、滚动、
命中测试、网络接驳），文档与子资源已在 app 后台线程经 `ReqwestFetcher` 真实抓取。
旧 goal 归档于
[docs/plans/2026-10-03-round-chrome-window-toolbar.md](docs/plans/2026-10-03-round-chrome-window-toolbar.md)。

## 本轮为什么做这些

上轮把「能打开真实网页」打通了——能抓、能排、能滚、能点、能后退。但工具带与滚动条
目前只是**显示**，不是**控件**：滚动条看得见拖不动（上轮明确降级为「仅指示」）、地址栏
只能退格删除（无光标、无选区，Home/End 无效）、键盘没有 Chrome 习惯的 Ctrl+L / Ctrl+R /
F5、鼠标悬停链接无手型反馈。这些不阻碍「打开」，但阻碍「像浏览器一样用」。

本轮只动 chrome 层：四条能力都是纯逻辑 + 既有渲染管线，可无头单测；窗口/GPU 胶水
照旧实机验收。

## 任务清单与退出条件

### T1 nexty-chrome/scrollbar：滚动条几何与拖拽（新模块）
- 把 `max_scroll` / `clamp_scroll` / thumb 几何自 `app.rs` 迁入 `scrollbar` 模块，成为
  公开纯函数；新增 `ScrollGeometry`（视口宽高 + 内容高 + 工具带高）。
- 交互状态机 `Scrollbar`：命中（thumb / 轨道上方 / 轨道下方 / 无）、按住 thumb 拖拽
  （记录抓取偏移，thumb 不跳变）、点击轨道翻一页（页高 = 轨道高）。
- 悬停态只改绘制色，不改命中区（悬停加宽会让拖拽抖动）。
- 退出条件：单测——几何（零溢出 / 半溢出 / 超长文档 thumb 下限）、命中四态、拖拽映射
  单调且端点对齐、抓取偏移不跳变、轨道上下翻页、零溢出不响应。

### T2 nexty-chrome/ui：地址栏编辑态
- `AddressBar` 增加光标（`caret`，字节偏移且始终落在 char 边界）与选区（`anchor`）。
- 编辑：`←`/`→`/Home/End 移动光标、Shift+方向扩选、Ctrl+A 全选、Backspace/Delete 删
  选区或单字符、输入字符替换选区；点击输入框 = 聚焦并全选（Chrome 语义）。
- 绘制：选区高亮带 + 光标竖线；文本超出输入框宽度时按光标位置横向滚动（用前缀整形
  取光标 x），不再从头截断。
- 退出条件：单测——上列每个编辑操作与边界（光标落 char 边界、选区归一化、删选区、
  替换选区、Home/End、Shift 扩选）、点击聚焦 + 全选；draw 在聚焦/选中时产出光标与
  高亮指令，整形失败仍降级不 panic。

### T3 nexty-chrome/ui + app：Chrome 快捷键
- `ui::shortcut(key, modifiers) -> Option<Shortcut>` 纯映射：Alt+`←`/`→`（后退/前进）、
  Ctrl+L（聚焦并全选地址栏）、Ctrl+R / F5（刷新）、Ctrl+A（地址栏全选）、Esc（失焦并
  还原当前页 URL）。
- app 用该映射替换现有 Alt+方向键硬编码；地址栏聚焦时方向键/Home/End 路由到光标，
  未聚焦时按页面滚动。
- 退出条件：映射单测（键 + 修饰 → 命令，未命中返回 `None`）；app 接线实机验收。

### T4 nexty-chrome/pipeline + app：悬停光标
- `hit_test_ex` 返回「节点 + 命中类型」（文本 run / 图片 / 盒）；`hit_test` 保留为薄封装。
- `hover_kind(page, root, x, y)`：命中祖先链有 `<a href>` → 手型；文本 run → I 形；
  其余 → 默认。
- app 在光标移动时按结果设置系统光标（仅在种类变化时调用）；滚动条拖拽中不覆盖。
- 退出条件：单测——链接文本 / 链接块 / 图片链接 → 手型，正文文本 → I 形，空白 → 默认；
  实机验收。

### T5 门禁与收尾
- `cargo check --workspace` 零 warning；`cargo test --workspace` 全绿；clippy
  `-D warnings` 零告警；`cargo fmt --all -- --check` 通过。
- AGENTS.md「当前状态」与 chrome 模块偏差清单同步；无头不可测项注明实机验收。
- 分任务 commit，最终 push（本机无凭据时留给用户）。

## 非目标（本轮不做）
- 页面文本选择与复制——需 text 层补 cluster→字符映射、layout 层 `TextRun` 携带原文，
  跨三层，另轮立项
- 多标签页、下载、右键菜单、表单交互
- 平滑滚动 / 滚动动画、Ctrl+滚轮缩放、滚动条 hover 加宽
- 地址栏按 x 落光标（需在事件层拿到整形度量；本轮「点击 = 聚焦 + 全选」，偏差记录在
  `ui` 模块文档）
- vello_hybrid GPU 光栅、paint 圆角/渐变/阴影
- JS 运行时、cookie/缓存层、HTTP/2 以上特性调优
- 触摸/手势输入、DPI 缩放策略（仍按物理像素 1:1）

## 验证流（AGENTS.md Verification Flow）
每层：cargo check 零 warning → cargo test 全绿 → fmt/clippy → commit。
app 胶水（系统光标设置、拖拽事件路由、实机快捷键）无头不可测：纯逻辑（滚动条状态机、
地址栏编辑、快捷键映射、悬停种类）进单测，其余列为实机验收待办并在模块文档注明。
