//! Nexty 自研盒级布局。
//!
//! 不依赖外部布局引擎：`taffy` 虽成熟，但布局是 WPT 对齐的关键层，需完全可控。
//! 决策见 `docs/decisions/2026-10-01-crate-selection.md`。
//!
//! 行为 ground truth：CSS Display、CSS Box Model、CSS Flexbox、CSS Grid。
//!
//! 当前为骨架：公开 API 尚未定义。布局算法由规范约束，需先读对应章节再动手
//! （AGENTS.md Behavior #1）。

#![forbid(unsafe_code)]
