//! 滚动容器元数据（CSS Overflow 3 §2–§3 简化档；K6 T2）。
//!
//! 职责：布局解算与片段装配完成后，为滚动/裁剪容器填充 [`Fragment`] 的
//! 滚动元数据——`scrollable` 标志与 `scrollable_overflow` rect。这是纯
//! 后处理 pass：不改几何、不改布局结果。滚动偏移不参与布局解算：本模块
//! 输出与滚动状态无关，滚动由消费方在绘制期以 `Scene::translated` 平移
//! 施加（crate 文档契约）。
//!
//! 识别（CSS Overflow 3 §2 简化档）：
//! - 裁剪容器：任一轴 overflow ≠ visible（`hidden`/`scroll`/`auto` 均裁剪）；
//! - 滚动容器：任一轴为 `scroll`/`auto`（用户可滚动；`hidden` 只裁剪）。
//!
//! scrollable overflow rect（CSS Overflow 3 §3 简化档）：自身内容盒 +
//! 后代边框盒（块级子盒、行内原子盒与文本行带）的并集，坐标相对容器自身
//! **边框盒**左上角。遇嵌套滚动/裁剪容器截断传播：其内部溢出由自身裁剪，
//! 对祖先的贡献收敛为自身边框盒；匿名块不截断（其样式借用自父元素，不是
//! 独立容器）。
//!
//! 已知偏差（简化档，按需再补）：
//! - 后代 margin 盒不计入（规范含 margin box，此处只取边框盒）；
//! - 负向（左/上）溢出照常并入并集，不做规范的方向截断；
//! - 文本行只计行盒带（横向右缘含末字形 advance、纵向 [top, top+height]），
//!   字形墨迹超出行盒的极端场景（巨型字形）不计；
//! - `position: fixed` / `sticky` / transform 后代不特判（本布局层本就未
//!   建模这些语义）。

use nexty_css::{ComputedStyle, OverflowValue};

use crate::fragment::{Fragment, Rect};
use crate::inline::line_width;

/// 是否裁剪容器：任一轴 overflow ≠ visible（CSS Overflow 3 §2）。
fn is_clip_container(style: &ComputedStyle) -> bool {
    style.overflow_x != OverflowValue::Visible || style.overflow_y != OverflowValue::Visible
}

/// 是否滚动容器：任一轴为 scroll/auto（用户可滚动；CSS Overflow 3 §2）。
fn is_scroll_container(style: &ComputedStyle) -> bool {
    matches!(
        style.overflow_x,
        OverflowValue::Scroll | OverflowValue::Auto
    ) || matches!(
        style.overflow_y,
        OverflowValue::Scroll | OverflowValue::Auto
    )
}

/// 为整棵片段树填充滚动容器元数据（自底向上的布局后处理 pass）。
///
/// 递归覆盖块级子树与行内原子盒（`LineFragment::boxes`）；匿名块自身不
/// 标注（见模块文档）。
pub(crate) fn annotate(fragment: &mut Fragment) {
    for child in &mut fragment.children {
        annotate(child);
    }
    for line in &mut fragment.lines {
        for atomic in &mut line.boxes {
            annotate(atomic);
        }
    }
    if fragment.anonymous || !is_clip_container(&fragment.style) {
        return;
    }
    fragment.scrollable = is_scroll_container(&fragment.style);
    fragment.scrollable_overflow = Some(scrollable_overflow_rect(fragment));
}

/// 计算容器的 scrollable overflow rect（相对自身边框盒左上角）。
///
/// 起点 = 自身内容盒（CSS Overflow §3「box's own content box」），随后并入
/// 全部后代（块级子盒、文本行带、行内原子盒）。
fn scrollable_overflow_rect(fragment: &Fragment) -> Rect {
    let mut rect = content_box(fragment);
    let inset = content_inset(fragment);
    for child in &fragment.children {
        union_subtree(
            child,
            inset.0 + child.border_box.x,
            inset.1 + child.border_box.y,
            &mut rect,
        );
    }
    for line in &fragment.lines {
        union_line(line, inset.0, inset.1, &mut rect);
        for atomic in &line.boxes {
            union_subtree(
                atomic,
                inset.0 + atomic.border_box.x,
                inset.1 + atomic.border_box.y,
                &mut rect,
            );
        }
    }
    rect
}

/// 把 `fragment`（边框盒原点在累加坐标系 `(ox, oy)`）的边框盒及其后代并入
/// `rect`；嵌套裁剪容器截断递归（其后代溢出由自身裁剪），匿名块照常下钻。
fn union_subtree(fragment: &Fragment, ox: f32, oy: f32, rect: &mut Rect) {
    union(
        rect,
        Rect {
            x: ox,
            y: oy,
            width: fragment.border_box.width,
            height: fragment.border_box.height,
        },
    );
    if fragment.anonymous || !is_clip_container(&fragment.style) {
        let inset = content_inset(fragment);
        let cx = ox + inset.0;
        let cy = oy + inset.1;
        for child in &fragment.children {
            union_subtree(
                child,
                cx + child.border_box.x,
                cy + child.border_box.y,
                rect,
            );
        }
        for line in &fragment.lines {
            union_line(line, cx, cy, rect);
            for atomic in &line.boxes {
                union_subtree(
                    atomic,
                    cx + atomic.border_box.x,
                    cy + atomic.border_box.y,
                    rect,
                );
            }
        }
    }
}

/// 把一行文本的行盒带并入 rect：横向 [content_x, content_x + 右缘]、
/// 纵向 [content_y + top, +height]（简化档，见模块偏差）。
fn union_line(line: &crate::fragment::LineFragment, cx: f32, cy: f32, rect: &mut Rect) {
    let right = line_width(line);
    if !right.is_finite() {
        return;
    }
    union(
        rect,
        Rect {
            x: cx,
            y: cy + line.top,
            width: right.max(0.0),
            height: line.height.max(0.0),
        },
    );
}

/// 内容盒（相对边框盒左上角）。
fn content_box(fragment: &Fragment) -> Rect {
    let inset = content_inset(fragment);
    let width =
        (fragment.border_box.width - inset.0 - fragment.border.right - fragment.padding.right)
            .max(0.0);
    let height =
        (fragment.border_box.height - inset.1 - fragment.border.bottom - fragment.padding.bottom)
            .max(0.0);
    Rect {
        x: inset.0,
        y: inset.1,
        width,
        height,
    }
}

/// 内容盒相对边框盒的 inset（border + padding，左/上）。
fn content_inset(fragment: &Fragment) -> (f32, f32) {
    (
        fragment.border.left + fragment.padding.left,
        fragment.border.top + fragment.padding.top,
    )
}

/// 矩形并集（就地）。
fn union(rect: &mut Rect, other: Rect) {
    let right = (rect.x + rect.width).max(other.x + other.width);
    let bottom = (rect.y + rect.height).max(other.y + other.height);
    rect.x = rect.x.min(other.x);
    rect.y = rect.y.min(other.y);
    rect.width = right - rect.x;
    rect.height = bottom - rect.y;
}
