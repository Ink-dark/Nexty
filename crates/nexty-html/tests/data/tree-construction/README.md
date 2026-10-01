# WPT tree-construction 语料

本目录是 `nexty-html` 的 WPT tree-construction 比对语料，**原样拷入**，未做任何修改。

- 来源仓库：<https://github.com/web-platform-tests/wpt>
- 路径：`html/syntax/parsing/`（tree-construction `.dat` 文件）
- 钉定 commit：`5cd8e3fa0a6c4ca11fa565f7c0956802c8e0045d`
- 许可证：3-Clause BSD，见同目录 [LICENSE.md](LICENSE.md)

上游 runner 与格式说明见 `resources/test.js` 与 `resources/README.md`。比对由
`crates/nexty-html/tests/tree_construction.rs` 执行，其行为对齐该 runner。

## `.dat` 格式

每个文件由若干用例组成，用例之间以空行分隔。一个用例包含以下段：

- `#data` — 喂给解析器的输入。段内各行原样保留，仅去掉最后一个换行。
- `#errors` — 期望的解析错误条数（本 harness 不比对错误，只比对树）。
- `#document-fragment` — 可选。片段解析的上下文元素；`svg ` / `math ` 前缀切换
  命名空间，其余按 HTML 处理。
- `#script-off` — 可选。出现时 `scripting flag` 置为 disabled，否则为 enabled。
- `#document` — 期望的 tree dump。

## tree dump 序列化规则

- 每行以 `|` 开头，缩进为「父节点数 × 2 + 1」个空格；根 `#document` 行无缩进。
- 元素：`<标签名>`，属性另起一行，按属性名字符串的 UTF-16 码元字典序排列。
- 属性：`名字="值"`。
- 文本：`"内容"`（不转义换行）。
- 注释：`<!-- 内容 -->`。
- DOCTYPE：`<!DOCTYPE 名字>`；public/system id 非空时为
  `<!DOCTYPE 名字 "public" "system">`。
- 处理指令：`<?target data?>`。
- 模板内容：`content` 行，其下为 `template` 的 template contents 子节点。
- 标签名 / 属性名按命名空间加前缀：HTML 无前缀，SVG 为 `svg `，MathML 为
  `math `，XLink / XML / XMLNS 分别为 `xlink ` / `xml ` / `xmlns `。

## 基线

部分用例失败源于上游 `html5ever` 尚未跟进的规范改动，而非 Nexty 本层。这些用例
在 `tree_construction.rs` 的 `KNOWN_UPSTREAM` 中逐条列出并标注原因。