# 2026-10-25 性能轮基线：管线基准与热点归属

本轮任务见 [goal.md](../goal.md)（性能轮：H4 / M1 / M5 / M6 / M7 / M10）。
本文档是 T1 的产出：基准的构造、机器环境、各场景基线数字、优化后数字与
热点归属结论。基准代码在 `crates/nexty-chrome/benches/pipeline.rs`（共享
语料与计数器在 `src/bench.rs`）。

## 运行方式

```bash
BENCH_MODE=1 cargo bench -p nexty-chrome --bench pipeline
```

无 `BENCH_MODE=1` 时打印 skip 并退出（`cargo test` / `cargo clippy
--all-targets` 只编译不执行；门控语义由 `bench_enabled_from` 单测钉住）。
计时为 std::time + 预热 1 次 + 5 次取中位数；机器无关的断言用调用计数
（CountingShaper 的 shape / metrics 次数）与执行次数（emit / 管线运行数）。

**注意**：bench 是独立 PE，`nexty-chrome/build.rs` 的 `/STACK`（`rustc
-link-arg-bins`）只作用于 bin 目标——bench 正文跑在显式 32 MB 栈线程里
（布局递归在 1 MiB 默认栈上直接压爆进程，实测为「分配失败 abort」形态）。

## 语料（确定性生成，入库为 bench 代码）

| 语料 | 规模 | 特征 |
| --- | --- | --- |
| text-heavy | 556,902 B html / 2,000 段中英混排 / 8,001 文本 chunk | 每段行内样式 ×2（字号颜色 + 字距），整形放大倍数与 metrics 探针的目标场景 |
| rules-heavy | 716,607 B html + 337,295 B css / 10,000 元素 × 5,000 条类规则 | 无内联样式；M5 规则匹配占比的目标场景 |
| image-heavy | 728 B html / 24 张 320×200 生成 RGBA（≈6 MB 像素） | 块级与行内 `<img>` 各半；M1 像素拷贝的目标场景 |

视口 1000×640（与 app 默认窗口一致）。

## 机器环境

- Windows 10（10.0.19045）x64，16 GB 内存 / 32 GB 提交上限
- Rust stable x86_64-pc-windows-msvc（1.90+，workspace MSRV）
- 基准 profile：`bench`（继承 release，opt-level 3）

## 基线（优化前：css 0.1.13 / text 0.1.7 / layout 0.1.24 / chrome 0.1.21）

正式基线（下表）以 10,000 元素规则页记录；更早一轮 20,000 元素的观察
（cascade 26,369 ms）仅用于确认线性扫描量级，不入表。

| 场景 | text-heavy | rules-heavy | image-heavy |
| --- | --- | --- | --- |
| 首屏 parse（ms） | 21.4 | 93.2 | 0.1 |
| 首屏 cascade（ms） | 87.0 | **20,213.0** | 0.5 |
| 首屏 layout（ms） | 605.1 | 1,074.0 | 0.8 |
| 首屏 scene（ms） | 6.7 | 10.8 | 1.0 |
| 单次布局 shape 调用 / chunk | 8,000 / 8,001 = **1.00×** | 10,000 / 10,001 = 1.00× | 0.96× |
| 单次布局 metrics 调用 | **64,818** | 105,585 | 132 |
| resize 风暴（20 事件，逐事件管线） | 总 12,811 ms，中位 633.1 ms | 总 226,706 ms，中位 11,302.8 ms | 1.2 ms |
| 滚动风暴 emit（ms/帧） | 7.5 | 8.0 | 1.2 |
| 滚动风暴 rasterize（ms/帧） | **10,468.9** | 11,436.3 | 23.9 |

### 基线结论（热点归属）

1. **规则匹配主导级联（M5 ④ 分桶：数据强支持）**。rules-heavy 首屏
   cascade 20.2 s，占该页全管线的 ~93%；resize 风暴 95% 时间在级联。
   元素 × 规则线性扫描（10,000 × 5,000 = 5,000 万次选择器测试/次级联）
   是绝对热点。→ T6 分桶已落地。
2. **metrics 探针整形是 layout 的第二大热点（H4 metrics 缓存：数据支持）**。
   text-heavy 单次布局 64,818 次 metrics 探针（≈8.1 次/chunk——flush 期
   每放置项一次 + 每 run 节点 strut 一次），占布局时间可观份额。
   shape 已是 1.0×/chunk：普通块流每 run 叶只走一次定宽 measure，H4 文档
   预期的「2–3 档 measure 放大」只在 flex/grid/内在尺寸路径出现（本语料
   未覆盖），逐 chunk 缓存仍按 T2 落地（多档路径 + resize 语义保证）。
3. **滚动光栅化被「全页场景无裁剪」主导（记录为已知瓶颈，本轮不做）**。
   滚动风暴 rasterize 10.5 s/帧（text/rules 页）：片段树不做滚动裁剪，
   场景含整页全部字形，每帧光栅整页。M6 的场景缓存把 emit 降为 0，但
   光栅侧需要视口裁剪/增量光栅（goal.md 列为非目标的后续大项）。
4. **字体数据每整形全量复制是阻塞级缺陷（基准发现，不在计划内）**：
   `ParleyTextShaper::shape` 每次 `Arc::from(blob)` 复制整个字体文件
   （CJK 回退字体 msyh.ttc ≈ 19.7 MB）——8,000 chunk × ~21 MB 在布局期
   冲到 9 GB 提交内存后 abort。已修：nexty-text 0.1.8 按
   （blob id, face index）缓存 ResolvedFont，一份字节全进程共享。
5. 图片页全管线本身便宜（首屏 < 3 ms）；其像素拷贝（M1）是常数因子，
   由 Arc 化消除。

## 优化后（css 0.1.14 / text 0.1.8 / layout 0.1.26 / paint 0.1.7 / chrome 0.1.24）

两次完整运行（中位数，ms；第二次为最终代码的正式记录，第一次兼作稳定度对照）：

| 场景 | text-heavy（A / B） | rules-heavy（A / B） | image-heavy（A / B） |
| --- | --- | --- | --- |
| 首屏 parse | 28.3 / 24.3 | 98.4 / 73.2 | 0.1 / 0.1 |
| 首屏 cascade | 44.3 / 88.8 | **702.4 / 645.0**（基线 20,213） | 0.3 / 0.2 |
| 首屏 layout | 856.3 / 922.1 | 832.6 / 894.2（基线 1,074） | 0.9 / 0.6 |
| 首屏 scene | 7.7 / 7.8 | 7.1 / 8.0 | 0.0 / 0.0 |
| 单次布局 shape / chunk | 1.00×（8,000/8,001） | 1.00× | 0.96× |
| 单次布局 metrics 调用 | **10**（基线 64,818） | **1**（基线 105,585） | **1**（基线 132） |
| resize 风暴管线执行 | **5**（20 事件 @ 4 事件/帧；基线 20） | **5**（基线 20） | **5** |
| resize 风暴中位/帧 | 1,040.6 / 928.6（基线中位/事件 633.1 = 每 2 帧 1,266） | **1,111.9 / 1,086.4**（基线 11,302.8/事件） | 1.1 / 0.9 |
| 滚动风暴 emit 次数 | **0**（基线 50 次 emit） | **0** | **0** |
| 滚动风暴平移中位 | 4.8 / 5.3（基线 emit 7.5） | 3.5 / 4.0（基线 8.0） | 0.012 / 0.014 |
| 滚动风暴 rasterize 中位 | 16,212.8 / 15,090.9（未变：全页无裁剪） | 10,631.5 / 10,279.8（未变） | 23.9 / 22.8 |

### 各任务实测效果（调用计数与阶段时间）

- **T2 H4（layout 0.1.25）**：metrics 调用 64,818 → **10**（text-heavy，
  即不同 TextStyle 数；rules-heavy → 1；图片页 → 1）。shape 保持
  1.0×/chunk（普通块流本就单档 measure；多档 measure 路径由逐 chunk
  缓存覆盖，片段树指纹测试钉住行为逐字节一致）。
- **T3 M1（paint 0.1.6 / chrome 0.1.22）**：显示列表构建与发送路径
  图片像素零拷贝（`Arc::ptr_eq` 单测钉住 scene 与 page 同一份像素；渲染
  线程按所有权收 Scene——类型签名即证）。
- **T4 M6（paint 0.1.7 / chrome 0.1.23）**：滚动风暴 emit 次数 50 → **0**
  （首帧构建后零 emit；emit 阶段中位 = 缓存命中 + origin 平移 ≈ 3–5 ms，
  低于基线 emit 的 7.5–8.0 ms 且不再触碰片段树）；resize 风暴 20 事件按
  每帧 4 事件连发经合帧器 → 管线执行 20 → **5**。
- **T5 M7（chrome 0.1.24）**：URL 未变 redraw 整形 0 次（计数测试钉住）；
  逐键输入每键 ≤ 1 次 URL 整形（上界断言）；前缀宽度零整形（cluster
  前缀累加，与整形前缀的宽度差 < 0.05 px 测试钉住）。
- **T6 M5（css 0.1.14）**：rules-heavy 首屏 cascade **20,213 ms → 645 ms
  （≈31×）**，resize 风暴中位/事件 **11,302.8 ms → 1,086.4 ms（≈10×，
  其中合帧再 ÷4 事件）**；桶化与全量扫描逐元素一致的对照测试钉住语义。
- **T7 M10+M12（layout 0.1.26）**：root_style 借用化（每元素 -1 次深克隆）、
  行盒缓存命中 take 化、ensure_style 免全字段比较；M12 越界 font 下标
  跳过不 panic（单测钉住）。数字叠加在 T6 后的 layout 与 resize 风暴列。

### 无收益/回退项

- 无（全部改动在基准或调用计数上有正收益；H4 的 shape 缓存在普通块流
  语料上无变化（已是 1.0×），但为多档 measure 路径保留了正确性与收益，
  见结论 2）。

### 遗留瓶颈（本轮不做，已立项依据）

- **滚动光栅化被「全页场景无裁剪」主导**：滚动风暴 rasterize 10–16 s/帧
  （text/rules 页）——片段树不做滚动裁剪，场景含整页全部字形。M6 把
  emit 降为 0 后，视口裁剪/增量光栅是滚动体感的下一个数量级收益点
  （goal.md 非目标清单中的「页面纹理缓存 / 增量光栅」，后续大项）。

## 附：稳定度

主导阶段（rules-heavy cascade / layout、resize 风暴、rasterize）两次运行
波动 ≤ 10%；毫秒级小阶段（text-heavy parse/cascade）受共享机器噪声影响
相对波动偏大（绝对差 ≤ 45 ms），以调用计数与执行次数断言为准（机器无关）。
