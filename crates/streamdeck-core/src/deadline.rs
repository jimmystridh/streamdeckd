//! One deadline queue for the whole daemon.
//!
//! Every timed behaviour — Pomodoro completion, visible countdowns, long-press
//! arming, panel dismissal, integration refresh, retry backoff, alert sound
//! repetition — registers a keyed deadline here. When nothing is due the process
//! sleeps instead of spinning one interval per feature.

use std::collections::HashMap;

use crate::model::IntegrationId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DeadlineId {
    /// The running Pomodoro phase ends.
    PomodoroCompletion,
    /// Repaint a visible countdown (Pomodoro timer, panel countdown).
    CountdownTick,
    /// A held key reaches the long-press threshold.
    LongPressArm,
    /// The temporary panel returns to its base page.
    PanelDismiss,
    /// Recompute meeting labels without refetching calendars.
    MeetingLabels,
    /// Replay the completion sound until acknowledged.
    AlertSound,
    /// A pressed weather tile stops showing its expanded reading.
    WeatherDetail,
    /// An integration is due for a refresh.
    Refresh(IntegrationId),
}

impl DeadlineId {
    pub fn describe(&self) -> String {
        match self {
            DeadlineId::PomodoroCompletion => "pomodoro-completion".to_string(),
            DeadlineId::CountdownTick => "countdown-tick".to_string(),
            DeadlineId::LongPressArm => "long-press-arm".to_string(),
            DeadlineId::PanelDismiss => "panel-dismiss".to_string(),
            DeadlineId::MeetingLabels => "meeting-labels".to_string(),
            DeadlineId::AlertSound => "alert-sound".to_string(),
            DeadlineId::WeatherDetail => "weather-detail".to_string(),
            DeadlineId::Refresh(integration) => format!("refresh:{integration}"),
        }
    }
}

/// A monotonic-millisecond keyed deadline queue. Setting an existing key replaces
/// its deadline, which is what every caller wants: one pending fire per concern.
#[derive(Debug, Default)]
pub struct DeadlineQueue {
    entries: HashMap<DeadlineId, u64>,
}

impl DeadlineQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, id: DeadlineId, at_ms: u64) {
        self.entries.insert(id, at_ms);
    }

    /// Sets the deadline only if it would fire sooner than an existing one.
    pub fn set_if_sooner(&mut self, id: DeadlineId, at_ms: u64) {
        match self.entries.get(&id) {
            Some(existing) if *existing <= at_ms => {}
            _ => {
                self.entries.insert(id, at_ms);
            }
        }
    }

    pub fn clear(&mut self, id: DeadlineId) {
        self.entries.remove(&id);
    }

    pub fn get(&self, id: DeadlineId) -> Option<u64> {
        self.entries.get(&id).copied()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// The next instant at which the runtime must wake. `None` means sleep until
    /// an external event arrives.
    pub fn next_deadline_ms(&self) -> Option<u64> {
        self.entries.values().copied().min()
    }

    /// Removes and returns every deadline that is due, in a stable order.
    pub fn take_due(&mut self, now_ms: u64) -> Vec<DeadlineId> {
        let mut due: Vec<DeadlineId> = self
            .entries
            .iter()
            .filter(|(_, at_ms)| **at_ms <= now_ms)
            .map(|(id, _)| *id)
            .collect();
        due.sort();
        for id in &due {
            self.entries.remove(id);
        }
        due
    }

    /// After a system wake, every wall-clock-anchored deadline must be re-derived
    /// by the coordinator. Reporting them lets it reconcile in one pass.
    pub fn drain_all(&mut self) -> Vec<DeadlineId> {
        let mut ids: Vec<DeadlineId> = self.entries.keys().copied().collect();
        ids.sort();
        self.entries.clear();
        ids
    }

    pub fn pending(&self) -> Vec<(DeadlineId, u64)> {
        let mut pending: Vec<(DeadlineId, u64)> =
            self.entries.iter().map(|(id, at)| (*id, *at)).collect();
        pending.sort_by_key(|(id, at)| (*at, *id));
        pending
    }
}

/// Exponential backoff with a cap, used for integration retries and device
/// discovery so a persistent failure never becomes a busy loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    pub base_ms: u64,
    pub max_ms: u64,
    failures: u32,
}

impl Backoff {
    pub const fn new(base_ms: u64, max_ms: u64) -> Self {
        Self {
            base_ms,
            max_ms,
            failures: 0,
        }
    }

    pub fn failures(&self) -> u32 {
        self.failures
    }

    pub fn reset(&mut self) {
        self.failures = 0;
    }

    /// Records a failure and returns how long to wait before retrying.
    pub fn fail(&mut self) -> u64 {
        self.failures = self.failures.saturating_add(1);
        let shift = (self.failures - 1).min(16);
        self.base_ms.saturating_mul(1u64 << shift).min(self.max_ms)
    }

    pub fn current_delay_ms(&self) -> u64 {
        if self.failures == 0 {
            return 0;
        }
        let shift = (self.failures - 1).min(16);
        self.base_ms.saturating_mul(1u64 << shift).min(self.max_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_queue_asks_the_runtime_to_sleep() {
        let queue = DeadlineQueue::new();
        assert!(queue.is_empty());
        assert_eq!(queue.next_deadline_ms(), None);
    }

    #[test]
    fn the_next_deadline_is_the_earliest_entry() {
        let mut queue = DeadlineQueue::new();
        queue.set(DeadlineId::PomodoroCompletion, 5_000);
        queue.set(DeadlineId::CountdownTick, 1_200);
        queue.set(DeadlineId::Refresh(IntegrationId::GitHub), 300_000);

        assert_eq!(queue.next_deadline_ms(), Some(1_200));
        assert_eq!(queue.len(), 3);
    }

    #[test]
    fn setting_a_key_twice_replaces_rather_than_duplicates() {
        let mut queue = DeadlineQueue::new();
        queue.set(DeadlineId::CountdownTick, 1_000);
        queue.set(DeadlineId::CountdownTick, 2_000);

        assert_eq!(queue.len(), 1);
        assert_eq!(queue.get(DeadlineId::CountdownTick), Some(2_000));
    }

    #[test]
    fn set_if_sooner_never_pushes_a_deadline_later() {
        let mut queue = DeadlineQueue::new();
        queue.set_if_sooner(DeadlineId::PanelDismiss, 5_000);
        queue.set_if_sooner(DeadlineId::PanelDismiss, 9_000);
        assert_eq!(queue.get(DeadlineId::PanelDismiss), Some(5_000));

        queue.set_if_sooner(DeadlineId::PanelDismiss, 2_000);
        assert_eq!(queue.get(DeadlineId::PanelDismiss), Some(2_000));
    }

    #[test]
    fn due_deadlines_are_taken_once_in_a_stable_order() {
        let mut queue = DeadlineQueue::new();
        queue.set(DeadlineId::Refresh(IntegrationId::Weather), 900);
        queue.set(DeadlineId::PomodoroCompletion, 1_000);
        queue.set(DeadlineId::CountdownTick, 1_000);
        queue.set(DeadlineId::PanelDismiss, 4_000);

        let due = queue.take_due(1_000);
        assert_eq!(
            due,
            vec![
                DeadlineId::PomodoroCompletion,
                DeadlineId::CountdownTick,
                DeadlineId::Refresh(IntegrationId::Weather),
            ]
        );
        assert!(queue.take_due(1_000).is_empty());
        assert_eq!(queue.next_deadline_ms(), Some(4_000));
    }

    #[test]
    fn a_wake_drains_every_deadline_for_reconciliation() {
        let mut queue = DeadlineQueue::new();
        queue.set(DeadlineId::PomodoroCompletion, 10_000);
        queue.set(DeadlineId::Refresh(IntegrationId::LakeCurrent), 20_000);

        let drained = queue.drain_all();
        assert_eq!(drained.len(), 2);
        assert!(queue.is_empty());
    }

    #[test]
    fn clearing_removes_only_the_named_deadline() {
        let mut queue = DeadlineQueue::new();
        queue.set(DeadlineId::LongPressArm, 600);
        queue.set(DeadlineId::CountdownTick, 1_000);

        queue.clear(DeadlineId::LongPressArm);
        assert_eq!(queue.get(DeadlineId::LongPressArm), None);
        assert_eq!(queue.get(DeadlineId::CountdownTick), Some(1_000));
    }

    #[test]
    fn backoff_doubles_and_then_caps() {
        let mut backoff = Backoff::new(5_000, 300_000);
        assert_eq!(backoff.fail(), 5_000);
        assert_eq!(backoff.fail(), 10_000);
        assert_eq!(backoff.fail(), 20_000);
        assert_eq!(backoff.fail(), 40_000);
        for _ in 0..40 {
            backoff.fail();
        }
        assert_eq!(backoff.current_delay_ms(), 300_000);

        backoff.reset();
        assert_eq!(backoff.failures(), 0);
        assert_eq!(backoff.current_delay_ms(), 0);
    }

    #[test]
    fn refresh_deadlines_are_per_integration() {
        let mut queue = DeadlineQueue::new();
        queue.set(DeadlineId::Refresh(IntegrationId::GitHub), 1_000);
        queue.set(DeadlineId::Refresh(IntegrationId::Weather), 2_000);

        assert_eq!(queue.len(), 2);
        assert_eq!(
            DeadlineId::Refresh(IntegrationId::GitHub).describe(),
            "refresh:github"
        );
    }
}
