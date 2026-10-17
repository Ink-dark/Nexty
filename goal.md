# goal.md — 本轮任务：把 UAX#14 断行接入真实布局（white-space / overflow-wrap·word-break / `<br>`）

日期：2026-10-17。前置：commit `35029cb` 已落地 UAX#14 conformance API
（`cluster_map` / `break_opportunities` / `min_content_width` / `split_spans`）与
`nexty_text::Glyph::cluster`，官方 `LineBreakTest-18.0.0` 的 19,346 条语料全绿；
`nexty-css` 0.1.10 / `nexty-text` 0.1.7 / `nexty-layout` 0.1.15 / `nexty-chrome` 0.1.17。
**本轮不写 justify**（T6 已明确移出本轮）。

## 本轮为什么做这些

上一轮把「UAX#14 断点 + cluster 映射」做成了**可逐条比对、但尚未被布局消费**的 API。
当前真实渲染路径仍是旧行为：

- `inline.rs::build_lines` 仍只用 ASCII 空白切词；中文段落（`<p>` 内 120 字、
  200px 视口）仍可能溢出为一行（此前实测首行宽 1920px）。conformance 测试绿
  不代表真实页面已折行。
- `nexty-css` 没有 `white-space` / `overflow-wrap` / `word-break` 属性，`<pre>` 的
  空白与换行仍被折叠，`white-space: nowrap` 无效果。
- `<br>` 仍被整个丢弃（layout 源码与 UA 表均无处理）。
- `inline.rs` 已涨到 1749 行；先把 conformance 代码拆成三分离模块，再接入
  `build_lines`，避免把单文件继续做大。

预期收益：真实页面正文（尤其中文）按容器宽正确折行；`<pre>`、`<br>`、长 URL 的
`overflow-wrap: break-word` 恢复；断点禁则与浏览器观感一致。改动集中在
nexty-css / nexty-layout 两层，几乎全部可无头单测。

### 为什么不是文本选择/复制或 justify

- 文本选择/复制依赖稳定的行几何与「折叠后文本 ↔ 原文」映射；先让真实断行正确，
  再在其上做选择不会返工。
- `text-align: justify` 是视觉细化，必须在核心接入稳定后再做；本轮明确不做。

## 任务清单与退出条件

### T1 nexty-css：`white-space` 解析、级联与 UA
- 新增 `PropertyId::WhiteSpace` 与 `WhiteSpaceValue`：`normal | pre | nowrap |
  pre-wrap | pre-line | break-spaces`（CSS Text §3.1 单属性取值；Level 4 的
  `white-space-collapse` + `text-wrap-mode` 拆分记为偏差，不实现）。
  属性**继承**，initial `normal`。
- UA 样式表按 HTML Rendering §15 补：`pre, textarea, listing, xmp { white-space: pre }`
  （`textarea`/`listing`/`xmp` 的 display 偏差不动）。
- 退出条件：六个取值解析成功；非法值（`pre nowrap`、`pre-wrap-x`、空值）整条丢弃；
  继承生效（父 `nowrap` 子继承）；UA 在 `<pre>` 上给出 `pre` 且可被作者声明覆盖。

### T2 nexty-css：`overflow-wrap` 与 `word-break`
- 新增 `PropertyId::OverflowWrap`（`normal | break-word | anywhere`）与
  `PropertyId::WordBreak`（`normal | break-all | keep-all`），均**继承**，initial
  `normal`（CSS Text §5.4 / §5.1）。
- 退出条件：各取值解析成功；`break-word` 与 `break-all` 不互相误判；继承生效；
  非法值丢弃。

### T3 nexty-layout：拆分 `inline.rs` 的 conformance 代码
- 当前 1749 行，先把上一轮的 `mod conformance` 按既有三分离约束拆开：
  - `inline/cluster_map.rs`：`Span` 与 `cluster_map`，**≤200 行**；
  - `inline/break_ops.rs`：`BreakOptions` / `WordBreak` / `OverflowWrap`、
    `break_opportunities` 与 CSS 选项增删，**≤200 行**；
  - `inline/span_split.rs`：`split_spans` 与 `min_content_width`，**≤200 行**；
  - `inline/uax14/`：Line_Break 类、UCD 数据解析、LB1–LB31 规则；按
    `classes` / `data` / `rules` 拆子模块。规则引擎**不套 200 行上限**，
    但单文件不得再回到千行级。
- 公开 API 路径与签名不变；UCD 数据文件仍在 `src/data/`，运行时解析方式不变。
- 退出条件：行为不变——`tests/inline_conformance.rs` 8/8、现有 45 条 layout
  测试全绿；模块文档写清三分离边界与 200 行例外理由。

### T4 nexty-layout：留白处理与 `<br>`
- `Collector` 按 `white-space` 处理文本项：
  - `normal` / `nowrap`：空格、制表、换行折叠为单个空格；行首/行尾可折叠空白丢弃；
  - `pre`：空格/制表/`\n` 逐个保留，且 `\n` 为硬断行；
  - `pre-wrap`：保留空格/制表/`\n`，允许软换行；
  - `pre-line`：折叠空格/制表，保留 `\n` 为硬断行；
  - `break-spaces`：保留空格且允许在空格处换行，行尾保留空格参与宽度。
  - `tab-size` 本轮不实现：制表按一个空格的宽度/保留语义处理，写入偏差清单。
- `<br>` 在收集阶段登记为**硬断行项**；不受 `nowrap` 影响（`a<br>b` 仍两行）。
- 退出条件：每个 `white-space` 取值一个无头单测；`<pre>` 行数 = `\n` 数 + 1；
  `nowrap` 单行；`pre-line` 折空格保留换行；`pre-wrap` 保留且可折；
  `<br>` 第二行 x 从 0 起。

### T5 nexty-layout：把 UAX#14 API 接入 `build_lines`
- 文本项仍**只整形一次**（沿用 `cache`），然后：
  1. `cluster_map` 把整形结果切成不可拆 cluster；
  2. `break_opportunities` 用 computed style 映射出的 `BreakOptions` 取断点；
  3. `span_split` 在 cluster 边界上切段，贪心放置。
- 用「断点段 + 空白段」模型替换现有 `pending_space` / `space_after_previous`
  的 ASCII 词模型；原子盒/图片仍是独立行内项，其前后断点按 UAX#14 边界处理。
- `white-space: nowrap` 只忽略软断点；`<br>` 与 `\n`（pre/pre-line）仍强断。
- 保持 `text-align` center/right 的现有行平移路径、基线计算与 taffy 的
  measure/intrinsic 路径不变；只替换断点来源。
- 退出条件（全部走 `layout_document` 真实管线，而非只调 API）：
  - 200px 视口、120 字中文 → **≥3 行且每行自然宽 ≤ 200 + 0.5**；
  - 行首不出现 `，。、）！？：；》】` 类收尾标点；
  - 现有 Latin 测试（`long_text_wraps_at_viewport_width`、
    `single_unbreakable_word_overflows`、`multiline_baselines_advance_by_line_height`
    等）保持绿；
  - `break-all` 在无空白处可断；`keep-all` 中文不折行（单行溢出）；
    `anywhere` / `break-word` 长 URL 的差异按 conformance API 语义；
  - 强制断点不切开 cluster（断言断点落在 `cluster_map` 边界上）。

### T6 门禁、文档与实机验收（不会包含 justify）
- `cargo check --workspace` 零 warning；`cargo test --workspace` 全绿；
  `cargo clippy --workspace --all-targets -- -D warnings`；
  `cargo fmt --all -- --check`；`cargo deny check` 零新增失败；
  feature 组合（`nexty-layout --no-default-features --features inline`、
  `--features taffy-map`、`nexty-css --no-default-features --features values` 等）
  零 warning。
- AGENTS.md 同步：`nexty-css` 属性清单加 `white-space` / `overflow-wrap` /
  `word-break`；`nexty-layout` 改为「`build_lines` 已接入 UAX#14」，
  偏差清单保留 float / table / 多行 baseline / `vertical-align` 非 baseline，
  `<br>`/white-space 从缺口清单移除。
- 更新 `docs/decisions/2026-10-17-line-breaking.md` 的边界段（从「未接入
  build_lines」改为已接入），记录 tab-size 与 `break-spaces` 的实现取值。
- 版本号随逻辑提交 +1：`nexty-css` 0.1.10 → 0.1.11、`nexty-layout`
  0.1.15 → 0.1.16；`nexty-text` 0.1.7 与 `nexty-chrome` 0.1.17 若本轮无代码改动则不动。
- 实机验收（无头不可测项，按项目记忆的 Computer Use 流程：
  `cargo run -p nexty-chrome --bin nexty` + Ctrl+L 输入本地 http 地址 +
  `.workbuddy/t3-test` 资产，新页面用 node 服务器）：中文正文多行不横向溢出、
  `<pre>` 保留空白与换行、`<br>` 生效、`break-word` 长 URL 不溢出；截图目测
  断行观感与浏览器一致。
- 分任务 commit（`[css]` / `[layout]` / `[docs]`），退出条件全部满足后 push。

## 非目标（本轮不做）

- **`text-align: justify` / `text-align-last` / `text-justify`**：本轮明确不做，
  留到核心接入稳定后的单独一轮。
- 页面文本选择与复制（以及「折叠后文本 ↔ DOM 原文偏移」映射）。
- `hyphens` 自动断字、`line-break: strict|normal|loose`、字典式 SA/CJK 词边界。
- `tab-size`（制表按一个空格处理）、`letter-spacing` / `word-spacing` /
  `text-indent` / `text-transform` / `text-decoration`。
- bidi / RTL；`vertical-align` 非 baseline；float / table；多行 leaf 的 baseline 组。
- paint 圆角/渐变/阴影/根背景传播、vello_hybrid GPU、`position: fixed`、表单与 JS。

## 方向评估（为什么是这轮）

| 候选 | 价值 | 成本 | 结论 |
| --- | --- | --- | --- |
| **接入 UAX#14 + white-space/`<br>`（本轮）** | conformance API 已就绪但真实页面仍不折行；这是渲染正确性缺口 | 中：CSS 两属性 + 行内收集/断行重构，几乎全可无头单测 | ✅ 本轮 |
| `text-align: justify` | 视觉细化 | 小–中：需在 build_lines 内分配间隙 | 下一轮（本轮明确不做） |
| 页面文本选择与复制 | 关键交互里程碑 | 中–大：跨三层 + 剪贴板；依赖稳定行几何 | 断行接入稳定后 |
| text-decoration / 链接下划线 | 可辨识度 | 小–中：paint 新指令 | 暂缓 |
| float / table | 老页面与邮件类 | 大：taffy 不覆盖 | 暂缓 |

## 验证流（AGENTS.md Verification Flow）

每个子任务先写 failing test（真实 `layout_document` 路径），看它红，再实现到绿；
每层：`cargo check` 零 warning → `cargo test` 全绿 → `fmt` / `clippy` → commit。
conformance 8/8 是回归门禁：接入 `build_lines` 不得改变 `break_opportunities`
的 UAX#14 结果。无头不可测项只有 T6 的实机目测。退出条件全部满足后 push。
