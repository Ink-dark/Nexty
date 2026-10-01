//! Nexty CSS 层 facade：解析、选择器匹配与自研 cascade。
//!
//! 上游：`cssparser` + `selectors`（均 MPL-2.0），只用于解析与选择器匹配；
//! cascade 与 computed style 自研，可从 MusKitty 迁移已有实现。决策见
//! `docs/decisions/2026-10-01-crate-selection.md`。
//!
//! 行为 ground truth：CSS Syntax、Selectors、CSS Cascade 规范。
//!
//! 当前为骨架：公开 API 尚未定义。cascade 行为由规范约束，需先读对应章节再动手
//! （AGENTS.md Behavior #1）。

#![forbid(unsafe_code)]
