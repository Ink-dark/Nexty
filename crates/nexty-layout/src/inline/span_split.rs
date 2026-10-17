//! 在 cluster 边界上按断行机会切段，以及 min-content 宽度。
//!
//! CSS Text 3 §5.1 / §5.4：`word-break: break-all` 与 `overflow-wrap:
//! anywhere` 的断点参与 min-content；`overflow-wrap: break-word` 的紧急
//! 断点**不**参与（计算时按 `normal` 断点重算）。

use std::collections::BTreeSet;

use nexty_text::ShapedText;

use super::break_ops::{BreakOptions, OverflowWrap, break_opportunities};
use super::cluster_map::{Span, cluster_map};

/// 按 min-content 语义计算最窄宽度（CSS px）。
///
/// `overflow-wrap: break-word` 的紧急断点不计入 min-content（CSS Text 3
/// §5.4）；`anywhere` 与 `word-break: break-all` 的断点计入（§5.1 末段）。
#[must_use]
pub fn min_content_width(text: &str, shaped: &ShapedText, options: &BreakOptions) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let clusters = cluster_map(text, shaped);
    let mut widths = vec![0.0_f32; clusters.len()];
    for glyph in &shaped.glyphs {
        let cluster = glyph.cluster as usize;
        if let Some(index) = clusters
            .iter()
            .position(|span| span.byte_start <= cluster && cluster < span.byte_end)
        {
            widths[index] += glyph.advance;
        }
    }

    let effective = if options.overflow_wrap == OverflowWrap::BreakWord {
        BreakOptions {
            overflow_wrap: OverflowWrap::Normal,
            ..*options
        }
    } else {
        *options
    };
    let breaks = break_opportunities(text, &effective);
    let mut max_width = 0.0_f32;
    let mut start = 0usize;
    for &offset in &breaks {
        if offset <= start {
            continue;
        }
        let width: f32 = clusters
            .iter()
            .zip(&widths)
            .filter(|(span, _)| span.byte_start >= start && span.byte_start < offset)
            .map(|(_, width)| *width)
            .sum();
        max_width = max_width.max(width);
        start = offset;
    }
    max_width
}

/// 在 cluster 边界上按断行机会切分文本；输出无缝覆盖输入。
///
/// 不是 cluster 边界的断点会被忽略（cluster 不可拆）。逐段 `&text[start..end]`
/// 拼回必须与输入逐字节相同。
#[must_use]
pub fn split_spans(text: &str, clusters: &[Span], breaks: &[usize]) -> Vec<Span> {
    if text.is_empty() {
        return Vec::new();
    }
    let boundaries: BTreeSet<usize> = clusters
        .iter()
        .flat_map(|span| [span.byte_start, span.byte_end])
        .collect();
    let mut points = BTreeSet::new();
    points.insert(0);
    points.insert(text.len());
    for &offset in breaks {
        if offset > 0 && offset < text.len() && boundaries.contains(&offset) {
            points.insert(offset);
        }
    }
    let points: Vec<usize> = points.into_iter().collect();
    points
        .windows(2)
        .map(|window| Span {
            byte_start: window[0],
            byte_end: window[1],
        })
        .collect()
}
