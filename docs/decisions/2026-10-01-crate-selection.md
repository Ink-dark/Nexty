# ADR: 分层 crate 选型

- 日期：2026-10-01
- 状态：已接受（渲染后端已定，JSRT 层待定）
- 背景：Nexty 是 MusKitty 的「不造轮子」分支，需在动手写代码前定下各层复用的成熟 crate

## 背景

MusKitty 从零手写浏览器核心模块（HTML/CSS/Layout/Render/Network 全自研）。Nexty 反向取舍：**已有成熟 crate 能覆盖的能力直接依赖**，只把精力留给真正没有轮子的部分。

选型通过三轮提问收敛，约束来自 AGENTS.md 的既有硬规则：仓库内代码零 unsafe、依赖类型不得进入 pub 导出、WHATWG/WPT 为行为 ground truth。

## 决策

### 管线形态：自有管线 + 逐层选轮子

保留 MusKitty 的分层（HTML → CSS → Layout → Paint → Network → Chrome），每层换成熟 crate，层间接口自定。

- 未选：整体采用 Blitz 生态（`blitz-dom`/`blitz-html`/`blitz-paint`）。理由是架构与迭代节奏会受制于上游，且 Blitz 内部把 Stylo 作为样式引擎，与本文档的 cascade 自研决策冲突。
- 未选：Blitz 骨架 + 自持顶层。同上，样式层冲突无法绕开。

### 逐层选型

| 层 | 选型 | 许可证 | 决策理由 |
| --- | --- | --- | --- |
| HTML 解析 | `html5ever` | MIT/Apache-2.0 | Servo 的 WHATWG 合规解析器，WPT 通过率最高，Blitz 等 Rust 浏览器项目在用 |
| DOM | 自研 arena DOM | — | 后续 JSRT 绑定需要可变、可共享引用的树；`kuchiki` 的 Rc 不可变树与 DOM 可变性直接冲突，`ego-tree`/`indextree` 仍需自接绑定层且受类型外泄规则约束要包一层 |
| CSS 解析 / 选择器 | `cssparser` + `selectors` | MPL-2.0 | Servo 出品，Firefox/Servo/lightningcss 共用底座，规范贴合度高 |
| Cascade | 自研 | — | 可从 MusKitty 迁移已有实现，保留对 cascade 行为的完全控制 |
| 盒级布局 | 自研 | — | 可从 MusKitty 迁移已有实现；`taffy` 虽成熟但布局是 WPT 对齐的关键层，需完全可控 |
| 文本整形 / 字体 | `parley` + `swash` + `fontique` | Apache-2.0/MIT | linebender 系，与 vello 同生态，支持 variable fonts 与 `@font-face`，自带 bidi/断行 |
| 绘制 / 光栅 | `vello_cpu` + `vello_hybrid` | Apache-2.0/MIT | CPU 与 GPU 双后端，API 同形不同名需适配层；详见下方「渲染后端调研」 |
| 窗口 / 输入 | `winit` + `wgpu` | Apache-2.0/MIT | `vello_hybrid` 需要 wgpu surface；`vello_cpu` 的 `Pixmap` 可作为纹理上传复用同一呈现路径 |
| 网络 | `reqwest` | MIT/Apache-2.0 | 异步 + blocking 双入口，TLS/重定向/连接池开箱即用，MusKitty 已验证 |
| URL | `url` | MIT/Apache-2.0 | 候选，导航与相对 URL 解析时引入 |
| bidi / 复杂脚本 | `unicode-bidi` + 按需 `icu` | MIT/Apache-2.0 | 候选，与 parley 配合，用到再引 |
| 日志 / 错误 | `tracing` + `thiserror` | MIT/Apache-2.0 | 候选，用到再引 |

未选 HTML 备选：`html5gum`（只做 tokenizer，tree construction 要自写，等于半造轮子）、`lol_html`（面向边缘流式改写，非完整树构建）。

未选网络备选：`ureq`（同步轻量，但与后续并发子资源抓取不匹配）、`hyper` + 自建（TLS/重定向/连接池要自写，变成造轮子）。

未选窗口备选：`winit` + `softbuffer`（CPU 呈现，与 vello 路径不统一）、`gpui`（应用 UI 框架而非浏览器窗口层，易与自研管线冲突）。

### 渲染后端调研（2026-10-01）

原计划选 `vello` + `wgpu`。调研后该前提不成立，改为 `vello_cpu` + `vello_hybrid` 双后端。

**项目结构已变**（[vello README](https://github.com/linebender/vello/blob/main/README.md)）：

| 渲染器 | crates.io | 定位 |
| --- | --- | --- |
| `vello`（compute 版） | 0.10.0（2026-08-14） | 已移入仓库 `research/` 目录，README 称其 "remains an experimental implementation" |
| `vello_cpu` | 0.2.0（2026-08-07） | sparse strips，CPU-only，README 称 "currently overall more mature" |
| `vello_hybrid` | 0.2.0（2026-08-07） | sparse strips，CPU/GPU 混合；仓库 main 已改名为 `vello_gpu`，但 crates.io 上 [`vello_gpu`](https://crates.io/crates/vello_gpu) 只是占名包（12 行代码，2026-08-26 发布） |

两个 sparse strips 渲染器共用 `vello_common` 与 `glifo`（文本/字形）。

**发布节奏**（[vello_cpu CHANGELOG](https://github.com/linebender/vello/blob/main/vello_cpu/CHANGELOG.md)）：15 个月 11 个版本，2026 年内明显加速——0.0.5(01-08) → 0.0.6(01-15) → 0.0.7(03-24) → 0.0.8(05-15) → 0.0.9(05-30) → 0.1.0(07-29) → 0.2.0(08-07)。最近 6 个版本中 3 个含破坏性变更：0.0.6 升 peniko/kurbo 大版本；0.0.8 文本渲染迁至 `glifo`、删除实验性 `vello_api` 抽象与 recordings 功能；0.1.0 把 `render_to_pixmap` 等方法合并为 `render`/`render_with`。MSRV 在 1.85 → 1.86 → 1.88 → 1.92 → 1.88 → 1.89 之间抖动，README 另声明 MSRV 提升不算破坏性变更、可出现在 patch 版本。

**决策理由**：`vello_cpu` 是官方标注的最成熟实现，纯 CPU 且自带 `png` feature 与 `Pixmap` 输出，headless 出图原生支持，直接解决 CI 与无显卡环境问题；GPU 侧用 `vello_hybrid` 接上，等 `vello_gpu` 正式发布后再评估，现在不绑占名包。

**已知代价**：

1. 两个后端是**同形不同名**的 API——`vello_cpu` 用 `RenderContext`，`vello_hybrid` 用 `Scene`，`Resources` / `Pixmap` / `GlyphRunBuilder` / `RenderSettings` 同名同形但非同一类型。facade 需写一层适配，不是零成本切换。
2. 两者对未支持特性**都是 panic 而非返回错误**：`vello_cpu` 的 complex filter graphs 会 panic；`vello_hybrid` 的 mask layers、complex filter graphs、部分非隔离混合模式会 panic，其文档另有 "Some failures panic instead of being reported through a user-facing error"。
3. 上游 pre-1.0 且改名进行中，绑定即接受后续破坏性变更。

**应对**：渲染在独立线程执行，panic 由 `catch_unwind` 兜住并上报。因此 workspace 与各 crate 不得设置 `panic = "abort"`（会禁用 unwind，使兜底失效）。该约束已写入 AGENTS.md 硬规则「渲染隔离」。

### 许可证政策

- 白名单：MIT / Apache-2.0 / BSD / ISC / MPL-2.0 / LGPL
- 禁止：GPL / AGPL
- MPL-2.0 是文件级弱 copyleft，Servo 的 `cssparser` / `selectors` 与 `lightningcss` 均为该许可，不放开则 CSS 侧只能自研
- 门禁：`cargo-deny` 扫传递依赖做 CI 硬门禁（白名单见 `deny.toml`）
- 清单：`cargo-about` 生成 `docs/dependencies.html` 入库，引入或升级依赖后必须重新生成

主要顾虑是**传递依赖可能引入不可控许可证**，故门禁必须覆盖传递依赖而非只查直接依赖。

### 隔离策略：facade crate + trait 抽象后端

- 每层一个 facade crate，外部 crate 只允许出现在该层 crate 内，其他层只见自有类型
- 对外能力先定 trait（`NetworkFetcher` / `TextShaper` / `Rasterizer` 等），具体后端作为可替换实现
- 配合 AGENTS.md 的「依赖类型不得外泄」硬规则：依赖类型不得出现在任何 pub 导出

该组合的目的：替换上游只需改一个 crate；上游许可证变化或 alpha API 变动时，隔离边界已就位。

## 待定项

- **JSRT 层**：JS 运行时设计为可插拔层，选型暂缓。已定约束是 DOM 与 JS runner 必须同处，且 JS 运行时类型不得渗入 DOM 公共 API。
- **GPU 后端归属**：`vello_hybrid` 正在改名为 `vello_gpu`，待 `vello_gpu` 在 crates.io 发布正式实现（而非占名包）后重新评估是否切换。

## 后果

- 需要在 `docs/` 之外维护 `deny.toml`、`about.hbs`、`docs/dependencies.html` 三项许可证相关产物
- 渲染层要额外承担两件事：一是为 `RenderContext` / `Scene` 两套同形 API 写适配层；二是维护隔离渲染线程与 `catch_unwind` 兜底。同时 workspace 与各 crate 永久不得设置 `panic = "abort"`
- 渲染后端的上游风险最高（pre-1.0、改名进行中、6 个月 3 次破坏性变更），`nexty-paint` 的 facade 边界因此是最需要严格守住的一层
- DOM / Cascade / Layout 三层自研意味着这三层不享受「不造轮子」红利，是主要的自研投入所在；AGENTS.md 已将其列为「不造轮子」规则的显式例外，且不得据此扩大到其他层
- 各层 facade crate 尚未创建，workspace `members` 仍为空