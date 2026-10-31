//! 会话历史栈：后退/前进导航的纯逻辑（无窗口依赖，可单测）。
//!
//! 语义对齐主流浏览器：新导航清空前进栈；后退/前进在两个栈间搬运
//! 当前条目；刷新不产生新条目（由外壳直接重载 `current`）。

/// 会话历史：当前条目 + 后退/前进两个栈。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History {
    back: Vec<String>,
    current: String,
    forward: Vec<String>,
}

impl History {
    /// 以初始条目创建历史（后退/前进栈为空）。
    #[must_use]
    pub fn new(initial: impl Into<String>) -> Self {
        Self {
            back: Vec::new(),
            current: initial.into(),
            forward: Vec::new(),
        }
    }

    /// 当前条目。
    #[must_use]
    pub fn current(&self) -> &str {
        &self.current
    }

    /// 是否可后退。
    #[must_use]
    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    /// 是否可前进。
    #[must_use]
    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// 记录一次新导航：当前条目入后退栈，前进栈清空。
    ///
    /// 与当前条目相同的 URL 不重复入栈（刷新场景由外壳不经此路径）。
    pub fn push(&mut self, url: impl Into<String>) {
        let url = url.into();
        if url != self.current {
            self.back.push(std::mem::take(&mut self.current));
            self.current = url;
            self.forward.clear();
        }
    }

    /// 后退：当前条目进前进栈，返回目标（新当前）URL；无后退条目时 `None`。
    pub fn go_back(&mut self) -> Option<String> {
        let previous = self.back.pop()?;
        let leaving = std::mem::replace(&mut self.current, previous);
        self.forward.push(leaving);
        Some(self.current.clone())
    }

    /// 前进：当前条目进后退栈，返回目标（新当前）URL；无前进条目时 `None`。
    pub fn go_forward(&mut self) -> Option<String> {
        let next = self.forward.pop()?;
        let leaving = std::mem::replace(&mut self.current, next);
        self.back.push(leaving);
        Some(self.current.clone())
    }

    /// 原地改写当前条目（不产生新条目、不动前后栈）。
    ///
    /// 302 落点语义（WHATWG 会话历史）：重定向结束后条目记为最终 URL——
    /// 地址栏、刷新、页内相对链接的解析基准（`current`）随之落在落点；
    /// 后退仍回前一页，前进仍进原前进项（由调用方在页面安装时调用）。
    pub fn replace_current(&mut self, url: impl Into<String>) {
        self.current = url.into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_moves_current_and_clears_forward() {
        let mut history = History::new("about:blank");
        history.push("https://a.test/");
        assert_eq!(history.current(), "https://a.test/");
        assert!(history.can_back());
        assert!(!history.can_forward());

        history.go_back();
        history.push("https://b.test/");
        assert_eq!(history.current(), "https://b.test/");
        // 新导航截断前进分支
        assert!(!history.can_forward());
        assert_eq!(history.go_back().as_deref(), Some("about:blank"));
    }

    #[test]
    fn back_and_forward_swap_stack_positions() {
        let mut history = History::new("a");
        history.push("b");
        history.push("c");
        assert_eq!(history.go_back().as_deref(), Some("b"));
        assert_eq!(history.go_back().as_deref(), Some("a"));
        assert!(!history.can_back());
        assert_eq!(history.go_back(), None, "空后退栈返回 None");
        assert_eq!(history.current(), "a");

        assert_eq!(history.go_forward().as_deref(), Some("b"));
        assert_eq!(history.go_forward().as_deref(), Some("c"));
        assert!(!history.can_forward());
        assert_eq!(history.go_forward(), None);
        assert_eq!(history.current(), "c");
    }

    #[test]
    fn push_same_url_is_ignored() {
        let mut history = History::new("a");
        history.push("a");
        assert!(!history.can_back(), "重复当前条目不入栈");
        assert!(history.go_back().is_none());
    }

    /// 302 落点语义（WHATWG 会话历史）：替换当前条目为最终 URL，
    /// 前后栈与可用性标志不变；后退仍回前一页，前进仍进原前进项。
    #[test]
    fn replace_current_rewrites_entry_in_place() {
        let mut history = History::new("about:blank");
        history.push("https://a.test/start");
        history.push("https://a.test/redirected");

        history.replace_current("https://final.test/landing");
        assert_eq!(history.current(), "https://final.test/landing");
        assert!(history.can_back(), "后退栈不变");
        assert!(!history.can_forward(), "前进栈不变");
        assert_eq!(history.go_back().as_deref(), Some("https://a.test/start"));
        // 前进重放进的是**改写后**的条目（替换重写当前，不保留旧值）
        assert_eq!(
            history.go_forward().as_deref(),
            Some("https://final.test/landing")
        );

        // 替换不产生新条目：连续替换只改当前值，栈深度不变
        history.replace_current("https://final.test/landing2");
        assert_eq!(history.current(), "https://final.test/landing2");
        assert!(history.can_back());
        assert!(!history.can_forward(), "替换不产生新条目");
    }

    #[test]
    fn flags_track_stack_tops() {
        let mut history = History::new("a");
        assert!(!history.can_back() && !history.can_forward());
        history.push("b");
        assert!(history.can_back() && !history.can_forward());
        let _ = history.go_back();
        assert!(!history.can_back() && history.can_forward());
    }
}
