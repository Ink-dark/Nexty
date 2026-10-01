# goal.md — 本轮任务：落地 nexty-css

日期：2026-10-01。前置：nexty-dom / nexty-html 已落地；本轮按管线顺序落地 CSS 层
（cssparser + selectors 封装 + 自研 cascade，选型见
docs/decisions/2026-10-01-crate-selection.md）。

## 任务清单与退出条件

### T1 stylesheet 解析
- 任务：`Stylesheet::parse`。样式规则 = 选择器列表 prelude + 声明块；声明级错误
  单条恢复（CSS Syntax §5.5）；选择器列表含无效选择器时整条规则丢弃
  （CSS Syntax §5.3.2 qualified rule 的 invalid 处理）；未知 @ 规则整块跳过
  （CSS Syntax §5.4 error recovery）；`!important` 解析（CSS Cascading §5.1）。
- 退出条件：上述行为各有单测；解析结果保持源顺序。

### T2 选择器匹配
- 任务：为 arena DOM 实现 `selectors` crate 的 `Element` trait；提供
  `parse_selector_list` 与 `matches_selector` 公共 API。HTML 元素的类型选择器
  按规范 ASCII case-insensitive 匹配（Selectors §3.1.3 case-sensitivity）。
- 退出条件：type/class/id/attribute 选择器、后代/子/兄弟组合器、`:first-child`
  等结构伪类、`:is()`/`:not()` 各有匹配单测。

### T3 自研 cascade
- 任务：对元素收集匹配声明，按 CSS Cascade 5 §6 排序：origin+importance →
  特异度 → 源顺序，同属性取最后胜者；内联 style 属性按 author origin 参与，
  normal 内联声明排在样式表 normal 声明之后（CSS Cascade 5 §6.4 style
  attribute 的排序位置）。
- 退出条件：origin/importance 分桶、特异度决胜、源顺序决胜、内联样式排序
  各有单测。

### T4 computed style（最小属性集）
- 任务：级联值 → 继承 → initial（CSS Cascading §4.3/§4.4）。属性表只收当前
  管线需要的最小集合（display/color/background-color/font-size/font-weight/
  font-style/font-family/text-align 等），inherited 标志与 initial 值逐条引用
  各属性规范；不扩充到布局属性。
- 退出条件：inherited 属性无级联值时取父值，根取 initial；非 inherited 取
  initial；各有单测。

### T5 门禁与收尾
- 任务 + 退出条件：
  - `cargo check --workspace` 零 warning
  - `cargo test --workspace` 全绿
  - `cargo clippy --workspace --all-targets -- -D warnings` 零告警
  - `cargo fmt --all -- --check` 通过
  - `cargo deny check` 通过（本轮不引新依赖，仍作为门禁复跑）
  - 覆盖率：本机无 tarpaulin/llvm-cov（Windows），以「每个公共 API 的
    success + error 路径、每条规范行为分支有单测」替代 ≥80% 判据，测试代码
    行数 ≥ 实现代码行数作为自检
  - nexty-css `Cargo.toml` 版本显式化为 0.1.1（骨架 0.1.0 → 落地 +1 patch，
    对齐 nexty-dom / nexty-html 的先例）
  - AGENTS.md「当前状态」更新
  - 分任务 commit；最终 push

## 非目标（本轮不做）
- `var()`/custom properties、`@media`/`@import`/`@font-face` 的求值（未知
  @ 规则按语法跳过）
- 伪元素、浏览器前缀
- 布局属性（margin/padding/width…）进入 computed 属性表——留给 layout 轮
- WPT CSS 语料钉版——待 css 层行为面扩大后由架构师组织比对

## 验证流（AGENTS.md Verification Flow）
每完成一个任务：cargo check 零 warning → cargo test 全绿 → fmt/clippy →
commit。本轮 WPT 语义比对以规范引用单测替代（比对范围见非目标），架构师
复核后允许 push。
