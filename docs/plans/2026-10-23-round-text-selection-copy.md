# goal.md — 本轮任务：页面文本选择与复制（源文本映射 / 选区几何 / 拖拽与双击 / 高亮 / 剪贴板）

> **状态（2026-10-24）：已完成，归档于 2026-10-24。** T1 见 `12b9218`
> （run 携带文本节点身份与源字节区间、行首来源），T2 见 `ee00119`
> （`TextMap` 选区几何：cluster 命中、逐行矩形、UAX#29 选词与复制文本重建，
> layout 0.1.17），T3 见 `f7e2b87`（拖拽 / Shift+点击 / 双击 / Ctrl+A 接线与
> 链接释放才导航），T4 见 `77903fd` + 坐标系修复 `0b4b55a` + 回归测试
> `0e76900`，T5 见 `57eff03`（arboard 剪贴板 + Ctrl+C，chrome 0.1.18），
> 文档同步 `cec92fc`。无头门禁与测试全绿；T7 实机验收清单保留在本文档
> 末节供实机核对。下一轮转入「不可信输入边界加固」（见根 `goal.md`），
> 触发依据为 [2026-10-24 性能与安全审查](../2026-10-24-perf-security-review.md)。

日期：2026-10-23。前置：UAX#14 断行轮已完成——`nexty-layout` 0.1.16 的
`build_lines` 已消费 `cluster_map → break_opportunities → split_spans`，
`white-space` 六取值、`<br>`、`overflow-wrap` / `word-break` 均已落地，
`text_align` center/right 的行平移路径稳定。旧 goal 归档于
[docs/plans/2026-10-17-round-uax14-inline-breaking.md](docs/plans/2026-10-17-round-uax14-inline-breaking.md)。
当前版本：css 0.1.11 / text 0.1.7 / layout 0.1.16 / paint 0.1.4 / chrome 0.1.17。

## 本轮为什么做这些

断行稳定之后，文本选择的前置条件才真正成立。当前缺口是系统性的：

- **没有任何页面文本选择**。`hit_test_ex` 只返回节点与命中类型（`TextRun` /
  `Image` / `Fragment`），没有「点在哪个字符上 / 两个位置之间选了哪些字符」的概念；
  `LineFragment` / `TextRun` 也只有 glyph 与 cluster，**没有回指 DOM 文本节点与
  源字节偏移**。拖拽、双击、Shift+点击、`Ctrl+A` 都无从实现。
- **`Ctrl+A` 的页面分支是空的**：`app.rs::apply_shortcut` 里明确写着「页面文本
  选择尚未实现」，只在地址栏聚焦时生效；`Ctrl+C` 完全没有映射，也没有剪贴板依赖。
- **折叠后文本 ↔ DOM 原文偏移的映射缺失**：`Collector` 按 `white-space` 折叠空白、
  把 `<br>` / 保留换行变成硬断行后，只留下处理后的文本；源文本节点身份与逐字符
  源偏移被丢弃。没有这层映射，选区端点无法在 resize 重排后重新定位，也无法把
  渲染文本与文档内容对应起来。

这三件事合起来就是「页面文本选择与复制」。它也是 AGENTS.md 里点名的**关键交互
里程碑**，并为后续 find-in-page、编辑、拖放、`::selection` 打底。

### 为什么先做选择而不是 `text-align: justify`

上一轮把 justify 排在下一轮，是因为当时断行本身还不稳定、justify 无处落脚。
现在断行已稳定，两项都具备前置条件；从价值看，justify 是**视觉细化**（小–中成本），
而选择/复制是浏览器可用性的硬缺口（复制一段正文都做不到）。选择还会产出
「渲染文本 ↔ 源文本」的映射，这是后续所有文本交互的地基，晚做只会让更多功能
依赖一个不存在的抽象。因此本轮做选择与复制；justify 与 `text-indent` /
`letter-spacing` / `word-spacing` 一起留到下一轮「CSS Text 排版细化」。

## 任务清单与退出条件

### T1 nexty-layout：文本项携带源文本映射（0.1.17）
- `Collector::push_text` 现在只拿到样式来源元素与文本切片；改为同时记录**文本
  节点 id**与逐字符**源字节区间**。`TextChunk` / `InlineItem::Text` 增加：
  - `text_node: NodeId`（区别于样式来源 `node`）；
  - 处理后的文本（现有 `text`）与每个字符在源文本节点中的 `(start, end)` 区间。
    折叠空白映射到被消费的整段空白；`pre` 制表映射到制表符本身；被行首/行尾
    裁掉的空白仍保留在映射里（供选择端点落位）。
- `TextRun`（`fragment.rs`）增加：`text_node`、处理后的整段文本（`Arc<str>`，
  一行只带自己的 glyph 子集，文本与映射按 chunk 共享）、逐字符源映射
  （`Arc<[...]>`）。`Glyph::cluster` 仍是该 chunk 文本内的字节偏移，映射把
  cluster 转成 `(text_node, source_offset)`。
- `LineFragment` 增加行首来源：软换行（并标记断点是否消费了一个可折叠空格）/
  硬断行（`<br>` 或保留的 `\n`）。复制文本要靠它区分「软折行不插换行」与
  「硬断行插 `\n`」。
- 退出条件：每个 glyph 的 cluster 都能解析到源文本节点内的字节区间；折叠空白、
  `pre` 制表、行尾被裁空白的映射各有一条单测；`<br>` 与 `pre` 换行的行首来源为
  硬断行；CJK 软折行不标记空格；现有 45 条 layout + 16 条 `text_breaking` 测试
  不改仍绿；paint 层零改动。

### T2 nexty-layout：`selection` 模块（选区几何，0.1.17）
- 新 feature `selection`（`selection = ["block"]`，进 default）与模块
  `selection.rs`。只消费 `Fragment` 树，不引入新的外部依赖
  （`unicode-segmentation` 已是本 crate 依赖）。
- 公共 API（doc comment 引用 DOM Selection / CSS Text 相关条款）：
  - `TextPosition { node: NodeId, offset: usize }`：源文本节点内的字节偏移；
  - `TextMap::new(&Fragment)`：按绘制顺序收集文本 run（含行内原子盒内的文本；
    图片与原子盒自身不参与）；
  - `hit(x, y)`：root 边框盒坐标系下的点 → 最近的 cluster 边界位置（左半/右半
    字形分别落在前/后边界；行尾右缘落到最后一个可见字符之后；**永不落在
    grapheme cluster 内部**）；
  - `nearest(x, y)`：点不在任何行内时按最近行/最近文本吸附；
  - `resolve(position)`：源位置 → 当前布局中的内部位置（resize 后重新定位；
    文本节点消失返回 `None`）；
  - `word_range(position)`：UAX#29 词边界；CJK 按 UAX#29 逐字（字典式分词记为
    偏差）；
  - `all()`：整页首尾位置；
  - `rects(anchor, focus)`：按文档/绘制顺序规范化两端点，逐行产出选区矩形
    （root 坐标系；空选区不产出矩形）；
  - `text(anchor, focus)`：复制文本重建（规则见下）。
- 复制文本规则：同一块内的软折行直接拼接（断点消费了可折叠空格时补一个空格）；
  硬断行与块级片段边界插 `\n`；`pre` 的空格/制表/换行原样保留；折叠空白输出单个
  空格；选区首尾空白裁掉；行内原子盒（inline-block）内的文本按行内拼接、不插
  换行；图片不输出。
- 退出条件（全部走 `layout_document` 真实管线）：点击每个字形左右半区得到预期
  边界；跨行选区产出 2+ 个矩形且 y/height 与行盒一致；resize 后 `resolve` 仍
  命中同一源位置；`word_range` 覆盖 Latin 单词与单个 CJK 字；`text` 的单测至少
  覆盖 `abc def` 软折行 → `abc def`、`a<br>b` → `a\nb`、两个 `<p>` → `a\nb`、
  `<pre>a\nb</pre>` → `a\nb`、`a <b>b</b>` → `a b`。

### T3 nexty-chrome：选区状态机与输入（0.1.18）
- 新 feature `selection` 与纯逻辑模块 `selection.rs`（不依赖 winit，可无头测试）：
  `anchor` / `focus`（`TextPosition`）、拖拽中标志、`begin_drag` / `extend_to` /
  `finish_drag` / `clear` / `select_all` / `word_at`；状态机只接收位置，不碰窗口。
- `app.rs` 接线（顺序与现有滚动条优先级一致）：
  - 页面左键按下（滚动条未命中）：失焦地址栏，按 `hit`/`nearest` 落锚点并进入
    拖拽；按住拖动持续 `extend_to`；释放结束；
  - `Shift` + 点击：有选区时扩展 focus，无选区时从点击处开始；
  - 双击（时间阈值 + 距离阈值，常量集中定义）：按 `word_range` 选词；
  - `Ctrl+A`：地址栏聚焦时仍全选地址栏，否则全选页面；
  - 链接导航改为**释放时且本次手势未拖动**才导航——拖过链接文字是选择，不是跳转；
  - 导航 / 刷新清空选区；滚动不清空也不改变源位置；resize 重排后 `resolve`
    端点，旧文本节点消失则清空。
- 退出条件：无头测试用 `pipeline::layout_page` 构造真实页面，覆盖拖拽得到预期
  范围、Shift+点击扩展、双击选词、`Ctrl+A` 全选、单击清空、导航清空、resize
  保留/清空、链接点击与拖拽的分流；不启动窗口即可跑。

### T4 nexty-chrome：选区高亮（0.1.18）
- 新增 `pipeline::build_scene_at_with_highlights(page, root, origin, highlights)`；
  现有 `build_scene_at` 保持签名并委托（空矩形时行为不变）。高亮在白色画布之后、
  页面内容之前插入半透明 `FillRect`（例如 rgba(0,100,255,64)）。
- 选区变化请求重绘；空选区不插入任何指令；矩形随 `origin`（工具带高度与滚动
  偏移）一起平移。
- 退出条件：显示列表测试断言高亮命令位于所有 `DrawText` / `DrawImage` 之前、
  颜色带 alpha、坐标已叠加 `origin`；空选区命令数与改动前一致。

### T5 nexty-chrome：复制到系统剪贴板（0.1.18）
- 新增可选依赖 `arboard 3.7`（MIT OR Apache-2.0，`default-features = false`，
  只要文本，不拉图片后端），挂在 chrome 的 `selection` feature 下
  （`selection = ["pipeline", "dep:arboard"]`——选区状态与测试都消费
  `layout_page` 产出的片段树）；`docs/decisions/2026-10-23-clipboard.md`
  记录选型（为什么是 arboard、为什么不是 `clipboard-win` / `copypasta`、
  依赖与 unsafe 边界、只做文本）。
- `ui::shortcut` 增加 `Ctrl+C`；地址栏聚焦时不复制页面；复制内容来自
  `TextMap::text`，写入失败只记录错误、不 panic。
- 退出条件：快捷键映射单测；`text` 重建规则由 T2 的测试覆盖；剪贴板写入在
  实机验收中验证（无头测试不触碰 OS 剪贴板）。
- 依赖门禁：`cargo deny check` 的 licenses/advisories 通过；按 AGENTS.md 流程
  重新生成 `docs/dependencies.html`，包含 NOTICE / 归属清单与多重许可人工核对
  区块。本机 `python` 是 WindowsApps 空壳（退出码 49），需把
  `.workbuddy/collect_notices.py` / `inject.py` 的等价逻辑用 node 重写后执行
  （脚本不入库，产物入库）。

### T6 门禁、文档与版本
- `cargo check --workspace` 零 warning；`cargo test --workspace` 全绿；
  `cargo clippy --workspace --all-targets -- -D warnings`；
  `cargo fmt --all -- --check`；`cargo deny check` 通过。
- feature 组合零 warning：`nexty-layout --no-default-features --features selection`
  与 `--features selection,inline`、`nexty-chrome --no-default-features --features
  selection`、以及既有各单 feature 组合（`replaced` 单独开启的零 warning 是上轮
  刚修的门禁，不得回归）。
- AGENTS.md 同步：`nexty-layout` 状态加 `selection`（命中/选区几何/复制文本），
  `nexty-chrome` 加选区状态机、高亮与剪贴板；Feature 映射表加两行；chrome 模块
  清单加 `selection`；把「页面文本选择与复制」从后续深化方向移出，roadmap 改为
  justify + CSS Text 排版细化。
- 版本号随逻辑提交 +1：`nexty-layout` 0.1.16 → 0.1.17、`nexty-chrome`
  0.1.17 → 0.1.18；css / text / paint 本轮无代码改动则不动。
- 分任务 commit（`[layout]` / `[chrome]` / `[docs]`），退出条件全部满足后 push。

### T7 实机验收（无头不可测项）
- 按项目记忆的 Computer Use 流程：`.workbuddy/t3-test/serve.mjs`（node 静态
  服务器，端口 8931）+ 新页面资产（中文段落、混合 `<b>`、`<pre>`、`<br>`、
  行内原子盒、长文可滚动）；`cargo run -p nexty-chrome --bin nexty`，
  Ctrl+L 输入本地地址。
- 验收动作：拖拽跨行选择（高亮在文字下方、逐行矩形）、双击 Latin 词、Shift+
  点击扩展、`Ctrl+A` 全选、`Ctrl+C` 后用 `powershell -Command Get-Clipboard`
  核对文本（软折行不插换行、`<br>` 插换行、块边界插换行、`pre` 保留空白）；
  滚动后选区与文字仍对齐；resize 后选区保留或按规则清空；点击链接跳转、拖过
  链接文字只选择不跳转；导航后选区清空。截图目测与 Edge/Chrome 同页对比。

## 非目标（本轮不做）

- **`text-align: justify` / `text-align-last`**、`text-indent`、`letter-spacing` /
  `word-spacing`、`text-decoration`（下一轮 CSS Text 排版细化）。
- 光标 / 编辑能力：插入符、键盘 caret 导航、`Shift+方向键`扩选、Home/End 选取、
  双击后的拖拽扩选。
- 选择/复制图片、原子盒本身或 `alt` 文本；复制 HTML 格式；剪贴板图片。
- 拖拽到视口外自动滚动、触摸 / 触控笔、IME、drag-and-drop。
- 地址栏的 `Ctrl+C`（页面复制优先；地址栏复制另行处理）、`::selection` 配色、
  `user-select` CSS 属性。
- bidi / RTL 选区顺序；find-in-page；`window.getSelection()` 等 DOM API。
- 多行 baseline 组、float / table、`vertical-align` 非 baseline 等既有缺口。

## 方向评估（为什么是这轮）

| 候选 | 价值 | 成本 | 结论 |
| --- | --- | --- | --- |
| **页面文本选择与复制（本轮）** | 关键交互里程碑；断行稳定后前置已满足；产出后续文本交互的源映射地基 | 中–大：layout 源映射 + 选区几何 + chrome 输入/高亮 + 剪贴板依赖 | ✅ 本轮 |
| `text-align: justify` + CSS Text 细化 | 视觉保真 | 小–中：行内间隙分配 + 三四个属性 | 下一轮 |
| text-decoration / 链接下划线 | 可辨识度 | 小–中：paint 新指令 | 暂缓 |
| float / table | 老页面与邮件类 | 大：taffy 不覆盖 | 暂缓 |
| vello_hybrid GPU 后端 | 性能 | 中：surface 接入与适配层 | 暂缓 |

## 验证流（AGENTS.md Verification Flow）

每个子任务先写 failing test（真实 `layout_document` / `layout_page` 路径），看它红，
再实现到绿；每层：`cargo check` 零 warning → `cargo test` 全绿 → `fmt` / `clippy`
→ commit。T1 是纯增量（既有 45 + 16 + conformance 8 条必须不改仍绿），T2–T4 的
选区测试全部无头可跑；只有剪贴板写入与交互手感依赖 T7 实机。退出条件全部满足后
push。
