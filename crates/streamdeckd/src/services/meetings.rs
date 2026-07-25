//! Google Calendar through the authenticated `gog` CLI.
//!
//! Both accounts are read concurrently. One account failing while the other
//! succeeds is a partial success, not an error: the tiles show what is known and
//! the failure is logged.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use streamdeck_core::config::MeetingsConfig;
use streamdeck_core::integrations::meetings::{self, Meeting};
use streamdeck_macos::{timeouts, CommandRunner};

#[derive(Debug, thiserror::Error)]
pub enum MeetingsError {
    #[error("no Google Calendar account could be read: {0}")]
    AllAccountsFailed(String),
    #[error("no Google Calendar account is configured")]
    NotConfigured,
}

/// The merged meetings plus any per-account failures worth logging.
#[derive(Debug, Clone)]
pub struct MeetingsResult {
    pub meetings: Vec<Meeting>,
    /// `(account, sanitized reason)` for each account that failed.
    pub failures: Vec<(String, String)>,
}

pub async fn fetch(
    runner: &Arc<dyn CommandRunner>,
    gog: &str,
    config: &MeetingsConfig,
    now: DateTime<Utc>,
) -> Result<MeetingsResult, MeetingsError> {
    if config.accounts.is_empty() {
        return Err(MeetingsError::NotConfigured);
    }

    let days = config.horizon_days.to_string();
    let max = config.max_events.to_string();

    let requests = config.accounts.iter().map(|account| {
        let runner = Arc::clone(runner);
        let gog = gog.to_string();
        let account = account.clone();
        let days = days.clone();
        let max = max.clone();
        async move {
            let result = runner
                .run(
                    &gog,
                    &[
                        "--enable-commands",
                        "calendar.events",
                        "--account",
                        &account,
                        "calendar",
                        "events",
                        "--from",
                        "now",
                        "--days",
                        &days,
                        "--max",
                        &max,
                        "--sort",
                        "start",
                        "--json",
                        "--results-only",
                        "--no-input",
                    ],
                    timeouts::CALENDAR,
                )
                .await;
            (account, result)
        }
    });

    let mut meetings = Vec::new();
    let mut failures = Vec::new();
    let mut succeeded = 0usize;

    for (account, result) in futures_join_all(requests).await {
        match result {
            Ok(output) => match meetings::parse_events(&account, &output.stdout, now) {
                Ok(mut parsed) => {
                    succeeded += 1;
                    meetings.append(&mut parsed);
                }
                Err(error) => failures.push((account, error.to_string())),
            },
            Err(error) => failures.push((account, error.to_string())),
        }
    }

    if succeeded == 0 {
        let reason = failures
            .iter()
            .map(|(account, error)| format!("{account}: {error}"))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(MeetingsError::AllAccountsFailed(reason));
    }

    Ok(MeetingsResult {
        meetings: meetings::merge(meetings),
        failures,
    })
}

/// Awaits a set of futures concurrently without pulling in a futures crate.
async fn futures_join_all<F, T>(futures: impl Iterator<Item = F>) -> Vec<T>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let handles: Vec<_> = futures.map(tokio::spawn).collect();
    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        if let Ok(value) = handle.await {
            results.push(value);
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use streamdeck_macos::fake::{FakeCommandRunner, Reply};

    const EVENTS: &str = include_str!("../../../../tests/fixtures/gog-calendar-events.json");

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-24T06:00:00Z")
            .expect("timestamp")
            .with_timezone(&Utc)
    }

    fn config() -> MeetingsConfig {
        MeetingsConfig {
            accounts: vec![
                "jimmy.stridh@gmail.com".to_string(),
                "jimmy.stridh@visma.com".to_string(),
            ],
            ..Default::default()
        }
    }

    fn as_runner(runner: &Arc<FakeCommandRunner>) -> Arc<dyn CommandRunner> {
        Arc::clone(runner) as Arc<dyn CommandRunner>
    }

    #[tokio::test]
    async fn both_accounts_are_read_and_merged() {
        let fake = Arc::new(FakeCommandRunner::new());
        fake.fallback(Reply::ok(EVENTS));

        let result = fetch(&as_runner(&fake), "/opt/homebrew/bin/gog", &config(), now())
            .await
            .expect("fetched");

        assert_eq!(fake.call_count(), 2);
        assert!(fake.called_with("--account jimmy.stridh@gmail.com"));
        assert!(fake.called_with("--account jimmy.stridh@visma.com"));
        assert!(result.failures.is_empty());
        // The same fixture from both accounts must collapse on the Meet URL.
        assert_eq!(result.meetings.len(), 3);
    }

    #[tokio::test]
    async fn the_command_carries_the_documented_horizon_and_limits() {
        let fake = Arc::new(FakeCommandRunner::new());
        fake.fallback(Reply::ok("[]"));

        fetch(&as_runner(&fake), "/opt/homebrew/bin/gog", &config(), now())
            .await
            .expect("fetched");

        assert!(fake.called_with("--enable-commands calendar.events"));
        assert!(fake.called_with("calendar events --from now --days 14 --max 100"));
        assert!(fake.called_with("--sort start --json --results-only --no-input"));
    }

    #[tokio::test]
    async fn one_failing_account_still_yields_the_other_accounts_meetings() {
        let fake = Arc::new(FakeCommandRunner::new());
        fake.on("jimmy.stridh@gmail.com", Reply::fails(1, "token expired"))
            .on("jimmy.stridh@visma.com", Reply::ok(EVENTS));

        let result = fetch(&as_runner(&fake), "/opt/homebrew/bin/gog", &config(), now())
            .await
            .expect("partial success");

        assert_eq!(result.meetings.len(), 3);
        assert_eq!(result.failures.len(), 1);
        assert_eq!(result.failures[0].0, "jimmy.stridh@gmail.com");
    }

    #[tokio::test]
    async fn both_accounts_failing_is_an_error_that_names_them() {
        let fake = Arc::new(FakeCommandRunner::new());
        fake.fallback(Reply::fails(1, "token expired"));

        let error = fetch(&as_runner(&fake), "/opt/homebrew/bin/gog", &config(), now())
            .await
            .expect_err("both failed");
        let message = error.to_string();
        assert!(message.contains("jimmy.stridh@gmail.com"), "{message}");
        assert!(message.contains("jimmy.stridh@visma.com"), "{message}");
    }

    #[tokio::test]
    async fn malformed_output_from_one_account_is_treated_as_that_account_failing() {
        let fake = Arc::new(FakeCommandRunner::new());
        fake.on("jimmy.stridh@gmail.com", Reply::ok("not json"))
            .on("jimmy.stridh@visma.com", Reply::ok(EVENTS));

        let result = fetch(&as_runner(&fake), "/opt/homebrew/bin/gog", &config(), now())
            .await
            .expect("partial success");
        assert_eq!(result.failures.len(), 1);
        assert!(!result.meetings.is_empty());
    }

    #[tokio::test]
    async fn a_timeout_counts_as_that_account_failing() {
        let fake = Arc::new(FakeCommandRunner::new());
        fake.on("jimmy.stridh@gmail.com", Reply::Timeout)
            .on("jimmy.stridh@visma.com", Reply::ok(EVENTS));

        let result = fetch(&as_runner(&fake), "/opt/homebrew/bin/gog", &config(), now())
            .await
            .expect("partial success");
        assert_eq!(result.failures.len(), 1);
        assert!(
            result.failures[0].1.contains("timed out"),
            "{:?}",
            result.failures
        );
    }

    #[tokio::test]
    async fn no_configured_account_is_a_configuration_error_not_a_fetch() {
        let fake = Arc::new(FakeCommandRunner::new());
        let error = fetch(
            &as_runner(&fake),
            "/opt/homebrew/bin/gog",
            &MeetingsConfig::default(),
            now(),
        )
        .await
        .expect_err("not configured");

        assert!(matches!(error, MeetingsError::NotConfigured), "{error}");
        assert_eq!(fake.call_count(), 0);
    }

    #[tokio::test]
    async fn meetings_come_back_sorted_by_start() {
        let fake = Arc::new(FakeCommandRunner::new());
        fake.fallback(Reply::ok(EVENTS));

        let result = fetch(&as_runner(&fake), "/opt/homebrew/bin/gog", &config(), now())
            .await
            .expect("fetched");
        let starts: Vec<_> = result
            .meetings
            .iter()
            .map(|meeting| meeting.start)
            .collect();
        let mut sorted = starts.clone();
        sorted.sort();
        assert_eq!(starts, sorted);
    }
}
