# ADR: 不接入 MusKitty 分词器

- 日期：2026-10-01
- 状态：已否决（评估后不引入）
- 背景：`nexty-html` 的 WPT tree-construction 比对里，`processing-instructions.dat` 等 88 条用例与语料期望不符，一度判为「html5ever 未实现处理指令词法状态」，遂评估用 MusKitty 的 `muskitty-html5-tokenizer`（自称 WHATWG §13.2.5 全状态实现）替掉 html5ever 的分词阶段，并以 feature 开关控制。

## 调研结论

**1. 现行规范里没有「处理指令」词法状态。**

现行 WHATWG HTML Living Standard §13.2.5 共 80 个状态：13.2.5.69–71 是 CDATA 三态，**13.2.5.72 直接就是 Character reference state**，一路到 13.2.5.80 Numeric character reference end state，中间无任何 PI 状态（[parsing.html §13.2.5](https://html.spec.whatwg.org/multipage/parsing.html)）。`<?…>` 在 tag open state 触发 `unexpected-question-mark-instead-of-tag-name`，转 bogus comment。

即：**html5ever 把 `<?…>` 降级为注释，正是规范要求的产出**，不是缺陷。

**2. MusKitty 分词器实现的是规范已删除的特性，且章节号是错的。**

其 `State` 枚举把 §13.2.5.72–76 标为五个处理指令状态，于是把 character reference 顺延为 §13.2.5.77–85（现行规范这批是 §13.2.5.72–80）。它对齐的是十几年前的草案，不是 Living Standard。

**3. 语料期望本身过时。**

`processing-instructions.dat` 期望的是 `| <?something ?>` 形式的 **PI 节点**，属 html5lib 遗留材料；浏览器与 html5ever 都产出注释节点。

**4. 技术上也接不进去。**

- html5ever 的 `Token` 枚举只有 Doctype / Tag / Comment / Characters / Null / EOF / ParseError，**没有 PI 变体**（`html5ever-0.40.1/src/tokenizer/interface.rs`）；`TreeBuilder::process_token` 亦无 PI 分支。桥接层无法把 PI token 送进树构建。
- `TreeBuilder::current_node()` 是**私有**的（`tree_builder/mod.rs`），桥接层拿不到当前插入点，PI 节点无处可插。

**5. 换分词器零收益。**

105 条基线 = 88 条语料过时 + 17 条非语料问题（11 条 html5ever 树构建未跟进规范 + 6 条需 JSRT 的 scripted 用例）。后 17 条全在**树构建**阶段，分词器替换碰不到；前 88 条要「修」就得让输出违反现行规范。

**6. 版本核对。**

本地 MusKitty 开发版与 crates.io `0.1.4` 的 `src/` 逐文件哈希一致（忽略行尾符差异），本地多出的只是测试与文档。

## 决策

不引入 `muskitty-html5-tokenizer` 到 `nexty-html`：移除可选依赖与 `muskitty-tokenizer` feature，分词阶段保持 html5ever 单实现，不设开关。

AGENTS.md 的参考优先级「**WHATWG 规范 > WPT 测试套件**」在此给出裁决：语料与规范冲突时以规范为准，不为了语料通过而让产出偏离规范。

## 边界与后续

- 语料侧：这 88 条留在 `KNOWN_DIVERGENCE` 基线里并注明「语料过时」，不修改语料文件（该目录声明为「原样拷入」）。若 WPT 后续更新该语料、或上游 html5ever 跟进，基线会报 stale，届时更新。
- 需求侧：若将来真要支持 XML 式 PI（例如把解析器扩到 XML/XHTML 文档模式），那是**新增一种文档解析模式**，不是「补充 html5ever 的分词器」，届时应单独立 ADR。
- MusKitty 分词器可作为独立参考实现，但不进 Nexty 的依赖树。

## 相关文件

- `AGENTS.md` —— 状态与基线分类描述
- `crates/nexty-html/Cargo.toml` —— 未引入依赖
- `crates/nexty-html/tests/tree_construction.rs` —— `KNOWN_DIVERGENCE` 基线