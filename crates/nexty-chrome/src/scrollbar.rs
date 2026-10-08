//! 滚动条几何与交互状态机（纯逻辑，无窗口依赖，可单测）。
//!
//! 几何：轨道自工具带下缘（`bar_height`）延伸到底、右贴视口边缘；thumb 高度按
//! 「轨道高 × 视口高 / 内容总高」取，并不低于 [`MIN_THUMB`]。页面不溢出时既不
//! 绘制也不响应。
//!
//! 交互对齐主流浏览器：按住 thumb 拖动（记录抓取偏移，thumb 不随光标跳变）、
//! 点击轨道空白处翻一页。悬停态只影响绘制色，不改变命中区——命中区随悬停加宽
//! 会让拖拽过程中尺寸抖动。

use nexty_paint::Rect;

/// 滚动条轨道宽度，px。
pub const TRACK_WIDTH: f32 = 8.0;
/// thumb 最小高度，px。
pub const MIN_THUMB: f32 = 24.0;

/// 把滚动量限制到 `[0, max]`。
#[must_use]
pub fn clamp_scroll(scroll: f32, max: f32) -> f32 {
    scroll.clamp(0.0, max)
}

/// 一次布局后的滚动几何。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollGeometry {
    /// 视口宽（轨道右贴边）。
    pub viewport_width: f32,
    /// 视口高。
    pub viewport_height: f32,
    /// 页面内容高（不含工具带）。
    pub content_height: f32,
    /// 顶部工具带高度（轨道起点）。
    pub bar_height: f32,
}

impl ScrollGeometry {
    /// 内容总高（页面 + 工具带）。
    fn total(&self) -> f32 {
        self.content_height + self.bar_height
    }

    /// 最大滚动量：内容总高超出视口的部分，下限 0。
    #[must_use]
    pub fn max_scroll(&self) -> f32 {
        (self.total() - self.viewport_height).max(0.0)
    }

    /// 页面是否溢出视口（决定滚动条是否出现）。
    #[must_use]
    pub fn overflows(&self) -> bool {
        self.total() > self.viewport_height
    }

    /// 可见页高（= 轨道高），轨道点击的翻页步长。
    #[must_use]
    pub fn page_step(&self) -> f32 {
        (self.viewport_height - self.bar_height).max(0.0)
    }

    /// 轨道矩形（视口坐标）；不溢出时为 `None`。
    #[must_use]
    pub fn track(&self) -> Option<Rect> {
        if !self.overflows() {
            return None;
        }
        Some(Rect {
            x: self.viewport_width - TRACK_WIDTH,
            y: self.bar_height,
            width: TRACK_WIDTH,
            height: self.page_step(),
        })
    }

    /// thumb 矩形（视口坐标）；不溢出时为 `None`。
    ///
    /// thumb 高 = 轨道高 × 视口高 / 内容总高，不低于 [`MIN_THUMB`]；y 按滚动
    /// 比例落在轨道内。
    #[must_use]
    pub fn thumb(&self, scroll: f32) -> Option<Rect> {
        if !self.overflows() {
            return None;
        }
        let track = self.page_step();
        let thumb_height = (track * self.viewport_height / self.total()).max(MIN_THUMB);
        let progress = scroll / self.max_scroll();
        Some(Rect {
            x: self.viewport_width - TRACK_WIDTH,
            y: self.bar_height + (track - thumb_height) * progress,
            width: TRACK_WIDTH,
            height: thumb_height,
        })
    }

    /// thumb 顶边 y 反解滚动量（拖拽映射；行程为 0 时固定返回 0）。
    fn scroll_for_thumb_y(&self, thumb_y: f32, thumb_height: f32) -> f32 {
        let travel = self.page_step() - thumb_height;
        if travel <= 0.0 {
            return 0.0;
        }
        let progress = ((thumb_y - self.bar_height) / travel).clamp(0.0, 1.0);
        progress * self.max_scroll()
    }
}

/// 光标落在滚动条的哪个区域（视口坐标）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollbarHit {
    /// 落在 thumb 上（可拖拽）。
    Thumb,
    /// 落在 thumb 上方的轨道空白（向上翻页）。
    TrackAbove,
    /// 落在 thumb 下方的轨道空白（向下翻页）。
    TrackBelow,
    /// 未落在滚动条上。
    None,
}

/// 滚动条交互状态（悬停 / 拖拽）。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Scrollbar {
    hovered: bool,
    dragging: bool,
    /// 按下 thumb 时，光标相对 thumb 顶边的偏移（px）。
    grab_offset: f32,
}

impl Scrollbar {
    /// 初始状态：未悬停、未拖拽。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 是否处于悬停态（拖拽中恒为真）。
    #[must_use]
    pub fn is_hovered(&self) -> bool {
        self.hovered || self.dragging
    }

    /// 是否正在拖拽 thumb。
    #[must_use]
    pub fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// 命中区域（视口坐标）。
    #[must_use]
    pub fn hit(&self, geometry: &ScrollGeometry, scroll: f32, x: f32, y: f32) -> ScrollbarHit {
        let Some(track) = geometry.track() else {
            return ScrollbarHit::None;
        };
        let Some(thumb) = geometry.thumb(scroll) else {
            return ScrollbarHit::None;
        };
        if x < track.x || x > track.x + track.width || y < track.y || y > track.y + track.height {
            return ScrollbarHit::None;
        }
        if y >= thumb.y && y <= thumb.y + thumb.height {
            ScrollbarHit::Thumb
        } else if y < thumb.y {
            ScrollbarHit::TrackAbove
        } else {
            ScrollbarHit::TrackBelow
        }
    }

    /// 更新悬停态；返回是否发生变化（外壳据此避免重复重绘/设置光标）。
    pub fn on_hover(&mut self, geometry: &ScrollGeometry, scroll: f32, x: f32, y: f32) -> bool {
        let hovered = self.hit(geometry, scroll, x, y) != ScrollbarHit::None;
        let changed = hovered != self.hovered;
        self.hovered = hovered;
        changed
    }

    /// 左键按下：thumb → 开始拖拽（记录抓取偏移，不改变滚动量）；轨道空白 →
    /// 翻一页。返回需要应用的滚动量，`None` 表示本次按下不改变滚动。
    pub fn on_press(
        &mut self,
        geometry: &ScrollGeometry,
        scroll: f32,
        x: f32,
        y: f32,
    ) -> Option<f32> {
        match self.hit(geometry, scroll, x, y) {
            ScrollbarHit::Thumb => {
                let thumb = geometry.thumb(scroll)?;
                self.dragging = true;
                self.grab_offset = y - thumb.y;
                None
            }
            ScrollbarHit::TrackAbove => Some(clamp_scroll(
                scroll - geometry.page_step(),
                geometry.max_scroll(),
            )),
            ScrollbarHit::TrackBelow => Some(clamp_scroll(
                scroll + geometry.page_step(),
                geometry.max_scroll(),
            )),
            ScrollbarHit::None => None,
        }
    }

    /// 拖拽中移动光标：返回新的滚动量（thumb 顶边跟着光标走，保持抓取偏移）。
    pub fn on_drag(&mut self, geometry: &ScrollGeometry, _x: f32, y: f32) -> Option<f32> {
        if !self.dragging {
            return None;
        }
        let thumb_height = geometry.thumb(0.0)?.height;
        // thumb 高度与滚动量无关，夹取到轨道行程内保证端点对齐
        let travel = geometry.page_step() - thumb_height;
        let thumb_y =
            (y - self.grab_offset).clamp(geometry.bar_height, geometry.bar_height + travel);
        Some(geometry.scroll_for_thumb_y(thumb_y, thumb_height))
    }

    /// 左键释放：结束拖拽。
    pub fn on_release(&mut self) {
        self.dragging = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAR: f32 = 36.0;

    fn geometry(content_height: f32, viewport_height: f32) -> ScrollGeometry {
        ScrollGeometry {
            viewport_width: 800.0,
            viewport_height,
            content_height,
            bar_height: BAR,
        }
    }

    /// 最大滚动量：不足视口为 0，超出为差值（含工具带高度）。
    #[test]
    fn max_scroll_is_overflow_above_viewport() {
        assert_eq!(geometry(0.0, 600.0).max_scroll(), 0.0);
        assert_eq!(geometry(564.0, 600.0).max_scroll(), 0.0, "恰好填满视口");
        assert_eq!(geometry(1064.0, 600.0).max_scroll(), 500.0);
        assert_eq!(geometry(2000.0, 100.0).max_scroll(), 1936.0);
    }

    /// 滚动量 clamp 到 [0, max]。
    #[test]
    fn clamp_scroll_bounds() {
        assert_eq!(clamp_scroll(-50.0, 100.0), 0.0);
        assert_eq!(clamp_scroll(42.0, 100.0), 42.0);
        assert_eq!(clamp_scroll(150.0, 100.0), 100.0);
        assert_eq!(clamp_scroll(5.0, 0.0), 0.0, "无溢出时滚动量归零");
    }

    /// thumb 几何：零溢出无 thumb；半溢出 thumb 占轨道一半（受最小高度约束）。
    #[test]
    fn scrollbar_thumb_geometry() {
        // 不溢出：None
        assert!(geometry(100.0, 600.0).thumb(0.0).is_none());
        assert!(geometry(564.0, 600.0).thumb(0.0).is_none());
        assert!(geometry(100.0, 600.0).track().is_none(), "不溢出无轨道");

        // 溢出 500（总高 1100）：thumb 高 = 564 × 600/1100 ≈ 307.6（> 24 下限），
        // 顶部对齐轨道顶
        let thumb = geometry(1064.0, 600.0).thumb(0.0).expect("thumb");
        assert_eq!(thumb.width, TRACK_WIDTH);
        assert_eq!(thumb.x, 800.0 - TRACK_WIDTH);
        let expected_height = (600.0 - BAR) * 600.0 / (1064.0 + BAR);
        assert!((thumb.height - expected_height).abs() < 0.01);
        assert!((thumb.y - BAR).abs() < 0.01, "scroll=0 时 thumb 贴轨道顶");

        // 滚到底：thumb 贴轨道底
        let thumb = geometry(1064.0, 600.0).thumb(500.0).expect("thumb");
        assert!((thumb.y + thumb.height - 600.0).abs() < 0.01);

        // 超长文档：thumb 触发 24px 下限
        let thumb = geometry(100000.0, 600.0).thumb(0.0).expect("thumb");
        assert_eq!(thumb.height, MIN_THUMB);
        assert!((thumb.y - BAR).abs() < 0.01);
    }

    /// 命中四态：thumb、轨道上方、轨道下方、滚动条之外。
    #[test]
    fn hit_discriminates_thumb_and_track_halves() {
        let g = geometry(1064.0, 600.0);
        let thumb = g.thumb(0.0).expect("thumb");
        let x = g.viewport_width - TRACK_WIDTH / 2.0;

        assert_eq!(
            Scrollbar::new().hit(&g, 0.0, x, thumb.y + 1.0),
            ScrollbarHit::Thumb
        );
        // thumb 下方（scroll=0 时仍有大片轨道空白）
        assert_eq!(
            Scrollbar::new().hit(&g, 0.0, x, 600.0 - 1.0),
            ScrollbarHit::TrackBelow
        );
        // 滚到底后 thumb 贴底，上方是轨道空白
        assert_eq!(
            Scrollbar::new().hit(&g, 500.0, x, BAR + 1.0),
            ScrollbarHit::TrackAbove
        );
        // 轨道左侧（页面区域）与工具带内：无命中
        assert_eq!(
            Scrollbar::new().hit(&g, 0.0, g.viewport_width - TRACK_WIDTH - 1.0, 300.0),
            ScrollbarHit::None
        );
        assert_eq!(
            Scrollbar::new().hit(&g, 0.0, x, BAR - 1.0),
            ScrollbarHit::None
        );
        // 不溢出：不响应
        let g = geometry(100.0, 600.0);
        assert_eq!(
            Scrollbar::new().hit(&g, 0.0, g.viewport_width - 1.0, 300.0),
            ScrollbarHit::None
        );
    }

    /// 点击轨道空白翻一页（方向由 thumb 上下决定），并在端点夹取。
    #[test]
    fn track_click_pages_and_clamps() {
        // 内容 2000：max_scroll = 1436 > 页高 564，可翻多页
        let g = geometry(2000.0, 600.0);
        let x = g.viewport_width - TRACK_WIDTH / 2.0;
        let mut scrollbar = Scrollbar::new();

        // 顶部时 thumb 贴顶，轨道下方是空白 → 向下翻一页（页高 = 视口 - 工具带 = 564）
        assert_eq!(
            scrollbar.on_press(&g, 0.0, x, 600.0 - 1.0),
            Some(564.0),
            "轨道下方点击向下翻页"
        );
        // 滚到中段后 thumb 上方是空白 → 向上翻一页
        assert_eq!(scrollbar.on_press(&g, 1000.0, x, BAR + 1.0), Some(436.0));
        // 向上翻页夹取到 0
        assert_eq!(scrollbar.on_press(&g, 100.0, x, BAR + 1.0), Some(0.0));
        // 已近底部：向下翻页夹取到 max_scroll
        assert_eq!(scrollbar.on_press(&g, 1200.0, x, 600.0 - 1.0), Some(1436.0));
        assert!(!scrollbar.is_dragging(), "轨道点击不进入拖拽");
    }

    /// 拖拽：抓取偏移使 thumb 不跳变；拖动到底/顶时端点对齐；未按下不响应。
    #[test]
    fn thumb_drag_keeps_grab_offset_and_aligns_endpoints() {
        let g = geometry(1064.0, 600.0);
        let x = g.viewport_width - TRACK_WIDTH / 2.0;
        let mut scrollbar = Scrollbar::new();

        // 未按下：拖拽无效
        assert_eq!(scrollbar.on_drag(&g, x, 300.0), None);

        let thumb = g.thumb(0.0).expect("thumb");
        // 在 thumb 中部按下：不改变滚动量，进入拖拽
        let grab_y = thumb.y + thumb.height / 2.0;
        assert_eq!(scrollbar.on_press(&g, 0.0, x, grab_y), None);
        assert!(scrollbar.is_dragging());
        assert!(scrollbar.is_hovered(), "拖拽中视为悬停");

        // 原地不动：滚动量不变（抓取偏移生效，thumb 不跳变）
        let scroll = scrollbar.on_drag(&g, x, grab_y).expect("drag");
        assert!(scroll.abs() < 0.01, "原地拖动不改变滚动量: {scroll}");

        // 拖到轨道底：滚动到最大
        let scroll = scrollbar.on_drag(&g, x, g.viewport_height).expect("drag");
        assert!(
            (scroll - g.max_scroll()).abs() < 0.01,
            "拖到底 = max_scroll"
        );
        // 拖到轨道顶以上：夹回 0
        let scroll = scrollbar.on_drag(&g, x, 0.0).expect("drag");
        assert!(scroll.abs() < 0.01, "拖出顶部 = 0");

        // 中间位置单调：y 越大滚动越大
        let a = scrollbar.on_drag(&g, x, 400.0).expect("drag");
        let b = scrollbar.on_drag(&g, x, 500.0).expect("drag");
        assert!(b > a, "拖拽映射随 y 单调");

        scrollbar.on_release();
        assert!(!scrollbar.is_dragging());
        assert_eq!(scrollbar.on_drag(&g, x, 400.0), None, "释放后不再拖拽");
    }

    /// 悬停态仅在跨越命中区时变化（避免每帧重复请求重绘）。
    #[test]
    fn hover_reports_only_transitions() {
        let g = geometry(1064.0, 600.0);
        let x = g.viewport_width - TRACK_WIDTH / 2.0;
        let mut scrollbar = Scrollbar::new();

        assert!(scrollbar.on_hover(&g, 0.0, x, 500.0), "首次进入轨道：变化");
        assert!(!scrollbar.on_hover(&g, 0.0, x, 501.0), "仍在轨道内：无变化");
        assert!(scrollbar.on_hover(&g, 0.0, 100.0, 500.0), "离开轨道：变化");
        assert!(
            !scrollbar.on_hover(&g, 0.0, 100.0, 500.0),
            "仍在页面区：无变化"
        );
        // 不溢出时轨道不可悬停
        let g = geometry(100.0, 600.0);
        assert!(!scrollbar.on_hover(&g, 0.0, x, 500.0));
    }
}
