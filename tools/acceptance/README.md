# Nexty 实机验收资产

浏览器外壳没有无头模式（winit 窗口 + wgpu surface，无法在 CI 里自动化），
所以历轮验收页面过去散落在本机 `.workbuddy/`，不可复现、也无法防回归。
本目录把验收资产纳入版本控制：**页面是固定 URL，判据是固定文案，计数端点是
`/stats`**，任何人（含后续 agent）都能按同一套流程重跑同一组判据。

## 运行

```bash
node tools/acceptance/server.cjs          # 默认 http://127.0.0.1:8472
PORT=9000 node tools/acceptance/server.cjs
```

浏览器侧：

```bash
cargo build -p nexty-chrome --bin nexty --release
./target/release/nexty.exe
```

窗口起来后在地址栏（`Ctrl+L`）输入 `http://127.0.0.1:8472/` 打开资产索引页，
按索引逐条点开。`/stats` 返回各 URL 的请求计数，用于断言缓存命中与字体去重；
`/reset` 清零（每次重跑验收前先打开一次 `/reset`）。

## 目录

```
tools/acceptance/
├── server.cjs                 # 静态 pages/ + 动态端点（GBK/302/缓存/字体/stats）
├── regenerate-gbk.sh          # gbk-page.src.html → gbk-page.hex（iconv）
├── README.md                  # 本文件：运行方式 + 回归清单 + 新增资产约定
└── pages/
    ├── index.html             # 资产索引（所有路由与判据）
    ├── batch1/k2/             # K2 绘制扩展对照（五组 / 放大 / 装饰线 / 链接）
    ├── batch1/k3/             # K3 GBK 字节、302 落点页、缓存页
    ├── batch1/k4/             # K4 @font-face 正常页与 404 回退页
    ├── core/                  # 长页主页 + 第二页 + dot.png（滚动/键盘/选区/链接）
    ├── links/                 # 链接点击与行内图片形态
    └── legacy/                # 历史轮次页面原样迁入（t3 文本/图片/float，float 冒烟）
```

## 回归清单（批次 1，chrome 0.1.30 基线）

| # | 路由 | 判据 | 判定方式 |
| --- | --- | --- | --- |
| K2-1 | `/k2` | 圆角卡片四角圆弧、深蓝边框沿圆角描边 | 截图目测 |
| K2-2 | `/k2` `/k2z` | 90deg 红→蓝水平、45° 三色过中心、to bottom 绿→黑 | 截图目测 |
| K2-3 | `/k2one` `/k2u` | 裸 `<a>` 带下划线（UA 表）；overline/line-through 同现 | 截图目测 |
| K2-4 | `/k2z` | 绿(3)>蓝(2)>红(1)，三块两两重叠区域顺序正确 | 截图目测 |
| K2-5 | `/k2` | body 背景传播到画布，滚动到底不变 | 滚动后截图 |
| K3-1 | `/gbk` | 标题/正文/标点全部正确解码，不进错误页 | 截图目测 |
| K3-2 | `/redir` | 落点 `/landing/dir/page.html`；相对图片按落点解析 | `/stats` 含 `/landing/dir/dot.png` |
| K3-3 | `/redir` → 点相对链接 | 应到 `/landing/dir/sub.html` | `/stats` 记录该路径 |
| K3-4 | `/cached` 导航两次 | 第二次不回服务器 | `/stats` 的 `/cached` 仍为 1 |
| K4-1 | `/font` | 两行字形明显不同 | 截图目测 |
| K4-2 | `/font404` | 两行字形相同（系统回退），无豆腐块 | 截图目测 |
| K4-3 | `/font` 二次导航 | 不重新抓取字体 | `/stats` 无新增 `space-mono.ttf` |
| K1-1 | `/core/` | 滚轮 / PageUp/PageDown / Home/End 滚动；链接导航；后退前进 | 键盘 + 截图 |
| K1-2 | `/links/*` | 单次点击链接导航；图片与链接同排 | 点击 + 标题变化 |
| K1-3 | `/core/` `/legacy/t3/selection.html` | 拖拽选区高亮、`Ctrl+C` 进系统剪贴板、块边界插换行 | 截图 + 读剪贴板 |

历史轮次页面（`/legacy/...`）的判据写在各页面末尾或对应轮次文档
（`docs/plans/`）中；这些页面在当年验收通过后原样迁入，改动需同步更新本文档。

## 已知缺陷 / 偏差（验收时不要误判为新问题）

1. **302 后相对链接基准未落到最终 URL**（2026-10-31 实机验收发现，未修）：
   `/redir` 落点页点相对链接会请求 `/sub.html`（404），而相对**子资源**已按
   `final_url` 正确解析。根因：链接解析用 `history.current()`（请求 URL），
   地址栏/历史条目也没更新为落点。见 `crates/nexty-chrome/src/app.rs`
   （`navigate` / 点击导航处）。
2. **@font-face 注册表是会话级**：后一页对**同名 family** 的 src 404 时会沿用
   前一页已注册的字体。`/font404` 故意用不同 family 名，就是为了让这条偏差
   不干扰「404 → 系统回退」的判定。
3. **地址栏持焦时页面不响应键盘滚动**：点击页面让地址栏失焦后才吃 PageDown/End
   （这是既有交互语义，不是回归）。
4. **CUA 连续两次同点等于双击**：第一跳后第二下会落在新页同坐标的链接上，
   测单次导航只点一次；点链接要精确落在文字行盒内，点空不会报错也不导航。

## 新增验收资产的约定

1. 页面放 `pages/<层或轮次>/`，文件名自带用途（`k2-zoom.html`）。
2. 需要特殊响应头（`302` / `Cache-Control` / `charset=gbk`）或跨目录取字节的，
   在 `server.cjs` 的 `ALIASES` / 动态分支登记，不要写进页面 HTML。
3. 在 `pages/index.html` 索引页登记「路由 / 资产 / 判据」，并在本 README 的回归
   清单里加一行；只加文件不登记视为未完成。
4. 依赖二进制（图片/字体）优先复用仓库已有文件（如
   `crates/nexty-text/tests/data/SpaceMono-Regular.ttf`），避免重复入库。
5. 生成类资产（GBK hex）保留 UTF-8 源 + 生成脚本，脚本幂等。
