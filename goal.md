# goal.md — 本轮任务：文本断行与留白（UAX#14 断行 / white-space / overflow-wrap·word-break / justify / `<br>`）

日期：2026-10-16。前置：行内呈现与文本能力补齐轮完成（
[docs/plans/2026-10-11-round-inline-text-fidelity.md](docs/plans/2026-10-11-round-inline-text-fidelity.md)，
归档于本次规划提交），`nexty-css` 0.1.10 / `nexty-text` 0.1.6 / `nexty-layout` 0.1.14 /
`nexty-chrome` 0.1.17；盒级几何由 taffy 接管（block/flex/grid/absolute），
**行内断行永久自研**（`inline.rs`）。

## 本轮为什么做这些

上一轮补齐了行内呈现的对齐（`text-align` left/center/right）、`font` 简写、视口单位、
CJK 字形归属与 flex baseline 之后，**行内文本的断行与留白**成为唯一「让真实页面不可读」
级别的缺口。本轮规划前实测（探针，2026-10-16，确认后已删除探针文件）：

- **中文段落完全不折行**：`<p>` 内 120 字中文（`"中文段落测试"` × 20）在 **200px 与
  400px** 视口下均渲染为 **1 行**，首行宽 **1920px**——溢出视口 4.8–9.6 倍。同长度拉丁
  文本对照在 200px 下正常折成 2 行。根因：`inline.rs::Collector::push_text` 只用
  `char::is_ascii_whitespace` 切词，UAX#14 断点（汉字 ID 类之间、禁则标点）从未被消费
  （`inline.rs` 模块文档偏差清单第 1 条）。**当前任何中文页面正文都不可读。**
- **`white-space` 属性完全未实现**（`value.rs` 无该 `PropertyId`）：`white-space: nowrap`
  无效果（探针：nowrap 文本仍折 3 行），`<pre>` 的空白与换行被折叠。
- **`<br>` 全无处理**（layout 源码与 UA 样式表 grep 均无命中）：HTML 里最常见的硬换行
  被整个丢弃。
- `text-align: justify` 上轮按偏差清单降级为 left，未消费。

预期收益：真实页面正文（尤其中文）从「溢出不可读」变为按容器宽正确折行；`<pre>` 代码块、
`<br>` 排版恢复；断点禁则（行首不出现 `，。、）！？` 类收尾标点）与浏览器观感一致。
改动集中在 nexty-css / nexty-text / nexty-layout 三层，**几乎全部可无头单测**。

### 为什么是断行，而不是上一轮记的「下轮首选：页面文本选择与复制」

上一轮 goal 与项目记忆都把「页面文本选择与复制」记为下轮首选。本轮规划**重新排序**，
理由是被实测证据推翻的优先级假设：

1. 断行是**渲染正确性**缺口，影响面是「所有中文页面正文全部溢出不可读」；选择/复制是
   交互增量。文本溢出行时选择体验同样畸形（拖选范围与实际视觉行不对应），
   先修断行再上选择不会返工，反之则要在畸形行几何上做选择。
2. 断行轮必须建立「**字形 → 源文本字节偏移**」的映射（见 T3：按 cluster 偏移把一次整形
   结果切到各断点段上）。这正是选择轮做「点击点 → 字符位置」与「字符区间 → x 区间」
   映射所必需的基础设施；先在断行轮落地，选择轮就不必再动 text 层的整形输出。
3. 成本不对称：断行轮依赖一个零依赖的成熟 crate（`unicode-linebreak`，Apache-2.0）+
   自研断行内核重构，全部可无头单测；选择轮跨三层并需要剪贴板依赖与平台接线，
   放在断行之后更小。

「页面文本选择与复制」因此**顺延为本轮之后的下一轮首选**，列入本轮非目标。

## 任务清单与退出条件

### T1 nexty-css：`white-space` 解析、级联与 UA
- 新增 `PropertyId::WhiteSpace` 与 `WhiteSpaceValue`：`normal | pre | nowrap | pre-wrap |
  pre-line | break-spaces`（CSS Text §3.1 单属性取值；Level 4 把它拆成
  `white-space-collapse` + `text-wrap-mode` 两个属性，按偏差清单记为简化，不实现拆分）。
  该属性**继承**（inherited），initial 为 `normal`。
- UA 样式表按 HTML Rendering §15 补规则：`pre, textarea, listing, xmp { white-space: pre }`
  （`textarea`/`listing`/`xmp` 当前连 `display` 都未声明，本轮只加 white-space，
  display 偏差不动）。
- 退出条件：单测——六个取值均解析成功；非法值（`pre nowrap`、`pre-wrap-x`、空值）整条
  声明丢弃且不回退 initial 之外的怪值；`white-space` 继承生效（父 `nowrap` 子继承）；
  UA 表在 `<pre>` 上给出 `pre` 且可被作者声明覆盖。

### T2 nexty-css：`overflow-wrap` 与 `word-break`
- 新增 `PropertyId::OverflowWrap`（`normal | break-word | anywhere`）与
  `PropertyId::WordBreak`（`normal | break-all | keep-all`），二者均**继承**，initial
  `normal`（CSS Text §5.5 / §5.6）。
- 退出条件：单测——各取值解析成功；`break-word` 与 `break-all` 不互相误判；继承生效；
  非法值丢弃。

### T3 nexty-text：`Glyph.cluster`（源文本字节偏移）
- `ShapedText.glyphs` 的每个字形增 `cluster: u32`——该字形所属 cluster 在**源文本中的
  起始字节偏移**。取法：遍历 `Run::clusters()`（parley 0.11 `Run::clusters()`），
  用 `Cluster::text_range().start` 标注该 cluster 内的所有字形；字形 x 由
  `Cluster::visual_offset()` 与该 cluster 内字形自身的偏移合成（保持现有「x 相对本段文本
  起点」语义不变）。
- **探针已核实**（2026-10-16）：`"ab中c def"` 的 cluster 依次为字节 `0..1 / 1..2 / 2..5 /
  5..6 / …`，`visual_offset()` 即该 cluster 的绝对 x（`0 / 8.9 / 17.8 / 33.8 / …`，
  与 `positioned_glyphs()` 的 x 一致），回退字体使一条文本被拆成多个 run（现有
  `ShapedText.fonts` / `Glyph.font` 归属不变）。
- 偏差：cluster 偏移相对**进入本层的折叠后文本**，不承诺 DOM 原文偏移（选择轮的
  「折叠后 ↔ 原文」映射另立项，见非目标）。
- 退出条件：单测——`"ab中c"` 的字形 cluster 为 `[0, 1, 2, 4]`（UTF-8 字节）；连字 /
  多字形 cluster 内字形共享同一偏移；现有整形测试全绿（字形数、字体归属、x 单调性不变）。

### T4 nexty-layout：UAX#14 断点驱动的行内断行（新依赖 `unicode-linebreak`）
- 引入 `unicode-linebreak`（0.1.5，Apache-2.0，零依赖，MSRV 1.56 < 本仓）到
  **nexty-layout**（断行是布局层职责，text 层保持纯整形）；`cargo deny check` 零新增
  失败，`cargo about` 重生成 `docs/dependencies.html`，ADR 见 T7。
- **算法**：文本项的整形仍**一次完成**（沿用现有 `cache: HashMap<(NodeId, String), ShapedText>`），
  再用 UAX#14 断点把该结果按 `Glyph.cluster` 切分到各「段」上做贪心放置——不按段重复
  整形（按段整形会让中文每字一次 parley 布局构建，量级不可接受；此取舍写入模块文档）。
- **空白按 `white-space` 折叠/保留**：可折叠空白（空格 / 制表 / 换行）在 `normal` /
  `nowrap` / `pre-line` 下折叠为单个空格；`pre` / `pre-wrap` / `break-spaces` 下逐个保留。
  行尾可折叠空白**不计入行宽**（normal/nowrap/pre-line），`pre-wrap`/`break-spaces` 保留。
  现有 `pending_space` + `space_after_previous` 机制被「空白即段」的模型取代
  （空格成为可断段，行首空白在换行后丢弃）。
- **硬断行**：`pre` / `pre-line` 下的 `\n` 与 **`<br>` 元素**产生强制断行（不可被
  填满与否取消）；`<br>` 在收集阶段登记为硬断行项（UA 样式表不动）。
- 退出条件（全部无头单测）：
  - 中文折行：200px 视口、120 字中文 → **≥3 行且每行自然宽 ≤ 200 + 0.5**（复现探针场景
    的 failing test 先红后绿）；
  - 禁则：行首不出现 `，。、）！？：；》】` 类收尾标点，行尾不出现 `（《【` 类起始标点
    （UAX#14 LB13/LB14 生效的证据）；
  - 拉丁不退化：现有 `long_text_wraps_at_viewport_width`、`single_unbreakable_word_overflows`、
    `multiline_baselines_advance_by_line_height` 等全部保持绿；
  - `white-space: nowrap` → 单行；`pre` → 保留空格且换行处断行（行数 = `\n` 数 + 1）；
    `pre-line` → 折叠空格但保留换行；`pre-wrap` → 保留且可折；
  - `<br>` 产生断行（`a<br>b` → 2 行，且第二行 x 从 0 起）。
- 若 `unicode-linebreak` 在某类断点上的行为与预期不符（如 LB21a/LB30 组合），
  按「先写探针定位、再动手」的既有教训记录实测结论后停下报告，不得绕过。

### T5 nexty-layout：`overflow-wrap` / `word-break` 强制断点
- 段本身超过可用宽且无更早断点时：`overflow-wrap: break-word | anywhere` 按 cluster
  紧急断行；`word-break: break-all` 允许在任意 cluster 之间断；`word-break: keep-all`
  禁止 CJK 字符间断行（`normal` 保持 UAX#14 默认）。
- `overflow-wrap: anywhere` 与 `break-word` 的差别（最小内容宽是否受影响）按偏差清单
  记录本轮实现取值——**不得**为了「都一样」而悄悄收敛语义。
- 退出条件：单测——长 URL 在 `break-word` 下折行且每行 ≤ 容器宽；`normal` 下仍溢出
  （现有 `single_unbreakable_word_overflows` 保持绿）；`break-all` 在无空白处也断；
  `keep-all` 下中文不折行（单行溢出）；强制断点**不得切碎 cluster**（`中` 不产生半个字形，
  断言断点落在 cluster 边界上）。

### T6 nexty-layout：`text-align: justify` 消费（撤销上轮降级）
- 多行块的**非末行**在断点间隙上均摊余量（拉丁按词间断隙、CJK 按字符间断隙，统一为
  「按可伸缩间隙分配」）；末行、单行块、以硬断行结束的行**不伸缩**。
- 施加位置：`build_lines` 内（只有那里同时拥有 `max_width`、间隙列表与「是否末行」信息）；
  center / right 的整行平移保持在 `block.rs` 现路径不变（避免两套平移逻辑）。
- 偏差记录：不实现 `text-justify` 取值细分、不实现 `text-align-last`。
- 退出条件：单测——`text-align: justify` 下非末行右缘 == 内容宽（±0.5）、末行与单行块
  保持自然宽；`justify` 继承生效；center / right / left 现有测试全绿（含溢出防负偏移）。

### T7 门禁、依赖、文档与实机验收
- 依赖门禁：`cargo deny check` 零新增失败；`cargo about generate about.hbs -o
  docs/dependencies.html` 重生成；新依赖写 ADR
  `docs/decisions/2026-10-16-line-breaking.md`，内容至少包含：
  - 为什么是 `unicode-linebreak`：UAX#14 纯规则、零依赖、与 parley 版本解耦、可无头单测；
  - 为什么**不是** parley 自带断行 / `icu_segmenter` 直连：parley 的 ICU4X 分段要
    `complex-scripts` 词典数据（当前未开，运行期已有
    `No segmentation model for complex script` 警告），且会按 parley 的 icu 版本耦合；
    直接依赖 `icu_segmenter` 同样受 CJK 数据与版本耦合所限；
  - 为什么**不是**自研 UAX#14 表（造轮子，违方针）；
  - 为什么**不是**把行内布局整体交给 parley inline box（inline 布局是本仓 ADR 明确保留
    的自研层，且会交出原子盒对齐 / 空白语义 / justify 的所有权）；
  - 边界：不实现 `hyphens` 自动断字、`line-break: loose/strict`、字典式 CJK 词边界。
- `cargo check --workspace` 零 warning；`cargo test --workspace` 全绿；`cargo clippy
  --workspace --all-targets -- -D warnings`；`cargo fmt --all -- --check`；
  feature 组合编译（`nexty-layout --no-default-features --features inline`、
  `--features taffy-map`、`nexty-css --no-default-features --features values` 等）零 warning；
  `[[bin]]` 的 `required-features` 不受影响（本轮不动 chrome 入口）。
- AGENTS.md 同步：`nexty-css` 属性清单加 `white-space` / `overflow-wrap` / `word-break`；
  `nexty-layout` 偏差清单**划掉** UAX#14 断行与 `text-align: justify` 两条，
  保留 float / table / 多行 baseline 组 / `vertical-align` 非 baseline；
  依赖清单加 `unicode-linebreak`（并说明它属布局层）。
- 版本号随提交 +1：`nexty-css` 0.1.10 → 0.1.11、`nexty-text` 0.1.6 → 0.1.7、
  `nexty-layout` 0.1.14 → 0.1.15（chrome 无代码改动则不动）。
- 实机验收（无头不可测项，按项目记忆的 Computer Use 流程：`cargo run -p nexty-chrome
  --bin nexty` + Ctrl+L 输入本地 http 地址 + `.workbuddy/t3-test` 资产，新页面用 node 服务器）：
  中文页面正文折成多行且不横向溢出、`<pre>` 保留空白与换行、`<br>` 生效、
  `overflow-wrap: break-word` 长 URL 页不溢出；截图目测断行观感与浏览器一致。
- 分任务 commit（`[css]` / `[text]` / `[layout]` / `[docs]`），退出条件全部满足后 push。

## 非目标（本轮不做）

- **页面文本选择与复制**（本轮之后的下一轮首选；本轮已为它落地 `Glyph.cluster` 源字节偏移）。
- 「折叠后文本 ↔ DOM 原文偏移」的映射（选择轮做「跨行/跨元素选中后复制出原文」时才需要；
  本轮 cluster 偏移只相对进 layout 的折叠文本）。
- `text-decoration`（链接下划线仍缺，属 paint 指令集另轮）、`letter-spacing` /
  `word-spacing` / `text-indent` / `text-transform`。
- `hyphens` 自动断字（需连字符词典）、`line-break: strict|normal|loose`（UAX#14 断点的
  收紧/放宽）、`word-break: auto-phrase`、`text-justify` / `text-align-last`。
- bidi / RTL（`direction` / `unicode-bidi`）；复杂脚本（泰文等）字典式断行。
- parley `complex-scripts`（ICU4X CJK 词典数据）——本轮断行不经 parley，运行期那条
  `No segmentation model for complex script` 警告维持已知噪音，不处理。
- `vertical-align` 非 baseline、float、table、多行 leaf 的 baseline 组语义（§8.5）。
- paint 圆角 / 渐变 / 阴影 / 根元素背景传播（CSS §14）、vello_hybrid GPU 后端、
  `position: fixed` 视口锚定、表单与 JS。

## 方向评估（为什么是这轮）

| 候选 | 价值 | 成本 | 结论 |
| --- | --- | --- | --- |
| **文本断行与留白（本轮）** | 中文页面当前**完全溢出不可读**；`white-space` / `<br>` / `justify` 缺失 | 小–中：一个零依赖 crate + `inline.rs` 断行内核重构，几乎全可无头单测 | ✅ 本轮 |
| 页面文本选择与复制 | 「像浏览器」的关键交互里程碑（AGENTS.md 已立项） | 中–大：跨 text/layout/chrome + 剪贴板依赖；依赖本轮的字符映射与稳定行几何 | 下轮首选 |
| text-decoration / 链接下划线 | 链接可辨识度（UA 表本就未给下划线，属 paint 指令集） | 小–中：需 paint 侧新指令 | 暂缓（可并入 paint 轮） |
| paint 圆角 / 渐变 + 根背景传播 | 视觉上限 | 中：vello_cpu 已支持圆角与渐变笔刷，主要成本在 css 值与 pipeline | 暂缓 |
| float / table 布局 | 老页面与邮件类内容 | 大：taffy 不覆盖，自研需 ADR 论证「无轮可用」 | 暂缓 |

## 验证流（AGENTS.md Verification Flow）

每层：`cargo check` 零 warning → `cargo test` 全绿 → `fmt` / `clippy` → commit（架构师
跑语义比对后提交；本轮 ground truth 为 UAX#14 与 CSS Text，无 WPT 语料比对项）。
修 bug 类（中文不折行）先写 failing test → 看它 fail → 修 → 看它 pass；探针已给出可直接
复用的断言（120 字中文 / 200px 视口 / 单行 1920px）。无头不可测项只有 T7 的实机目测。
