//! Nexty 浏览器外壳：窗口、输入、导航与自绘 UI。
//!
//! 上游：`winit`（窗口与输入）+ `wgpu`（呈现 surface）。`vello_cpu` 的
//! `Pixmap` 经纹理上传复用同一呈现路径；`vello_hybrid`（GPU 光栅）待接入时
//! 共用本层的 surface（决策见 `docs/decisions/2026-10-01-crate-selection.md`）。
//!
//! 本层把各下层串成一条导航管线，并持有渲染隔离线程与 `catch_unwind` 兜底
//! （见 AGENTS.md 硬规则「渲染隔离」）。模块分工：
//!
//! - [`pipeline`]：HTML 字符串 → 级联 → 布局 → 片段树 → paint 显示列表；
//! - [`render`]：渲染隔离线程（panic 兜底，线程存活可继续服务）；
//! - [`ui`]：自绘地址栏状态机（纯逻辑，可测试）；
//! - `app`（私有）：winit 事件循环 + wgpu 呈现胶水——**无法在无头环境
//!   自动化测试**，实机运行验证（`cargo run -p nexty-chrome --bin nexty`）。
//!
//! 上游类型一律不得出现在本 crate 的 pub 导出中（AGENTS.md 硬规则）。

#![forbid(unsafe_code)]

pub mod pipeline;
pub mod render;
pub mod ui;

mod app;

/// 启动浏览器窗口（阻塞至关闭）。
///
/// # Errors
///
/// 事件循环或 GPU 初始化失败时返回错误。
pub fn run() -> Result<(), crate::app::AppError> {
    crate::app::run()
}
