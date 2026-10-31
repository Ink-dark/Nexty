//! 事件数据面（[UI Events](https://www.w3.org/TR/uievents/) 简化档）。
//!
//! 各类型组合核心 [`Event`](crate::Event) 与自身数据字段：派发传
//! `&mut ev.event`（监听器只见核心面，见 `event` 模块偏差清单），派发方随后
//! 读自身数据字段与 `default_prevented` 决定默认动作（先派发后默认）。
//!
//! 偏差：`WheelEvent` 不含鼠标坐标面（DOM 中 WheelEvent 继承 MouseEvent），
//! 不建模 delta_z；`FocusEvent` 仅类型面（无派发点，K10 补），不建模
//! related_target；`UIEvent` 的 view 属性未建模（无 window 对象）。

use super::Event;

/// UIEvent 数据面（[UI Events §5.1](https://www.w3.org/TR/uievents/#interface-uievent)）。
#[derive(Debug)]
pub struct UiEvent {
    /// 核心 Event 面：派发与监听回调都作用于此。
    pub event: Event,
    /// `detail` 属性：实现相关细节，鼠标类事件为同一位置的点击计数。
    pub detail: u32,
}

impl UiEvent {
    /// 构造 UIEvent，数据字段取零值，由派发方按需填充。
    #[must_use]
    pub fn new(type_: impl Into<String>, bubbles: bool, cancelable: bool) -> Self {
        Self {
            event: Event::new(type_, bubbles, cancelable),
            detail: 0,
        }
    }
}

/// MouseEvent 数据面（[UI Events §5.2](https://www.w3.org/TR/uievents/#interface-mouseevent)）。
#[derive(Debug)]
pub struct MouseEvent {
    /// 核心 Event 面：派发与监听回调都作用于此。
    pub event: Event,
    /// `detail` 属性：同一位置的点击计数（mousedown/up 为 1，dblclick 为 2）。
    pub detail: u32,
    /// `screenX`：相对屏幕原点的横坐标。
    pub screen_x: i32,
    /// `screenY`：相对屏幕原点的纵坐标。
    pub screen_y: i32,
    /// `clientX`：相对视口的横坐标（命中测试结果）。
    pub client_x: i32,
    /// `clientY`：相对视口的纵坐标。
    pub client_y: i32,
    /// `button`：按下/释放的按键——0 主、1 中、2 次。
    pub button: u16,
    /// `buttons`：当前按住位掩码——bit0 主、bit1 中、bit2 次。
    pub buttons: u16,
    /// `ctrlKey`。
    pub ctrl_key: bool,
    /// `shiftKey`。
    pub shift_key: bool,
    /// `altKey`。
    pub alt_key: bool,
    /// `metaKey`。
    pub meta_key: bool,
}

impl MouseEvent {
    /// 构造 MouseEvent：button=0（主键）、坐标与修饰键零值，由派发方按命中
    /// 测试结果填充。
    #[must_use]
    pub fn new(type_: impl Into<String>, bubbles: bool, cancelable: bool) -> Self {
        Self {
            event: Event::new(type_, bubbles, cancelable),
            detail: 0,
            screen_x: 0,
            screen_y: 0,
            client_x: 0,
            client_y: 0,
            button: 0,
            buttons: 0,
            ctrl_key: false,
            shift_key: false,
            alt_key: false,
            meta_key: false,
        }
    }
}

/// KeyboardEvent 数据面（[UI Events §6](https://www.w3.org/TR/uievents/#events-keyboard-events)）。
#[derive(Debug)]
pub struct KeyboardEvent {
    /// 核心 Event 面：派发与监听回调都作用于此。
    pub event: Event,
    /// `key` 属性：按键对应的键值字符串（如 `"a"`、`"Enter"`）。
    pub key: String,
    /// `code` 属性：物理键位（如 `"KeyA"`、`"ArrowDown"`）。
    pub code: String,
    /// `ctrlKey`。
    pub ctrl_key: bool,
    /// `shiftKey`。
    pub shift_key: bool,
    /// `altKey`。
    pub alt_key: bool,
    /// `metaKey`。
    pub meta_key: bool,
    /// `repeat`：持续按住触发的自动重复。
    pub repeat: bool,
}

impl KeyboardEvent {
    /// 构造 KeyboardEvent：key / code 为空串、修饰键与 repeat 零值，由派发方填充。
    #[must_use]
    pub fn new(type_: impl Into<String>, bubbles: bool, cancelable: bool) -> Self {
        Self {
            event: Event::new(type_, bubbles, cancelable),
            key: String::new(),
            code: String::new(),
            ctrl_key: false,
            shift_key: false,
            alt_key: false,
            meta_key: false,
            repeat: false,
        }
    }
}

/// 滚动增量单位（UI Events
/// [WheelEvent 的 deltaMode](https://www.w3.org/TR/uievents/#dom-wheelevent-deltamode)）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaMode {
    /// `DOM_DELTA_PIXEL`。
    Pixel,
    /// `DOM_DELTA_LINE`。
    Line,
    /// `DOM_DELTA_PAGE`。
    Page,
}

/// WheelEvent 数据面（[UI Events §5.3](https://www.w3.org/TR/uievents/#interface-wheelevent)）。
#[derive(Debug)]
pub struct WheelEvent {
    /// 核心 Event 面：派发与监听回调都作用于此。
    pub event: Event,
    /// `deltaX`：横向滚动增量。
    pub delta_x: f64,
    /// `deltaY`：纵向滚动增量。
    pub delta_y: f64,
    /// `deltaMode`：增量单位。
    pub delta_mode: DeltaMode,
}

impl WheelEvent {
    /// 构造 WheelEvent：增量为 0、单位为 Pixel，由派发方填充。
    #[must_use]
    pub fn new(type_: impl Into<String>, bubbles: bool, cancelable: bool) -> Self {
        Self {
            event: Event::new(type_, bubbles, cancelable),
            delta_x: 0.0,
            delta_y: 0.0,
            delta_mode: DeltaMode::Pixel,
        }
    }
}

/// FocusEvent 数据面（[UI Events §5.4](https://www.w3.org/TR/uievents/#interface-focusevent)）。
///
/// 本轮仅类型面：无派发点（K10 焦点模型补），不建模 related_target。
#[derive(Debug)]
pub struct FocusEvent {
    /// 核心 Event 面。
    pub event: Event,
}

impl FocusEvent {
    /// 构造 FocusEvent。
    #[must_use]
    pub fn new(type_: impl Into<String>, bubbles: bool, cancelable: bool) -> Self {
        Self {
            event: Event::new(type_, bubbles, cancelable),
        }
    }
}
