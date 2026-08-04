//! GitHub Actions run state reduced to what the dashboard needs.

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::ParseError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiState {
    Running,
    Success,
    Failure,
    Cancelled,
    Neutral,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiRun {
    pub repository: String,
    pub workflow: String,
    pub title: String,
    pub state: CiState,
    pub updated_at: DateTime<Utc>,
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CiSnapshot {
    pub runs: Vec<CiRun>,
}

impl CiSnapshot {
    pub fn running(&self) -> usize {
        self.runs
            .iter()
            .filter(|run| run.state == CiState::Running)
            .count()
    }

    pub fn failures(&self) -> usize {
        self.runs
            .iter()
            .filter(|run| run.state == CiState::Failure)
            .count()
    }

    pub fn successes(&self) -> usize {
        self.runs
            .iter()
            .filter(|run| run.state == CiState::Success)
            .count()
    }

    pub fn actionable(&self) -> Option<&CiRun> {
        self.runs
            .iter()
            .find(|run| run.state == CiState::Failure)
            .or_else(|| self.runs.iter().find(|run| run.state == CiState::Running))
            .or_else(|| self.runs.first())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRun {
    name: String,
    display_title: String,
    status: String,
    conclusion: String,
    updated_at: DateTime<Utc>,
    url: String,
}

pub fn parse_runs(repository: &str, body: &str) -> Result<Vec<CiRun>, ParseError> {
    let raw: Vec<RawRun> = serde_json::from_str(body).map_err(|source| ParseError::Json {
        integration: "ci-radar",
        source,
    })?;
    let url_prefix = format!("https://github.com/{repository}/actions/runs/");

    raw.into_iter()
        .take(10)
        .map(|run| {
            if !run.url.starts_with(&url_prefix)
                || run.url[url_prefix.len()..]
                    .chars()
                    .any(|character| !character.is_ascii_digit())
            {
                return Err(ParseError::shape(
                    "ci-radar",
                    format!("unexpected run URL for {repository}"),
                ));
            }
            let state = if run.status != "completed" {
                CiState::Running
            } else {
                match run.conclusion.as_str() {
                    "success" => CiState::Success,
                    "failure" | "timed_out" | "action_required" => CiState::Failure,
                    "cancelled" | "skipped" => CiState::Cancelled,
                    _ => CiState::Neutral,
                }
            };
            Ok(CiRun {
                repository: repository.to_string(),
                workflow: clean(&run.name, 30),
                title: clean(&run.display_title, 100),
                state,
                updated_at: run.updated_at,
                url: run.url,
            })
        })
        .collect()
}

pub fn merge(runs: impl IntoIterator<Item = CiRun>) -> CiSnapshot {
    let mut runs: Vec<_> = runs.into_iter().collect();
    runs.sort_by_key(|run| std::cmp::Reverse(run.updated_at));
    CiSnapshot { runs }
}

fn clean(value: &str, limit: usize) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(limit)
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUNS: &str = r#"[
      {"name":"ci","displayTitle":"fix: dashboard","status":"completed","conclusion":"failure","updatedAt":"2026-07-31T12:00:00Z","url":"https://github.com/jimmystridh/streamdeckd/actions/runs/42"},
      {"name":"release","displayTitle":"release","status":"in_progress","conclusion":"","updatedAt":"2026-07-31T12:01:00Z","url":"https://github.com/jimmystridh/streamdeckd/actions/runs/43"}
    ]"#;

    #[test]
    fn runs_parse_and_preserve_failure_and_running_state() {
        let runs = parse_runs("jimmystridh/streamdeckd", RUNS).expect("runs");
        let snapshot = merge(runs);
        assert_eq!(snapshot.running(), 1);
        assert_eq!(snapshot.failures(), 1);
        assert_eq!(
            snapshot.actionable().expect("actionable").state,
            CiState::Failure
        );
    }

    #[test]
    fn a_foreign_run_url_is_refused() {
        let body = RUNS.replace("jimmystridh/streamdeckd", "attacker/repository");
        assert!(parse_runs("jimmystridh/streamdeckd", &body).is_err());
    }
}
