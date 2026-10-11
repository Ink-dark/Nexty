# goal.md — 本轮任务：行内呈现与文本能力补齐（text-align / font 简写 / 视口单位 / CJK 回退 / flex baseline）

日期：2026-10-11。前置：taffy 轮完成（盒级几何 block/flex/grid/absolute 由 taffy 接管，
`nexty-layout` 0.1.10，布局完备度约 70–75%），旧 goal 归档于
[docs/plans/2026-10-10-round-taffy-layout.md](docs/plans/2026-10-10-round-taffy-layout.md)。

## 本轮为什么做这些

taffy 轮之后盒级几何已不是短板。对真实页面渲染质量的剩余差距里，**行内与文本侧**集中了
密度最高的一批缺口（均已实测核实）：

- **`text-align` 未消费**：css 层已解析（value.rs `TextAlignValue`），但 inline 行盒与
  taffy 映射都不消费——行内居中/右对齐在真实页面无处不在，是当前最刺眼的呈现偏差。
- **`font` 简写未解析**：`font: 12px/1.5 serif` 这类一行简写在真实 CSS 里占比极高，
  现在整条声明被丢弃后退 UA 默认。
- **视口单位未实现**：`vw`/`vh` 完全没有（value 层无该词法），`50vw` 直接解析失败。
- **CJK 缺字**：默认字体栈无 CJK 字形时整段豆腐块（text 层无按脚本的字体回退）。
- **flex baseline 退化**：taffy run 叶 measure 返回 `Baselines::NONE`，
  `align-items: baseline` 退化为顶对齐（block.rs 差异清单第 6 条）。

为什么这轮优先于其他候选（见文末「方向评估」）：单点成本小、几乎全部可无头单测、
且是「页面文本选择与复制」（AGENTS.md 已列另轮立项）的前置卫生——text-align 改变行盒
几何、TextRun 携带原文也在这几层，先做避免选择轮返工。

预期收益：真实页面（含 CJK 内容）的行内呈现质量显著提升；盒级完备度数字不变，
新增行内/文本侧能力清单。

## 任务清单与退出条件

### T1 nexty-css：`font` 简写解析与展开
- 按 CSS Fonts §3.3 简写语法展开：`<font-style> || <font-variant-css2> || <font-weight>
  || <font-width>? <font-size> [ / <line-height> ]? <font-family>#`（normal 关键字占位
  吞掉；已有 font-style/weight/size/line-height/family 逐属性解析，只补简写展开与
  `/` 行高形式）。
- 系统关键字（`caption`/`icon`/`menu` 等）按偏差记录降级为整条声明无效（不走 initial
  重置，简写语义 §3.3 要求重置全部子属性——解析失败整条丢弃，与现有错误恢复一致）。
- `font: menu` 等与 UA 设置相关的系统字体明确不支持，记入偏差清单。
- 退出条件：单测——`font: italic bold 12px/1.5 "Helvetica Neue", serif` 各子属性展开
  正确；`font: 12px serif`（无行高）、`font: normal normal 400 1em sans-serif`（normal
  占位）通过；非法值（缺 size/family、只有关键字）整条丢弃。

### T2 nexty-css + chrome：视口单位 `vw` / `vh`（含 `vmin` / `vmax`）
- value 层新增视口单位词法与 `LengthValue::Viewport(f32, ViewportUnit)`；
  cascade 的 computed value 阶段按**传入的视口尺寸**解析为 px——`cascade` 入参增加
  视口尺寸（`viewport: (f32, f32)` 或现有上下文结构），chrome pipeline 以窗口内容区
  实际尺寸传入；`1v* = 视口对应边的 1%`（CSS Values §6.2）。
- style_map 无需改动（cascade 出来的已是 px）；`@media` 视口条件求值沿用现有视口入参。
- 退出条件：单测——视口 800×600 时 `50vw`=400px、`10vh`=60px、`1vmin`=6px、`1vmax`=8px；
  chrome 接线实机验收（resize 后重排取新视口）。

### T3 nexty-layout：inline 行盒 `text-align` 消费
- `inline.rs` 行盒放置支持 left / center / right：行内容整体相对 content box 平移
  （center 偏移 `(content_width - line_width)/2`，right 偏移全额）；多 run 行、
  含行内原子盒的行同样适用（平移整个行盒，不改 run 内部间距）。
- `justify` 按偏差记录降级为 left（两端对齐需逐空格伸缩，另轮评估）；
  taffy 映射不引入对齐字段（行内对齐永远是自研 inline 的事，style_map 偏差清单同步）。
- 退出条件：单测——单 run / 多 run / 含图片的行在 center 与 right 下 x 坐标正确；
  行宽超容器（可断行贪心已换行）时退化为 left；`text-align` 继承生效。

### T4 nexty-text：按脚本字体回退（CJK 缺字修复）
- 现状：FontResolver 按 CSS 族列表在 fontique 系统查询取字体，族内无该码点字形时产出
  notdef（豆腐块）。补齐：主族覆盖不了码点时，按字符脚本（fontique `Query` 支持按
  script 过滤）继续查询系统回退字体——Windows 上 CJK 应命中 Microsoft YaHei 等系统
  字体；回退结果按码点区间分段 shaping（同 run 内混排中西文各用各自字体）。
- 回退字体不入用户可见族列表缓存污染；每 run 仍一次 harfrust 整形。
- 若 fontique API 不支持粒度（如无法按 script 查询），如实记录偏差与替代方案后停在
  最小可行（允许降级为「探测常用 CJK 族名」方案，但先查 fontique 能力再定，不许猜）。
- 退出条件：单测——纯中文文本整形结果无 notdef（Windows 实机系统字体）；中西文混排
  run 分段正确、各自字形非 notdef；纯拉丁文本路径行为不变（现有测试全绿）。

### T5 nexty-layout：taffy run 叶上报 baseline
- 匿名块 run 叶的 measure function 目前返回 `Baselines::NONE`；改为上报**首行 baseline**
  （`inline::build_lines` 已算出行盒 baseline 相对行顶的距离，随 measure 上下文带出），
  使 flex `align-items: baseline` 与 grid baseline 对齐生效。
- 只报第一行（多行叶的 baseline 组语义 §8.5 另轮评估，记入偏差清单）。
- 退出条件：单测——两个文本 run 叶在 `align-items: baseline` 的 flex 容器中首行基线
  对齐（不同 font-size 下 y 差值等于 ascent 差）；无文本容器不 panic。

### T6 门禁与收尾
- `cargo check --workspace` 零 warning；`cargo test --workspace` 全绿；clippy
  `-D warnings` 零告警；`cargo fmt --all -- --check` 通过；改动 feature 组合
  （css 无 `values`、layout 无 `inline` 等）零 warning 编译。
- AGENTS.md 同步：nexty-css 属性清单加 `font` 简写与视口单位；nexty-text 加按脚本
  回退；nexty-layout 偏差清单划掉 baseline 退化与 text-align 缺失、保留 justify/UAX#14；
  feature 映射表无新增模块则不动。
- 记忆修正：「paint 层无 background-color」条目过期——`pipeline.rs` 自首版即按
  fragment 填充 `background_color`，真实缺口是根元素/画布背景传播（CSS §14）与渐变，
  改写该条避免重复排查。
- 分任务 commit，最终 push。

## 非目标（本轮不做）
- 页面文本选择与复制（跨 text/layout/chrome 三层，**下轮候选**，AGENTS.md 已立项）。
- float 布局与 table（taffy 不覆盖，自研成本高，单独评估）。
- `text-align: justify` 两端对齐、UAX#14 完整断行（维持 inline.rs 偏差清单）。
- `text-decoration`、`letter-spacing`、`white-space` 非 normal 取值。
- 根元素/画布背景传播（CSS §14）、渐变、圆角、阴影（paint 指令集另轮）。
- vello_hybrid GPU 后端、`position: fixed` 视口锚定。
- `font` 系统关键字字体、`font-variant` 完整展开、可变字体轴。
- JS 引擎、表单、多标签页、网络缓存。

## 方向评估（为什么是这轮）

| 候选 | 价值 | 成本 | 结论 |
| --- | --- | --- | --- |
| **行内/文本补齐（本轮）** | 真实页面行内呈现质量，CJK 可读性 | 小–中，几乎全可无头单测 | ✅ 本轮 |
| 页面文本选择与复制 | 「像浏览器」的关键交互里程碑 | 中–大，跨三层，依赖本轮的行几何稳定 | 下轮首选 |
| float / table 布局 | 老页面与邮件类内容 | 大（taffy 不覆盖，自研违「不造轮子」需 ADR 论证无轮可用） | 暂缓 |
| paint 圆角/渐变 + GPU 后端 | 视觉上限，但底层数量级低于行内缺口 | 中（GPU 有 winit surface 接缝工作） | 暂缓 |

## 验证流（AGENTS.md Verification Flow）
每层：cargo check 零 warning → cargo test 全绿 → fmt/clippy → commit。
T2 的 chrome 侧 resize 接线、T4 的 Windows 实机字形为无头不可测项，列入实机验收；
其余全部单测覆盖。修 bug 类（CJK notdef）先写 failing test 再修。
