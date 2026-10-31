# 批次 1 并行轮执行报告

日期：2026-10-31。任务定义：[2026-10-31-round-batch1-parallel.md](2026-10-31-round-batch1-parallel.md)
（批次 1：K1 ∥ K2 ∥ K3 ∥ K4）。执行方式：Agent 集群——6 个子代理并行
（crate 泳道互不相交），chrome 触碰任务按串行链 K1 T3 → K2 T3 → K3 T5 →
K4 T3 推进，编排者负责收口、修复与提交。

## 一、结果总览

| 泳道 | 任务 | 状态 | 版本 | 证据锚点 |
| --- | --- | --- | --- | --- |
| A (K1) | T1+T2 dom 事件模型 | ✅ | dom 0.1.4 | 46 测试全绿（12 新增）；API 清单见执行留档 |
| A (K1) | T3 chrome 接线 | ✅ | chrome 0.1.26 | 93 测试；DCL/load 生命周期集成测试 3 条 |
| B (K2) | T0 vello_cpu 侦察 | ✅ | — | `.workbuddy/t0-k2-recon/T0-report.md`（三项原生支持） |
| B (K2) | T1 css 新属性 | ✅ | css 0.1.16 | 101 测试（15 新增）、6 组 feature 配置 |
| B (K2) | T2 paint 指令 | ✅ | paint 0.1.8 | 像素级测试；translated 三指令平移钉住 |
| B (K2) | T3 pipeline 显示列表 | ✅ | chrome（合流） | 渐变端点/圆角/z-index 定序/背景传播单测 + 对照页 |
| B (K2) | UA 链接下划线 | ✅ | css 0.1.18 | UA 表 `a{text-decoration:underline}` + 镜像单测 |
| D (K3) | T1–T4 network | ✅ | network 0.1.6 | 50 lib 测试 + 14 组 feature 矩阵；两份 ADR |
| D (K3) | T5 chrome 导航接线 | ✅ | chrome 0.1.28 | GBK 解码/缓存命中/落点 base/规范化 8 条新测试 |
| E (K4) | T1 css @font-face | ✅ | css 0.1.17 | 115 测试（14 新增）、8 组 feature 配置 |
| E (K4) | T2 text 字体注册表 | ✅ | text 0.1.9 | 22 测试（5 新增）；SpaceMono（OFL）入库 |
| E (K4) | T3 chrome 字体编排 | ✅ | chrome 0.1.29 | 121 测试（6 新增）、11 组 feature 配置 |

版本落点：dom **0.1.4** / css **0.1.18** / network **0.1.6** / text **0.1.9** /
paint **0.1.8** / chrome **0.1.29**（layout 0.1.29 为 float 轮既有）。

## 二、批次门禁（6 项）

1. `cargo check --workspace` ✅（3.2s，零 warning）
2. `cargo clippy --workspace --all-targets -- -D warnings` ✅（零告警）
3. `cargo fmt --all -- --check` ✅（零 diff）
4. `cargo test --workspace` ✅（exit 0；chrome 121 / css 116 / dom 46 /
   network 50 / text 23+ / paint 26+ 全矩阵）
5. `cargo deny check` ✅ + `cargo about` 双产出 + 注入脚本 ✅（NOTICES 8 项
   / MULTI 382 项；url 2.5.8 与 encoding_rs 0.8.42 均锁内已有、白名单内）
6. **bench 回归 ✅（部分运行，用户裁定提前结束）**：text-heavy 段全量
   数据无回退——parse 18.3（基线 24.3–28.3）/ cascade 39.9（44.3–88.8）/
   layout **557.6**（856–922，改善 ~35%）/ scene 8.8（噪声内）；调用计数
   全部一致（shape 1.00×、metrics 10、resize runs 5、metrics 50=5×10、
   shape 5.0×=5×1.0×）——K1–K4 改动未触碰热路径（bench 语料无圆角/渐变/
   装饰/定位元素，emit 新路径不在语料上生效，符合预期）。rules-heavy /
   image-heavy 与滚动风暴按用户指示跳过（滚动 rasterize 维持 10–16s/帧
   本属预期，视口裁剪属 K9）。

## 三、执行事件与编排者修复记录

1. **K2 T3 子代理被框架挂起中断在收尾**（代码主体已在，回执丢失）：编排者
   复盘后补三件——①渐变四正向（to top/right/bottom/left）改精确轴向量
   （f32 三角函数 90° 噪声 ~4e-6，最常用方向端点严格确定）；②选区高亮
   测试载体迁移（body 白底已按 §14.2 传播到画布，白底挪入包裹 div，测试
   意图不变）；③clippy 残留 2 类（`too_many_arguments` 针对性 allow +
   match guard 改 f32 字面量模式）与 fmt 收口。
2. **架构决策（K1 衍生）**：dom 0.1.4 起 `Document` 携带监听器注册表
   （`Box<dyn EventListener>`，!Send），chrome 页面传输改为「后台线程只产
   出 Send 数据面（HTML + CSS 字节 + 已解码图片），正式 Document 主线程
   构建」——每次导航两次解析是常态成本。**K5（JSRT）前是评估「注册表移出
   Document」恢复 Send 的最低成本窗口**，建议届时决策。
3. K3 T5 期间 chrome 的 5 处 clippy 报错/ fmt diff 经核实全部为 K2 T3
   中断残留（K3 T5 正确地未顺手修改跨泳道文件）。

## 四、实机验收清单（架构师执行，无头不可自动化）

1. `.workbuddy/t-k2/k2-visual.html`——五组：圆角卡片 / 渐变横幅 / UA 链接
   下划线 / z-index 叠放 / body 背景传播（滚动一致）。
2. GBK 测试页（K3）：`Content-Type: text/html; charset=gbk` 正确渲染，
   不再进错误页。
3. 自托管 ttf 字体页（K4）：`@font-face` 字形与系统回退可区分；404 字体
   回退正常；二次导航不重复抓取。
4. 事件行为回归（K1）：链接点击 / 滚轮 / 键盘滚动与快捷键 / 选区复制
   手感与改造前一致。
5. 二次导航缓存命中（K3）：同 URL 第二次导航瞬时（HTTP 缓存）。

## 五、偏差与决策点汇总

- 各 crate 模块文档偏差清单已随轮登记（css：text-decoration 单关键字 /
  仅 solid / 椭圆语法 / 多重背景；text：woff2 / unicode-range；network：
  meta 预扫简化档；chrome：样式表 lossy、快捷键可拦截、两次解析、
  host:port 简写不支持、screenX 置 0）。
- 待决策：①`Document` !Send 与注册表外置（K5 窗口）；②UA `:link`/`:visited`
  以 `a` 近似（伪类轮收口）。
- X 批次（CSS 补全 / WPT 合规 / 排版细节）按路线图殿后，见
  [kernel-roadmap](2026-10-31-kernel-roadmap.md)。

## 六、提交记录（fix/justify-flush 分支，待 push）

```
969b1fc [workspace] batch-1 round: AGENTS registration, roadmap + round goal, deps manifest regen
26c4c46 [chrome] K1 event wiring + K2 drawing emit + K3 navigation orchestration + K4 font orchestration (0.1.29)
866ee39 [paint] FillRoundedRect/StrokeRoundedRect/FillGradient commands (0.1.8)
b07e19e [text] session FontRegistry delegating @font-face matching to fontique (0.1.9)
80d4e86 [network] url direct dep, encoding detection, redirects + final_url, HTTP cache (0.1.6)
1f8919d [css] K2 drawing properties + K4 @font-face + UA link underline (0.1.18)
ec5ea19 [dom] add K1 event model (0.1.4)
```
