# ADR: 系统剪贴板选用 `arboard`（仅文本）

- 日期：2026-10-23
- 状态：已采纳
- 背景：页面文本选择落地后需要 `Ctrl+C` 把选区写入系统剪贴板。仓库硬规则禁止
  自研代码出现 unsafe（`#![forbid(unsafe_code)]`），因此不能用裸 Win32 FFI 自己
  调 `OpenClipboard` / `SetClipboardData`；winit 0.30 也不提供剪贴板 API，必须引入
  一个成熟 crate。

## 候选

| 候选 | 许可证 | 结论 |
| --- | --- | --- |
| `arboard` 3.7 | MIT OR Apache-2.0 | **采纳**。1Password 维护，跨平台（Windows / macOS / Linux X11/Wayland），API 全 safe；可关闭默认的 `image-data`，只保留文本路径，不引入图片剪贴板后端 |
| `clipboard-win` 5.4 | BSL-1.0 | 不直接选：仅 Windows，破坏本层跨平台；它是 arboard 的 Windows 后端（传递依赖），其 BSL-1.0 按“宽松、非 copyleft”加入 `deny.toml` 白名单 |
| `copypasta` 0.10 | MIT / Apache-2.0 | 否决：可用但主要服务 Alacritty 终端场景，维护者与发布节奏不如 arboard；Linux 端会把 X11/Wayland 依赖一并拉入，能力（纯文本）与 arboard 重叠；Windows 后端同样走 `clipboard-win` |
| `clipboard` 0.5（rust-clipboard） | MIT / Apache-2.0 | 否决：已停止维护，Windows 端锁在 `clipboard-win` 2.x，Linux 端 `x11-clipboard` 0.3 过旧；维护活跃度不满足选型要求 |

## 决策

1. **只依赖 `nexty-chrome`**，并挂在 `selection` feature 下：
   `selection = ["pipeline", "dep:arboard"]`。布局/文本层不感知剪贴板；
   `TextMap` 只负责产出复制文本，由 chrome 写入系统剪贴板。
2. **关闭默认 feature**（`default-features = false`）：只需要文本，不引入
   `image-data`（会拉 `image` 与 macOS CoreGraphics 绑定）。本仓已有 `image`
   （paint 解码用），但剪贴板图片不在本轮范围，能不加就不加。
3. **能力边界**：`selection::write_clipboard(&str)` 只写纯文本；不写 HTML 格式、
   不写图片、不读取剪贴板（粘贴不在本轮）。失败返回 `Result<(), String>`，窗口
   外壳记录错误并继续，不 panic、不影响渲染。
4. **平台边界**：Windows 走 `windows-sys`（已在 winit/wgpu 依赖树中）；macOS 走
   `objc2`；Linux 走 X11（Wayland 数据控制不在本轮启用）。三者都由 arboard 内部
   处理 unsafe，本仓代码保持 `forbid(unsafe_code)`。
5. **测试边界**：复制文本的重建逻辑（软折行/硬断行/块边界/`pre` 空白）在
   `nexty-layout` 的 `TextMap::text` 与 chrome 选区测试里无头覆盖；实际
   `SetClipboardData` 只在实机验收中用 `Get-Clipboard` 核对，避免无头 CI 触碰
   OS 剪贴板。

## 许可证说明（BSL-1.0）

arboard 在 Windows 上传递依赖 `clipboard-win` / `error-code`（BSL-1.0），不在原
白名单内。取舍：

- BSL-1.0 与 MIT 等价宽松：无 copyleft、无源码公开义务、OSI 与 FSF 均认可；
- 替代路径只有两条——用 unsafe 直接调 Win32（违反 `#![forbid(unsafe_code)]`），
  或换成已停维护的 `clipboard` 0.5（Windows 后端仍是 2018 年的
  `clipboard-win` 2.x）。两者都更差。
- 因此把 BSL-1.0 加入 `deny.toml` 白名单并在注释中写明来源
  （`clipboard-win` / `error-code` ← arboard）；AGENTS.md 的许可证政策同步更新。
  这不是放开策略，而是把“已选 crate 栈无法回避的宽松传递许可”逐条登记。

## 依赖与门禁

- `cargo deny check`：arboard 及其传递依赖必须在 `deny.toml` 的许可证白名单内
  （MIT / Apache-2.0 / BSD / ISC / MPL-2.0 / LGPL / Zlib / CC0-1.0 / Unicode-3.0 /
  CDLA-Permissive-2.0 / BSL-1.0）。BSL-1.0 的引入理由见上节；若后续升级再引入
  白名单外许可证，则回到候选表重新选型。
- `docs/dependencies.html` 随依赖清单重新生成（NOTICE / 归属清单与多重许可人工
  核对区块按 `.workbuddy` 的注入流程补齐）。
- 版本：`nexty-chrome` 0.1.17 → 0.1.18。

## 验证

- 无头：`cargo test -p nexty-chrome --test selection` 覆盖选区文本重建与快捷键映射；
  `cargo deny check` 覆盖许可证与 advisory。
- 实机：拖拽选中正文 → `Ctrl+C` → `powershell -Command Get-Clipboard` 与页面
  文本逐字一致；`<br>` / 块边界为换行，软折行不插换行，`<pre>` 保留空白。
