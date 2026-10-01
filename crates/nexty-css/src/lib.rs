//! Nexty CSS 层 facade：解析、选择器匹配与自研 cascade。
//!
//! 上游：`cssparser` + `selectors`（均 MPL-2.0），只用于解析与选择器匹配；
//! cascade 与 computed style 自研，可从 MusKitty 迁移已有实现。决策见
//! `docs/decisions/2026-10-01-crate-selection.md`。
//!
//! 行为 ground truth：CSS Syntax、Selectors、CSS Cascade 规范。
//!
//! 已落地：属性值模型与逐属性解析（`value`）、选择器解析与 arena DOM 匹配
//! （`selector`）、样式表与声明块解析（`parser`）、自研 cascade 与 computed
//! style（`cascade`）。

#![forbid(unsafe_code)]

mod cascade;
mod parser;
mod selector;
#[cfg(test)]
mod test_support;
mod value;

pub use cascade::{ComputedStyle, cascade, compute_document_styles};
pub use parser::{Declaration, StyleRule, Stylesheet, parse_inline_style};
pub use selector::{SelectorError, matches_selector};
pub use value::{
    AbsoluteSize, CssWideKeyword, DeclaredValue, DisplayValue, FontFamilyValue, FontSizeValue,
    FontStyleValue, FontWeightValue, PropertyId, PropertyValue, Rgba, TextAlignValue,
};
