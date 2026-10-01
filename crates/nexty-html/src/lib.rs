//! Nexty HTML 解析层 facade。
//!
//! 上游：`html5ever`（MIT/Apache-2.0），按 WHATWG HTML §13 做词法与树构建。
//! 本层的职责是实现 html5ever 的 `TreeSink`，把解析结果写进 [`nexty_dom`] 的
//! arena，使上游类型不越过本层边界。决策见
//! `docs/decisions/2026-10-01-crate-selection.md`。
//!
//! 行为 ground truth：WHATWG HTML Standard §13（Parsing）。
//!
//! 当前为骨架：公开 API 尚未定义。树构建行为完全由规范约束，需先读 §13 再动手
//! （AGENTS.md Behavior #1），且 TreeSink 的实现要等 `nexty_dom` 的树变更算法
//! 落地后才能接。

#![forbid(unsafe_code)]
