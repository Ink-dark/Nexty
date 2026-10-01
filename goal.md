# goal.md — 本轮任务：落地 nexty-layout（自研盒级布局）

日期：2026-10-01。前置：dom / html / css / network / text / paint 已落地。
layout 是自研层（选型 ADR 例外三条之一）；本轮同时把 layout 需要而 css 层尚缺的
盒模型属性补齐，并给 text 层补字体度量 API。

## 范围裁定（对照 CSS 2.1 / CSS Display）

本轮实现**普通流（normal flow）的块级 + 行内布局**：

- 盒模型：content + padding + border + margin（content-box，无 box-sizing）
- 宽度解析：§10.3.3（块级非替换：auto/长度/百分比；ltr 过约束规则）
- 高度：§10.6.3（auto = 内容高；指定高度直接生效，溢出不裁剪）
- margin 折叠：§8.3.1（相邻兄弟、父子（首/末子）、空块自折叠、正负混合取值）
- 匿名块盒：§9.2.1.1（连续行内级内容归入匿名块）
- 行内格式化：§10.8 行盒与 strut；行内元素样式随行（`<b>` 加粗等）；
  断行基于空白（ASCII 空白折叠 + 行首去空白 + 行尾悬挂）
- display：none（子树不参与）、block、inline；**inline-block 本轮不做**（按
  偏差记录，后续轮次补 shrink-to-fit 与原子行内盒）

非目标：float、绝对/相对定位、flex/grid、表格、min/max-width、box-sizing、
overflow、竖排/bidi、UAX#14 完整断行（CJK 等）。

## 任务清单与退出条件

### T1 nexty-css：布局属性扩展（bump 0.1.2）
- 属性：margin-{top,right,bottom,left}（可负、可 auto/百分比）、
  padding-{...}（禁 auto/负值）、border-{...}-{width,style,color}、
  width/height（auto/长度/百分比）、line-height（normal/数值/长度/百分比；
  数值按 CSS 2.1 语义以数值继承）。
- 简写展开：margin、padding、border、border-width、border-style、border-color
  （1–4 值顺时针；border 按规范 `||` 语法解析，缺省部分取 initial；
  CSS-wide 关键字作用于全部 longhand）。
- computed：border-width 在 style 为 none/hidden 时为 0；border-color initial
  = currentcolor（按本元素 color 解析）；line-height 百分比 → 绝对值，
  数值保持数值继承。
- 退出条件：上述每条解析/计算行为有单测（含负 margin 合法、padding 负值
  无效、宽度负值无效、简写各位置展开正确）。

### T2 nexty-text：字体度量（bump 0.1.2）
- `FontMetrics { ascent, descent, line_height }` + `TextShaper::metrics`，
  供 layout 计算 strut。
- 退出条件：metrics 单测（正值、随字号缩放、空族列表报错）。

### T3 nexty-layout：盒级布局（bump 0.1.1）
- API：`layout_document(document, styles, viewport_width) -> Fragment` 片段树；
  片段携带 border-box、padding/border 宽度、computed style、子块片段与
  文本行（行内每个 run 带自身 TextStyle 与字形）。
- 退出条件（逐条单测）：
  1. 块级堆叠：子块依次纵向排列，宽度 auto = 包含块内容宽
  2. 宽度解析：指定宽度、百分比宽度、auto margin = 0、过约束时 ltr 忽略
     margin-right
  3. padding/border/margin 定位：内容盒偏移正确
  4. margin 折叠：相邻兄弟取正值最大；正负混合 = max(pos)+min(neg)；
     父子折叠（无 border/padding 隔断）；空块自折叠穿透
  5. display:none 子树无片段
  6. 匿名块分组：块级与行内兄弟混排时行内内容归入匿名块
  7. 断行：超宽文本按空白折行；行首无空白；行尾空白悬挂不计宽
  8. 行盒：strut 生效（line-height 决定行高下限）；行内元素样式进入对应 run
  9. 百分比 margin/padding/width 相对包含块宽度解析

### T4 门禁与收尾
- `cargo check --workspace` 零 warning；`cargo test --workspace` 全绿；
  clippy `-D warnings` 零告警；`cargo fmt --all -- --check` 通过；
  `cargo deny check` 通过（本轮无新依赖）。
- AGENTS.md「当前状态」与选型表状态列更新。
- 分任务 commit（css 扩展 / text 度量 / layout 落地 / 文档），最终 push。

## 已知偏差（进模块文档）
- inline-block、float、定位、表格、min/max 尺寸、box-sizing、overflow 未实现
- 断行仅在空白处（无 UAX#14 完整断点、无连字符；CJK 不折行）
- 行内垂直对齐仅 baseline（vertical-align 其他取值未实现）
- 匿名块不含「连续匿名行内盒之间插入块级」之外的边角（§9.2.1.1 全量规则
  待 WPT 对齐时补）
