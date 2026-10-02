//! 自绘浏览器 UI：地址栏状态机与绘制。
//!
//! 没有控件库——地址栏是纯状态机 + 显示列表指令（背景填充、输入框描边、
//! 文本 run），与页面走同一条渲染管线。绘制坐标系：视口左上角为原点。

use nexty_paint::{Color, Command, Rect, Scene, TextGlyph};
use nexty_text::TextShaper;

/// 地址栏高度，px。
pub const BAR_HEIGHT: f32 = 36.0;
/// 输入框内边距，px。
const INSET: f32 = 4.0;
/// 文本左缘留白，px。
const TEXT_PADDING: f32 = 12.0;
/// 地址栏字号，px。
const FONT_SIZE: f32 = 14.0;

/// 地址栏。
#[derive(Debug, Clone)]
pub struct AddressBar {
    url: String,
    focused: bool,
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
    /// 鼠标左键按下（按视口坐标决定是否聚焦地址栏）。
    Click { x: f32, y: f32 },
}

/// 一次事件处理的结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UiAction {
    /// 需要导航的 URL（归一化后）；为空表示无需导航。
    pub navigate: Option<String>,
}

impl AddressBar {
    /// 创建地址栏并展示初始 URL（不聚焦）。
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            focused: true,
        }
    }

    /// 当前展示的 URL 文本。
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// 是否处于聚焦状态。
    #[must_use]
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// 展示一个 URL（导航完成后由外壳调用）。
    pub fn set_url(&mut self, url: impl Into<String>) {
        self.url = url.into();
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
                    };
                }
            }
            UiEvent::Click { x, y } => {
                self.focused = y <= BAR_HEIGHT && x > INSET;
            }
        }
        UiAction::default()
    }

    /// 把地址栏绘制为显示列表指令（画在视口顶部）。
    ///
    /// 文本超出输入框宽度时截断（不做省略号，简化处理）。
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
        // 输入框：白底 + 描边（聚焦时高亮）
        let input = Rect {
            x: INSET,
            y: INSET,
            width: viewport_width - 2.0 * INSET,
            height: BAR_HEIGHT - 2.0 * INSET,
        };
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
        let families = vec!["sans-serif".to_owned()];
        let style = nexty_text::TextStyle {
            families: families.clone(),
            size: FONT_SIZE,
        };
        let max_width = input.width - 2.0 * TEXT_PADDING;
        let mut text = self.url.clone();
        // 每轮缩短一个字符直到放得下，用 `loop` 而非 `while let`：终止条件是
        // 「宽度达标或文本为空」，不是「Option 为 None」。
        #[allow(clippy::while_let_loop)]
        loop {
            // 整形失败（系统字体缺失等）不能 panic：draw 在主线程执行，
            // 渲染线程的 catch_unwind 兜底覆盖不到这里。降级为不画文本，
            // 地址栏的背景条与输入框仍正常显示。
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
        // 点击地址栏 → 聚焦，输入生效
        let _ = bar.handle(UiEvent::Click { x: 100.0, y: 10.0 });
        assert!(bar.is_focused());
        let _ = bar.handle(UiEvent::Character('x'));
        assert_eq!(bar.url(), "about:blankx");
    }

    #[test]
    fn draw_emits_bar_commands() {
        let bar = AddressBar::new("example.test");
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
        assert!(fills >= 2, "背景条 + 输入框白底");
        assert_eq!(strokes, 1, "输入框描边");
        assert_eq!(texts, 1, "URL 文本");
        // 文本 run 的字形非空
        match scene
            .commands
            .iter()
            .find(|command| matches!(command, Command::DrawText { .. }))
        {
            Some(Command::DrawText { glyphs, .. }) => assert!(!glyphs.is_empty()),
            _ => unreachable!(),
        }
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
