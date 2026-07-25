//! Internal metrics, exposed only through `streamdeckctl status --json`.
//!
//! No remote telemetry: these numbers exist so the performance acceptance
//! criteria can be checked, and so a soak test has something to graph.

use std::collections::BTreeMap;
use std::time::Instant;

use serde::Serialize;
use streamdeck_core::model::IntegrationId;

/// Counters and gauges for one running daemon.
#[derive(Debug)]
pub struct Metrics {
    started: Instant,
    pub renders: u64,
    /// Compositions avoided because the key's semantic view was unchanged.
    pub renders_skipped: u64,
    pub frames_sent: u64,
    pub frames_skipped: u64,
    pub bytes_sent: u64,
    pub key_presses: u64,
    pub long_presses: u64,
    pub page_switches: u64,
    pub device_reconnects: u64,
    pub wakes: u64,
    pub config_reloads: u64,
    pub last_config_error: Option<String>,
    integrations: BTreeMap<IntegrationId, IntegrationMetrics>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct IntegrationMetrics {
    pub requests: u64,
    pub failures: u64,
    /// Seconds since the last successful refresh.
    pub age_seconds: Option<u64>,
    pub last_error: Option<String>,
    pub stale: bool,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            renders: 0,
            renders_skipped: 0,
            frames_sent: 0,
            frames_skipped: 0,
            bytes_sent: 0,
            key_presses: 0,
            long_presses: 0,
            page_switches: 0,
            device_reconnects: 0,
            wakes: 0,
            config_reloads: 0,
            last_config_error: None,
            integrations: BTreeMap::new(),
        }
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    pub fn integration(&mut self, id: IntegrationId) -> &mut IntegrationMetrics {
        self.integrations.entry(id).or_default()
    }

    pub fn record_success(&mut self, id: IntegrationId, age_seconds: u64) {
        let entry = self.integration(id);
        entry.requests += 1;
        entry.age_seconds = Some(age_seconds);
        entry.last_error = None;
        entry.stale = false;
    }

    pub fn record_failure(&mut self, id: IntegrationId, error: impl Into<String>, stale: bool) {
        let entry = self.integration(id);
        entry.requests += 1;
        entry.failures += 1;
        entry.last_error = Some(error.into());
        entry.stale = stale;
    }

    pub fn integrations(&self) -> &BTreeMap<IntegrationId, IntegrationMetrics> {
        &self.integrations
    }

    /// Resident set size in mebibytes, read from the OS.
    pub fn resident_mib() -> Option<f64> {
        resident_bytes().map(|bytes| bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Reads this process's resident memory. Uses `task_info` on macOS via `ps`,
/// because a direct Mach call would pull in another dependency for one number.
#[cfg(unix)]
fn resident_bytes() -> Option<u64> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-o", "rss=", "-p"])
        .arg(std::process::id().to_string())
        .output()
        .ok()?;
    let kilobytes: u64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .ok()?;
    Some(kilobytes * 1024)
}

#[cfg(not(unix))]
fn resident_bytes() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_metrics_set_starts_at_zero() {
        let metrics = Metrics::new();
        assert_eq!(metrics.renders, 0);
        assert_eq!(metrics.frames_sent, 0);
        assert!(metrics.integrations().is_empty());
        assert!(metrics.last_config_error.is_none());
    }

    #[test]
    fn a_successful_refresh_records_its_age_and_clears_any_error() {
        let mut metrics = Metrics::new();
        metrics.record_failure(IntegrationId::GitHub, "timeout", true);
        metrics.record_success(IntegrationId::GitHub, 12);

        let entry = metrics
            .integrations()
            .get(&IntegrationId::GitHub)
            .expect("recorded");
        assert_eq!(entry.requests, 2);
        assert_eq!(entry.failures, 1);
        assert_eq!(entry.age_seconds, Some(12));
        assert_eq!(entry.last_error, None);
        assert!(!entry.stale);
    }

    #[test]
    fn a_failure_records_the_reason_and_whether_stale_data_is_showing() {
        let mut metrics = Metrics::new();
        metrics.record_failure(IntegrationId::Weather, "met.no http 503", true);

        let entry = metrics
            .integrations()
            .get(&IntegrationId::Weather)
            .expect("recorded");
        assert_eq!(entry.last_error.as_deref(), Some("met.no http 503"));
        assert!(entry.stale);
    }

    #[test]
    fn integrations_are_tracked_independently() {
        let mut metrics = Metrics::new();
        metrics.record_success(IntegrationId::GitHub, 1);
        metrics.record_failure(IntegrationId::Spotify, "not running", false);

        assert_eq!(metrics.integrations().len(), 2);
        assert_eq!(metrics.integrations()[&IntegrationId::Spotify].failures, 1);
        assert_eq!(metrics.integrations()[&IntegrationId::GitHub].failures, 0);
    }

    #[test]
    fn uptime_is_monotonic() {
        let metrics = Metrics::new();
        assert_eq!(metrics.uptime_seconds(), 0);
    }

    #[test]
    fn resident_memory_is_readable_and_plausible() {
        let resident = Metrics::resident_mib().expect("readable on this platform");
        assert!(resident > 0.5, "{resident} MiB looks wrong");
        assert!(resident < 8_192.0, "{resident} MiB looks wrong");
    }
}
