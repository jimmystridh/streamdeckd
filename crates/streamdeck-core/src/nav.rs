//! Page navigation and the temporary panel.
//!
//! A temporary panel is a page shown for a bounded time that returns to the page
//! it was opened from. Any interaction restarts its timeout; pressing Home
//! dismisses it immediately.

use crate::model::PageId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Panel {
    page: PageId,
    return_to: PageId,
    expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Navigator {
    page: PageId,
    panel: Option<Panel>,
    panel_duration_ms: u64,
}

impl Navigator {
    pub fn new(page: PageId, panel_duration_ms: u64) -> Self {
        Self {
            page,
            panel: None,
            panel_duration_ms,
        }
    }

    /// Applies a reloaded timeout. An already-open panel keeps the deadline it
    /// was opened with; the new value takes effect on the next interaction.
    pub fn set_panel_duration_ms(&mut self, duration_ms: u64) {
        self.panel_duration_ms = duration_ms;
    }

    /// The page whose keys should currently be rendered.
    pub fn visible_page(&self) -> PageId {
        self.panel.map_or(self.page, |panel| panel.page)
    }

    /// The page the deck returns to when a panel closes.
    pub fn base_page(&self) -> PageId {
        self.page
    }

    pub fn panel_is_open(&self) -> bool {
        self.panel.is_some()
    }

    /// Seconds left before the panel closes, rounded up, for the countdown tile.
    pub fn panel_seconds_remaining(&self, now_ms: u64) -> Option<u64> {
        let panel = self.panel?;
        Some(panel.expires_at_ms.saturating_sub(now_ms).div_ceil(1_000))
    }

    pub fn panel_total_seconds(&self) -> u64 {
        self.panel_duration_ms.div_ceil(1_000)
    }

    pub fn panel_deadline_ms(&self) -> Option<u64> {
        self.panel.map(|panel| panel.expires_at_ms)
    }

    /// Switches page permanently, closing any open panel.
    pub fn go_to(&mut self, page: PageId) -> bool {
        let changed = self.visible_page() != page || self.panel.is_some();
        self.panel = None;
        self.page = page;
        changed
    }

    /// Opens `page` as a temporary panel over the current page.
    pub fn open_panel(&mut self, page: PageId, now_ms: u64) -> bool {
        let return_to = self.panel.map_or(self.page, |panel| panel.return_to);
        self.panel = Some(Panel {
            page,
            return_to,
            expires_at_ms: now_ms + self.panel_duration_ms,
        });
        true
    }

    /// Restarts the panel timeout. Called on any interaction while a panel is open.
    pub fn touch_panel(&mut self, now_ms: u64) {
        if let Some(panel) = self.panel.as_mut() {
            panel.expires_at_ms = now_ms + self.panel_duration_ms;
        }
    }

    /// Closes the panel and returns to the page it was opened from.
    pub fn dismiss_panel(&mut self) -> bool {
        match self.panel.take() {
            Some(panel) => {
                self.page = panel.return_to;
                true
            }
            None => false,
        }
    }

    /// Closes the panel if its timeout has elapsed.
    pub fn poll_panel(&mut self, now_ms: u64) -> bool {
        match self.panel {
            Some(panel) if now_ms >= panel.expires_at_ms => self.dismiss_panel(),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn navigator() -> Navigator {
        Navigator::new(PageId::Home, 10_000)
    }

    #[test]
    fn navigation_switches_pages() {
        let mut navigator = navigator();
        assert!(navigator.go_to(PageId::Mixer));
        assert_eq!(navigator.visible_page(), PageId::Mixer);
        assert!(!navigator.go_to(PageId::Mixer), "no change is reported");
    }

    #[test]
    fn a_panel_shows_over_the_base_page_and_returns_to_it() {
        let mut navigator = navigator();
        navigator.go_to(PageId::Home);
        navigator.open_panel(PageId::Stensjon, 1_000);

        assert_eq!(navigator.visible_page(), PageId::Stensjon);
        assert_eq!(navigator.base_page(), PageId::Home);
        assert!(navigator.panel_is_open());

        assert!(navigator.dismiss_panel());
        assert_eq!(navigator.visible_page(), PageId::Home);
        assert!(!navigator.panel_is_open());
    }

    #[test]
    fn a_panel_closes_itself_when_the_timeout_elapses() {
        let mut navigator = navigator();
        navigator.open_panel(PageId::Stensjon, 1_000);

        assert!(!navigator.poll_panel(10_999));
        assert_eq!(navigator.visible_page(), PageId::Stensjon);
        assert!(navigator.poll_panel(11_000));
        assert_eq!(navigator.visible_page(), PageId::Home);
    }

    #[test]
    fn interaction_restarts_the_panel_timeout() {
        let mut navigator = navigator();
        navigator.open_panel(PageId::Stensjon, 1_000);
        assert_eq!(navigator.panel_deadline_ms(), Some(11_000));

        navigator.touch_panel(6_000);
        assert_eq!(navigator.panel_deadline_ms(), Some(16_000));
        assert!(!navigator.poll_panel(11_000));
        assert!(navigator.poll_panel(16_000));
    }

    #[test]
    fn navigating_away_dismisses_the_panel_immediately() {
        let mut navigator = navigator();
        navigator.open_panel(PageId::Stensjon, 1_000);

        assert!(navigator.go_to(PageId::Home));
        assert!(!navigator.panel_is_open());
        assert_eq!(navigator.visible_page(), PageId::Home);
        assert_eq!(navigator.panel_deadline_ms(), None);
    }

    #[test]
    fn the_countdown_reports_whole_seconds_remaining() {
        let mut navigator = navigator();
        navigator.open_panel(PageId::Stensjon, 0);

        assert_eq!(navigator.panel_total_seconds(), 10);
        assert_eq!(navigator.panel_seconds_remaining(0), Some(10));
        assert_eq!(navigator.panel_seconds_remaining(500), Some(10));
        assert_eq!(navigator.panel_seconds_remaining(9_100), Some(1));
        assert_eq!(navigator.panel_seconds_remaining(10_000), Some(0));
    }

    #[test]
    fn a_panel_opened_over_a_panel_still_returns_to_the_original_page() {
        let mut navigator = navigator();
        navigator.go_to(PageId::Mixer);
        navigator.open_panel(PageId::Stensjon, 1_000);
        navigator.open_panel(PageId::Pomodoro, 2_000);

        assert!(navigator.dismiss_panel());
        assert_eq!(navigator.visible_page(), PageId::Mixer);
    }

    #[test]
    fn dismissing_without_a_panel_is_a_no_op() {
        let mut navigator = navigator();
        assert!(!navigator.dismiss_panel());
        assert_eq!(navigator.visible_page(), PageId::Home);
    }
}
