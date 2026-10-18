# ADR: 自研 UAX#14 断行引擎（Unicode 18.0.0）

- 日期：2026-10-17
- 状态：已采纳
- 背景：`nexty-layout` 的 `inline` 长期只在 ASCII 空白处断行，CJK 段落不折行。本仓要求以 UAX#14 与 `LineBreakTest.txt` 为行为 ground truth；现有成熟 crate 在 Unicode 18.0.0 语料上都不能做到逐条一致。

## 候选与实测

| 候选 | 结果 | 结论 |
| --- | --- | --- |
| `unicode-linebreak` 0.1.5 | Unicode 数据停在 15.0.0，对 `LineBreakTest-18.0.0` 通过 18,229/19,346（1,117 条失败） | 不可用：新增 HH 类、AK/AP/AS/VF/VI、LB12a/LB15c/LB15d/LB19a/LB25/LB28a 等规则缺失 |
| ICU4X `icu_segmenter` 2.3（V2 / V3 数据） | Unicode 17 数据，最好 19,316/19,346（30 条失败）；V3 需 `unstable` feature | 不可用：仍是 Unicode 18 规则/属性差异；且引入字典/LSTM 数据与 CSS 定制，体积与语义都超出本层所需 |
| 自研 UAX#14 + 官方 UCD 18.0.0 数据 | 19,346/19,346 | 采纳 |

## 决策

1. **规则**按 UAX #14（Unicode 18.0.0）§6 实现，覆盖 LB1–LB31 及 LB15c/LB15d/LB19a/LB20a/LB28a 等子规则；规则顺序与例外（尤其 LB15c 必须早于 LB13 的 `× IS`）按规范原文处理。
2. **类数据**取 UCD 18.0.0 官方文件（`DerivedLineBreak.txt`、`EastAsianWidth.txt`，以及从 `DerivedGeneralCategory.txt` / `emoji-data.txt` 提取的小表），落入 `crates/nexty-layout/src/data/`。运行时 `include_str!` + `OnceLock` 解析一次；文件版本与 SHA-256 见该目录 `README.md`。
3. **cluster 真值**取 shaping 后端实际输出的 glyph cluster：`nexty-text::Glyph.cluster` 是源文本字节偏移（harfrust/harfbuzz 的约定，已用 `"a\u{301}b"` 实测为字节而非码点）；`cluster_map` 再与 UAX#29 字形簇边界取并集，保证“整形未合并的 ZWJ/旗帜/印度文连写”也不会被按字符切开。
4. **字素边界**用 `unicode-segmentation` 1.13.3（MIT/Apache-2.0，已在依赖树中）。它是 CSS Text 3 §5.5 “typographic character unit” 的下界。
5. **CSS 语义**：`word-break: break-all/keep-all` 与 `overflow-wrap: normal/break-word/anywhere` 在 UAX#14 结果上增删断点；`anywhere` 参与 min-content，`break-word` 不参与（CSS Text 3 §5.4）；`break-all` 的效果参与 intrinsic size（§5.1 末段），且绝不切开 cluster（§5.5）。

## 接入 `build_lines`（2026-10-17 后续轮次）

`break_opportunities` / `cluster_map` / `split_spans` 已接入 `inline::build_lines`：

- 文本项按 computed style 切块，只整形一次；按 `cluster_map → break_opportunities →
  split_spans` 切段后贪心放置。软断点只来自 UAX#14 + CSS 选项；`white-space:
  nowrap|pre` 关闭软断点，`<br>` 与保留的 `\n` 是硬断行项。
- CSS 选项由 `ComputedStyle` 映射：`word-break: normal|break-all|keep-all`、
  `overflow-wrap: normal|break-word|anywhere`。`break-word` / `anywhere` 的
  cluster 紧急断点只在整段放不下一行时启用（先放整词、再拆词）；`break-word`
  不参与 min-content，`anywhere` 与 `break-all` 参与。
- `white-space` 六种取值按 CSS Text 3 §3.1 实现：`normal`/`nowrap`/`pre-line`
  折叠可折叠空白（行首/行尾折叠空格丢弃），`pre`/`pre-wrap`/`break-spaces`
  逐字保留、`\n` 为硬断行；`pre` 不软换行，其余允许 UAX#14 软断点。
- 实现取值（偏差）：`tab-size` 未实现，制表按一个空格保留/计宽；
  `pre-wrap` 行尾保留空格按占宽处理（未做悬挂）；`break-spaces` 的行尾空格
  按规范参与宽度。
- 跨行内元素边界的断行上下文只回看左侧两个字符（`breaks_between_text`），
  长距离数字/引号规则（LB15c/LB25 等）在跨盒子边界处可能退化；原子行内盒
  按 U+FFFC（Line_Break=CB）处理，两侧可断。

## 边界（本轮不做）

- 字典式断行（泰/缅/高棉/老挝）与 `hyphens` 自动断字；SA 按 LB1 默认解析为 AL。
- CSS `line-break: strict|normal|loose` 的收紧/放宽与 `text-align: justify` 的断点分配。
- bidi / RTL、`vertical-align` 非 baseline、页面文本选择与复制。

## 验证

`crates/nexty-layout/tests/inline_conformance.rs` 读取钉死的官方
`LineBreakTest-18.0.0.txt`（19,346 条，SHA-256 见 `tests/data/README.md`），
全量比对 `÷`/`×`，并另按 LB4-8 / LB9-10 / LB21-31 规则组过滤重跑；同一文件还覆盖
cluster 覆盖不变量、`break-all`/`anywhere` 的 min-content 差异与 span 逐段拼回。
`crates/nexty-layout/tests/text_breaking.rs` 走真实 `layout_document` 管线覆盖：
`white-space` 六个取值、`<br>` 硬断（含 `nowrap`）、120 字中文折行与行首禁则、
`break-all` / `keep-all` / `break-word` / `anywhere`、组合符字形簇不被切开。
