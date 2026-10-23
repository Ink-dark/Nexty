//! 页面文本选区状态机（纯逻辑，可无头测试）。
//!
//! 选区端点用 [`TextPosition`]（文本节点 + 源字节偏移）表示，几何与文本由
//! `nexty_layout::TextMap` 提供。本模块只管理 anchor/focus 与拖拽状态，
//! 不依赖 winit，也不直接触碰剪贴板——便于单测。
//!
//! 行为参考 DOM Selection 的 anchor/focus 模型：拖拽或 Shift+点击移动 focus，
//! 单击重置为折叠选区，导航/重排后可重新解析端点。

use nexty_layout::{Rect, TextMap, TextPosition};

/// 按下与释放的位移阈值（px）：小于等于该值算点击，否则算拖拽选择。
pub const CLICK_SLOP: f32 = 4.0;

/// 按下 → 释放是否为「点击」（拖拽选择时不触发链接导航）。
#[must_use]
pub fn is_click(press: (f32, f32), release: (f32, f32)) -> bool {
    (release.0 - press.0).abs() <= CLICK_SLOP && (release.1 - press.1).abs() <= CLICK_SLOP
}

/// 页面文本选区。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    anchor: Option<TextPosition>,
    focus: Option<TextPosition>,
    dragging: bool,
}

impl Selection {
    /// 空选区。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 按下：重置锚点并进入拖拽；`extend`（Shift）且已有选区时扩展 focus。
    pub fn begin(&mut self, position: TextPosition, extend: bool) {
        if extend && self.anchor.is_some() {
            self.focus = Some(position);
        } else {
            self.anchor = Some(position);
            self.focus = Some(position);
        }
        self.dragging = true;
    }

    /// 拖拽中移动 focus（未处于拖拽状态时忽略）。
    pub fn extend_to(&mut self, position: TextPosition) {
        if self.dragging && self.anchor.is_some() {
            self.focus = Some(position);
        }
    }

    /// 结束拖拽。
    pub fn finish(&mut self) {
        self.dragging = false;
    }

    /// 直接设置选区（双击选词、全选）。
    pub fn select(&mut self, from: TextPosition, to: TextPosition) {
        self.anchor = Some(from);
        self.focus = Some(to);
        self.dragging = false;
    }

    /// 清空选区。
    pub fn clear(&mut self) {
        self.anchor = None;
        self.focus = None;
        self.dragging = false;
    }

    /// 是否正在拖拽。
    #[must_use]
    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// 选区是否为空（无端点或 anchor == focus）。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match (self.anchor, self.focus) {
            (Some(anchor), Some(focus)) => anchor == focus,
            _ => true,
        }
    }

    /// 选区的两个端点（未选中任何文本时为 `None`）。
    #[must_use]
    pub fn positions(&self) -> Option<(TextPosition, TextPosition)> {
        Some((self.anchor?, self.focus?))
    }

    /// 重排后把端点夹到当前布局的文本范围内。
    ///
    /// 文本节点已消失（导航/重排后内容变化）时清空选区；返回是否仍有非空选区。
    pub fn resolve(&mut self, map: &TextMap) -> bool {
        let Some((anchor, focus)) = self.positions() else {
            return false;
        };
        match (map.resolve(anchor), map.resolve(focus)) {
            (Some(anchor), Some(focus)) => {
                self.anchor = Some(anchor);
                self.focus = Some(focus);
                !self.is_empty()
            }
            _ => {
                self.clear();
                false
            }
        }
    }

    /// 选区矩形（逐行，空选区为空）。
    #[must_use]
    pub fn rects(&self, map: &TextMap) -> Vec<Rect> {
        self.positions()
            .map_or_else(Vec::new, |(anchor, focus)| map.rects(anchor, focus))
    }

    /// 复制文本（空选区为空字符串）。
    #[must_use]
    pub fn text(&self, map: &TextMap) -> String {
        self.positions()
            .map_or_else(String::new, |(anchor, focus)| map.text(anchor, focus))
    }
}
