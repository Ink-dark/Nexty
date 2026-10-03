# goal.md — 本轮任务：子资源加载；补齐现代排版最小集

日期：2026-10-02。前置：八层已落地并端到端连通（network→html→dom→css→
layout→paint→chrome），WPT tree-construction 1854/1959。

## 本轮为什么先做这些

评估「距离可打开无 JS 网页」后的结论：八层连通已不是瓶颈，**真实页面打不开**
才是瓶颈。两个硬约束决定优先级：

1. **`img` / `link` / `script` 零实现** —— 现在打开任何真实网页连一个 logo 都
   出不来，`<link rel=stylesheet>` 的外链 CSS 也不会被应用。这是「能不能用」
   的直接门槛，收益高于任何布局特性。
2. **CSS 属性只覆盖约 30 个**（`PropertyId` 枚举到 border 系列为止，无
   `box-sizing` / `float` / `position` / `overflow`），且失效方式最糟的不是
   「算错」而是「跳过」：`@media` 整块跳过、`var()` 不求值。

故本轮顺序为：**子资源加载 → 低成本高收益 CSS → flex 单行**。GPU 光栅
（vello_hybrid）与 paint 指令集扩展（圆角/渐变）本轮降级——它们对「能打开
网页」零贡献，CPU 光栅在 1080p 下够用。

## 任务清单与退出条件

### T1 nexty-network：相对 URL 解析（0.1.2）
- 新增 `NetworkFetcher::resolve(base: &str, target: &str) -> Result<String, NetworkError>`
  （或等价 API）：以文档 URL 为 base 解析相对路径，供 chrome 层拼子资源地址。
- 退出条件：回环测试——相对 URL（`/a.png`、`./b.css`、`../c.js`、带 query/
  fragment）解析为绝对 URL；跨协议相对路径（base=http、target=//host/x）
  正确取协议；非法 base → `InvalidUrl`。

### T2 nexty-paint：DrawImage 指令（0.1.2）
- `Command::DrawImage`：图像矩形 + 像素来源 + 缩放模式（先只做 1:1 整数缩放，
  不引入插值）；图像解码**不做在 paint**——paint 只消费已解码的 RGBA8。
- 退出条件：像素级单测——DrawImage 落点正确；越界裁剪；未覆盖区仍透明。

### T3 nexty-network + paint：图片解码归属（0.1.2）
- 决策：**解码器不进 paint，也不进 network**（network 只管字节传输）。新增
  facade 内的最小解码路径，或作为 `nexty-paint` 的可选 feature。
- 退出条件：能解码 PNG（回环服务返回构造的 PNG 字节）→ RGBA8；不支持格式
  （如 GIF/WebP 首版）返回明确错误而非 panic。

### T4 nexty-html + css：外链样式表接入（0.1.3）
- `nexty-html`：`<link rel=stylesheet>` 与 `<style>` 收集为资源清单（不解析）。
- `nexty-css`：`Stylesheet` 支持多张表合并级联（当前 `compute_document_styles`
  已接受 `&[&sheet]`，需验证多表与顺序即优先级）。
- 退出条件：无头测试——`<style>` + 外链 CSS 同时存在时，作者样式按源顺序
  级联，后者覆盖前者；外链失败不阻塞页面渲染。

### T5 chrome：子资源抓取与页面重组（0.1.2）
- `app.rs`：文档加载后解析出子资源清单（CSS + 图片），**并发**抓取后重组
  页面再布局（复用 `set_page`）；图片按 `DrawImage` 下发。
- 退出条件：无头集成测试——构造含外链 CSS + 图片的页面，产出预期
  `Scene` 指令序列；单个子资源失败不影响其余渲染（降级而非白屏）。

### T6 nexty-css：低成本高收益属性（0.1.4）
- `box-sizing`（`content-box` / `border-box`，影响 width/height/margin/padding
  解析）、`overflow`（先只做 `visible` / `hidden` 的裁剪标记）、`min-width` /
  `min-height` / `max-width` / `max-height`。
- 退出条件：单测——`box-sizing: border-box` 下 width 含 padding+border；
  `max-width` 收窄盒宽；UA 默认 `content-box` 不回归既有布局测试。

### T7 nexty-css：`@media` 求值（0.1.5）
- 从「整块跳过」改为按视口宽度求值：`Stylesheet::parse_with_context` 接受视口
  条件；cascade 按匹配结果决定该规则是否参与。
- 退出条件：单测——窄视口命中 `max-width` 分支、宽视口命中 `min-width` 分支、
  不匹配的规则不参与级联；UA 样式表不受影响。

### T8 nexty-layout：inline-block 与 flex 单行（0.1.2）
- `inline-block`：按 shrink-to-fit 定宽，参与行内行盒（当前按 inline 处理）。
- flex **只做单行 `flex-direction: row`**：`display: flex` 容器内子项按
  `flex-grow` / `flex-basis` 分配主轴尺寸，交叉轴对齐先只支持 `stretch`。
- 退出条件：单测——inline-block 不再被拆行；单行 flex 三项等分 / 按 grow
  比例分配；容器不换行；未指定 display 的元素行为不变（不回归）。

### T9 门禁与收尾
- `cargo check --workspace` 零 warning；`cargo test --workspace` 全绿；
  clippy `-D warnings` 零告警；`cargo fmt --all -- --check` 通过。
- 新增依赖（图片解码极可能引入，如 `png`）→ `cargo deny check` + `cargo about`
  重跑，许可证须在白名单内。
- AGENTS.md「当前状态」与各模块偏差清单同步更新（尤其 layout / css 的
  「未实现」条目逐条收敛）。
- 分任务 commit，最终 push。

## 非目标（本轮不做）
- **GPU 加速光栅（vello_hybrid）**——对「能打开网页」零贡献，降级到后续轮次
- paint 圆角 / 渐变 / 阴影 / 变换
- grid 布局、多行 flex、`float`、绝对定位
- `var()` 自定义属性求值（自定义属性收集先只存不发）
- JS 运行时、DOM 事件与绑定、表单提交
- 页面滚动、链接点击、后退/前进历史、多标签
- winit/wgpu 胶水的自动化测试（无头环境不可行，见上轮 T6 备注）

## 里程碑与顺序

- **M1（本轮前半）**：T1～T5 —— 能打开带外链 CSS 与图片的真实静态页面
- **M2（本轮后半）**：T6～T8 —— 现代页面排版基本正确

M1 是首个可用里程碑，达成后应立即实机打开若干真实站点验收（用户侧），
用真实页面反馈驱动 M2，而不是在合成测试页上调数值。

## 验证流（AGENTS.md Verification Flow）
每层：cargo check 零 warning → cargo test 全绿 → fmt/clippy → commit。
管线端到端以无头像素断言替代 WPT 比对；app 模块的 GPU 胶水仍不可无头测试，
实机验收列为用户侧待办并在模块文档注明。