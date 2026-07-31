//! Latest GitHub Actions runs for a configured set of repositories.

use std::sync::Arc;

use streamdeck_core::config::CiConfig;
use streamdeck_core::integrations::ci::{self, CiSnapshot};
use streamdeck_macos::{timeouts, CommandRunner};

#[derive(Debug, thiserror::Error)]
pub enum CiError {
    #[error("every configured CI repository failed to refresh")]
    AllRepositoriesFailed,
}

#[derive(Debug)]
pub struct CiFetch {
    pub snapshot: CiSnapshot,
    pub failures: Vec<(String, String)>,
}

pub async fn fetch(
    runner: &Arc<dyn CommandRunner>,
    gh: &str,
    config: &CiConfig,
) -> Result<CiFetch, CiError> {
    let mut tasks = tokio::task::JoinSet::new();
    for repository in &config.repositories {
        let runner = Arc::clone(runner);
        let gh = gh.to_string();
        let repository = repository.clone();
        tasks.spawn(async move {
            let output = runner
                .run(
                    &gh,
                    &[
                        "run",
                        "list",
                        "--repo",
                        &repository,
                        "--limit",
                        "1",
                        "--json",
                        "name,displayTitle,status,conclusion,updatedAt,url",
                    ],
                    timeouts::GITHUB,
                )
                .await;
            (repository, output)
        });
    }

    let mut runs = Vec::new();
    let mut failures = Vec::new();
    let mut succeeded = 0usize;
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok((repository, Ok(output))) => match ci::parse_runs(&repository, &output.stdout) {
                Ok(parsed) => {
                    succeeded += 1;
                    runs.extend(parsed);
                }
                Err(error) => failures.push((repository, error.to_string())),
            },
            Ok((repository, Err(error))) => failures.push((repository, error.to_string())),
            Err(error) => failures.push(("worker".to_string(), error.to_string())),
        }
    }

    if succeeded == 0 && !config.repositories.is_empty() {
        return Err(CiError::AllRepositoriesFailed);
    }
    Ok(CiFetch {
        snapshot: ci::merge(runs),
        failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use streamdeck_macos::fake::{FakeCommandRunner, Reply};

    #[tokio::test]
    async fn repositories_are_queried_and_partial_failure_keeps_good_runs() {
        let fake = Arc::new(FakeCommandRunner::new());
        fake.on(
            "--repo owner/good",
            Reply::ok(r#"[{"name":"ci","displayTitle":"green","status":"completed","conclusion":"success","updatedAt":"2026-07-31T12:00:00Z","url":"https://github.com/owner/good/actions/runs/42"}]"#),
        )
        .on("--repo owner/bad", Reply::fails(1, "not found"));
        let config = CiConfig {
            repositories: vec!["owner/good".to_string(), "owner/bad".to_string()],
        };

        let fetched = fetch(
            &(fake as Arc<dyn CommandRunner>),
            "/opt/homebrew/bin/gh",
            &config,
        )
        .await
        .expect("partial success");

        assert_eq!(fetched.snapshot.successes(), 1);
        assert_eq!(fetched.failures.len(), 1);
    }
}
