//! 整形结果 → 源文本字节区间的 cluster 映射。
//!
//! 边界取「UAX#29 扩展字形簇边界」与「harfrust 实际 glyph cluster 起点」的
//! 并集：断行永不切开 typographic character unit（CSS Text 3 §5.5），连字
//! 等多字符 cluster 保持不可拆。

use std::collections::BTreeSet;

use nexty_text::ShapedText;
use unicode_segmentation::UnicodeSegmentation;

/// 源文本中的一段字节区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// 起始字节偏移（含）。
    pub byte_start: usize,
    /// 结束字节偏移（不含）。
    pub byte_end: usize,
}

/// 把一次整形结果的 glyph cluster 映射为源文本字节区间。
///
/// 边界是「UAX#29 字形簇边界」与「harfrust 实际 glyph cluster 起点」的
/// 并集：从不出现在字形簇内部；若整形把多个字形簇合成一个 cluster
/// （连字），则合并（不切开）。因此 1 char ≠ 1 cluster。
#[must_use]
pub fn cluster_map(text: &str, shaped: &ShapedText) -> Vec<Span> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut grapheme_starts = BTreeSet::new();
    let mut graphemes = Vec::new();
    for (offset, grapheme) in text.grapheme_indices(true) {
        grapheme_starts.insert(offset);
        graphemes.push((offset, offset + grapheme.len()));
    }
    grapheme_starts.insert(text.len());

    let mut starts = BTreeSet::new();
    starts.insert(0);
    starts.insert(text.len());
    for glyph in &shaped.glyphs {
        let cluster = glyph.cluster as usize;
        if cluster <= text.len() && grapheme_starts.contains(&cluster) {
            starts.insert(cluster);
        }
    }
    if shaped.glyphs.is_empty() {
        starts.extend(grapheme_starts.iter().copied());
    }

    let starts: Vec<usize> = starts.into_iter().collect();
    let group_of = |offset: usize| -> usize {
        let index = starts.partition_point(|&start| start <= offset);
        if index == 0 { 0 } else { starts[index - 1] }
    };

    let mut spans = Vec::new();
    let mut group_start = graphemes[0].0;
    let mut group_cluster = group_of(group_start);
    for &(grapheme_start, _) in &graphemes[1..] {
        let cluster = group_of(grapheme_start);
        if cluster != group_cluster {
            spans.push(Span {
                byte_start: group_start,
                byte_end: grapheme_start,
            });
            group_start = grapheme_start;
            group_cluster = cluster;
        }
    }
    spans.push(Span {
        byte_start: group_start,
        byte_end: text.len(),
    });
    spans
}
