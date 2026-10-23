//! 文本选区几何：点 → 源位置、选区矩形与复制文本重建。
//!
//! 选区端点用 [`TextPosition`]（文本节点 id + 源字节偏移）表示，与渲染结果
//! 解耦：重排后可用 [`TextMap::resolve`] 重新定位。几何与文本来自
//! [`TextRun::text`](crate::TextRun) / [`SourceChar`] 映射，以及 run 实际渲染的
//! 字节区间 `text_start..text_end`（行首/行尾被裁掉的折叠空白不在其中）。
//!
//! 行为参考：[DOM Standard §6 Selection](https://dom.spec.whatwg.org/#selection-interface)
//! 的 anchor/focus 模型与 CSS Text 的空白处理。坐标与
//! `hit_test_ex` 一致：以根片段边框盒原点为 (0, 0)。
//!
//! # 已知偏差
//!
//! - 仅 LTR；选区顺序按绘制顺序（文本 run 在行内原子盒之前），不按 bidi 重排。
//! - 行内原子盒内的文本排在同行所有文本 run 之后（沿用 paint 的绘制顺序），
//!   因此 `a<span style="display:inline-block">b</span>c` 的复制顺序是
//!   `acb`；多行原子盒的后续行照常按行排列。
//! - 软折行处被裁掉的折叠空格在复制时补回一个空格；被裁掉的行尾空白不参与
//!   选区矩形。

use std::sync::Arc;

use nexty_dom::NodeId;
use unicode_segmentation::UnicodeSegmentation;

use crate::fragment::{Fragment, LineStart, Rect, SourceChar, TextRun};

/// 选区端点：文本节点内的源字节偏移。
///
/// 文本节点被移除或替换后 `resolve` 返回 `None`；重排不会改变该位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextPosition {
    /// 文本节点 id。
    pub node: NodeId,
    /// 源文本节点内的字节偏移。
    pub offset: usize,
}

/// 一行的选区几何信息。
struct Line {
    top: f32,
    height: f32,
    start: LineStart,
    block: usize,
    segments: Vec<usize>,
}

/// 一段不可再按行拆分的文本 run（同一行内的连续字形）。
struct Segment {
    text_node: NodeId,
    text: Arc<str>,
    source_map: Arc<[SourceChar]>,
    clusters: Vec<Cluster>,
    byte_start: u32,
    byte_end: u32,
    collapses: bool,
}

impl Segment {
    fn new(run: &TextRun, origin_x: f32) -> Option<Self> {
        if run.glyphs.is_empty() {
            return None;
        }
        let mut clusters: Vec<Cluster> = Vec::new();
        for glyph in &run.glyphs {
            let x = origin_x + glyph.x;
            if let Some(last) = clusters.last_mut()
                && last.byte == glyph.cluster
            {
                last.end_x = last.end_x.max(x + glyph.advance);
                continue;
            }
            let (source, source_end) = source_at(&run.source_map, glyph.cluster);
            clusters.push(Cluster {
                byte: glyph.cluster,
                x,
                end_x: x + glyph.advance,
                source,
                source_end,
            });
        }
        let first = clusters.first()?;
        let last = clusters.last()?;
        let byte_start = run.text_start.min(first.byte);
        let byte_end = run.text_end.max(last.byte);
        Some(Self {
            text_node: run.text_node,
            text: Arc::clone(&run.text),
            source_map: Arc::clone(&run.source_map),
            clusters,
            byte_start,
            byte_end,
            collapses: run.collapses_whitespace,
        })
    }

    fn source_start(&self) -> u32 {
        self.clusters.first().map_or(0, |cluster| cluster.source)
    }

    fn source_end(&self) -> u32 {
        self.clusters.last().map_or(0, |cluster| cluster.source_end)
    }
}

/// 一个不可拆字形簇的几何与源区间。
struct Cluster {
    byte: u32,
    x: f32,
    end_x: f32,
    source: u32,
    source_end: u32,
}

/// 片段树的文本地图：命中、选区矩形与复制文本。
pub struct TextMap {
    lines: Vec<Line>,
    segments: Vec<Segment>,
    /// segment → line 下标。
    segment_line: Vec<usize>,
}

impl TextMap {
    /// 从布局片段树建立文本地图（按绘制顺序收集）。
    #[must_use]
    pub fn new(root: &Fragment) -> Self {
        let mut builder = Builder {
            lines: Vec::new(),
            segments: Vec::new(),
            segment_line: Vec::new(),
            next_block: 0,
        };
        builder.collect(root, 0.0, 0.0, None);
        Self {
            lines: builder.lines,
            segments: builder.segments,
            segment_line: builder.segment_line,
        }
    }

    /// 视口点（根片段边框盒坐标系）命中的源位置。
    ///
    /// 位置吸附到字形簇边界：字形左半取前边界、右半取后边界；行内没有文本
    /// （空行）时取相邻行的首/尾位置。点不在任何行内返回 `None`。
    #[must_use]
    pub fn hit(&self, x: f32, y: f32) -> Option<TextPosition> {
        let line = self
            .lines
            .iter()
            .position(|line| y >= line.top && y <= line.top + line.height)?;
        self.position_in_line(line, x)
    }

    /// 最近文本位置：点不在行内时按纵向最近行、横向最近边界吸附。
    #[must_use]
    pub fn nearest(&self, x: f32, y: f32) -> Option<TextPosition> {
        let mut best: Option<(f32, usize)> = None;
        for (index, line) in self.lines.iter().enumerate() {
            let distance = if y < line.top {
                line.top - y
            } else if y > line.top + line.height {
                y - line.top - line.height
            } else {
                0.0
            };
            if best.is_none_or(|(current, _)| distance < current) {
                best = Some((distance, index));
            }
        }
        let (_, index) = best?;
        self.position_in_line(index, x)
            .or_else(|| self.fallback_position(index))
    }

    /// 把源位置夹到当前布局中该文本节点的有效范围（重排后端点复用）。
    ///
    /// 文本节点已不在片段树中时返回 `None`。
    #[must_use]
    pub fn resolve(&self, position: TextPosition) -> Option<TextPosition> {
        let mut min = u32::MAX;
        let mut max = 0;
        let mut found = false;
        for segment in &self.segments {
            if segment.text_node != position.node {
                continue;
            }
            min = min.min(segment.source_start());
            max = max.max(segment.source_end());
            found = true;
        }
        if !found {
            return None;
        }
        Some(TextPosition {
            node: position.node,
            offset: position.offset.clamp(min as usize, max as usize),
        })
    }

    /// UAX#29 词边界；位置落在空白上时返回 `None`。
    ///
    /// CJK 按 UAX#29 逐字成词（不做字典式分词）。
    #[must_use]
    pub fn word_range(&self, position: TextPosition) -> Option<(TextPosition, TextPosition)> {
        let (segment_index, chunk) = self.resolved(position)?;
        let segment = &self.segments[segment_index];
        let chunk = chunk as usize;
        for (start, word) in segment.text.split_word_bound_indices() {
            let end = start + word.len();
            if start <= chunk && chunk < end {
                if word.chars().all(char::is_whitespace) {
                    return None;
                }
                let from = self.position_from_chunk(segment_index, start as u32)?;
                let to = self.position_from_chunk(segment_index, end as u32)?;
                return Some((from, to));
            }
        }
        None
    }

    /// 整份文本的首尾位置（无文本时 `None`）。
    #[must_use]
    pub fn all(&self) -> Option<(TextPosition, TextPosition)> {
        let first = self.segments.first()?;
        let last_index = self.segments.len() - 1;
        let last = &self.segments[last_index];
        let from = self.position_from_chunk(0, first.byte_start)?;
        let to = self.position_from_chunk(last_index, last.byte_end)?;
        Some((from, to))
    }

    /// 选区的矩形（逐行；空选区或反向端点自动规范化）。
    #[must_use]
    pub fn rects(&self, anchor: TextPosition, focus: TextPosition) -> Vec<Rect> {
        let Some(start) = self.resolved(anchor) else {
            return Vec::new();
        };
        let Some(end) = self.resolved(focus) else {
            return Vec::new();
        };
        let (start, end) = order(start, end);
        let start_line = self.segment_line[start.0];
        let end_line = self.segment_line[end.0];
        let mut rects = Vec::new();
        for line_index in start_line..=end_line {
            let line = &self.lines[line_index];
            let mut left = f32::INFINITY;
            let mut right = f32::NEG_INFINITY;
            for &segment_index in &line.segments {
                if segment_index < start.0 || segment_index > end.0 {
                    continue;
                }
                let segment = &self.segments[segment_index];
                let from = if segment_index == start.0 {
                    start.1
                } else {
                    segment.byte_start
                };
                let to = if segment_index == end.0 {
                    end.1
                } else {
                    segment.byte_end
                };
                for cluster in &segment.clusters {
                    if cluster.byte >= from && cluster.byte < to {
                        left = left.min(cluster.x);
                        right = right.max(cluster.end_x);
                    }
                }
            }
            if left < right {
                rects.push(Rect {
                    x: left,
                    y: line.top,
                    width: right - left,
                    height: line.height,
                });
            }
        }
        rects
    }

    /// 选区对应的复制文本。
    ///
    /// 同一块内的软折行直接拼接（断点处被裁掉的折叠空格补一个空格）；硬断行
    /// 与块级片段边界插入 `\n`；`white-space` 保留模式的空白原样保留；折叠模式
    /// 下选区首尾的空格裁掉。
    #[must_use]
    pub fn text(&self, anchor: TextPosition, focus: TextPosition) -> String {
        let Some(start) = self.resolved(anchor) else {
            return String::new();
        };
        let Some(end) = self.resolved(focus) else {
            return String::new();
        };
        let (start, end) = order(start, end);
        let start_line = self.segment_line[start.0];
        let end_line = self.segment_line[end.0];
        let mut out = String::new();
        let mut previous_line: Option<usize> = None;
        let mut first_collapses = true;
        let mut last_collapses = true;
        for line_index in start_line..=end_line {
            let line = &self.lines[line_index];
            let mut line_text = String::new();
            for &segment_index in &line.segments {
                if segment_index < start.0 || segment_index > end.0 {
                    continue;
                }
                let segment = &self.segments[segment_index];
                let from = if segment_index == start.0 {
                    start.1
                } else {
                    segment.byte_start
                };
                let to = if segment_index == end.0 {
                    end.1
                } else {
                    segment.byte_end
                };
                if from >= to {
                    continue;
                }
                if line_text.is_empty() {
                    first_collapses = first_collapses && segment.collapses;
                }
                last_collapses = segment.collapses;
                line_text.push_str(&segment.text[from as usize..to as usize]);
            }
            if let Some(previous) = previous_line {
                let hard = line.start == LineStart::Hard;
                let block_break = self.lines[previous].block != line.block;
                if hard || block_break {
                    out.push('\n');
                } else if let LineStart::Soft { space: true } = line.start
                    && !out.ends_with(' ')
                    && !line_text.starts_with(' ')
                {
                    out.push(' ');
                }
            }
            out.push_str(&line_text);
            previous_line = Some(line_index);
        }
        if first_collapses {
            out = out.trim_start_matches(' ').to_owned();
        }
        if last_collapses {
            out = out.trim_end_matches(' ').to_owned();
        }
        out
    }

    /// 点所在行的位置；行内无文本时退回相邻行。
    fn position_in_line(&self, line_index: usize, x: f32) -> Option<TextPosition> {
        let line = &self.lines[line_index];
        let mut best: Option<(f32, TextPosition)> = None;
        for &segment_index in &line.segments {
            let segment = &self.segments[segment_index];
            for (index, cluster) in segment.clusters.iter().enumerate() {
                if x < cluster.x {
                    consider(
                        &mut best,
                        cluster.x - x,
                        TextPosition {
                            node: segment.text_node,
                            offset: cluster.source as usize,
                        },
                    );
                    continue;
                }
                if x <= cluster.end_x {
                    let offset = if x < cluster.x + (cluster.end_x - cluster.x) / 2.0 {
                        cluster.source
                    } else {
                        after_cluster(segment, index)
                    };
                    return Some(TextPosition {
                        node: segment.text_node,
                        offset: offset as usize,
                    });
                }
                consider(
                    &mut best,
                    x - cluster.end_x,
                    TextPosition {
                        node: segment.text_node,
                        offset: after_cluster(segment, index) as usize,
                    },
                );
            }
        }
        best.map(|(_, position)| position)
            .or_else(|| self.fallback_position(line_index))
    }

    /// 空行的位置：前一行末尾，否则后一行开头。
    fn fallback_position(&self, line_index: usize) -> Option<TextPosition> {
        for index in (0..line_index).rev() {
            if let Some(position) = self.line_last_position(index) {
                return Some(position);
            }
        }
        for index in line_index + 1..self.lines.len() {
            if let Some(position) = self.line_first_position(index) {
                return Some(position);
            }
        }
        None
    }

    fn line_first_position(&self, line_index: usize) -> Option<TextPosition> {
        let line = &self.lines[line_index];
        let segment_index = *line.segments.first()?;
        let segment = &self.segments[segment_index];
        let cluster = segment.clusters.first()?;
        Some(TextPosition {
            node: segment.text_node,
            offset: cluster.source as usize,
        })
    }

    fn line_last_position(&self, line_index: usize) -> Option<TextPosition> {
        let line = &self.lines[line_index];
        let segment_index = *line.segments.last()?;
        let segment = &self.segments[segment_index];
        let cluster = segment.clusters.last()?;
        Some(TextPosition {
            node: segment.text_node,
            offset: cluster.source_end as usize,
        })
    }

    /// 源位置 → (segment 下标, 吸附到字形簇边界的 chunk 字节)。
    fn resolved(&self, position: TextPosition) -> Option<(usize, u32)> {
        let mut chosen: Option<usize> = None;
        for (index, segment) in self.segments.iter().enumerate() {
            if segment.text_node != position.node {
                continue;
            }
            let offset = position.offset as u32;
            if offset < segment.source_start() {
                if chosen.is_none() {
                    chosen = Some(index);
                }
                break;
            }
            if offset < segment.source_end() {
                chosen = Some(index);
                break;
            }
            chosen = Some(index);
        }
        let index = chosen?;
        let segment = &self.segments[index];
        let offset = position.offset as u32;
        let chunk = if offset >= segment.source_end() {
            segment.byte_end
        } else {
            let map = &segment.source_map;
            let after = map.partition_point(|entry| entry.source <= offset);
            let chunk = match after.checked_sub(1).and_then(|index| map.get(index)) {
                Some(entry) if offset < entry.source_end => entry.chunk,
                Some(_) => map.get(after).map_or(segment.byte_end, |entry| entry.chunk),
                None => map.first().map_or(segment.byte_start, |entry| entry.chunk),
            };
            chunk.clamp(segment.byte_start, segment.byte_end)
        };
        let chunk = if chunk >= segment.byte_end {
            segment.byte_end
        } else {
            segment
                .clusters
                .iter()
                .rev()
                .find(|cluster| cluster.byte <= chunk)
                .map_or(segment.byte_start, |cluster| cluster.byte)
        };
        Some((index, chunk))
    }

    /// chunk 字节 → 源位置（边界取该字符起点；chunk 等于可见区间末尾取终点）。
    fn position_from_chunk(&self, segment_index: usize, chunk: u32) -> Option<TextPosition> {
        let segment = &self.segments[segment_index];
        let chunk = chunk.clamp(segment.byte_start, segment.byte_end);
        let offset = if chunk >= segment.byte_end {
            segment.clusters.last()?.source_end
        } else {
            source_at(&segment.source_map, chunk).0
        };
        Some(TextPosition {
            node: segment.text_node,
            offset: offset as usize,
        })
    }
}

/// 片段树遍历器：按绘制顺序产出 line / segment。
struct Builder {
    lines: Vec<Line>,
    segments: Vec<Segment>,
    segment_line: Vec<usize>,
    next_block: usize,
}

impl Builder {
    /// `block` 为 `Some` 表示本片段是行内原子盒，沿用包含块的 id。
    fn collect(&mut self, fragment: &Fragment, ox: f32, oy: f32, block: Option<usize>) {
        let border_x = ox + fragment.border_box.x;
        let border_y = oy + fragment.border_box.y;
        let content_x = border_x + fragment.border.left + fragment.padding.left;
        let content_y = border_y + fragment.border.top + fragment.padding.top;
        let block = block.unwrap_or_else(|| {
            let id = self.next_block;
            self.next_block += 1;
            id
        });

        let mut line_offset = 0.0_f32;
        for line in &fragment.lines {
            let top = content_y + line_offset;
            let line_index = self.lines.len();
            let mut segments = Vec::new();
            for run in &line.runs {
                if let Some(segment) = Segment::new(run, content_x) {
                    let index = self.segments.len();
                    self.segments.push(segment);
                    self.segment_line.push(line_index);
                    segments.push(index);
                }
            }
            self.lines.push(Line {
                top,
                height: line.height,
                start: line.start,
                block,
                segments,
            });
            // 行内原子盒：沿用包含块的 block id（视觉上仍在同一行内）
            for atomic in &line.boxes {
                self.collect(atomic, content_x, top, Some(block));
            }
            line_offset += line.height;
        }
        for child in &fragment.children {
            self.collect(child, content_x, content_y, None);
        }
    }
}

fn order(start: (usize, u32), end: (usize, u32)) -> ((usize, u32), (usize, u32)) {
    if start <= end {
        (start, end)
    } else {
        (end, start)
    }
}

fn consider(best: &mut Option<(f32, TextPosition)>, distance: f32, position: TextPosition) {
    if best.is_none_or(|(current, _)| distance < current) {
        *best = Some((distance, position));
    }
}

fn after_cluster(segment: &Segment, index: usize) -> u32 {
    segment
        .clusters
        .get(index + 1)
        .map_or(segment.clusters[index].source_end, |next| next.source)
}

/// chunk 字节对应的源区间；用于 cluster 边界，通常有精确匹配。
fn source_at(map: &[SourceChar], chunk: u32) -> (u32, u32) {
    let index = map.partition_point(|entry| entry.chunk < chunk);
    match map.get(index) {
        Some(entry) if entry.chunk == chunk => (entry.source, entry.source_end),
        _ => match index.checked_sub(1).and_then(|index| map.get(index)) {
            Some(entry) => (entry.source_end, entry.source_end),
            None => (0, 0),
        },
    }
}
