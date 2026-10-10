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
//! style（`cascade`）、UA 默认样式表（`ua`）。
//!
//! **Feature**：按上表的五个模块一一对应，默认全开。依赖链为
//! `values` → `selectors` → `stylesheets` → {`cascade`, `ua-stylesheet`}，
//! 各 feature 按需上拉依赖（见 Cargo.toml）。

#![forbid(unsafe_code)]

#[cfg(feature = "cascade")]
mod cascade;
#[cfg(feature = "stylesheets")]
mod parser;
#[cfg(feature = "selectors")]
mod selector;
#[cfg(test)]
mod test_support;
#[cfg(feature = "ua-stylesheet")]
mod ua;
#[cfg(feature = "values")]
mod value;

#[cfg(feature = "cascade")]
pub use cascade::{ComputedStyle, Edges, cascade, compute_document_styles};
#[cfg(feature = "stylesheets")]
pub use parser::{Declaration, MediaViewport, StyleRule, Stylesheet, parse_inline_style};
#[cfg(feature = "selectors")]
pub use selector::{SelectorError, matches_selector};
#[cfg(feature = "ua-stylesheet")]
pub use ua::html_ua_stylesheet;
#[cfg(feature = "values")]
pub use value::{
    AbsoluteSize, AlignItemsValue, AlignSelfValue, BorderColorValue, BorderStyle, BorderWidthValue,
    BoxSizingValue, CssWideKeyword, DeclaredValue, DisplayValue, FlexDirectionValue, FlexWrapValue,
    FontFamilyValue, FontSizeValue, FontStyleValue, FontWeightValue, GapValue, GridAutoFlowValue,
    GridTrack, GridTrackList, InsetValue, JustifyContentValue, LineHeightValue, MarginValue,
    OverflowValue, PaddingValue, PositionValue, PropertyId, PropertyValue, Rgba, SizeValue,
    TextAlignValue,
};
