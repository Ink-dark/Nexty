# goal.md — 本轮任务：nexty-layout 引入 taffy 接管盒级几何（block/flex/grid/absolute）

日期：2026-10-10。前置：上轮 chrome 交互补全（[docs/plans/2026-10-08-round-chrome-interaction.md](docs/plans/2026-10-08-round-chrome-interaction.md)）
完成，`nexty-chrome` 0.1.14；八层骨架端到端连通，`nexty-layout` 0.1.4 自研块级流 + 单行 flex + 自研 inline 断行已落地。
旧 chrome 交互 goal 归档于 `docs/plans/2026-10-08-round-chrome-interaction.md`。

## 本轮为什么做这些

上一轮对「日常可用浏览器」的差距评估中，**盒级布局完备度仅约 35%**，最大缺口是
grid、多行 flex、绝对定位与复杂块级流——这些恰恰是现代真实页面渲染质量的核心，自研补齐
成本极高。

当初 ADR（[docs/decisions/2026-10-01-crate-selection.md](docs/decisions/2026-10-01-crate-selection.md)）把布局定为自研的理由是
"**`taffy` 虽成熟但布局是 WPT 对齐的关键层，需完全可控**"。但该理由已过时：taffy 0.14
现已实现 **CSS Block / Flexbox / CSS Grid** 三套算法，被 Servo、Blitz 等成熟浏览器引擎
采用，且为 MIT 许可（已在 `deny.toml` 白名单）。这与 Nexty「不造轮子」的总方针一致——
布局层本就是 ADR 列出的"自研例外"之一，现在有条件收回这个例外。

**引入 taffy ≠ 全替换**，边界由能力决定：
- taffy **做**：块级盒几何（普通流块级子盒排列、宽度解析、margin）、flex 容器（含多行
  wrap / flex-shrink / 完整对齐值）、grid 容器、绝对/固定定位盒。
- taffy **不做**：inline / 文本布局、table。故 `inline.rs`（行内断行）必须保留；table
  维持现状（taffy roadmap 尚未覆盖，本轮不引入 taffy table）。

本轮目标：以**不破坏现有渲染**为前提，把块级流与 flex 的几何解算迁移到 taffy，并**顺带
借 taffy 补齐 grid / 多行 flex / 绝对定位**；paint 层零改动——`layout_document` 的
输入输出契约（`Document + ComputedStyle + image_sizes + viewport_width` → `Fragment` 树，
定义见 `crates/nexty-layout/src/fragment.rs`）冻结。

预期收益：盒级布局完备度从约 35% 提升至约 70–75%（grid / 多行 flex / 绝对定位补齐；
剩余为 table + 完整 UAX#14 断行 + 部分 edge case）。

## 任务清单与退出条件

### T0 决策更新与门禁准备
- 在 `docs/decisions/2026-10-01-crate-selection.md` 追加「修正记录」：将"盒级布局"选型由
  "自研"改为"**taffy 接管盒级几何 + 自研 inline/table**"，记录理由（taffy 0.14 已支持
  Block/Flex/Grid、MIT、被 Servo/Blitz 采用；inline 与 text 仍自研因 taffy 不做文本布局；
  table 维持现状待 taffy roadmap）、能力边界、与"不造轮子"方针的关系、风险。
- `taffy` 加入 `crates/nexty-layout/Cargo.toml`（MIT，MSRV 1.71 < 本仓 1.90，兼容）；
  跑 `cargo deny check` 确认其传递依赖许可证全过（白名单已含 MIT/Apache/BSD/ISC/MPL/Zlib/
  CC0/Unicode/CDLA/LGPL）；`cargo about` 重生成 `docs/dependencies.html`。
- 隔离策略落实：taffy 的 `Style` / `TaffyTree` / `Layout` 等类型**只出现在 nexty-layout
  内部**，**绝不进入 pub 导出**（AGENTS.md「依赖类型不得外泄」硬规则）。
- 退出条件：deny check 零新增失败（或仅已有放行项）；dependencies.html 更新；ADR 修正
  记录就位。

### T1 样式映射层（`style_map.rs`，新模块）
- 实现 `ComputedStyle -> taffy::Style` 单向映射，覆盖已支持属性：
  - `display` 计算值（block/inline/flex/grid/none/contents → taffy `Display` + 必要的
    blockify；inline 由 `inline.rs` 处理，不在 taffy 建节点）；
  - `position`（static/relative/absolute/fixed）；
  - `box-sizing`；四边 `margin` / `padding` / `border`（edges，border 宽度计入盒模型）；
  - `width` / `height`（auto / px / % / min-content / max-content 映射；% 相对包含块）；
  - `min/max-width` / `min/max-height`；
  - flex-*（`flex-direction` / `grow` / `shrink` / `basis` / `wrap` / `align-items` /
    `justify-content` 及 align-self/justify-self）；
  - grid-*（`grid-template-columns` / `rows` / `gap` / `align-items` / `justify-items` /
    `grid-auto-flow` 等，视 taffy 0.14 支持面取舍）；
  - `inset`（top/right/bottom/left，绝对定位用）。
- 明确降级项（记入模块偏差清单）：`var()`、CSS 逻辑属性、`aspect-ratio` 等暂按 initial
  处理；影响布局的（如 % 高度依赖父高度）按 taffy 语义走。
- 退出条件：单测——典型取值映射正确（px/% / auto、flex basis、grid template、border
  edges、box-sizing）；降级项有断言与偏差记录。

### T2 盒树构建（`tree_build.rs`，新模块）
- 遍历 arena DOM + computed style，按生成盒规则构造 taffy `TaffyTree<()>`：
  - `display:none` 不建节点（跳过子树，Fragment 也不产）；
  - `display:contents` 不产盒，子节点提升为父的参与盒；
  - 块级替换元素 `<img>` 以 `image_sizes` 自然尺寸（或默认 300×150）作 definite size
    建叶节点；
  - **文本 / inline 内容暂不建 taffy 节点**，留待 T4 断行后包装成匿名块（taffy 不做
    text，必须用 `measure` 机制提供 line box 的 intrinsic size）；
  - 容器节点建立 children 关系（块级子盒 + 后续匿名块）。
- 维护 `NodeId(DOM) ↔ taffy Node` 双向映射表，供 T5 回填 Fragment。
- 退出条件：单测——已知 DOM 结构下，构造出的 taffy 树节点数 / 父子关系正确（含
  none / contents / inline 提升 / `<img>` 替换元素）。

### T3 taffy 接管块级与 flex 几何（改造 `block.rs`）
- 用 T1 的 `taffy::Style` 与 T2 的 `TaffyTree` 替换 `block.rs` 中"块级流几何解算"与
  "单行 flex 主轴分配"的计算逻辑；`tree.compute_layout(root, available)` 后读回各节点
  `Layout`（x/y/size）填入 `Fragment.border_box`。
- 保留：宽度解析所需的根 available size（视口宽度入参）；`Fragment` 的 `border` /
  `padding` / `style` 仍由 computed style 填充（taffy 只给几何，不重复造盒模型）。
- **行为差异处理**：taffy 与自研在 margin 折叠、百分比高度、min/max 收束等处可能存在差异，
  差异处**优先固化 taffy**（其 WPT 对齐更优），文档化并记录；不得引入 panic。
- 退出条件：单测——与现有自研 block 测试等效的几何断言仍通过（块级垂直堆叠、宽度解析、
  单行 flex grow/basis）；差异项有文档化比对结论。

### T4 inline 内容前置排版（衔接 `inline.rs` 与 taffy）
- 对每个 block/flex/grid 容器内的 inline 内容，先以容器 content width 跑 `inline.rs`
  断行，得到若干 `LineFragment`（每行高度已知）；把每行包装为一个**匿名块 taffy 节点**，
  用 taffy 的 **measure function** 提供其 intrinsic size（line box 尺寸由 nexty 断行
  决定，非 taffy 计算），与真实块级子盒一起参与该容器的 taffy 块级流 / 弹性流。
- 处理匿名块盒的 `Fragment` 标记（`anonymous = true`，`node` 指向父元素，绘制按透明处理）。
- 退出条件：单测——inline 内容 + 块级子盒混排时，行盒垂直位置与块级子盒互不重叠、顺序
  正确；与现有 inline 行为一致（字形 / 坐标不变）。

### T5 几何回填与 Fragment 生成
- 从 taffy `Layout` 读回每个节点几何，结合 computed style 生成 `Fragment` 树：`border_box`
  相对包含块内容盒（与现有坐标约定一致）、`border` / `padding` edges、`style`、`children`
  （块级 / 匿名块）、`lines`（来自 T4 的 `LineFragment`）。
- 绝对定位盒：taffy 已相对其包含块定位，回填时坐标归一到 Fragment 约定。
- 退出条件：单测——`Fragment` 结构与现有契约字段一致（node / anonymous / border_box /
  border / padding / style / children / lines）；`display:none` / `contents` 产物断言。

### T6 回归与专项测试
- 复用现有静态页 fixture（Wikipedia 风格 / 博客类）做布局结构比对（`Fragment` 树 diff），
  确保渲染**不退化**；新增布局专项单测：grid 双列模板、多行 flex wrap、绝对定位 inset、
  `display:contents` 提升、匿名块混排。
- inline 行为不变断言（沿用 `inline.rs` 现有测试）。
- 退出条件：布局回归 fixture 无结构性退化（允许 T3 列明的 taffy 与自研已知差异，差异需
  文档化且经人工核验）；全部单测绿。

### T7 门禁与收尾
- `cargo check --workspace` 零 warning；`cargo test --workspace` 全绿；clippy `-D warnings`
  零告警；`cargo fmt --all -- --check` 通过。
- AGENTS.md「当前状态」同步：`nexty-layout` 改为"**taffy 接管盒级几何（block/flex/grid/
  absolute）+ 自研 inline 断行**"，记录偏差清单（inline / table 仍自研、`var()` / 逻辑
  属性降级、UAX#14 完整断行未做）；更新布局完备度数字（约 35% → 约 70–75%）。
- 分任务 commit，最终 push（本机无凭据时留给用户）。

## 非目标（本轮不做）
- table 布局（taffy roadmap 未覆盖，维持现状：按块级化或忽略；本轮不引入 taffy table）。
- 完整 UAX#14 断行（仍是 `inline.rs` 偏差，不在范围）。
- paint 层改动（圆角 / 渐变 / 阴影不引入）；`Fragment` 契约冻结，paint 零改动。
- JS 引擎、交互基础（选择 / 表单 / 标签）、GPU 后端、网络缓存 / cookie（无关本轮）。
- 把 inline 文本布局交给 taffy（taffy 不做文本布局，**永不在本轮范围**）。
- 删除 `block.rs` 全部逻辑——保留宽度 §10.3.3 解析与 inline 衔接作为 taffy 前的预处理；
  被 taffy 替代的仅是"几何解算"部分。

## 验证流（AGENTS.md Verification Flow）
每层：cargo check 零 warning → cargo test 全绿 → fmt/clippy → commit。
taffy 行为差异需人工核验处（margin 折叠 / 百分比高度 / min-max 收束）在 T3/T6 注明并文档化，
列为人工核验待办。
