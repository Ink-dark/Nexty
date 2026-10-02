# Nexty 代码审计报告

- 日期：2026-10-02
- 基线：HEAD `b2e8e9b`（工作区除本地工具产物外干净）
- 性质：**只读审计**，未改动任何源码；行号均以当前工作树为准
- 范围：全部 8 个 facade crate（24 个 `.rs` 源文件 + 各 `Cargo.toml` + 根配置）

## 1. 审计方法

1. **机械检查**（全库 grep）：`unsafe`、`#![forbid(unsafe_code)]`、`panic =` / `[profile]`、`todo!`/`unimplemented!`/`FIXME`、`pub use` re-export。
2. **逐 crate API 审读**：枚举每个 crate 的全部 pub 项，逐项核对 doc comment、规范引用、签名中是否出现外部依赖类型（html5ever / cssparser / selectors / precomputed-hash / parley / skrifa / vello_cpu / image / reqwest / winit / wgpu / pollster）。同仓库 `nexty-*` 互引不算外泄。
3. **门禁复跑**：check / clippy(-D warnings) / test / fmt / deny 全套实跑取证（见 §3）。

## 2. 总体结论

**Hard Rules 规则层基本零违例**：unsafe 禁令、依赖类型封装、层隔离、doc comment、许可证门禁全部达标。架构上的 trait 接缝（`NetworkFetcher` / `TextShaper` / `Rasterizer`）真实成立，外部类型被自有类型完整包住。

发现 **2 项中等风险**（均在 nexty-chrome 运行时路径，会削弱「panic 不拖垮主进程」的实际成效）与 **1 项版本纪律违例**（nexty-network），其余为低风险改进项与文档滞后。

| crate | 版本 | 依赖类型外泄 | doc 覆盖 | 测试 | 风险 |
| --- | --- | --- | --- | --- | --- |
| nexty-dom | 0.1.1 | 无 | 全覆盖 | 26 单测 | 低 |
| nexty-html | 0.1.1 | 无 | 全覆盖 | 15 单测 + WPT 1 | 低 |
| nexty-css | 0.1.3 | 无 | 全覆盖 | 55 单测 | 低 |
| nexty-layout | 0.1.1 | 无外部依赖 | 全覆盖 | 18 集成 | 低 |
| nexty-text | 0.1.3 | 无 | 全覆盖 | 11 单测 | 低 |
| nexty-paint | 0.1.2 | 无 | 全覆盖 | 26 单测 | 低 |
| nexty-network | 0.1.1 | 无 | 全覆盖 | 10 单测 + doctest 1 | 低 |
| nexty-chrome | 0.1.1 | 无 | 全覆盖 | 6 单测 + 6 集成 | **中** |

## 3. 门禁状态（本次实跑结果）

| 门禁 | 结果 |
| --- | --- |
| `cargo check --workspace` | 通过，无代码 warning |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过（exit 0） |
| `cargo test --workspace` | 通过（exit 0），**175/175** |
| `cargo fmt --all -- --check` | 通过（无 diff） |
| `cargo deny check` | advisories / bans / licenses / sources 全 ok |

测试分布：dom 26、html 15 + WPT tree-construction 1（语料钉 `5cd8e3fa`，1854/1959 通过、105 条基线内）、css 55、layout 18（全部集成测试）、text 11、paint 26、network 10 + doctest 1、chrome 6 单测 + 6 集成。

构建噪音备注：日志中出现 incremental 目录「os error 5 拒绝访问」与 MSVC 链接器 stdout 提示（`linker_messages`），均为 Windows 环境噪音，非代码问题，不影响门禁判定。

## 4. Hard Rules 合规矩阵

| 规则 | 结果 | 证据 |
| --- | --- | --- |
| 严禁 unsafe | ✅ | 8/8 个 `lib.rs` 顶部 `#![forbid(unsafe_code)]`；全库 `unsafe` 出现 **0** 次 |
| 禁 `panic = "abort"` | ✅ | workspace 与 8 个 crate `Cargo.toml` 均未设置 profile（根仅 `[profile.release] strip = true`） |
| 依赖类型不得外泄 | ✅ | 8 crate 逐项核查，**0 处**外泄（关键封装点见 §6） |
| 层隔离 | ✅ | 外部依赖仅存在于对应 facade crate；layout 无外部依赖 |
| pub 项必须有 doc | ✅ | 全部 pub 项有 doc；变更算法类方法均链接规范锚点（少数平凡构造器如 `Rgba::opaque` 未挂条款，可接受） |
| 许可证门禁 | ✅ | `cargo deny` 全绿；唯一放行 `RUSTSEC-2026-0192`（ttf-parser unmaintained，非漏洞，理由记录在 [deny.toml](../deny.toml)） |
| 版本纪律 | ⚠ | 1 处违例，见发现 L1 |
| 每个新依赖记 ADR | ⚠ | `image`、`skrifa`、`pollster` 无 ADR 记录，见发现 D2 |
| todo!/unimplemented!/FIXME | ✅ | 全库 0 处（chrome 测试内 1 处 `unreachable!`，无害） |

## 5. 发现清单

### 中风险（建议优先处理）

**M1 · chrome 主线程 `expect` 链绕过渲染隔离**
- [ui.rs:147](../crates/nexty-chrome/src/ui.rs#L147)：draw 路径 `shaper.shape(...).expect(...)`，经 [app.rs:247](../crates/nexty-chrome/src/app.rs#L247) 在**主线程**执行，panic 直接崩溃进程；渲染线程的 `catch_unwind` 兜底（[render.rs:66-70](../crates/nexty-chrome/src/render.rs#L66-L70)，实测覆盖完整、panic 后线程存活）救不到这里。
- 同类：[app.rs:128](../crates/nexty-chrome/src/app.rs#L128)（fetcher 构造 expect）、[app.rs:193](../crates/nexty-chrome/src/app.rs#L193)（fetch 线程 spawn expect），属启动路径，风险较低。
- 方向（记录不改）：draw 路径把失败降级为「跳过绘制/空 UI」，启动路径可保留 fail-fast。

**M2 · `bytes_per_row` 不满足 WebGPU 256 字节对齐**
- [app.rs:497](../crates/nexty-chrome/src/app.rs#L497)：`bytes_per_row: Some(4 * width)` 直接写纹理。WebGPU 要求 `bytes_per_row` 为 256 的倍数；默认窗口宽 960 时 4×960=3840=256×15 **恰好通过**，但窗口宽度非 64 的倍数时（如 1000 → 4000）`write_texture` 校验失败将 panic 主进程。
- 方向：行尾 padding 到 256 对齐（staging buffer / 逐行拷贝）。

### 低风险 / 纪律项

**L1 · network 版本纪律违例**：提交 `44d1549` 向 [network/src/lib.rs](../crates/nexty-network/src/lib.rs) 新增 117 行 base-relative URL `resolve` 特性，commit message 声称 `(0.1.2)`，但 [network/Cargo.toml](../crates/nexty-network/Cargo.toml) **未被修改**，版本仍是 0.1.1。按 Versioning Discipline 应 bump 至 0.1.2。

**L2 · skrifa 应为 dev-dependency**：[text/Cargo.toml:13-14](../crates/nexty-text/Cargo.toml#L13-L14) 将 skrifa 列为正式依赖，但全 crate 仅 [lib.rs:498](../crates/nexty-text/src/lib.rs#L498) 的 `#[cfg(test)]` 测试使用。skrifa 已是 parley 传递依赖，降级到 `[dev-dependencies]` 零成本。

**L3 · dom 免校验写入路径为 pub**：[tree.rs:168](../crates/nexty-dom/src/tree.rs#L168) `insert_node` / [tree.rs:186](../crates/nexty-dom/src/tree.rs#L186) `remove_node` doc 声明为解析器专用，但外部 crate 同样可调用，可构造「Document 下多元素」等非法树。后续可收敛 `pub(crate)` 或引入 sealed 访问。

**L4 · 数据 struct 的 pub 字段可绕过派生不变量**：如 [ComputedStyle](../crates/nexty-css/src/cascade.rs#L31)（16 个 pub 字段）、layout 的 [Fragment/Rect/Edges](../crates/nexty-layout/src/fragment.rs#L14)、dom 的 `ElementData`/`Attribute`。外部可自由构造出违反不变量的值（border_width 未随 none/hidden 归零、Fragment 坐标系约定等）。当前消费方仅仓库内部，实际风险低；对外发布 API 前需收紧。

**L5 · paint 的 gif/webp 解码无测试**：`image` feature 显式开启了 gif/webp（[paint/Cargo.toml:19-23](../crates/nexty-paint/Cargo.toml#L19-L23)，feature 配置正确、成功排除 AVIF/rayon），但 26 个测试只覆盖 png/jpeg 解码路径。

**L6 · layout 源码无单元测试**：[nexty-layout](../crates/nexty-layout/src) `src` 内 0 个 `#[test]`，全靠 [tests/layout.rs](../crates/nexty-layout/tests/layout.rs) 的 18 个集成测试兜底；block/inline 内部函数（margin 折叠分支、断行边界）缺细粒度覆盖。

**L7 · 字体解析重复全量发现**：[text/lib.rs:258-260](../crates/nexty-text/src/lib.rs#L258-L260) `ParleyTextShaper::resolve_font` 每次新建 `FontResolver`（全量系统字体发现）；[paint/lib.rs:306](../crates/nexty-paint/src/lib.rs#L306) `font_for` 缓存未命中时同样新建。缓存键为族列表拼接串，可接受但不理想。

**L8 · paint 文档与错误语义小疵**：[paint/lib.rs:279-280](../crates/nexty-paint/src/lib.rs#L279-L280) doc 首行重复；模块 doc 中 vello_cpu 介绍段重复（L3-6 与 L11）；[RasterError::SizeExceedsLimit](../crates/nexty-paint/src/lib.rs#L270-L273) 把 vello_cpu 的 u16 尺寸上限写进了双后端共享错误语义，GPU 后端（vello_hybrid）接入时需重述。

**L9 · 地址栏点击聚焦未查右边界**：[ui.rs:96](../crates/nexty-chrome/src/ui.rs#L96) 只判 `x > INSET`，点击超出地址栏右端也会聚焦。

### 文档与仓库卫生

**D1 · AGENTS.md 滞后**：「已落地八层」描述未反映近期变更：paint 的图片解码（`Image`/`DrawImage`，`41b1b7d`/`b2e8e9b`）、network 的 base-relative URL resolve（`44d1549`）、GitHub Actions CI（`8141f8b`/`3761e84`）。

**D2 · 新依赖缺 ADR**：Hard Rules 要求「每个新依赖在 `docs/decisions/` 记一条 ADR」。`image`（paint 0.1.2）、`pollster`（chrome）、`skrifa`（text）在 [2026-10-01-crate-selection.md](decisions/2026-10-01-crate-selection.md) 中均无对应条目（`deny.toml` 注释仅提及 skrifa 是 winit 中 ttf-parser 的替代品）。

**H1 · 未跟踪目录**：`.mimosa/`、`.workbuddy/`、`.zcodeignore` 为本地工具产物，建议加入 `.gitignore` 或清理。

## 6. 依赖类型封装核查要点（正面确认）

以下封装点经逐项核查成立，是各 crate 隔离的范本：

- **nexty-html**：`pub use` 零 re-export；html5ever 的 `TreeSink`/`QualName`/`HtmlAttribute` 等全部止步于私有 `HtmlTreeSink`；`type Handle = NodeId` 用自有句柄；公开 API 仅 2 函数 + 2 结构体。
- **nexty-css**：selectors/cssparser 类型（`NextySelector`、`NextySelectorImpl`、`Atom`、`DomElement`）全部 `pub(crate)`；匹配接口对外只收 `&str` 返回 `Result<(), SelectorError>`，未泄 `Element` trait；[test_support.rs](../crates/nexty-css/src/test_support.rs) 为 `#[cfg(test)]` 模块，不进生产构建。
- **nexty-text**：`FontResolver.context: Mutex<parley::FontContext>` 为私有字段并自定义 Debug；`ResolvedFont` 字节私有 + 访问器；`build_layout` 返回的 `parley::Layout` 封闭在内部。
- **nexty-paint**：vello_cpu 的 `RenderContext`/`peniko`/`kurbo` 类型全部封闭在 impl 与私有函数内；`Pixmap`/`Image` 为自有类型（非预乘 RGBA8 契约）；`Rasterizer` trait 签名 100% 自有类型，双后端接缝真实后端无关。
- **nexty-network**：reqwest client 存于 `ReqwestFetcher` 私有字段；`NetworkFetcher::fetch` 返回自有 `Response`/`NetworkError`；InvalidUrl/Timeout/Transport 三错误路径均有离线测试。
- **nexty-chrome**：winit/wgpu/pollster 共 89 处引用全部封闭在私有模块 [app.rs](../crates/nexty-chrome/src/app.rs)；lib 公共项（pipeline/render/ui）只暴露同仓库类型。
- **nexty-dom**：`NodeId(pub(crate) usize)` 外部不可伪造；`Document` 字段私有，不变量（单元素、层级合法）由变更算法集中守护。

## 7. 局限性

- chrome 的窗口/GPU 路径按 AGENTS.md 约定无法在无头环境自动化验证，M1/M2 为静态代码审查结论，未实机复现。
- 测试「覆盖率 ≥ 80%」未做工具级度量（如 tarpaulin/llvm-cov），仅按用例分布与模块对称性做了定性评估。
- WPT 语料比对基线（105 条）沿用既有结论，本次复跑确认 harness 通过，未重新逐条裁决。

## 8. 建议处理顺序

1. M2（一行级修复：256 对齐 padding）→ M1（draw 路径去 expect）——两者都直接关系「渲染失败不拖垮主进程」的底线。
2. L1 版本补 bump + L2 skrifa 降级 dev-dep + D2 补 ADR——纯纪律项，一次提交可清完。
3. D1 更新 AGENTS.md、H1 清理未跟踪目录。
4. L3–L9 按后续迭代顺带处理，无需专项投入。
