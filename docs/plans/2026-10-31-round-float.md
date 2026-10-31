# goal.md — 本轮任务：float 布局轮（taffy float_layout 接线 + 行盒避让验证）

日期：2026-10-31。前置：性能轮已完成（2026-10-25，css 0.1.14 / text 0.1.8 /
paint 0.1.7 / layout 0.1.27 / chrome 0.1.25），归档于
[docs/plans/2026-10-25-round-perf.md](docs/plans/2026-10-25-round-perf.md)。
本轮是 roadmap 排定的下一项（用户决策：paint 扩展先放一放，优先堆基础功能；
float 优先于 table / position:fixed）。

**关键侦察结论（动手前确认）**：taffy 0.14 自带 `float_layout` cargo feature
——`Style.float` / `Style.clear` 字段 + block 算法内的完整浮动实现
（`compute/float.rs`：`FloatContext`、BFC 槽位、float 内容高度贡献、clear
与 margin 折叠交互、内在宽度计算）。本轮**不自研浮动放置**，按「不造轮子」
规则以接线为主：

1. nexty-css 解析 `float` / `clear` → `ComputedStyle` 字段；
2. nexty-layout 启用 taffy `float_layout`，`style_map` 映射两属性；
3. 验证自研行盒（run 叶 measure）在浮动旁收缩可用宽后正确环绕；
4. 片段树 / paint / hit-test 路径验证浮动盒自然流出。

## 任务清单与退出条件

### T1 nexty-css：`float` / `clear` 属性解析（css 0.1.14 → 0.1.15）
- `values` 模块：`float: none | left | right`、`clear: none | left | right |
  both`（CSS 2.1 §9.5.1/§9.5.2 取值；`inline-start/end` 等逻辑值不做，记偏差）。
- `ComputedStyle` 增字段；初始值 none；继承性：两者均不继承。
- **退出条件**：属性解析单测（合法/非法/继承/级联覆盖）；`cargo check -p
  nexty-css` 零 warning；不改行为时版本 bump 记入 commit。

### T2 nexty-layout：启用 taffy `float_layout` + 样式映射（layout 0.1.27 → 0.1.28）
- `Cargo.toml`：taffy 依赖追加 features `float_layout`（taffy 常驻原则不变，
  feature 是能力开关）。
- `style_map`：`ComputedStyle.float/clear → taffy Style.float/clear`；
  **blockify 修正**——`position: absolute/fixed` 与 flex/grid item 上的
  `float` 计算为 none（CSS 2.1 §9.7 / CSS Flexbox §4），映射层拦截。
- `tree_build`：浮动盒作为普通 taffy 子节点建树（float 是块级化的）；
  验证 run 叶 measure 在 taffy 浮动算法下收到的 `AvailableSpace` 已扣除
  浮动占位（若 taffy 只对块级子盒收缩、不给 run 叶收缩宽度，需在
  `block` 装配或 measure 侧适配——先写探针测试确定行为再动手）。
- **退出条件**：探针测试钉住——①单左浮动 + 文本环绕；②右浮动；
  ③`clear: both` 下方下移；④浮动包含（父块高度含浮动，BFC 情形
  `overflow: hidden` 与普通块两种）；⑤双浮动并排。行盒字形坐标相对
  浮动避让后的可用区正确。逐 feature `--no-default-features` 编译零 warning。

### T3 nexty-layout：片段树 / 选区路径验证（并入 0.1.28）
- 浮动盒经 `assemble_element` 自然进入片段树（taffy 子节点递归）；验证
  `border_box` 相对包含块内容盒的坐标约定成立（浮动 x 位置非 0 时）。
- 选区/命中：浮动盒内文本参与 `TextMap`（按行盒几何，预期自动成立），
  探针测试钉住「命中浮动盒内文本返回该盒」。
- **退出条件**：上述测试绿；既有测试（含 UAX#14 conformance、CSS Text 轮
  测试）不改仍绿。

### T4 nexty-chrome：实机验证（chrome 不改则不 bump）
- `.workbuddy/t3-test` 加一个图文环绕测试页（左图环绕段落 + 右浮动侧栏 +
  clear 分节）。
- **退出条件**：实机冒烟——环绕文本不与浮动重叠、滚动不重排不闪烁、
  浮动盒内文本可选可复制、链接点击命中浮动盒正常。

### T5 收尾：文档与登记
- AGENTS.md：layout 状态段补 float 落地说明 + feature 映射表（float 能力
  挂在既有 `block` feature 下，不新增 feature——taffy float 是 block 算法
  的一部分）；偏差清单登记（逻辑值 `inline-start/end`、float 跨 BFC 边缘
  情形按 taffy 实现为准）。
- `docs/decisions/` 若涉及新决策（直接采用 taffy float 而非自研）补记一条
  短 ADR 或在选型 ADR 修正记录追加一段。
- **退出条件**：`cargo check/test/clippy/fmt --workspace` 全绿；`cargo deny
  check` 通过（未引入新依赖，预期无变化）；逐任务独立 commit。

## 明确不做（偏差清单）
- `float: inline-start / inline-end`、`clear: inline-start/inline-end`（逻辑值）。
- shape-outside、`position: fixed` 视口锚定、「最近定位祖先」搜索（独立轮）。
- table（独立整轮）、paint 扩展（下一轮候选）。

## 整轮退出条件
T1–T5 退出条件全部满足；`goal.md` 归档 + push；等用户验收。
