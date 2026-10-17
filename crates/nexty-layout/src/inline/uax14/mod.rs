//! UAX#14（Unicode 18.0.0）断行规则引擎。
//!
//! 自研理由与边界见 `inline` 模块文档与
//! `docs/decisions/2026-10-17-line-breaking.md`：`unicode-linebreak` 0.1.5 的
//! Unicode 数据停在 15.0，对 18.0.0 官方语料有 1,117 条差异，故规则与数据
//! 自研，语料逐条比对见 `tests/inline_conformance.rs`。
//!
//! 三分离（`inline/mod.rs` 模块文档）：`data` 只做码点属性表，`classes` 做
//! 字符属性与 LB9/LB10 单元切分，`rules` 只做 LB1–LB31 判定。

mod classes;
mod data;
mod rules;

pub(crate) use classes::{LineBreakClass, Unit, build_units};
pub(crate) use rules::uax14_breaks;
