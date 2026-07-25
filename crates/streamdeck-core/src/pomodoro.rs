//! Pure Pomodoro state machine.
//!
//! Ported from the TypeScript `PomodoroService` so behaviour, bounds, wraparound,
//! statistics, and completion acknowledgement stay identical. Every transition is
//! a free function over `PomodoroState`, so the daemon only has to add persistence
//! and scheduling on top.

use std::collections::BTreeMap;

use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

pub const FOCUS_BOUNDS: (u32, u32) = (5, 90);
pub const SHORT_BREAK_BOUNDS: (u32, u32) = (1, 30);
pub const LONG_BREAK_BOUNDS: (u32, u32) = (5, 60);
pub const LONG_BREAK_EVERY: u32 = 4;

const DEFAULT_FOCUS_MINUTES: u32 = 25;
const DEFAULT_SHORT_BREAK_MINUTES: u32 = 5;
const DEFAULT_LONG_BREAK_MINUTES: u32 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Focus,
    ShortBreak,
    LongBreak,
}

impl Phase {
    pub const fn label(self) -> &'static str {
        match self {
            Phase::Focus => "FOCUS",
            Phase::ShortBreak => "SHORT BREAK",
            Phase::LongBreak => "LONG BREAK",
        }
    }

    pub const fn slug(self) -> &'static str {
        match self {
            Phase::Focus => "focus",
            Phase::ShortBreak => "short-break",
            Phase::LongBreak => "long-break",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "focus" => Some(Phase::Focus),
            "short-break" | "shortbreak" | "short" | "break" => Some(Phase::ShortBreak),
            "long-break" | "longbreak" | "long" => Some(Phase::LongBreak),
            _ => None,
        }
    }

    pub const fn bounds(self) -> (u32, u32) {
        match self {
            Phase::Focus => FOCUS_BOUNDS,
            Phase::ShortBreak => SHORT_BREAK_BOUNDS,
            Phase::LongBreak => LONG_BREAK_BOUNDS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ready,
    Running,
    Paused,
}

/// Persistent Pomodoro state. `ends_at` is a wall-clock instant so a running
/// timer survives daemon restarts and system sleep.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PomodoroState {
    pub phase: Phase,
    pub status: Status,
    pub ends_at_ms: Option<i64>,
    pub remaining_seconds: u32,
    pub focus_minutes: u32,
    pub short_break_minutes: u32,
    pub long_break_minutes: u32,
    pub cycle_focus_sessions: u32,
    pub completed_focus_sessions: u32,
    pub completed_short_breaks: u32,
    pub completed_long_breaks: u32,
    pub total_focus_minutes: u32,
    pub pending_completion_phase: Option<Phase>,
    #[serde(default)]
    pub daily_focus_minutes: BTreeMap<String, u32>,
    #[serde(default)]
    pub daily_focus_sessions: BTreeMap<String, u32>,
}

impl Default for PomodoroState {
    fn default() -> Self {
        Self {
            phase: Phase::Focus,
            status: Status::Ready,
            ends_at_ms: None,
            remaining_seconds: DEFAULT_FOCUS_MINUTES * 60,
            focus_minutes: DEFAULT_FOCUS_MINUTES,
            short_break_minutes: DEFAULT_SHORT_BREAK_MINUTES,
            long_break_minutes: DEFAULT_LONG_BREAK_MINUTES,
            cycle_focus_sessions: 0,
            completed_focus_sessions: 0,
            completed_short_breaks: 0,
            completed_long_breaks: 0,
            total_focus_minutes: 0,
            pending_completion_phase: None,
            daily_focus_minutes: BTreeMap::new(),
            daily_focus_sessions: BTreeMap::new(),
        }
    }
}

impl PomodoroState {
    pub fn phase_minutes(&self, phase: Phase) -> u32 {
        match phase {
            Phase::Focus => self.focus_minutes,
            Phase::ShortBreak => self.short_break_minutes,
            Phase::LongBreak => self.long_break_minutes,
        }
    }

    fn set_phase_minutes(&mut self, phase: Phase, minutes: u32) {
        match phase {
            Phase::Focus => self.focus_minutes = minutes,
            Phase::ShortBreak => self.short_break_minutes = minutes,
            Phase::LongBreak => self.long_break_minutes = minutes,
        }
    }

    /// Clamps every field into its documented bounds. Applied after deserializing
    /// persisted state so a hand-edited or migrated file can never produce an
    /// out-of-range timer.
    pub fn normalize(&mut self) {
        self.focus_minutes = clamp(self.focus_minutes, FOCUS_BOUNDS);
        self.short_break_minutes = clamp(self.short_break_minutes, SHORT_BREAK_BOUNDS);
        self.long_break_minutes = clamp(self.long_break_minutes, LONG_BREAK_BOUNDS);
        self.cycle_focus_sessions = self.cycle_focus_sessions.min(LONG_BREAK_EVERY);
        if self.status == Status::Running && self.ends_at_ms.is_none() {
            self.status = Status::Paused;
        }
        if self.status != Status::Running {
            self.ends_at_ms = None;
        }
        self.remaining_seconds = self
            .remaining_seconds
            .clamp(1, self.phase_minutes(self.phase).max(1) * 60 * 4);
    }
}

/// A derived, render-ready view of the timer at a point in time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PomodoroSnapshot {
    pub phase: Phase,
    pub status: Status,
    pub ends_at_ms: Option<i64>,
    pub remaining_seconds: u32,
    pub progress_permille: u32,
    pub focus_minutes: u32,
    pub short_break_minutes: u32,
    pub long_break_minutes: u32,
    pub cycle_focus_sessions: u32,
    pub completed_focus_sessions: u32,
    pub completed_short_breaks: u32,
    pub completed_long_breaks: u32,
    pub total_focus_minutes: u32,
    pub today_focus_minutes: u32,
    pub today_focus_sessions: u32,
    pub pending_completion_phase: Option<Phase>,
}

impl PomodoroSnapshot {
    /// Remaining fraction of the current phase, in `0.0..=1.0`.
    pub fn progress(&self) -> f32 {
        self.progress_permille as f32 / 1000.0
    }
}

/// Recomputes the timer against wall-clock time, completing the phase when its
/// deadline has passed. Called on load, on wake, and before every mutation so a
/// deadline crossed while the daemon was asleep still fires exactly once.
pub fn reconcile(state: &mut PomodoroState, now_ms: i64, timezone: Tz) -> Option<Phase> {
    if state.status != Status::Running {
        return None;
    }
    let ends_at = state.ends_at_ms?;
    if ends_at > now_ms {
        return None;
    }
    let completed = state.phase;
    complete_phase(state, now_ms, timezone);
    Some(completed)
}

/// Short press on the timer: start, pause, or resume.
pub fn toggle(state: &mut PomodoroState, now_ms: i64, timezone: Tz) {
    reconcile(state, now_ms, timezone);
    if state.status == Status::Running && state.ends_at_ms.is_some() {
        state.remaining_seconds = remaining_seconds(state, now_ms);
        state.ends_at_ms = None;
        state.status = Status::Paused;
        return;
    }
    state.ends_at_ms = Some(now_ms + i64::from(state.remaining_seconds) * 1_000);
    state.status = Status::Running;
}

/// Explicitly starts a phase from its full configured duration.
pub fn start_phase(state: &mut PomodoroState, phase: Phase, now_ms: i64) {
    let seconds = state.phase_minutes(phase) * 60;
    state.phase = phase;
    state.remaining_seconds = seconds;
    state.ends_at_ms = Some(now_ms + i64::from(seconds) * 1_000);
    state.status = Status::Running;
}

/// Queues the phase that would follow the current one without crediting statistics.
pub fn skip(state: &mut PomodoroState) {
    let next = next_phase(state, true);
    if state.phase == Phase::LongBreak {
        state.cycle_focus_sessions = 0;
    }
    state.phase = next;
    state.ends_at_ms = None;
    state.remaining_seconds = state.phase_minutes(next) * 60;
    state.status = Status::Ready;
}

/// Returns to a ready focus phase and clears the four-session cycle.
pub fn reset_session(state: &mut PomodoroState) {
    state.cycle_focus_sessions = 0;
    state.phase = Phase::Focus;
    state.ends_at_ms = None;
    state.remaining_seconds = state.focus_minutes * 60;
    state.status = Status::Ready;
}

/// Clears a pending completion. Returns `true` when something was acknowledged.
pub fn acknowledge(state: &mut PomodoroState) -> bool {
    state.pending_completion_phase.take().is_some()
}

/// Adjusts a configured duration, wrapping at the bounds, and keeps an in-flight
/// timer for the same phase consistent with the new length.
pub fn adjust_duration(
    state: &mut PomodoroState,
    duration: Phase,
    delta_minutes: i32,
    now_ms: i64,
) {
    let (minimum, maximum) = duration.bounds();
    let previous = state.phase_minutes(duration);
    let next = if delta_minutes > 0 && previous >= maximum {
        minimum
    } else if delta_minutes < 0 && previous <= minimum {
        maximum
    } else {
        (previous as i64 + i64::from(delta_minutes)).clamp(minimum as i64, maximum as i64) as u32
    };
    state.set_phase_minutes(duration, next);

    let delta_seconds = (i64::from(next) - i64::from(previous)) * 60;
    if state.phase != duration || delta_seconds == 0 {
        return;
    }

    match state.status {
        Status::Running => {
            if let Some(ends_at) = state.ends_at_ms {
                // Never leave the deadline in the past: a shrink that would land
                // behind `now` becomes one more second of the current phase.
                let shifted = (ends_at + delta_seconds * 1_000).max(now_ms + 1_000);
                state.ends_at_ms = Some(shifted);
                state.remaining_seconds = ((shifted - now_ms + 999) / 1_000).max(1) as u32;
            }
        }
        Status::Ready => state.remaining_seconds = next * 60,
        Status::Paused => {
            state.remaining_seconds =
                (i64::from(state.remaining_seconds) + delta_seconds).max(1) as u32
        }
    }
}

/// The phase that follows the current one. `skipping` counts the current focus
/// session towards the long-break cycle even though it was not completed.
pub fn next_phase(state: &PomodoroState, skipping: bool) -> Phase {
    if state.phase != Phase::Focus {
        return Phase::Focus;
    }
    let focus_count = state.cycle_focus_sessions + u32::from(skipping);
    if focus_count > 0 && focus_count % LONG_BREAK_EVERY == 0 {
        Phase::LongBreak
    } else {
        Phase::ShortBreak
    }
}

pub fn snapshot(state: &PomodoroState, now_ms: i64, timezone: Tz) -> PomodoroSnapshot {
    let seconds = remaining_seconds(state, now_ms);
    let duration_seconds = (state.phase_minutes(state.phase) * 60).max(1);
    let today = local_date_key(now_ms, timezone);
    PomodoroSnapshot {
        phase: state.phase,
        status: state.status,
        ends_at_ms: state.ends_at_ms,
        remaining_seconds: seconds,
        progress_permille: ((u64::from(seconds) * 1000) / u64::from(duration_seconds)).min(1000)
            as u32,
        focus_minutes: state.focus_minutes,
        short_break_minutes: state.short_break_minutes,
        long_break_minutes: state.long_break_minutes,
        cycle_focus_sessions: state.cycle_focus_sessions,
        completed_focus_sessions: state.completed_focus_sessions,
        completed_short_breaks: state.completed_short_breaks,
        completed_long_breaks: state.completed_long_breaks,
        total_focus_minutes: state.total_focus_minutes,
        today_focus_minutes: state
            .daily_focus_minutes
            .get(&today)
            .copied()
            .unwrap_or_default(),
        today_focus_sessions: state
            .daily_focus_sessions
            .get(&today)
            .copied()
            .unwrap_or_default(),
        pending_completion_phase: state.pending_completion_phase,
    }
}

fn complete_phase(state: &mut PomodoroState, now_ms: i64, timezone: Tz) {
    let finished = state.phase;
    state.pending_completion_phase = Some(finished);
    state.ends_at_ms = None;
    state.status = Status::Ready;

    match finished {
        Phase::Focus => {
            state.cycle_focus_sessions += 1;
            state.completed_focus_sessions += 1;
            state.total_focus_minutes += state.focus_minutes;
            let today = local_date_key(now_ms, timezone);
            *state.daily_focus_minutes.entry(today.clone()).or_default() += state.focus_minutes;
            *state.daily_focus_sessions.entry(today).or_default() += 1;
            let next = if state.cycle_focus_sessions % LONG_BREAK_EVERY == 0 {
                Phase::LongBreak
            } else {
                Phase::ShortBreak
            };
            state.phase = next;
            state.remaining_seconds = state.phase_minutes(next) * 60;
        }
        Phase::LongBreak => {
            state.completed_long_breaks += 1;
            state.cycle_focus_sessions = 0;
            state.phase = Phase::Focus;
            state.remaining_seconds = state.focus_minutes * 60;
        }
        Phase::ShortBreak => {
            state.completed_short_breaks += 1;
            state.phase = Phase::Focus;
            state.remaining_seconds = state.focus_minutes * 60;
        }
    }
}

fn remaining_seconds(state: &PomodoroState, now_ms: i64) -> u32 {
    match (state.status, state.ends_at_ms) {
        (Status::Running, Some(ends_at)) => {
            let millis = (ends_at - now_ms).max(0);
            ((millis + 999) / 1_000) as u32
        }
        _ => state.remaining_seconds,
    }
}

fn clamp(value: u32, bounds: (u32, u32)) -> u32 {
    value.clamp(bounds.0, bounds.1)
}

/// `YYYY-MM-DD` in the configured timezone, matching the `sv-SE` keys the
/// TypeScript plugin wrote so imported daily statistics line up.
pub fn local_date_key(now_ms: i64, timezone: Tz) -> String {
    let utc: DateTime<Utc> = DateTime::from_timestamp_millis(now_ms).unwrap_or_default();
    timezone
        .from_utc_datetime(&utc.naive_utc())
        .format("%Y-%m-%d")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::Europe::Stockholm;

    const T0: i64 = 1_753_300_000_000;

    fn running_focus() -> PomodoroState {
        let mut state = PomodoroState::default();
        toggle(&mut state, T0, Stockholm);
        state
    }

    #[test]
    fn toggle_walks_ready_running_paused_running() {
        let mut state = PomodoroState::default();
        assert_eq!(state.status, Status::Ready);

        toggle(&mut state, T0, Stockholm);
        assert_eq!(state.status, Status::Running);
        assert_eq!(state.ends_at_ms, Some(T0 + 25 * 60 * 1_000));

        toggle(&mut state, T0 + 60_000, Stockholm);
        assert_eq!(state.status, Status::Paused);
        assert_eq!(state.ends_at_ms, None);
        assert_eq!(state.remaining_seconds, 24 * 60);

        toggle(&mut state, T0 + 120_000, Stockholm);
        assert_eq!(state.status, Status::Running);
        assert_eq!(state.ends_at_ms, Some(T0 + 120_000 + 24 * 60 * 1_000));
    }

    #[test]
    fn focus_completion_credits_statistics_and_queues_short_break() {
        let mut state = running_focus();
        let completed = reconcile(&mut state, T0 + 25 * 60 * 1_000, Stockholm);

        assert_eq!(completed, Some(Phase::Focus));
        assert_eq!(state.phase, Phase::ShortBreak);
        assert_eq!(state.status, Status::Ready);
        assert_eq!(state.pending_completion_phase, Some(Phase::Focus));
        assert_eq!(state.completed_focus_sessions, 1);
        assert_eq!(state.cycle_focus_sessions, 1);
        assert_eq!(state.total_focus_minutes, 25);
        assert_eq!(state.remaining_seconds, 5 * 60);

        let key = local_date_key(T0 + 25 * 60 * 1_000, Stockholm);
        assert_eq!(state.daily_focus_minutes.get(&key), Some(&25));
        assert_eq!(state.daily_focus_sessions.get(&key), Some(&1));
    }

    #[test]
    fn fourth_focus_session_queues_a_long_break_and_long_break_resets_the_cycle() {
        let mut state = PomodoroState::default();
        let mut now = T0;
        for expected in 1..=4 {
            start_phase(&mut state, Phase::Focus, now);
            now += i64::from(state.focus_minutes) * 60 * 1_000;
            assert_eq!(reconcile(&mut state, now, Stockholm), Some(Phase::Focus));
            assert_eq!(state.cycle_focus_sessions, expected);
        }
        assert_eq!(state.phase, Phase::LongBreak);

        start_phase(&mut state, Phase::LongBreak, now);
        now += i64::from(state.long_break_minutes) * 60 * 1_000;
        assert_eq!(
            reconcile(&mut state, now, Stockholm),
            Some(Phase::LongBreak)
        );
        assert_eq!(state.cycle_focus_sessions, 0);
        assert_eq!(state.phase, Phase::Focus);
        assert_eq!(state.completed_long_breaks, 1);
    }

    #[test]
    fn reconcile_completes_a_deadline_crossed_during_sleep_exactly_once() {
        let mut state = running_focus();
        let woke = T0 + 9 * 60 * 60 * 1_000;

        assert_eq!(reconcile(&mut state, woke, Stockholm), Some(Phase::Focus));
        assert_eq!(reconcile(&mut state, woke, Stockholm), None);
        assert_eq!(state.completed_focus_sessions, 1);
    }

    #[test]
    fn skip_queues_the_next_phase_without_crediting_statistics() {
        let mut state = running_focus();
        skip(&mut state);

        assert_eq!(state.phase, Phase::ShortBreak);
        assert_eq!(state.status, Status::Ready);
        assert_eq!(state.completed_focus_sessions, 0);
        assert_eq!(state.cycle_focus_sessions, 0);
        assert_eq!(state.pending_completion_phase, None);
    }

    #[test]
    fn skipping_a_fourth_focus_session_queues_a_long_break() {
        let mut state = PomodoroState {
            cycle_focus_sessions: 3,
            ..PomodoroState::default()
        };
        skip(&mut state);
        assert_eq!(state.phase, Phase::LongBreak);
    }

    #[test]
    fn reset_returns_to_a_ready_focus_phase() {
        let mut state = running_focus();
        state.cycle_focus_sessions = 2;
        reset_session(&mut state);

        assert_eq!(state.phase, Phase::Focus);
        assert_eq!(state.status, Status::Ready);
        assert_eq!(state.cycle_focus_sessions, 0);
        assert_eq!(state.remaining_seconds, state.focus_minutes * 60);
    }

    #[test]
    fn acknowledge_clears_a_pending_completion_once() {
        let mut state = running_focus();
        reconcile(&mut state, T0 + 25 * 60 * 1_000, Stockholm);

        assert!(acknowledge(&mut state));
        assert!(!acknowledge(&mut state));
        assert_eq!(state.pending_completion_phase, None);
    }

    #[test]
    fn duration_adjustments_wrap_at_both_bounds() {
        let mut state = PomodoroState {
            focus_minutes: 90,
            ..PomodoroState::default()
        };

        adjust_duration(&mut state, Phase::Focus, 5, T0);
        assert_eq!(state.focus_minutes, 5);

        adjust_duration(&mut state, Phase::Focus, -5, T0);
        assert_eq!(state.focus_minutes, 90);

        state.short_break_minutes = 30;
        adjust_duration(&mut state, Phase::ShortBreak, 1, T0);
        assert_eq!(state.short_break_minutes, 1);

        adjust_duration(&mut state, Phase::ShortBreak, -1, T0);
        assert_eq!(state.short_break_minutes, 30);

        state.long_break_minutes = 60;
        adjust_duration(&mut state, Phase::LongBreak, 5, T0);
        assert_eq!(state.long_break_minutes, 5);
    }

    #[test]
    fn adjusting_the_active_phase_shifts_a_running_deadline() {
        let mut state = running_focus();
        let ends_at = state.ends_at_ms.expect("running");

        adjust_duration(&mut state, Phase::Focus, 5, T0);
        assert_eq!(state.ends_at_ms, Some(ends_at + 5 * 60 * 1_000));
        assert_eq!(state.remaining_seconds, 30 * 60);
    }

    #[test]
    fn shrinking_a_nearly_finished_timer_keeps_it_in_the_future() {
        let mut state = running_focus();
        let now = T0 + 25 * 60 * 1_000 - 500;

        adjust_duration(&mut state, Phase::Focus, -5, now);
        assert_eq!(state.ends_at_ms, Some(now + 1_000));
        assert_eq!(state.remaining_seconds, 1);
    }

    #[test]
    fn adjusting_an_inactive_phase_leaves_the_timer_alone() {
        let mut state = running_focus();
        let ends_at = state.ends_at_ms;

        adjust_duration(&mut state, Phase::LongBreak, 5, T0);
        assert_eq!(state.ends_at_ms, ends_at);
        assert_eq!(state.long_break_minutes, 20);
    }

    #[test]
    fn snapshot_reports_countdown_and_progress() {
        let state = running_focus();
        let snapshot = snapshot(&state, T0 + 5 * 60 * 1_000, Stockholm);

        assert_eq!(snapshot.remaining_seconds, 20 * 60);
        assert_eq!(snapshot.progress_permille, 800);
        assert_eq!(snapshot.status, Status::Running);
    }

    #[test]
    fn normalize_repairs_impossible_persisted_state() {
        let mut state = PomodoroState {
            status: Status::Running,
            ends_at_ms: None,
            focus_minutes: 500,
            short_break_minutes: 0,
            long_break_minutes: 999,
            cycle_focus_sessions: 12,
            remaining_seconds: 0,
            ..PomodoroState::default()
        };
        state.normalize();

        assert_eq!(state.status, Status::Paused);
        assert_eq!(state.focus_minutes, 90);
        assert_eq!(state.short_break_minutes, 1);
        assert_eq!(state.long_break_minutes, 60);
        assert_eq!(state.cycle_focus_sessions, 4);
        assert_eq!(state.remaining_seconds, 1);
    }

    #[test]
    fn local_date_key_uses_the_configured_timezone() {
        // 2026-07-24 22:30 UTC is already 2026-07-25 in Stockholm (UTC+2).
        let midnight_edge = 1_784_932_200_000;
        assert_eq!(local_date_key(midnight_edge, Stockholm), "2026-07-25");
        assert_eq!(local_date_key(midnight_edge, chrono_tz::UTC), "2026-07-24");
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;

        fn any_phase() -> impl Strategy<Value = Phase> {
            prop_oneof![
                Just(Phase::Focus),
                Just(Phase::ShortBreak),
                Just(Phase::LongBreak)
            ]
        }

        proptest! {
            #[test]
            fn durations_always_stay_within_bounds(
                phase in any_phase(),
                deltas in prop::collection::vec(-10i32..=10, 1..40),
            ) {
                let mut state = PomodoroState::default();
                for delta in deltas {
                    adjust_duration(&mut state, phase, delta, T0);
                    let (minimum, maximum) = phase.bounds();
                    let value = state.phase_minutes(phase);
                    prop_assert!((minimum..=maximum).contains(&value), "{value} outside {minimum}..={maximum}");
                }
            }

            #[test]
            fn a_running_timer_never_reports_more_than_its_phase_length(
                elapsed_seconds in 0u32..7200,
            ) {
                let state = running_focus();
                let snap = snapshot(&state, T0 + i64::from(elapsed_seconds) * 1_000, Stockholm);
                prop_assert!(snap.remaining_seconds <= state.focus_minutes * 60);
                prop_assert!(snap.progress_permille <= 1000);
            }

            #[test]
            fn arbitrary_command_sequences_keep_state_consistent(
                commands in prop::collection::vec(0u8..7, 1..60),
                phase in any_phase(),
            ) {
                let mut state = PomodoroState::default();
                let mut now = T0;
                for command in commands {
                    now += 37_000;
                    match command {
                        0 => toggle(&mut state, now, Stockholm),
                        1 => skip(&mut state),
                        2 => reset_session(&mut state),
                        3 => start_phase(&mut state, phase, now),
                        4 => { acknowledge(&mut state); }
                        5 => adjust_duration(&mut state, phase, 5, now),
                        _ => { reconcile(&mut state, now, Stockholm); }
                    }

                    prop_assert_eq!(
                        state.status == Status::Running,
                        state.ends_at_ms.is_some(),
                        "running iff a deadline exists"
                    );
                    prop_assert!(state.cycle_focus_sessions <= LONG_BREAK_EVERY);
                    prop_assert!(state.remaining_seconds >= 1);
                    let snap = snapshot(&state, now, Stockholm);
                    prop_assert!(snap.progress_permille <= 1000);
                }
            }
        }
    }
}
