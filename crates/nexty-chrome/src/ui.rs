//! 自绘浏览器 UI：工具带（导航按钮 + 地址栏）状态机与绘制。
//!
//! 没有控件库——工具带是纯状态机 + 显示列表指令（背景填充、按钮底色、
//! 输入框描边、文本 run、选区高亮、光标竖线），与页面走同一条渲染管线。
//! 绘制坐标系：视口左上角为原点。
//!
//! 地址栏是可编辑的单行输入框：光标（`caret`，字节偏移且始终落在 char 边界）
//! 与选区（`anchor`），支持 `←`/`→`/Home/End 移动、Shift+方向扩选、Ctrl+A 全选、
//! Backspace/Delete 删选区或单字符、输入字符替换选区；点击输入框 = 聚焦并全选
//! （Chrome 语义）。文本超出输入框宽度时按光标位置横向滚动。
//!
//! 已知偏差：按钮字形（←/→/⟳）依赖字体回退，整形失败时降级为只画按钮底色；
//! 按钮无悬停态（未跟踪光标）；点击输入框不按 x 落光标（事件层拿不到整形度量，
//! 故只做聚焦 + 全选）；加载进度条为静态指示，无动画。

use nexty_paint::{Color, Command, Rect, Scene, TextGlyph};
use nexty_text::{TextShaper, TextStyle};

/// 工具带高度，px。
pub const BAR_HEIGHT: f32 = 36.0;
/// 输入框内边距，px。
const INSET: f32 = 4.0;
/// 控件间距，px。
const GAP: f32 = 4.0;
/// 文本左缘留白，px。
const TEXT_PADDING: f32 = 12.0;
/// 地址栏字号，px。
const FONT_SIZE: f32 = 14.0;
/// 导航按钮边长（正方形），px。
pub const BUTTON_SIZE: f32 = 28.0;
/// 工具按钮数量（后退/前进/刷新）。
const BUTTON_COUNT: usize = 3;
/// 加载进度条厚度，px。
const LOADING_STRIP_HEIGHT: f32 = 3.0;
/// 选区高亮色（半透明蓝，叠加在输入框白底上）。
const SELECTION_COLOR: Color = Color {
    r: 0x1a,
    g: 0x73,
    b: 0xe8,
    a: 0x59,
};
/// 光标竖线宽度，px。
const CARET_WIDTH: f32 = 1.0;
/// 光标距输入框上下缘的留白，px。
const CARET_INSET: f32 = 3.0;

/// 工具带按钮（从左到右）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarButton {
    /// 后退。
    Back,
    /// 前进。
    Forward,
    /// 刷新。
    Reload,
}

impl BarButton {
    fn index(self) -> usize {
        match self {
            BarButton::Back => 0,
            BarButton::Forward => 1,
            BarButton::Reload => 2,
        }
    }

    fn all() -> [BarButton; BUTTON_COUNT] {
        [BarButton::Back, BarButton::Forward, BarButton::Reload]
    }

    /// 按钮字形；整形失败时按钮仍可点击，只是不画字形。
    fn glyph(self) -> &'static str {
        match self {
            BarButton::Back => "\u{2190}",
            BarButton::Forward => "\u{2192}",
            BarButton::Reload => "\u{27f3}",
        }
    }
}

/// 按钮触发的工具命令（由窗口外壳执行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarCommand {
    /// 后退到上一条历史。
    Back,
    /// 前进到下一条历史。
    Forward,
    /// 重新加载当前页。
    Reload,
}

/// 光标移动方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaretDirection {
    /// 左移一个字符（或收拢到选区起点）。
    Left,
    /// 右移一个字符（或收拢到选区终点）。
    Right,
    /// 移到文本开头。
    Home,
    /// 移到文本末尾。
    End,
}

/// 窗口事件的抽象键（与具体窗口库解耦，便于单测）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// 字符键（组合键用；大小写不敏感）。
    Character(char),
    /// 左方向键。
    Left,
    /// 右方向键。
    Right,
    /// Home。
    Home,
    /// End。
    End,
    /// F5。
    F5,
    /// Esc。
    Escape,
}

/// 修饰键状态。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    /// Ctrl。
    pub ctrl: bool,
    /// Shift。
    pub shift: bool,
    /// Alt。
    pub alt: bool,
}

/// 浏览器级快捷键命令（由窗口外壳执行）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    /// 后退。
    Back,
    /// 前进。
    Forward,
    /// 重新加载当前页。
    Reload,
    /// 聚焦地址栏并全选。
    FocusAddressBar,
    /// 全选（本轮只对地址栏生效，页面文本选择未实现）。
    SelectAll,
    /// 失焦并还原当前页 URL（丢弃未提交的编辑）。
    Escape,
}

/// 把「键 + 修饰键」映射为浏览器命令（Chrome 常用快捷键）。
///
/// 未命中的组合返回 `None`，由调用方按普通编辑/滚动键处理。
#[must_use]
pub fn shortcut(key: Key, modifiers: Modifiers) -> Option<Shortcut> {
    match key {
        Key::Left if modifiers.alt => Some(Shortcut::Back),
        Key::Right if modifiers.alt => Some(Shortcut::Forward),
        Key::Character('l' | 'L') if modifiers.ctrl => Some(Shortcut::FocusAddressBar),
        Key::Character('a' | 'A') if modifiers.ctrl => Some(Shortcut::SelectAll),
        Key::Character('r' | 'R') if modifiers.ctrl => Some(Shortcut::Reload),
        Key::F5 => Some(Shortcut::Reload),
        Key::Escape => Some(Shortcut::Escape),
        _ => None,
    }
}

/// 按钮的命中矩形（视口坐标）。
fn button_rect(button: BarButton) -> Rect {
    let x = INSET + button.index() as f32 * (BUTTON_SIZE + GAP);
    Rect {
        x,
        y: (BAR_HEIGHT - BUTTON_SIZE) / 2.0,
        width: BUTTON_SIZE,
        height: BUTTON_SIZE,
    }
}

/// 地址栏输入框的矩形（视口坐标；右侧给按钮让位）。
///
/// 供外壳判定悬停光标（输入框范围内是 I 形）。
#[must_use]
pub fn input_rect(viewport_width: f32) -> Rect {
    let x = INSET + BUTTON_COUNT as f32 * (BUTTON_SIZE + GAP);
    Rect {
        x,
        y: INSET,
        width: (viewport_width - x - INSET).max(0.0),
        height: BAR_HEIGHT - 2.0 * INSET,
    }
}

/// 工具带（导航按钮 + 地址栏）。
#[derive(Debug, Clone)]
pub struct AddressBar {
    url: String,
    focused: bool,
    loading: bool,
    can_back: bool,
    can_forward: bool,
    /// 光标位置（字节偏移，始终落在 char 边界）。
    caret: usize,
    /// 选区锚点（字节偏移）；`None` 表示无选区。
    anchor: Option<usize>,
}

/// UI 输入事件（由窗口事件转译而来）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UiEvent {
    /// 输入一个字符（仅聚焦时生效；有选区时替换选区）。
    Character(char),
    /// 退格：删选区，或删除光标前一个字符。
    Backspace,
    /// 前向删除：删选区，或删除光标后一个字符。
    Delete,
    /// 移动光标；`extend` 为真（Shift）时扩选而非取消选区。
    MoveCaret {
        /// 移动方向。
        direction: CaretDirection,
        /// 是否按住 Shift 扩选。
        extend: bool,
    },
    /// 全选输入框内容。
    SelectAll,
    /// 提交（Enter）。
    Submit,
    /// 鼠标左键按下（按视口坐标路由到按钮或输入框）。
    Click {
        /// 视口 x。
        x: f32,
        /// 视口 y。
        y: f32,
    },
    /// 聚焦输入框并全选（Ctrl+L 等快捷键）。
    FocusAndSelect,
}

/// 一次事件处理的结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UiAction {
    /// 需要导航的 URL（归一化后）；为空表示无需导航。
    pub navigate: Option<String>,
    /// 需要执行的工具命令（按钮触发）。
    pub command: Option<BarCommand>,
}

impl AddressBar {
    /// 创建工具带并展示初始 URL（输入框聚焦，光标在末尾）。
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        let url = url.into();
        Self {
            caret: url.len(),
            url,
            focused: true,
            loading: false,
            can_back: false,
            can_forward: false,
            anchor: None,
        }
    }

    /// 当前展示的 URL 文本。
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// 输入框是否处于聚焦状态。
    #[must_use]
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// 当前选区 `(起点, 终点)`（归一化，字节偏移）；无选区为 `None`。
    #[must_use]
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        let (start, end) = (anchor.min(self.caret), anchor.max(self.caret));
        (start != end).then_some((start, end))
    }

    /// 展示一个 URL（导航完成后由外壳调用）；光标移到末尾并清空选区。
    pub fn set_url(&mut self, url: impl Into<String>) {
        self.url = url.into();
        self.caret = self.url.len();
        self.anchor = None;
    }

    /// 更新加载状态（外壳在导航开始/结束时调用）。
    pub fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
    }

    /// 更新后退/前进可用标志（外壳随历史栈变化调用）。
    pub fn set_history(&mut self, can_back: bool, can_forward: bool) {
        self.can_back = can_back;
        self.can_forward = can_forward;
    }

    /// 取消聚焦并清空选区（Esc）。
    pub fn blur(&mut self) {
        self.focused = false;
        self.anchor = None;
    }

    /// 处理一次 UI 事件。
    #[must_use]
    pub fn handle(&mut self, event: UiEvent) -> UiAction {
        match event {
            UiEvent::Character(character) => {
                if self.focused && !character.is_control() {
                    self.delete_selection();
                    self.url.insert(self.caret, character);
                    self.caret += character.len_utf8();
                    self.anchor = None;
                }
            }
            UiEvent::Backspace => {
                if self.focused {
                    if !self.delete_selection() && self.caret > 0 {
                        let previous = self.previous_boundary(self.caret);
                        self.url.replace_range(previous..self.caret, "");
                        self.caret = previous;
                    }
                    self.anchor = None;
                }
            }
            UiEvent::Delete => {
                if self.focused {
                    if !self.delete_selection() && self.caret < self.url.len() {
                        let next = self.next_boundary(self.caret);
                        self.url.replace_range(self.caret..next, "");
                    }
                    self.anchor = None;
                }
            }
            UiEvent::MoveCaret { direction, extend } => {
                if self.focused {
                    let target = match direction {
                        // 未扩选时，带选区的左/右移动先收拢到选区端点
                        CaretDirection::Left => match self.selection() {
                            Some((start, _)) if !extend => start,
                            _ => self.previous_boundary(self.caret),
                        },
                        CaretDirection::Right => match self.selection() {
                            Some((_, end)) if !extend => end,
                            _ => self.next_boundary(self.caret),
                        },
                        CaretDirection::Home => 0,
                        CaretDirection::End => self.url.len(),
                    };
                    self.move_caret(target, extend);
                }
            }
            UiEvent::SelectAll => {
                if self.focused {
                    self.anchor = Some(0);
                    self.caret = self.url.len();
                }
            }
            UiEvent::FocusAndSelect => {
                self.focused = true;
                self.anchor = Some(0);
                self.caret = self.url.len();
            }
            UiEvent::Submit => {
                if self.focused
                    && let Some(url) = normalize(&self.url)
                {
                    return UiAction {
                        navigate: Some(url),
                        command: None,
                    };
                }
            }
            UiEvent::Click { x, y } => {
                if y > BAR_HEIGHT {
                    // 页面区域：取消聚焦
                    self.focused = false;
                    self.anchor = None;
                    return UiAction::default();
                }
                // 工具带内：按钮命中优先（不改变聚焦态），否则进输入框
                for button in BarButton::all() {
                    let rect = button_rect(button);
                    if x >= rect.x
                        && x <= rect.x + rect.width
                        && y >= rect.y
                        && y <= rect.y + rect.height
                    {
                        let enabled = match button {
                            BarButton::Back => self.can_back,
                            BarButton::Forward => self.can_forward,
                            BarButton::Reload => true,
                        };
                        if enabled {
                            return UiAction {
                                navigate: None,
                                command: Some(match button {
                                    BarButton::Back => BarCommand::Back,
                                    BarButton::Forward => BarCommand::Forward,
                                    BarButton::Reload => BarCommand::Reload,
                                }),
                            };
                        }
                        return UiAction::default();
                    }
                }
                // 输入框：未聚焦时点击 = 聚焦并全选（Chrome 语义）
                if !self.focused {
                    self.anchor = Some(0);
                    self.caret = self.url.len();
                }
                self.focused = true;
            }
        }
        UiAction::default()
    }

    /// 把工具带绘制为显示列表指令（画在视口顶部）。
    pub fn draw(&self, shaper: &dyn TextShaper, scene: &mut Scene, viewport_width: f32) {
        // 背景条
        scene.commands.push(Command::FillRect {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: viewport_width,
                height: BAR_HEIGHT,
            },
            color: Color::opaque(0xdf, 0xdf, 0xdf),
        });
        // 加载进度条：静态强调色条（无动画），画在工具带下缘
        if self.loading {
            scene.commands.push(Command::FillRect {
                rect: Rect {
                    x: 0.0,
                    y: BAR_HEIGHT - LOADING_STRIP_HEIGHT,
                    width: viewport_width,
                    height: LOADING_STRIP_HEIGHT,
                },
                color: Color::opaque(0x1a, 0x73, 0xe8),
            });
        }

        let families = vec!["sans-serif".to_owned()];
        let style = TextStyle {
            families: families.clone(),
            size: FONT_SIZE,
        };

        // 导航按钮：底色 + 字形（整形失败降级为只画底色）
        for button in BarButton::all() {
            let rect = button_rect(button);
            let enabled = match button {
                BarButton::Back => self.can_back,
                BarButton::Forward => self.can_forward,
                BarButton::Reload => true,
            };
            if enabled {
                scene.commands.push(Command::FillRect {
                    rect,
                    color: Color::opaque(0xcc, 0xcc, 0xcc),
                });
            }
            if !enabled {
                continue;
            }
            let Ok(shaped) = shaper.shape(button.glyph(), &style) else {
                continue;
            };
            if shaped.glyphs.is_empty() {
                continue;
            }
            let glyph_width = shaped
                .glyphs
                .last()
                .map(|last| last.x + last.advance)
                .unwrap_or(0.0);
            let centering = (rect.width - glyph_width) / 2.0;
            scene.commands.push(Command::DrawText {
                glyphs: shaped
                    .glyphs
                    .iter()
                    .map(|glyph| TextGlyph {
                        id: glyph.id,
                        x: rect.x + centering + glyph.x,
                        y: glyph.y,
                    })
                    .collect(),
                baseline: rect.y + rect.height - 8.0,
                families: families.clone(),
                size: FONT_SIZE,
                color: Color::opaque(0x30, 0x30, 0x30),
            });
        }

        // 输入框：白底 + 描边（聚焦时高亮）
        let input = input_rect(viewport_width);
        scene.commands.push(Command::FillRect {
            rect: input,
            color: Color::opaque(0xff, 0xff, 0xff),
        });
        let border_color = if self.focused {
            Color::opaque(0x33, 0x66, 0xcc)
        } else {
            Color::opaque(0x99, 0x99, 0x99)
        };
        scene.commands.push(Command::StrokeRect {
            rect: input,
            color: border_color,
            width: 1.5,
        });

        self.draw_text(shaper, scene, &style, &families, input);
    }

    /// 输入框内的文本、选区高亮与光标。
    ///
    /// 文本超出输入框宽度时按光标位置横向滚动（光标移出右缘则整体左移）。
    /// 整形失败（系统字体缺失等）降级为不画文本：`draw` 在主线程执行，
    /// 渲染线程的 `catch_unwind` 兜底覆盖不到这里。
    fn draw_text(
        &self,
        shaper: &dyn TextShaper,
        scene: &mut Scene,
        style: &TextStyle,
        families: &[String],
        input: Rect,
    ) {
        let Ok(shaped) = shaper.shape(&self.url, style) else {
            return;
        };
        let max_width = (input.width - 2.0 * TEXT_PADDING).max(0.0);
        let caret_x = self.prefix_width(&self.url[..self.caret], shaper, style);
        // 横向滚动：把光标保持在输入框内
        let offset = if shaped.width > max_width {
            let mut offset = 0.0_f32;
            if caret_x > max_width {
                offset = caret_x - max_width;
            }
            if caret_x < offset {
                offset = caret_x;
            }
            offset.clamp(0.0, shaped.width - max_width)
        } else {
            0.0
        };
        let origin_x = input.x + TEXT_PADDING - offset;
        let baseline = input.y + input.height - 8.0;

        // 选区高亮（画在文本之下），夹取到输入框内
        if let Some((start, end)) = self.selection() {
            let start_x = origin_x + self.prefix_width(&self.url[..start], shaper, style);
            let end_x = origin_x + self.prefix_width(&self.url[..end], shaper, style);
            let left = start_x.max(input.x);
            let right = end_x.min(input.x + input.width);
            if right > left {
                scene.commands.push(Command::FillRect {
                    rect: Rect {
                        x: left,
                        y: input.y,
                        width: right - left,
                        height: input.height,
                    },
                    color: SELECTION_COLOR,
                });
            }
        }

        // 文本：逐字形按输入框范围裁剪，避免溢出到按钮上
        let glyphs: Vec<TextGlyph> = shaped
            .glyphs
            .iter()
            .filter(|glyph| {
                let x = origin_x + glyph.x;
                x + glyph.advance >= input.x && x <= input.x + input.width
            })
            .map(|glyph| TextGlyph {
                id: glyph.id,
                x: origin_x + glyph.x,
                y: glyph.y,
            })
            .collect();
        if !glyphs.is_empty() {
            scene.commands.push(Command::DrawText {
                glyphs,
                baseline,
                families: families.to_vec(),
                size: FONT_SIZE,
                color: Color::opaque(0x20, 0x20, 0x20),
            });
        }

        // 光标：聚焦且无选区时画竖线
        if self.focused && self.selection().is_none() {
            let caret = (origin_x + caret_x).clamp(input.x, input.x + input.width - CARET_WIDTH);
            scene.commands.push(Command::FillRect {
                rect: Rect {
                    x: caret,
                    y: input.y + CARET_INSET,
                    width: CARET_WIDTH,
                    height: input.height - 2.0 * CARET_INSET,
                },
                color: Color::opaque(0x20, 0x20, 0x20),
            });
        }
    }

    /// 前缀整形宽度（光标与选区端点的定位依据）。
    fn prefix_width(&self, text: &str, shaper: &dyn TextShaper, style: &TextStyle) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        shaper.shape(text, style).map_or(0.0, |shaped| shaped.width)
    }

    /// 光标左侧的 char 边界。
    fn previous_boundary(&self, index: usize) -> usize {
        self.url[..index]
            .chars()
            .next_back()
            .map_or(0, |character| index - character.len_utf8())
    }

    /// 光标右侧的 char 边界。
    fn next_boundary(&self, index: usize) -> usize {
        self.url[index..]
            .chars()
            .next()
            .map_or(self.url.len(), |character| index + character.len_utf8())
    }

    /// 把索引吸附到最近的 char 边界（防御中间偏移）。
    fn snap_boundary(&self, index: usize) -> usize {
        let mut index = index.min(self.url.len());
        while index > 0 && !self.url.is_char_boundary(index) {
            index -= 1;
        }
        index
    }

    /// 移动光标；`extend` 为真时以原光标为锚点扩选。
    fn move_caret(&mut self, index: usize, extend: bool) {
        let index = self.snap_boundary(index);
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.caret);
            }
        } else {
            self.anchor = None;
        }
        self.caret = index;
    }

    /// 删除当前选区并返回是否确有删除。
    fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection() else {
            return false;
        };
        self.url.replace_range(start..end, "");
        self.caret = start;
        self.anchor = None;
        true
    }
}

/// URL 归一化：无协议时补 `https://`；空串不导航。
fn normalize(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains("://") || trimmed.starts_with("about:") {
        return Some(trimmed.to_owned());
    }
    Some(format!("https://{trimmed}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexty_text::ParleyTextShaper;

    fn click_input(bar: &mut AddressBar) {
        let _ = bar.handle(UiEvent::Click { x: 100.0, y: 10.0 });
    }

    fn move_caret(bar: &mut AddressBar, direction: CaretDirection, extend: bool) {
        let _ = bar.handle(UiEvent::MoveCaret { direction, extend });
    }

    #[test]
    fn typing_accumulates_and_backspace_pops() {
        let mut bar = AddressBar::new("");
        for character in "example.test".chars() {
            let _ = bar.handle(UiEvent::Character(character));
        }
        assert_eq!(bar.url(), "example.test");
        let _ = bar.handle(UiEvent::Backspace);
        assert_eq!(bar.url(), "example.tes");
    }

    #[test]
    fn submit_normalizes_url() {
        let mut bar = AddressBar::new("example.test");
        let action = bar.handle(UiEvent::Submit);
        assert_eq!(action.navigate.as_deref(), Some("https://example.test"));

        bar.set_url("https://already.test/");
        let action = bar.handle(UiEvent::Submit);
        assert_eq!(action.navigate.as_deref(), Some("https://already.test/"));

        bar.set_url("about:blank");
        let action = bar.handle(UiEvent::Submit);
        assert_eq!(action.navigate.as_deref(), Some("about:blank"));

        bar.set_url("   ");
        let action = bar.handle(UiEvent::Submit);
        assert!(action.navigate.is_none());
    }

    #[test]
    fn click_sets_focus_and_gates_typing() {
        let mut bar = AddressBar::new("about:blank");
        // 点击页面区域 → 取消聚焦，输入不生效
        let _ = bar.handle(UiEvent::Click { x: 100.0, y: 200.0 });
        assert!(!bar.is_focused());
        let _ = bar.handle(UiEvent::Character('x'));
        assert_eq!(bar.url(), "about:blank");
        // 点击输入框 → 聚焦并全选，输入替换整段
        click_input(&mut bar);
        assert!(bar.is_focused());
        assert_eq!(bar.selection(), Some((0, "about:blank".len())));
        let _ = bar.handle(UiEvent::Character('x'));
        assert_eq!(bar.url(), "x");
        assert!(bar.selection().is_none(), "输入后选区被替换清除");
    }

    #[test]
    fn button_click_emits_commands_gated_by_flags() {
        let mut bar = AddressBar::new("about:blank");
        // 默认无历史：后退/前进禁用（点击无命令），刷新始终可用
        let back = button_rect(BarButton::Back);
        let action = bar.handle(UiEvent::Click {
            x: back.x + 1.0,
            y: back.y + 1.0,
        });
        assert_eq!(action.command, None);
        let action = bar.handle(UiEvent::Click {
            x: button_rect(BarButton::Reload).x + 1.0,
            y: 10.0,
        });
        assert_eq!(action.command, Some(BarCommand::Reload));

        // 可用标志打开后：后退/前进各自出命令
        bar.set_history(true, true);
        let action = bar.handle(UiEvent::Click {
            x: button_rect(BarButton::Back).x + 1.0,
            y: 10.0,
        });
        assert_eq!(action.command, Some(BarCommand::Back));
        let action = bar.handle(UiEvent::Click {
            x: button_rect(BarButton::Forward).x + 1.0,
            y: 10.0,
        });
        assert_eq!(action.command, Some(BarCommand::Forward));
    }

    #[test]
    fn button_click_does_not_steal_input_focus() {
        let mut bar = AddressBar::new("about:blank");
        // 输入框聚焦中点刷新按钮：聚焦保持不变，URL 不被输入干扰
        assert!(bar.is_focused());
        let _ = bar.handle(UiEvent::Click {
            x: button_rect(BarButton::Reload).x + 1.0,
            y: 10.0,
        });
        assert!(bar.is_focused());
        // 页面区域点击仍取消聚焦
        let _ = bar.handle(UiEvent::Click { x: 300.0, y: 200.0 });
        assert!(!bar.is_focused());
    }

    #[test]
    fn caret_moves_by_character_and_home_end() {
        let mut bar = AddressBar::new("abc");
        let _ = bar.handle(UiEvent::SelectAll); // 先把光标置于末尾
        assert!(bar.selection().is_some());

        move_caret(&mut bar, CaretDirection::Left, false);
        // 带选区时的左移先收拢到选区起点
        assert_eq!(bar.selection(), None);
        assert_eq!(bar.url(), "abc");
        // 再左移一次：光标在起点，插入点应在最前
        move_caret(&mut bar, CaretDirection::Left, false);
        let _ = bar.handle(UiEvent::Character('X'));
        assert_eq!(bar.url(), "Xabc");

        move_caret(&mut bar, CaretDirection::End, false);
        let _ = bar.handle(UiEvent::Character('Y'));
        assert_eq!(bar.url(), "XabcY");

        move_caret(&mut bar, CaretDirection::Home, false);
        let _ = bar.handle(UiEvent::Character('Z'));
        assert_eq!(bar.url(), "ZXabcY");
    }

    #[test]
    fn caret_stays_on_char_boundary() {
        // 多字节字符：按 char 移动/删除，不会切出非法 UTF-8
        let mut bar = AddressBar::new("中a");
        move_caret(&mut bar, CaretDirection::Home, false);
        // Home 后在 '中' 之前插入
        let _ = bar.handle(UiEvent::Character('文'));
        assert_eq!(bar.url(), "文中a");
        // 光标在插入的 '文' 之后 → 退格删 '文'
        let _ = bar.handle(UiEvent::Backspace);
        assert_eq!(bar.url(), "中a");
        // 光标此时在 '中' 之前 → Delete 前向删 '中'（三字节整体删除）
        let _ = bar.handle(UiEvent::Delete);
        assert_eq!(bar.url(), "a");
        let _ = bar.handle(UiEvent::Delete);
        assert_eq!(bar.url(), "");
    }

    #[test]
    fn shift_extends_selection_and_typing_replaces_it() {
        let mut bar = AddressBar::new("abcdef");
        move_caret(&mut bar, CaretDirection::Home, false);
        for _ in 0..3 {
            move_caret(&mut bar, CaretDirection::Right, true);
        }
        assert_eq!(bar.selection(), Some((0, 3)));
        assert_eq!(bar.url(), "abcdef", "扩选不改文本");
        // 输入替换选区
        let _ = bar.handle(UiEvent::Character('X'));
        assert_eq!(bar.url(), "Xdef");
        assert!(bar.selection().is_none());
    }

    #[test]
    fn extension_follows_caret_and_normalizes_reversed_range() {
        let mut bar = AddressBar::new("abcdef");
        move_caret(&mut bar, CaretDirection::End, false);
        for _ in 0..2 {
            move_caret(&mut bar, CaretDirection::Left, true);
        }
        // 反向扩选：选区归一化后仍是（起点, 终点）
        assert_eq!(bar.selection(), Some((4, 6)));
        let _ = bar.handle(UiEvent::Backspace);
        assert_eq!(bar.url(), "abcd", "退格删除选区");
        assert_eq!(bar.selection(), None);
    }

    #[test]
    fn delete_removes_selection_or_next_character() {
        let mut bar = AddressBar::new("abc");
        move_caret(&mut bar, CaretDirection::Home, false);
        let _ = bar.handle(UiEvent::Delete);
        assert_eq!(bar.url(), "bc", "Delete 删除光标后的字符");

        let _ = bar.handle(UiEvent::SelectAll);
        let _ = bar.handle(UiEvent::Delete);
        assert_eq!(bar.url(), "", "Delete 删除选区");
    }

    #[test]
    fn select_all_then_backspace_clears_and_replace_resets_caret() {
        let mut bar = AddressBar::new("https://example.test/");
        let _ = bar.handle(UiEvent::SelectAll);
        assert_eq!(bar.selection(), Some((0, bar.url.len())));
        let _ = bar.handle(UiEvent::Backspace);
        assert!(bar.url().is_empty());
        // 空输入框上 Backspace/Delete 无副作用
        let _ = bar.handle(UiEvent::Backspace);
        let _ = bar.handle(UiEvent::Delete);
        assert!(bar.url().is_empty());

        // set_url 重置编辑态：光标在末尾、无选区
        bar.set_url("about:blank");
        assert_eq!(bar.selection(), None);
        let _ = bar.handle(UiEvent::Backspace);
        assert_eq!(bar.url(), "about:blan");
    }

    #[test]
    fn blur_clears_selection_and_gates_editing() {
        let mut bar = AddressBar::new("abc");
        let _ = bar.handle(UiEvent::SelectAll);
        bar.blur();
        assert!(!bar.is_focused());
        assert_eq!(bar.selection(), None);
        let _ = bar.handle(UiEvent::Character('x'));
        assert_eq!(bar.url(), "abc", "失焦后输入不生效");
        // FocusAndSelect：重新聚焦并全选
        let _ = bar.handle(UiEvent::FocusAndSelect);
        assert!(bar.is_focused());
        assert_eq!(bar.selection(), Some((0, 3)));
    }

    #[test]
    fn caret_and_moves_are_inert_when_unfocused() {
        let mut bar = AddressBar::new("abc");
        let _ = bar.handle(UiEvent::Click { x: 10.0, y: 200.0 });
        assert!(!bar.is_focused());
        move_caret(&mut bar, CaretDirection::Home, false);
        let _ = bar.handle(UiEvent::SelectAll);
        let _ = bar.handle(UiEvent::Delete);
        assert_eq!(bar.url(), "abc");
        assert_eq!(bar.selection(), None);
    }

    #[test]
    fn draw_emits_bar_commands() {
        let mut bar = AddressBar::new("example.test");
        bar.set_history(true, false);
        let shaper = ParleyTextShaper::new();
        let mut scene = Scene::default();
        bar.draw(&shaper, &mut scene, 400.0);

        let fills = scene
            .commands
            .iter()
            .filter(|command| matches!(command, Command::FillRect { .. }))
            .count();
        let strokes = scene
            .commands
            .iter()
            .filter(|command| matches!(command, Command::StrokeRect { .. }))
            .count();
        let texts = scene
            .commands
            .iter()
            .filter(|command| matches!(command, Command::DrawText { .. }))
            .count();
        // 背景条 + 可用按钮底色（后退/刷新）+ 输入框白底 + 聚焦光标 = 5
        assert_eq!(fills, 5);
        assert_eq!(strokes, 1, "输入框描边");
        // URL 文本 + 两个可用按钮字形
        assert_eq!(texts, 3);
    }

    /// 选区高亮与光标互斥：有选区画高亮带，无选区画光标竖线。
    #[test]
    fn draw_emits_selection_highlight_or_caret() {
        let shaper = ParleyTextShaper::new();
        let mut bar = AddressBar::new("example.test");

        // 无选区（默认光标在末尾）：有光标竖线、无选区色
        let mut scene = Scene::default();
        bar.draw(&shaper, &mut scene, 400.0);
        assert!(
            scene
                .commands
                .iter()
                .any(|command| matches!(command, Command::FillRect { color, .. }
                    if *color == Color::opaque(0x20, 0x20, 0x20))),
            "聚焦且无选区应有光标竖线"
        );
        assert!(
            !scene
                .commands
                .iter()
                .any(|command| matches!(command, Command::FillRect { color, .. }
                    if *color == SELECTION_COLOR)),
            "无选区时不应有高亮带"
        );

        // 全选后有高亮带、无光标竖线
        let _ = bar.handle(UiEvent::SelectAll);
        let mut scene = Scene::default();
        bar.draw(&shaper, &mut scene, 400.0);
        assert!(
            scene
                .commands
                .iter()
                .any(|command| matches!(command, Command::FillRect { color, .. }
                    if *color == SELECTION_COLOR)),
            "选中应有高亮带"
        );
        assert!(
            !scene
                .commands
                .iter()
                .any(|command| matches!(command, Command::FillRect { color, .. }
                    if *color == Color::opaque(0x20, 0x20, 0x20))),
            "有选区时不画光标竖线"
        );
    }

    /// 选区高亮带不越出输入框：超长 URL 全选后高亮带仍夹在输入框内。
    #[test]
    fn selection_highlight_is_clipped_to_input_box() {
        let shaper = ParleyTextShaper::new();
        let mut bar = AddressBar::new("https://example.test/".repeat(20));
        let _ = bar.handle(UiEvent::SelectAll);
        let mut scene = Scene::default();
        bar.draw(&shaper, &mut scene, 400.0);

        let input = input_rect(400.0);
        let highlight = scene
            .commands
            .iter()
            .find_map(|command| match command {
                Command::FillRect { rect, color } if *color == SELECTION_COLOR => Some(*rect),
                _ => None,
            })
            .expect("超长文本全选应有高亮带");
        assert!(highlight.x >= input.x - 0.01);
        assert!(highlight.x + highlight.width <= input.x + input.width + 0.01);
        assert!(highlight.width > 0.0);
    }

    #[test]
    fn shortcut_maps_chrome_keys_and_rejects_others() {
        let none = Modifiers::default();
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };

        assert_eq!(shortcut(Key::Left, alt), Some(Shortcut::Back));
        assert_eq!(shortcut(Key::Right, alt), Some(Shortcut::Forward));
        // 大小写不敏感
        assert_eq!(
            shortcut(Key::Character('l'), ctrl),
            Some(Shortcut::FocusAddressBar)
        );
        assert_eq!(
            shortcut(Key::Character('L'), ctrl),
            Some(Shortcut::FocusAddressBar)
        );
        assert_eq!(
            shortcut(Key::Character('a'), ctrl),
            Some(Shortcut::SelectAll)
        );
        assert_eq!(shortcut(Key::Character('r'), ctrl), Some(Shortcut::Reload));
        assert_eq!(shortcut(Key::F5, none), Some(Shortcut::Reload));
        assert_eq!(shortcut(Key::Escape, none), Some(Shortcut::Escape));

        // 未命中：无修饰的方向键（须走光标/滚动）、无 Ctrl 的字母键
        assert_eq!(shortcut(Key::Left, none), None);
        assert_eq!(shortcut(Key::Right, none), None);
        assert_eq!(shortcut(Key::Home, none), None);
        assert_eq!(shortcut(Key::Character('l'), none), None);
        assert_eq!(shortcut(Key::Character('x'), ctrl), None);
        // Alt+字母 不是快捷键
        assert_eq!(
            shortcut(Key::Character('l'), alt),
            None,
            "Alt+L 不应聚焦地址栏"
        );
    }

    #[test]
    fn loading_strip_drawn_only_while_loading() {
        let shaper = ParleyTextShaper::new();
        let mut bar = AddressBar::new("example.test");
        let mut scene = Scene::default();
        bar.draw(&shaper, &mut scene, 400.0);
        assert!(
            !scene
                .commands
                .iter()
                .any(|command| matches!(command, Command::FillRect { color, .. }
                    if *color == Color::opaque(0x1a, 0x73, 0xe8))),
            "未加载时无进度条"
        );

        bar.set_loading(true);
        let mut scene = Scene::default();
        bar.draw(&shaper, &mut scene, 400.0);
        let strip = scene.commands.iter().any(|command| {
            matches!(command, Command::FillRect { rect, color }
                if *color == Color::opaque(0x1a, 0x73, 0xe8)
                    && rect.y + rect.height == BAR_HEIGHT)
        });
        assert!(strip, "加载中应有下缘进度条");
    }

    /// 整形失败的 shaper：验证 draw 降级为「不画文本」而非 panic。
    struct FailingShaper;

    impl nexty_text::TextShaper for FailingShaper {
        fn shape(
            &self,
            _text: &str,
            _style: &nexty_text::TextStyle,
        ) -> Result<nexty_text::ShapedText, nexty_text::TextError> {
            Err(nexty_text::TextError::FontUnavailable)
        }

        fn metrics(
            &self,
            _style: &nexty_text::TextStyle,
        ) -> Result<nexty_text::FontMetrics, nexty_text::TextError> {
            Err(nexty_text::TextError::FontUnavailable)
        }
    }

    /// draw 在主线程执行，整形失败不能 panic——须降级：地址栏背景与输入框
    /// 照常绘制，只是不产出文本指令。
    #[test]
    fn draw_degrades_gracefully_when_shaping_fails() {
        let bar = AddressBar::new("https://example.test/some/path");
        let mut scene = Scene::default();
        bar.draw(&FailingShaper, &mut scene, 400.0);

        // 背景条与输入框仍绘制
        let fills = scene
            .commands
            .iter()
            .filter(|command| matches!(command, Command::FillRect { .. }))
            .count();
        assert!(fills >= 2, "背景条 + 输入框白底仍应绘制");
        // 但没有文本指令
        let texts = scene
            .commands
            .iter()
            .filter(|command| matches!(command, Command::DrawText { .. }))
            .count();
        assert_eq!(texts, 0, "整形失败时不应产出文本指令");
    }
}
