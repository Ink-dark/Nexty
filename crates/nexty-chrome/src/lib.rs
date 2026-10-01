//! Nexty 浏览器外壳：窗口、输入与导航。
//!
//! 上游：`winit`（窗口与输入）+ `wgpu`（呈现 surface）。`vello_hybrid` 需要
//! wgpu surface，`vello_cpu` 的 `Pixmap` 可作为纹理上传复用同一呈现路径。
//! 决策见 `docs/decisions/2026-10-01-crate-selection.md`。
//!
//! 本层把各下层串成一条导航管线，并持有渲染隔离线程与 `catch_unwind` 兜底
//! （见 AGENTS.md 硬规则「渲染隔离」）。
//!
//! 当前为骨架：公开 API 尚未定义，等待下层各 facade 就位后再定管线形状。

#![forbid(unsafe_code)]
