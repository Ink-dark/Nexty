# goal.md — 本轮任务：CSS Text 排版细化 + 行内布局正确性（justify / text-indent / 字距词距 / tab-size / pre-wrap 悬挂 / M8 选区遮蔽 / M9 块嵌行内丢内容）

日期：2026-10-24。前置：输入边界加固轮已完成（dom 0.1.3 / html 0.1.5 /
css 0.1.12 / paint 0.1.5 / layout 0.1.18 / chrome 0.1.20），H1–H3 三项进程级
DoS 已闭合并有全链回归钉住，旧 goal 归档于
[docs/plans/2026-10-24-round-input-hardening.md](docs/plans/2026-10-24-round-input-hardening.md)。
justify + CSS Text 细化在 roadmap 里已连续顺延三轮（先让位断行、再让位选区、
再让位加固），本轮兑现；同时并入审查报告
（[2026-10-24 性能与安全审查](docs/2026-10-24-perf-security-review.md)）§8.6
点名的 M8 / M9 两个行内正确性问题——它们与本轮改的是同两个文件
（`inline/mod.rs`、`selection.rs`），分开做要重复读同一片上下文。
当前版本：css 0.1.12 / text 0.1.7 / paint 0.1.5 / layout 0.1.18 /
chrome 0.1.20 / html 0.1.5 / dom 0.1.3。

## 本轮为什么做这些

- **justify 是排版保真的显性缺口**：`text-align: justify` 早已能解析
  （`TextAlignValue::Justify`），但 [block.rs:287](crates/nexty-layout/src/block.rs)
  的 `alignment_shift` 把 `justify` / `justify-all` / `match-parent` 一律降级为
  左对齐（模块文档明记为偏差）。中文正文与两栏式版面在当前引擎里全部左 ragged。
- **字距词距与首行缩进整族缺失**：`letter-spacing` / `word-spacing` /
  `text-indent` / `tab-size` 在 css 层完全没有（无 PropertyId、无解析、无
  computed 字段）；layout 侧 `inline/mod.rs:19,38` 明记「制表按一个空格」
  「pre-wrap 行尾空格未做悬挂」。这些是正牌 CSS Text 3 属性，现代页面
  （尤其中文排版）使用率高。
- **M9 内容静默消失（正确性 > 美观）**：块级子元素嵌在行内子树里被直接丢弃
  （[inline/mod.rs:483](crates/nexty-layout/src/inline/mod.rs) 的 `_ => {}` 分支）。
  `<div>a <span><div>CONTENT</div></span> b</div>` 中 CONTENT 整棵子树不产生
  任何盒。CSS 2.2 §9.2.1.1 的规范行为是拆分行内流生成匿名块；审查判定
  「内容丢失比几何偏差严重」，且该项不在 AGENTS.md 已知未实现清单内（是
  计划外缺陷，不是已登记偏差）。
- **M8 选区对原子盒内容系统性失效**：`TextMap::hit`
  （[selection.rs:146](crates/nexty-layout/src/selection.rs)）取**第一个** y 命中
  的行，而 `collect`（[selection.rs:533](crates/nexty-layout/src/selection.rs)）
  先 push 父行、后递归原子盒——父行的 y 区间完全覆盖其内部 inline-block 的行，
  点击 inline-block 内文本永远先命中外层行。上一轮修的是高亮坐标（0b4b55a），
  命中遮蔽仍在。

### 为什么不是性能轮（H4）或 float / table

H4（整形缓存）与 M5/M6/M10 是**结构性优化**，不改变可观察行为，适合在
没有行为轮排队时做，且审查建议先以大页面基准确认热点归属；float / table
是各自独立的大项（taffy 不覆盖，需整轮预算）。本轮全部任务都改
`inline` / `selection` 同一片代码，合并成一轮上下文复用最高；性能轮排下一轮。

## 任务清单与退出条件

规范依据：CSS Text 3（各属性锚点
[text-align](https://drafts.csswg.org/css-text-3/#text-align-property) /
[justification](https://drafts.csswg.org/css-text-3/#justification) /
[word-spacing](https://drafts.csswg.org/css-text-3/#word-spacing-property) /
[letter-spacing](https://drafts.csswg.org/css-text-3/#letter-spacing-property) /
[text-indent](https://drafts.csswg.org/css-text-3/#text-indent-property) /
[tab-size](https://drafts.csswg.org/css-text-3/#tab-size-property)）；
M9 依据 CSS 2.2 §9.2.1.1；M8 无规范条款（是我们自己的命中顺序缺陷）。

### T1 nexty-css：新增五个文本属性（0.1.13）
- `PropertyId` + 解析 + computed 字段 + 继承（四个全部是 inherited）：
  - `letter-spacing`：`normal` | `<length>`（负值合法）；`normal` 计算 0；
  - `word-spacing`：`normal` | `<length>`；`normal` 的行为留给 layout
    （computed 存「normal 或定值」语义）；
  - `text-indent`：`<length-percentage>`；percentage 留给 layout 按包含块
    解析，computed 存原值；
  - `tab-size`：`<integer ≥ 0>` | `<length>`（CSS Text 3 两形态都收）。
- `text-align-last` 本轮**不做**（见非目标），justify 的最后一行走 initial
  （start）行为即可。
- 数值卫生沿用现有口径：`MAX_CSS_LENGTH` clamp、非有限值整条声明无效。
- 退出条件：四属性各有 parse/compute/inherit 单测（含非法值拒绝、负
  letter-spacing 接受、percentage 保留原值、`em` 折算与现有长度属性一致）；
  css 既有测试不改仍绿。

### T2 nexty-layout：letter-spacing 与 word-spacing（0.1.19）
- 在 `build_lines` 的放置阶段按字形 cluster 施加（整形只做一次，间距是
  放置期修正，不改 `ShapedText`）：
  - letter-spacing：每个 grapheme cluster 边界处增加定值；**行末** cluster
    的字距不计入行宽（对齐与断行判定用净宽）；
  - word-spacing：word separator（U+0020 / U+00A0 / U+3000 等 CSS Text 3
    word-spacing 词条列举集）的 cluster advance 增加定值；计入 min-content
    （一个分隔符 = 自身净宽 + spacing，spacing 为负时收缩不小于 0）；
  - 间距参与断行机会判定（word-spacing 后的分隔符仍是断点；letter-spacing
    不产生也不消灭断点）。
- 退出条件：单测钉住——`letter-spacing: 4px` 使等长两段文本行宽差 =
  (字符数−1)×4px；右对齐下行末字距不参与（净宽对齐）；`word-spacing: 8px`
  的 min-content 相差 8px×分隔符数；负 letter-spacing 收窄且不 panic；
  选区命中/矩形在加间距文本上仍逐字形正确（cluster→源偏移映射不受影响）。

### T3 nexty-layout：`text-align: justify`（0.1.19）
- 机会集（`auto` 方法的可执行近似，偏差记账）：行内全部 justify 机会 =
  软折行**消费掉的可折叠空格**处 + CJK 断点（UAX#14 break opportunity，
  排除行首）。断行结果不变（先按现有断行成行，再在行内分配自由空间）；
  均匀分配到每个机会点（不做 inter-word 空格收缩，不做 `text-justify`
  分档——均为偏差，记入模块文档）。
- 施加位置：非末行且不以硬断行（`<br>` / 保留换行）结尾的行；最后一行
  与硬断行前的行按 start 对齐（即现有左对齐路径）。`justify` 从
  `alignment_shift` 的降级名单移除。
- 内在尺寸不受影响：measure 路径（min-content / max-content）不分配间距，
  与 justify 无关——单测钉住「justify 与 left 的 min/max-content 相等」。
- 选区兼容：分配发生在 `build_lines` 放置期，glyph 坐标即最终坐标；
  TextMap 命中与逐行矩形在 justified 行上照常工作——必须有跨 justify 行
  的拖拽选区单测。
- 退出条件：三行文本 justify 后前两行右缘对齐、末行左 ragged；无机会行
  （单 CJK 长词行）退化为左对齐不崩溃；`white-space: pre` 下 justify 不生效
  （无折叠空格无软折行）；自由空间为 0 时与 left 逐字节同构。

### T4 nexty-layout：`text-indent`（0.1.19）
- 只作用于块容器的**首个格式化行**（每块一次，`<br>` 后不缩进——
  `each-line` / `hanging` 关键字不做）；percentage 在布局期按包含块宽度
  折算；缩进从首行可用宽中扣减（影响首行断行），min-content /
  max-content 测量不含缩进（css-sizing 语义，与 Chromium 一致）。
- 退出条件：`text-indent: 40px` 首行右移且第二行不动；`text-indent: 10%`
  按包含块宽折算；与 justify / center 组合各一条单测（缩进后再对齐 /
  分配）；内在尺寸测试钉住「含缩进与不含缩进的 min-content 相等」。

### T5 nexty-layout：`tab-size` 与 pre-wrap 行尾悬挂（0.1.19）
- `tab-size`（initial 8）：只作用于**保留的**制表符（`pre` / `pre-wrap` /
  `break-spaces` 下）；advance = 推进到 tab-size × U+0020 advance 的下一
  整数倍；normal 流里制表仍按现行折叠语义。现状是
  [inline/mod.rs:299](crates/nexty-layout/src/inline/mod.rs) 的「按一个空格
  保留」——改掉后模块文档 19/38 行的两条偏差注销。
- pre-wrap 悬挂：软折行处行尾的保留空格**悬挂**在行尾（不触发折行、不占
  对齐宽度、可溢出边沿），`break-spaces` 不悬挂（照常占宽）。复制文本与
  选区映射仍含被悬挂的空格（源映射不丢字符）。
- 退出条件：`<pre>` 内 `a\tb`（tab-size 4）advance 到第 4 列而非 1 空格；
  `tab-size: 0` 制表零宽；pre-wrap 行尾空格不换行且右对齐时行净宽不含它；
  选区跨悬挂空格复制文本完整。

### T6 nexty-layout：M9 — 行内子树中的块级子元素不再丢弃（0.1.19）
- 规范路径（CSS 2.2 §9.2.1.1）：`Collector` 在行内收集途中遇到块级子元素时
  **终止当前 run**、上抛块级标记，`tree_build` 的块级分组把前半 run 与后半
  run 各自包装成匿名块、块级子元素按普通块盒布局——即「行内盒被拆分到
  匿名块两侧」的既有语义扩展到嵌套层。偏差记账：拆分处行内盒的
  border / background 只画在各半自身范围（不跨块连续）。
- 退出条件（全部真实 `layout_document` 路径、先写 failing 测试）：
  `<div>a <span><div>X</div></span> b</div>` 中 X 可见、a 与 b 分居 X 上下、
  X 的文本可被 TextMap 命中并复制；嵌套两层 inline 包一层 block 的组合
  同样成立；`display: contents` / `display: none` 混合原语义不回归；
  WPT 基线 1854/1959 不动。

### T7 nexty-layout：M8 — TextMap 命中不再被父行遮蔽（0.1.19）
- 修 `collect` 顺序：原子盒内部行**先于**父行入列（先深后浅），或 `hit`
  收集全部 y 命中行取最深处——取一种实现，落选方案与理由写进模块文档。
  `nearest` 的吸附逻辑同步检查（点在原子盒行间空隙时不再跳到外层）。
- 退出条件：先写 failing 测试——点击 inline-block 内文本命中其内部文本
  而非外层行/相邻行；inline-block 内双击选词、跨内外拖拽选区、rects 的
  y/height 与内部行盒一致；纯块级页面的既有选区测试全部不改仍绿。

### T8 门禁、文档与版本
- `cargo check --workspace` 零 warning；`cargo test --workspace` 全绿；
  `cargo clippy --workspace --all-targets -- -D warnings`；
  `cargo fmt --all -- --check`；`cargo deny check` 通过。
- feature 组合零 warning：重点复验 `nexty-layout --no-default-features
  --features inline` 与 `--features selection`、`nexty-chrome
  --no-default-features --features selection`（本轮不动 feature 面，
  不得回归）。
- AGENTS.md 同步：layout 行补 justify / text-indent / letter-spacing /
  word-spacing / tab-size / pre-wrap 悬挂 / 块嵌行内拆分；从「未实现」
  清单划掉对应项；`block.rs` 与 `inline/mod.rs` 的偏差清单同步（新增
  justify 的 `auto` 近似偏差）；Feature 映射表无新增行；roadmap 改为
  性能轮（H4 + M5/M6/M10）优先、paint 扩展（text-decoration / 圆角 /
  渐变 + 背景传播）其后、float / table 再后。
- 版本号随逻辑提交 +1：nexty-css 0.1.12 → 0.1.13、nexty-layout
  0.1.18 → 0.1.19；text / paint / chrome 本轮无代码改动则不动。
- 分任务 commit（`[css]` / `[layout]` / `[docs]`），退出条件全部满足后
  push。

### T9 实机验收（无头不可测项）
- 按项目记忆的 Computer Use 流程：`.workbuddy/t3-test/serve.mjs` + 新页面
  资产（长中文段落 justify、缩进段落、`letter-spacing` 大字标题、
  `pre` 含制表、`pre-wrap` 行尾空格、inline-block 内文本选择、
  `<span><div>` 块嵌行内页面）；`cargo run -p nexty-chrome --bin nexty`。
- 验收动作：目测 justify 双端对齐与 Edge/Chrome 同页对比；缩进/字距/制表
  列对齐目测；inline-block 内拖拽选择、双击选词、复制核对；块嵌行内页面
  内容完整可见。截图留档。

## 非目标（本轮不做）

- `text-align-last` / `text-justify` / `text-indent` 的 `each-line` /
  `hanging` 关键字；`hanging-punctuation`。
- `text-decoration`（下划线/删除线）与链接下划线——归「paint 指令集扩展」
  轮（与圆角 / 渐变、`body` 背景传播同轮，都是 paint 新指令 + chrome 消费）。
- H4 整形缓存与 M5 / M6 / M10 性能项（下一轮性能轮）；M11 / M12 / 低项
  随对应模块顺带。
- float、table、`position: fixed` 包含块、`vertical-align` 非 baseline、
  bidi / RTL（含 justify 的 RTL 行——本轮 justify 只定义 LTR 行为）。
- `::first-line` 伪元素。

## 方向评估（为什么是这轮）

| 候选 | 价值 | 成本 | 结论 |
| --- | --- | --- | --- |
| **CSS Text 细化 + M8 / M9（本轮）** | 排版保真（justify / 缩进 / 字距）+ 两个正确性缺陷闭口；同文件高复用；roadmap 欠账兑现 | 中：css 解析一任务 + layout 五任务 | ✅ 本轮 |
| 性能轮（H4 + M5 / M6 / M10） | 全量重排开销降档 | 中–大：结构改造，建议先基准 | 下一轮 |
| paint 扩展（text-decoration / 圆角 / 渐变）+ 背景传播 | 视觉保真 | 小–中：paint 新指令 + chrome 消费 | 后续 |
| float / table | 老页面 / 邮件类兼容 | 大：taffy 不覆盖，各需整轮 | 后续 |
| vello_hybrid GPU 后端 | 性能 | 中：surface 接入与适配层 | 后续 |

## 验证流（AGENTS.md Verification Flow）

每个子任务先写 failing 测试（真实 `layout_document` 路径）看它红，再实现到
绿；T2–T7 的预期几何都要给出可断言的数值（行宽差 / 列位置 / 矩形 y/height），
不写「大致对齐」类模糊断言。M9（T6）/ M8（T7）按审查建议先以 failing 测试
钉住当前错误行为再修。每层：`cargo check` 零 warning → `cargo test` 全绿 →
`fmt` / `clippy` → commit；WPT 基线 1854/1959 与既有测试（含 UAX#14
conformance 全量）不改仍绿是硬前提。退出条件全部满足后 push。

---

## 完成记录（2026-10-24）

T1–T8 全部退出条件满足：workspace 419 测试全绿、clippy/fmt/deny 通过、
feature 组合（layout `inline` / `selection`、chrome `selection`）零 warning、
WPT 基线 1854/1959 无回归、UAX#14 conformance 全量绿。版本落地：
css 0.1.13 / layout 0.1.24（每逻辑提交 +1）。

与计划的三处偏差（均为实证修正）：

1. **layout 版本号按提交逐次 +1**（0.1.19 → 0.1.24，T2–T7 各一次），高于
   goal 预估的 0.1.19——AGENTS.md 的 Versioning Discipline（每次改逻辑
   随手 +1）优先于 goal 的单轮目标版本。
2. **顺带修正加固轮引入的 max-content 退化**（T3）：29dcdc0 的
   「非有限 max_width 按 0 降级」把 MaxContent 测量的合法 +inf 也降级了，
   shrink-to-fit 被压到最宽 span。justify 需要区分「合法的无穷」与污染，
   改为仅 NaN 按 0 降级、+inf 保留（游标累加的都是有限宽度，坐标不扩散
   非有限值）。
3. **text-indent 百分比按 run 叶可用宽（元素自身内容盒宽）折算**，非规范
   要求的「元素包含块宽」——后者需要 measure 期拿不到的父级定宽，auto 宽
   元素（正文段落的常见情形）二者相同，显式宽度元素的差异记为偏差
   （`inline/mod.rs` 模块文档 + 测试钉住现状）。

M9 修法说明：`Collector` 把行内流产出为 `InlineSegment` 流（Inline 段 +
块级子元素标记），`flush_run` 把 Inline 段各自包装成匿名 run 叶、Block 段
走 `build_box`——「行内盒拆分到匿名块两侧」的既有语义自然扩展到嵌套层。
M8 修法说明：`Line` 携带嵌套深度，`hit`/`nearest` 按「y 距离 → x 距离 →
深度」选行并向嵌套原子盒内部行下钻（含行间空隙 / padding 区的吸附）；
落选方案（先深后浅重排行入列）与理由见 `selection.rs` 模块文档——会破坏
「绘制顺序 = 复制顺序」的既有语义。

实机验收（T9）：见 `.workbuddy` 冒烟记录与截图。
