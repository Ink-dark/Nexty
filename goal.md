# goal.md — 本轮任务：paint 补文本/边框指令；chrome 层落地

日期：2026-10-02。前置：dom / html / css / network / text / paint / layout 已落地。
本轮补齐 paint 的文本与边框绘制能力，并落地 chrome 集成层（wgpu surface 呈现、
渲染隔离线程、catch_unwind 兜底、自绘地址栏 UI），把
network→html→dom→css→layout→paint 串成完整管线。

## 任务清单与退出条件

### T1 nexty-text：字体数据解析（0.1.3）
- `ParleyTextShaper::resolve_font(families) -> Option<ResolvedFont>`：
  fontique 选字（命名族 + generic 族 + 回退），返回字体文件字节与 face 索引。
- 退出条件：单测——系统字体可解析出非空字节；空族列表 → None；字节可被
  skrifa 读取为合法 face（paint 侧消费验证）。

### T2 nexty-paint：DrawText / StrokeRect 指令（0.1.2）
- `Command::DrawText`（字形 id + 绝对坐标 + 基线 + 字体族列表 + 字号 + 颜色）、
  `Command::StrokeRect`（边框矩形：颜色 + 线宽）。
- paint 新增对 nexty-text 的依赖（字体选择归 text 层；文本/字体本就是
  paint 的上游）。
- 退出条件：像素级单测——DrawText 渲染出非空字形像素（系统字体）、
  StrokeRect 只描边不填心、文本颜色正确；DrawText 与 FillRect 叠加顺序正确。

### T3 chrome：管线与显示列表（0.1.1）
- `pipeline.rs`：HTML 字符串 → html 解析 → 级联（UA+author）→ 布局 →
  Fragment 树 → paint `Scene`（背景 FillRect、四边 border、文本 run →
  DrawText；匿名片段不画背景/边框）。
- 退出条件：无头集成测试——含样式 div + 文本的页面产出预期指令序列；
  纯 CPU 光栅出像素验证（背景色/边框色/文字非透明像素）。

### T4 chrome：渲染隔离线程与兜底（0.1.1）
- `render.rs`：`RenderThread`——独立线程持有 Rasterizer，请求/应答通道；
  渲染包 `catch_unwind`，panic → `RenderError::Panic`，线程存活可继续服务。
- 退出条件：单测——正常渲染往返；注入 panic 的 Rasterizer 返回
  `RenderError::Panic` 且线程仍能服务后续请求。

### T5 chrome：自绘地址栏（0.1.1）
- `ui.rs`：纯状态机 `BrowserUi`——地址栏矩形布局（自绘：FillRect + 边框 +
  文本 run）、键盘输入（字符插入/退格/Enter 导航）、点击聚焦；产出
  （可选导航 URL，UI Scene 追加指令）。
- 退出条件：状态机单测——输入累积、退格、Enter 回调 URL、初始 URL 展示。

### T6 chrome：winit + wgpu 呈现与入口（0.1.1）
- `app.rs`：winit 0.30 ApplicationHandler + wgpu 29 surface（Rgba8Unorm 优先）
  + Pixmap→纹理上传 + blit 呈现；resize 重配；页面渲染走 RenderThread。
- `src/bin/nexty.rs` 入口：默认加载 about:blank 占位页，地址栏可输入 URL 导航
  （reqwest 抓取 → 管线 → 渲染）。
- 退出条件：`cargo check/clippy` 编译通过（无头环境无法起窗口，实机运行
  验证列为待办并在模块文档注明）；状态机/线程/管线逻辑均有测试覆盖。

### T7 门禁与收尾
- `cargo check --workspace` 零 warning；`cargo test --workspace` 全绿；
  clippy `-D warnings` 零告警；`cargo fmt --all -- --check` 通过。
- 新增依赖（若有，如 pollster）→ `cargo deny check` + `cargo about` 重跑。
- AGENTS.md「当前状态」更新（八层全部落地）。
- 分任务 commit（text / paint / chrome / 文档），最终 push。

## 非目标（本轮不做）
- 页面滚动、hover/链接点击、多标签、历史记录
- GPU 加速光栅（vello_hybrid 接入仍待 vello_gpu 正式发布评估）
- 文本输入框的光标/选区细节（地址栏只有最简插入/退格）
- winit/wgpu 胶水的自动化测试（无头环境不可行，见 T6）

## 验证流（AGENTS.md Verification Flow）
每层：cargo check 零 warning → cargo test 全绿 → fmt/clippy → commit。
管线端到端以无头像素断言替代 WPT 比对（比对任务待架构师组织）。
