//! 自绘浏览器 UI：工具带（导航按钮 + 地址栏）状态机与绘制。
//!
//! 没有控件库——工具带是纯状态机 + 显示列表指令（背景填充、按钮底色、
//! 输入框描边、文本 run），与页面走同一条渲染管线。绘制坐标系：视口
//! 左上角为原点。
//!
//! 已知偏差：按钮字形（←/→/⟳）依赖字体回退，整形失败时降级为只画按钮
//! 底色；按钮无悬停态（未跟踪光标）；加载进度条为静态指示，无动画。

use nexty_paint::{Color, Command, Rect, Scene, TextGlyph};
use nexty_text::TextShaper;

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
fn input_rect(viewport_width: f32) -> Rect {
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
}

/// UI 输入事件（由窗口事件转译而来）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UiEvent {
    /// 输入一个字符（仅聚焦时生效）。
    Character(char),
    /// 退格（仅聚焦时生效）。
    Backspace,
    /// 提交（Enter）。
    Submit,
    /// 鼠标左键按下（按视口坐标路由到按钮或输入框）。
    Click { x: f32, y: f32 },
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
    /// 创建工具带并展示初始 URL（输入框聚焦）。
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            focused: true,
            loading: false,
            can_back: false,
            can_forward: false,
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

    /// 展示一个 URL（导航完成后由外壳调用）。
    pub fn set_url(&mut self, url: impl Into<String>) {
        self.url = url.into();
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

    /// 处理一次 UI 事件。
    #[must_use]
    pub fn handle(&mut self, event: UiEvent) -> UiAction {
        match event {
            UiEvent::Character(character) => {
                if self.focused && !character.is_control() {
                    self.url.push(character);
                }
            }
            UiEvent::Backspace => {
                if self.focused {
                    self.url.pop();
                }
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
                self.focused = true;
            }
        }
        UiAction::default()
    }

    /// 把工具带绘制为显示列表指令（画在视口顶部）。
    ///
    /// URL 文本超出输入框宽度时截断（不做省略号，简化处理）。
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
        let style = nexty_text::TextStyle {
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

        // URL 文本（超出宽度时按字符截断）
        let max_width = input.width - 2.0 * TEXT_PADDING;
        let mut text = self.url.clone();
        // 每轮缩短一个字符直到放得下，用 `loop` 而非 `while let`：终止条件是
        // 「宽度达标或文本为空」，不是「Option 为 None」。
        #[allow(clippy::while_let_loop)]
        loop {
            // 整形失败（系统字体缺失等）不能 panic：draw 在主线程执行，
            // 渲染线程的 catch_unwind 兜底覆盖不到这里。降级为不画文本，
            // 输入框仍正常显示。
            let Ok(shaped) = shaper.shape(&text, &style) else {
                break;
            };
            if shaped.width <= max_width || text.is_empty() {
                if !shaped.glyphs.is_empty() {
                    scene.commands.push(Command::DrawText {
                        glyphs: shaped
                            .glyphs
                            .iter()
                            .map(|glyph| TextGlyph {
                                id: glyph.id,
                                x: input.x + TEXT_PADDING + glyph.x,
                                y: glyph.y,
                            })
                            .collect(),
                        baseline: input.y + input.height - 8.0,
                        families: families.clone(),
                        size: FONT_SIZE,
                        color: Color::opaque(0x20, 0x20, 0x20),
                    });
                }
                break;
            }
            text.pop();
        }
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
        // 点击输入框 → 聚焦，输入生效
        let _ = bar.handle(UiEvent::Click { x: 100.0, y: 10.0 });
        assert!(bar.is_focused());
        let _ = bar.handle(UiEvent::Character('x'));
        assert_eq!(bar.url(), "about:blankx");
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
        // 背景条 + 可用按钮底色（后退/刷新）+ 输入框白底 = 4
        assert_eq!(fills, 4);
        assert_eq!(strokes, 1, "输入框描边");
        // URL 文本 + 两个可用按钮字形
        assert_eq!(texts, 3);
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
