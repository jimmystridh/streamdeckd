//! Press and long-press state machine.
//!
//! Physical key events go in; the outcomes the coordinator must act on come out.
//! Time is passed in as milliseconds so every threshold is testable without a
//! clock.

use std::collections::HashMap;

use crate::model::KeyPosition;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PressConfig {
    pub long_press_ms: u64,
}

impl Default for PressConfig {
    fn default() -> Self {
        Self { long_press_ms: 600 }
    }
}

/// What the coordinator should do in response to a press event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressOutcome {
    /// Draw the pressed-key treatment. Must reach the deck within 50 ms.
    ShowPressed(KeyPosition),
    /// The long-press threshold was crossed: draw the armed affordance and fire
    /// the long action once.
    Armed(KeyPosition),
    /// A short press completed; run the key's primary action.
    ShortPress(KeyPosition),
    /// A long press was released; the action already fired at `Armed`.
    LongPressReleased(KeyPosition),
    /// Nothing to do, but the key must be repainted from current state.
    Release(KeyPosition),
}

#[derive(Debug, Clone, Copy)]
struct KeyPress {
    pressed_at_ms: u64,
    armed: bool,
    /// Whether this key has a long action at all. Keys without one still get
    /// pressed feedback but never arm.
    has_long_action: bool,
}

/// Tracks every physically held key.
#[derive(Debug, Default)]
pub struct PressTracker {
    config: PressConfig,
    held: HashMap<KeyPosition, KeyPress>,
}

impl PressTracker {
    pub fn new(config: PressConfig) -> Self {
        Self {
            config,
            held: HashMap::new(),
        }
    }

    pub fn set_config(&mut self, config: PressConfig) {
        self.config = config;
    }

    pub fn is_held(&self, position: KeyPosition) -> bool {
        self.held.contains_key(&position)
    }

    pub fn is_armed(&self, position: KeyPosition) -> bool {
        self.held.get(&position).is_some_and(|press| press.armed)
    }

    /// Clears every held key, for example after a device disconnect, so a stale
    /// press cannot fire an action on reconnect.
    pub fn clear(&mut self) {
        self.held.clear();
    }

    pub fn key_down(
        &mut self,
        position: KeyPosition,
        now_ms: u64,
        has_long_action: bool,
    ) -> PressOutcome {
        self.held.insert(
            position,
            KeyPress {
                pressed_at_ms: now_ms,
                armed: false,
                has_long_action,
            },
        );
        PressOutcome::ShowPressed(position)
    }

    /// The instant at which a held key should arm, if any. Feeds the deadline
    /// scheduler so no per-key timer task is needed.
    pub fn next_arm_deadline_ms(&self) -> Option<u64> {
        self.held
            .values()
            .filter(|press| press.has_long_action && !press.armed)
            .map(|press| press.pressed_at_ms + self.config.long_press_ms)
            .min()
    }

    /// Arms every held key whose threshold has been crossed. Each key arms once.
    pub fn poll_arm(&mut self, now_ms: u64) -> Vec<PressOutcome> {
        let threshold = self.config.long_press_ms;
        let mut armed = Vec::new();
        for (position, press) in self.held.iter_mut() {
            if press.has_long_action && !press.armed && now_ms >= press.pressed_at_ms + threshold {
                press.armed = true;
                armed.push(PressOutcome::Armed(*position));
            }
        }
        armed.sort_by_key(|outcome| match outcome {
            PressOutcome::Armed(position) => *position,
            _ => KeyPosition::new(0, 0),
        });
        armed
    }

    pub fn key_up(&mut self, position: KeyPosition, now_ms: u64) -> PressOutcome {
        let Some(press) = self.held.remove(&position) else {
            // A key-up without a matching key-down happens after a reconnect or
            // when state was cleared mid-press. Repaint and do nothing else.
            return PressOutcome::Release(position);
        };

        let held_ms = now_ms.saturating_sub(press.pressed_at_ms);
        if press.has_long_action && (press.armed || held_ms >= self.config.long_press_ms) {
            PressOutcome::LongPressReleased(position)
        } else {
            PressOutcome::ShortPress(position)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: KeyPosition = KeyPosition::new(2, 1);

    fn tracker() -> PressTracker {
        PressTracker::new(PressConfig { long_press_ms: 600 })
    }

    #[test]
    fn key_down_immediately_requests_pressed_feedback() {
        let mut tracker = tracker();
        assert_eq!(
            tracker.key_down(KEY, 1_000, true),
            PressOutcome::ShowPressed(KEY)
        );
        assert!(tracker.is_held(KEY));
        assert!(!tracker.is_armed(KEY));
    }

    #[test]
    fn a_quick_release_is_a_short_press() {
        let mut tracker = tracker();
        tracker.key_down(KEY, 1_000, true);
        assert_eq!(tracker.key_up(KEY, 1_120), PressOutcome::ShortPress(KEY));
        assert!(!tracker.is_held(KEY));
    }

    #[test]
    fn crossing_the_threshold_arms_exactly_once() {
        let mut tracker = tracker();
        tracker.key_down(KEY, 1_000, true);

        assert!(tracker.poll_arm(1_599).is_empty());
        assert_eq!(tracker.poll_arm(1_600), vec![PressOutcome::Armed(KEY)]);
        assert!(tracker.is_armed(KEY));
        assert!(
            tracker.poll_arm(2_400).is_empty(),
            "a key must not arm twice"
        );
    }

    #[test]
    fn releasing_after_arming_does_not_also_fire_the_short_action() {
        let mut tracker = tracker();
        tracker.key_down(KEY, 1_000, true);
        tracker.poll_arm(1_600);
        assert_eq!(
            tracker.key_up(KEY, 1_900),
            PressOutcome::LongPressReleased(KEY)
        );
    }

    #[test]
    fn a_long_hold_still_counts_as_long_even_if_arming_was_never_polled() {
        let mut tracker = tracker();
        tracker.key_down(KEY, 1_000, true);
        assert_eq!(
            tracker.key_up(KEY, 5_000),
            PressOutcome::LongPressReleased(KEY)
        );
    }

    #[test]
    fn keys_without_a_long_action_never_arm_and_always_short_press() {
        let mut tracker = tracker();
        tracker.key_down(KEY, 1_000, false);

        assert!(tracker.poll_arm(9_000).is_empty());
        assert_eq!(tracker.next_arm_deadline_ms(), None);
        assert_eq!(tracker.key_up(KEY, 9_000), PressOutcome::ShortPress(KEY));
    }

    #[test]
    fn the_next_arm_deadline_is_the_earliest_unarmed_hold() {
        let mut tracker = tracker();
        tracker.key_down(KeyPosition::new(1, 1), 1_000, true);
        tracker.key_down(KeyPosition::new(3, 5), 1_200, true);

        assert_eq!(tracker.next_arm_deadline_ms(), Some(1_600));
        tracker.poll_arm(1_600);
        assert_eq!(tracker.next_arm_deadline_ms(), Some(1_800));
        tracker.poll_arm(1_800);
        assert_eq!(tracker.next_arm_deadline_ms(), None);
    }

    #[test]
    fn a_key_up_without_a_key_down_only_asks_for_a_repaint() {
        let mut tracker = tracker();
        assert_eq!(tracker.key_up(KEY, 1_000), PressOutcome::Release(KEY));
    }

    #[test]
    fn clearing_state_discards_holds_so_a_reconnect_cannot_fire_an_action() {
        let mut tracker = tracker();
        tracker.key_down(KEY, 1_000, true);
        tracker.clear();

        assert!(!tracker.is_held(KEY));
        assert_eq!(tracker.key_up(KEY, 2_000), PressOutcome::Release(KEY));
    }

    #[test]
    fn a_reconfigured_threshold_applies_to_keys_already_held() {
        let mut tracker = tracker();
        tracker.key_down(KEY, 1_000, true);
        tracker.set_config(PressConfig { long_press_ms: 200 });

        assert_eq!(tracker.poll_arm(1_200), vec![PressOutcome::Armed(KEY)]);
    }

    #[test]
    fn several_simultaneous_holds_arm_in_coordinate_order() {
        let mut tracker = tracker();
        tracker.key_down(KeyPosition::new(3, 2), 1_000, true);
        tracker.key_down(KeyPosition::new(1, 4), 1_000, true);

        assert_eq!(
            tracker.poll_arm(1_600),
            vec![
                PressOutcome::Armed(KeyPosition::new(1, 4)),
                PressOutcome::Armed(KeyPosition::new(3, 2)),
            ]
        );
    }
}
