use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

pub const MIN_SPEED_TENTHS: u8 = 5;
pub const MAX_SPEED_TENTHS: u8 = 60;
pub const SPEED_STEP_TENTHS: u8 = 2;
pub const STATUS_STALE_AFTER_MS: i64 = 3_000;
pub const COMMAND_CONFIRMATION_TIMEOUT_MS: i64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkingPadConnection {
    Disconnected,
    Connecting,
    Connected,
    Locked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkingPadCommand {
    Start,
    Stop,
    Increase,
    Decrease,
    SetSpeed(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkingPadRequest {
    Start,
    Stop,
    SetSpeed(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkingPadMode {
    Automatic,
    Manual,
    Standby,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WalkingPadCounters {
    pub distance_hundredths: u32,
    pub steps: u32,
    pub elapsed_seconds: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkingPadTelemetry {
    pub counters: WalkingPadCounters,
    pub speed_tenths: u8,
    pub target_speed_tenths: u8,
    pub belt_state: u8,
    pub mode: WalkingPadMode,
}

impl WalkingPadTelemetry {
    pub fn is_moving(&self) -> bool {
        self.speed_tenths > 0
    }

    pub fn control_speed_tenths(&self) -> u8 {
        if self.is_moving()
            && (MIN_SPEED_TENTHS..=MAX_SPEED_TENTHS).contains(&self.target_speed_tenths)
        {
            self.target_speed_tenths
        } else {
            self.speed_tenths
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalkingPadUpdate {
    Connection {
        state: WalkingPadConnection,
        error: Option<String>,
    },
    Status {
        telemetry: WalkingPadTelemetry,
        received_at_ms: i64,
    },
    CommandSucceeded(WalkingPadRequest),
    CommandFailed {
        request: WalkingPadRequest,
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingWalkingPadCommand {
    pub source: WalkingPadCommand,
    pub request: WalkingPadRequest,
    pub awaiting_status: bool,
    pub confirmation_deadline_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkingPadFeedback {
    pub source: WalkingPadCommand,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkingPadState {
    pub connection: WalkingPadConnection,
    pub connection_error: Option<String>,
    pub telemetry: Option<WalkingPadTelemetry>,
    pub last_status_at_ms: Option<i64>,
    pub pending: Option<PendingWalkingPadCommand>,
    pub feedback: Option<WalkingPadFeedback>,
}

impl Default for WalkingPadState {
    fn default() -> Self {
        Self {
            connection: WalkingPadConnection::Disconnected,
            connection_error: None,
            telemetry: None,
            last_status_at_ms: None,
            pending: None,
            feedback: None,
        }
    }
}

impl WalkingPadState {
    pub fn status_age_ms(&self, now_ms: i64) -> Option<i64> {
        self.last_status_at_ms
            .map(|received| now_ms.saturating_sub(received).max(0))
    }

    pub fn has_fresh_status(&self, now_ms: i64) -> bool {
        self.connection == WalkingPadConnection::Connected
            && self
                .status_age_ms(now_ms)
                .is_some_and(|age| age <= STATUS_STALE_AFTER_MS)
    }

    pub fn prepare(
        &mut self,
        command: WalkingPadCommand,
        now_ms: i64,
    ) -> Result<Option<WalkingPadRequest>, &'static str> {
        if self.pending.is_some() && command != WalkingPadCommand::Stop {
            return Err("COMMAND BUSY");
        }
        if self.connection != WalkingPadConnection::Connected {
            return Err("DISCONNECTED");
        }

        let current = self
            .telemetry
            .as_ref()
            .map(WalkingPadTelemetry::control_speed_tenths);
        if command != WalkingPadCommand::Stop && !self.has_fresh_status(now_ms) {
            return Err("STATUS STALE");
        }
        let request = request_for(command, current)?;
        if let Some(request) = request {
            self.feedback = None;
            self.pending = Some(PendingWalkingPadCommand {
                source: command,
                request,
                awaiting_status: false,
                confirmation_deadline_at_ms: now_ms.saturating_add(COMMAND_CONFIRMATION_TIMEOUT_MS),
            });
        }
        Ok(request)
    }

    pub fn reject(&mut self, source: WalkingPadCommand, message: impl Into<String>) {
        self.feedback = Some(WalkingPadFeedback {
            source,
            message: message.into(),
        });
    }

    pub fn connection_changed(&mut self, connection: WalkingPadConnection, error: Option<String>) {
        self.connection = connection;
        self.connection_error = error;
        if connection != WalkingPadConnection::Connected {
            if let Some(pending) = self.pending.take() {
                self.reject(pending.source, "CONNECTION LOST");
            }
        }
    }

    pub fn command_succeeded(&mut self, request: WalkingPadRequest) {
        if let Some(pending) = self.pending.as_mut() {
            if pending.request == request {
                pending.awaiting_status = true;
            }
        }
    }

    pub fn command_failed(&mut self, request: WalkingPadRequest, error: impl Into<String>) {
        let source = self
            .pending
            .take()
            .filter(|pending| pending.request == request)
            .map(|pending| pending.source)
            .unwrap_or_else(|| command_for_request(request));
        self.reject(source, error);
    }

    pub fn apply_status(&mut self, telemetry: WalkingPadTelemetry, received_at_ms: i64) {
        if let Some(pending) = self.pending.take() {
            let confirmed =
                pending.awaiting_status && request_confirmed(pending.request, &telemetry);
            if !pending.awaiting_status
                || (!confirmed && received_at_ms < pending.confirmation_deadline_at_ms)
            {
                self.pending = Some(pending);
            } else if !confirmed {
                self.reject(pending.source, "NOT CONFIRMED");
            }
        }
        self.telemetry = Some(telemetry);
        self.last_status_at_ms = Some(received_at_ms);
        self.connection = WalkingPadConnection::Connected;
        self.connection_error = None;
    }

    pub fn clear_feedback(&mut self) {
        self.feedback = None;
    }
}

pub fn request_for(
    command: WalkingPadCommand,
    current_tenths: Option<u8>,
) -> Result<Option<WalkingPadRequest>, &'static str> {
    match command {
        WalkingPadCommand::Stop => Ok(Some(WalkingPadRequest::Stop)),
        WalkingPadCommand::Start => match current_tenths {
            Some(0) => Ok(Some(WalkingPadRequest::Start)),
            Some(_) => Err("ALREADY MOVING"),
            None => Err("NO STATUS"),
        },
        WalkingPadCommand::Increase => {
            let current = moving_speed(current_tenths)?;
            if current >= MAX_SPEED_TENTHS {
                return Err("MAX 6.0 KM/H");
            }
            Ok(Some(WalkingPadRequest::SetSpeed(
                current
                    .saturating_add(SPEED_STEP_TENTHS)
                    .min(MAX_SPEED_TENTHS),
            )))
        }
        WalkingPadCommand::Decrease => {
            let current = moving_speed(current_tenths)?;
            if current <= MIN_SPEED_TENTHS {
                return Err("MIN 0.5 KM/H");
            }
            Ok(Some(WalkingPadRequest::SetSpeed(
                current
                    .saturating_sub(SPEED_STEP_TENTHS)
                    .max(MIN_SPEED_TENTHS),
            )))
        }
        WalkingPadCommand::SetSpeed(tenths) => {
            moving_speed(current_tenths)?;
            if !(MIN_SPEED_TENTHS..=MAX_SPEED_TENTHS).contains(&tenths) {
                return Err("INVALID SPEED");
            }
            Ok(Some(WalkingPadRequest::SetSpeed(tenths)))
        }
    }
}

fn moving_speed(current_tenths: Option<u8>) -> Result<u8, &'static str> {
    match current_tenths {
        Some(0) => Err("START FIRST"),
        Some(speed) => Ok(speed.clamp(MIN_SPEED_TENTHS, MAX_SPEED_TENTHS)),
        None => Err("NO STATUS"),
    }
}

fn request_confirmed(request: WalkingPadRequest, telemetry: &WalkingPadTelemetry) -> bool {
    match request {
        WalkingPadRequest::Start => telemetry.speed_tenths >= MIN_SPEED_TENTHS,
        WalkingPadRequest::Stop => telemetry.speed_tenths == 0,
        WalkingPadRequest::SetSpeed(tenths) => telemetry.target_speed_tenths == tenths,
    }
}

fn command_for_request(request: WalkingPadRequest) -> WalkingPadCommand {
    match request {
        WalkingPadRequest::Start => WalkingPadCommand::Start,
        WalkingPadRequest::Stop => WalkingPadCommand::Stop,
        WalkingPadRequest::SetSpeed(tenths) => WalkingPadCommand::SetSpeed(tenths),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Calendar-day totals derived from deltas the daemon observed while connected.
/// Belt activity completed entirely while the daemon was offline cannot be backfilled safely.
pub struct WalkingPadDailyTotals {
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub distance_hundredths: u64,
    #[serde(default)]
    pub steps: u64,
    #[serde(default)]
    pub elapsed_seconds: u64,
    #[serde(default)]
    pub last_observed: Option<WalkingPadCounters>,
    #[serde(default)]
    pub last_observed_at_ms: Option<i64>,
}

impl WalkingPadDailyTotals {
    pub fn rollover(&mut self, now: DateTime<Utc>, timezone: Tz) -> bool {
        let date = local_date(now, timezone);
        if self.date == date {
            return false;
        }
        self.date = date;
        self.distance_hundredths = 0;
        self.steps = 0;
        self.elapsed_seconds = 0;
        self.last_observed = None;
        self.last_observed_at_ms = None;
        true
    }

    pub fn observe(
        &mut self,
        counters: WalkingPadCounters,
        observed_at: DateTime<Utc>,
        timezone: Tz,
        continuous: bool,
    ) {
        let rolled_over = self.rollover(observed_at, timezone);
        if continuous && !rolled_over {
            if let Some(previous) = self.last_observed {
                if counters.distance_hundredths >= previous.distance_hundredths
                    && counters.steps >= previous.steps
                    && counters.elapsed_seconds >= previous.elapsed_seconds
                {
                    self.distance_hundredths = self.distance_hundredths.saturating_add(u64::from(
                        counters.distance_hundredths - previous.distance_hundredths,
                    ));
                    self.steps = self
                        .steps
                        .saturating_add(u64::from(counters.steps - previous.steps));
                    self.elapsed_seconds = self.elapsed_seconds.saturating_add(u64::from(
                        counters.elapsed_seconds - previous.elapsed_seconds,
                    ));
                }
            }
        }
        self.last_observed = Some(counters);
        self.last_observed_at_ms = Some(observed_at.timestamp_millis());
    }
}

pub fn speed_tenths(speed_kph: f32) -> u8 {
    if !speed_kph.is_finite() || speed_kph <= 0.0 {
        return 0;
    }
    (speed_kph * 10.0).round().clamp(0.0, 60.0) as u8
}

pub fn distance_hundredths(distance_km: f32) -> u32 {
    if !distance_km.is_finite() || distance_km <= 0.0 {
        return 0;
    }
    (distance_km * 100.0).round().clamp(0.0, u32::MAX as f32) as u32
}

fn local_date(now: DateTime<Utc>, timezone: Tz) -> String {
    now.with_timezone(&timezone).format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use chrono_tz::Europe::Stockholm;

    use super::*;

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        chrono_tz::Europe::Stockholm
            .with_ymd_and_hms(2026, 8, day, hour, 0, 0)
            .single()
            .expect("time")
            .with_timezone(&Utc)
    }

    fn counters(distance: u32, steps: u32, elapsed: u32) -> WalkingPadCounters {
        WalkingPadCounters {
            distance_hundredths: distance,
            steps,
            elapsed_seconds: elapsed,
        }
    }

    fn telemetry(speed: u8) -> WalkingPadTelemetry {
        WalkingPadTelemetry {
            counters: WalkingPadCounters::default(),
            speed_tenths: speed,
            target_speed_tenths: speed,
            belt_state: u8::from(speed > 0),
            mode: WalkingPadMode::Manual,
        }
    }

    #[test]
    fn speed_controls_use_exact_tenths_and_respect_boundaries() {
        assert_eq!(
            request_for(WalkingPadCommand::Increase, Some(34)),
            Ok(Some(WalkingPadRequest::SetSpeed(36)))
        );
        assert_eq!(
            request_for(WalkingPadCommand::Decrease, Some(34)),
            Ok(Some(WalkingPadRequest::SetSpeed(32)))
        );
        assert_eq!(
            request_for(WalkingPadCommand::Decrease, Some(5)),
            Err("MIN 0.5 KM/H")
        );
        assert_eq!(
            request_for(WalkingPadCommand::Increase, Some(60)),
            Err("MAX 6.0 KM/H")
        );
    }

    #[test]
    fn quick_speeds_are_exact_and_never_start_a_stopped_belt() {
        for speed in [26, 30, 34, 42, 45] {
            assert_eq!(
                request_for(WalkingPadCommand::SetSpeed(speed), Some(20)),
                Ok(Some(WalkingPadRequest::SetSpeed(speed)))
            );
            assert_eq!(
                request_for(WalkingPadCommand::SetSpeed(speed), Some(0)),
                Err("START FIRST")
            );
        }
        assert_eq!(
            request_for(WalkingPadCommand::Start, Some(0)),
            Ok(Some(WalkingPadRequest::Start))
        );
    }

    #[test]
    fn target_confirmation_does_not_wait_for_the_belt_to_finish_ramping() {
        let mut state = WalkingPadState {
            connection: WalkingPadConnection::Connected,
            telemetry: Some(telemetry(30)),
            last_status_at_ms: Some(1_000),
            ..WalkingPadState::default()
        };
        let request = state
            .prepare(WalkingPadCommand::SetSpeed(34), 1_000)
            .expect("prepared")
            .expect("request");
        state.command_succeeded(request);
        let mut ramping = telemetry(30);
        ramping.target_speed_tenths = 34;
        state.apply_status(ramping, 2_000);

        assert!(state.pending.is_none());
        assert!(state.feedback.is_none());
        assert_eq!(state.telemetry.expect("status").speed_tenths, 30);
    }

    #[test]
    fn an_unconfirmed_command_remains_pending_until_its_deadline() {
        let mut state = WalkingPadState {
            connection: WalkingPadConnection::Connected,
            telemetry: Some(telemetry(30)),
            last_status_at_ms: Some(1_000),
            ..WalkingPadState::default()
        };
        let request = state
            .prepare(WalkingPadCommand::SetSpeed(34), 1_000)
            .expect("prepared")
            .expect("request");
        state.command_succeeded(request);
        state.apply_status(telemetry(30), 2_000);
        assert!(state.pending.is_some());
        assert!(state.feedback.is_none());

        state.apply_status(telemetry(30), 11_000);
        assert!(state.pending.is_none());
        assert_eq!(state.feedback.expect("feedback").message, "NOT CONFIRMED");
    }

    #[test]
    fn daily_totals_accumulate_only_positive_continuous_deltas() {
        let mut totals = WalkingPadDailyTotals::default();
        totals.observe(counters(100, 1_000, 600), at(4, 9), Stockholm, false);
        totals.observe(counters(112, 1_150, 660), at(4, 9), Stockholm, true);

        assert_eq!(totals.distance_hundredths, 12);
        assert_eq!(totals.steps, 150);
        assert_eq!(totals.elapsed_seconds, 60);
    }

    #[test]
    fn a_counter_reset_starts_a_new_baseline_without_adding_the_new_run() {
        let mut totals = WalkingPadDailyTotals::default();
        totals.observe(counters(100, 1_000, 600), at(4, 9), Stockholm, false);
        totals.observe(counters(110, 1_100, 660), at(4, 9), Stockholm, true);
        totals.observe(counters(2, 20, 12), at(4, 10), Stockholm, true);
        totals.observe(counters(5, 50, 30), at(4, 10), Stockholm, true);

        assert_eq!(totals.distance_hundredths, 13);
        assert_eq!(totals.steps, 130);
        assert_eq!(totals.elapsed_seconds, 78);
    }

    #[test]
    fn process_restart_and_reconnect_seed_a_baseline_conservatively() {
        let mut totals = WalkingPadDailyTotals::default();
        totals.observe(counters(100, 1_000, 600), at(4, 9), Stockholm, false);
        totals.observe(counters(110, 1_100, 660), at(4, 9), Stockholm, true);

        let persisted = serde_json::to_string(&totals).expect("serialize");
        let mut restarted: WalkingPadDailyTotals =
            serde_json::from_str(&persisted).expect("deserialize");
        restarted.observe(counters(130, 1_300, 780), at(4, 10), Stockholm, false);
        restarted.observe(counters(135, 1_350, 810), at(4, 10), Stockholm, true);

        assert_eq!(restarted.distance_hundredths, 15);
        assert_eq!(restarted.steps, 150);
        assert_eq!(restarted.elapsed_seconds, 90);
    }

    #[test]
    fn midnight_rollover_resets_totals_and_does_not_cross_attribute_a_sample() {
        let mut totals = WalkingPadDailyTotals::default();
        totals.observe(counters(100, 1_000, 600), at(4, 23), Stockholm, false);
        totals.observe(counters(110, 1_100, 660), at(4, 23), Stockholm, true);
        totals.observe(counters(120, 1_200, 720), at(5, 0), Stockholm, true);
        totals.observe(counters(125, 1_250, 750), at(5, 0), Stockholm, true);

        assert_eq!(totals.date, "2026-08-05");
        assert_eq!(totals.distance_hundredths, 5);
        assert_eq!(totals.steps, 50);
        assert_eq!(totals.elapsed_seconds, 30);
    }

    #[test]
    fn protocol_floats_are_rounded_once_at_the_boundary() {
        assert_eq!(speed_tenths(3.399), 34);
        assert_eq!(distance_hundredths(1.519), 152);
        assert_eq!(speed_tenths(f32::NAN), 0);
        assert_eq!(distance_hundredths(f32::INFINITY), 0);
    }
}
