//! GitHub through the authenticated `gh` CLI.
//!
//! Using the CLI keeps authentication out of this process entirely and costs
//! nothing while idle. The four queries run concurrently under separate timeouts.

use std::sync::Arc;

use chrono::Utc;
use streamdeck_core::config::GitHubConfig;
use streamdeck_core::integrations::github::{self, GitHubSnapshot};
use streamdeck_macos::{timeouts, CommandRunner};

#[derive(Debug, thiserror::Error)]
pub enum GitHubError {
    #[error("`gh` failed: {0}")]
    Command(#[from] streamdeck_macos::CommandError),
    #[error(transparent)]
    Parse(#[from] streamdeck_core::integrations::ParseError),
    #[error("`gh` is not authenticated; run `gh auth login`")]
    NotAuthenticated,
}

/// Recognises the CLI's own authentication failure so the tile can say what to fix.
fn is_auth_failure(error: &streamdeck_macos::CommandError) -> bool {
    let text = error.to_string();
    text.contains("gh auth login")
        || text.contains("authentication")
        || text.contains("Bad credentials")
        || text.contains("HTTP 401")
}

pub async fn fetch(
    runner: &Arc<dyn CommandRunner>,
    gh: &str,
    config: &GitHubConfig,
) -> Result<GitHubSnapshot, GitHubError> {
    let updated_since = github::updated_since(Utc::now(), config.updated_within_days);
    let updated_filter = format!(">={updated_since}");
    let limit = config.item_limit.to_string();

    let search = |kind: &'static str, filter: [&'static str; 2]| {
        let runner = Arc::clone(runner);
        let gh = gh.to_string();
        let updated_filter = updated_filter.clone();
        let limit = limit.clone();
        async move {
            runner
                .run(
                    &gh,
                    &[
                        "search",
                        kind,
                        filter[0],
                        filter[1],
                        "--state",
                        "open",
                        "--updated",
                        &updated_filter,
                        "--sort",
                        "updated",
                        "--order",
                        "desc",
                        "--limit",
                        &limit,
                        "--json",
                        "number,repository,title,url,updatedAt",
                    ],
                    timeouts::GITHUB,
                )
                .await
        }
    };

    let notifications = {
        let runner = Arc::clone(runner);
        let gh = gh.to_string();
        async move {
            runner
                .run(
                    &gh,
                    &["api", "notifications?per_page=100", "--jq", "length"],
                    timeouts::GITHUB,
                )
                .await
        }
    };

    let (reviews, prs, assigned, inbox) = tokio::join!(
        search("prs", ["--review-requested", "@me"]),
        search("prs", ["--author", "@me"]),
        search("issues", ["--assignee", "@me"]),
        notifications,
    );

    let unwrap =
        |result: Result<streamdeck_macos::Output, streamdeck_macos::CommandError>| match result {
            Ok(output) => Ok(output),
            Err(error) if is_auth_failure(&error) => Err(GitHubError::NotAuthenticated),
            Err(error) => Err(GitHubError::Command(error)),
        };

    let limit = config.item_limit as usize;
    let (inbox_count, inbox_overflow) = github::parse_notification_count(unwrap(inbox)?.trimmed())?;

    Ok(GitHubSnapshot {
        reviews: github::parse_search(&unwrap(reviews)?.stdout, limit)?,
        prs: github::parse_search(&unwrap(prs)?.stdout, limit)?,
        assigned: github::parse_search(&unwrap(assigned)?.stdout, limit)?,
        inbox_count,
        inbox_overflow,
        updated_since,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use streamdeck_macos::fake::{FakeCommandRunner, Reply};

    const SEARCH: &str = include_str!("../../../../tests/fixtures/github-search-prs.json");

    fn runner() -> Arc<FakeCommandRunner> {
        let runner = Arc::new(FakeCommandRunner::new());
        runner
            .on("--review-requested @me", Reply::ok(SEARCH))
            .on("--author @me", Reply::ok(SEARCH))
            .on("--assignee @me", Reply::ok("[]"))
            .on("api notifications", Reply::ok("17\n"));
        runner
    }

    fn as_runner(runner: &Arc<FakeCommandRunner>) -> Arc<dyn CommandRunner> {
        Arc::clone(runner) as Arc<dyn CommandRunner>
    }

    #[tokio::test]
    async fn all_four_queries_run_and_populate_the_snapshot() {
        let fake = runner();
        let snapshot = fetch(
            &as_runner(&fake),
            "/opt/homebrew/bin/gh",
            &GitHubConfig::default(),
        )
        .await
        .expect("fetched");

        assert_eq!(snapshot.reviews.len(), 6);
        assert_eq!(snapshot.prs.len(), 6);
        assert!(snapshot.assigned.is_empty());
        assert_eq!(snapshot.inbox_count, 17);
        assert!(!snapshot.inbox_overflow);
        assert_eq!(fake.call_count(), 4);
    }

    #[tokio::test]
    async fn the_queries_carry_the_documented_filters_and_limit() {
        let fake = runner();
        let config = GitHubConfig {
            item_limit: 100,
            updated_within_days: 30,
            ..GitHubConfig::default()
        };

        fetch(&as_runner(&fake), "/opt/homebrew/bin/gh", &config)
            .await
            .expect("fetched");

        assert!(fake.called_with("search prs --review-requested @me --state open"));
        assert!(fake.called_with("search prs --author @me --state open"));
        assert!(fake.called_with("search issues --assignee @me --state open"));
        assert!(fake.called_with("--sort updated --order desc --limit 100"));
        assert!(fake.called_with("--json number,repository,title,url,updatedAt"));
        assert!(fake.called_with("api notifications?per_page=100 --jq length"));

        let expected = github::updated_since(Utc::now(), 30);
        assert!(fake.called_with(&format!("--updated >={expected}")));
        assert_eq!(snapshot_updated_since(&fake), expected);
    }

    fn snapshot_updated_since(fake: &Arc<FakeCommandRunner>) -> String {
        fake.calls()
            .into_iter()
            .find_map(|call| {
                let index = call.args.iter().position(|arg| arg == "--updated")?;
                Some(
                    call.args
                        .get(index + 1)?
                        .trim_start_matches(">=")
                        .to_string(),
                )
            })
            .expect("an updated filter was sent")
    }

    #[tokio::test]
    async fn a_configured_limit_truncates_each_result_list() {
        let fake = runner();
        let config = GitHubConfig {
            item_limit: 2,
            ..GitHubConfig::default()
        };

        let snapshot = fetch(&as_runner(&fake), "/opt/homebrew/bin/gh", &config)
            .await
            .expect("fetched");
        assert_eq!(snapshot.prs.len(), 2);
        assert!(fake.called_with("--limit 2"));
    }

    #[tokio::test]
    async fn a_capped_inbox_is_flagged() {
        let fake = runner();
        fake.on("api notifications", Reply::ok("100"));

        let snapshot = fetch(
            &as_runner(&fake),
            "/opt/homebrew/bin/gh",
            &GitHubConfig::default(),
        )
        .await
        .expect("fetched");
        assert_eq!(snapshot.inbox_count, 100);
        assert!(snapshot.inbox_overflow);
    }

    #[tokio::test]
    async fn an_unauthenticated_cli_is_its_own_diagnostic() {
        let fake = runner();
        fake.on(
            "--review-requested @me",
            Reply::fails(
                4,
                "gh: To get started with GitHub CLI, please run: gh auth login",
            ),
        );

        let error = fetch(
            &as_runner(&fake),
            "/opt/homebrew/bin/gh",
            &GitHubConfig::default(),
        )
        .await
        .expect_err("not authenticated");
        assert!(matches!(error, GitHubError::NotAuthenticated), "{error}");
    }

    #[tokio::test]
    async fn a_timeout_on_one_query_fails_the_whole_refresh_rather_than_reporting_zero() {
        let fake = runner();
        fake.on("--author @me", Reply::Timeout);

        let error = fetch(
            &as_runner(&fake),
            "/opt/homebrew/bin/gh",
            &GitHubConfig::default(),
        )
        .await
        .expect_err("timed out");
        assert!(matches!(error, GitHubError::Command(_)), "{error}");
    }

    #[tokio::test]
    async fn malformed_json_is_a_parse_error() {
        let fake = runner();
        fake.on("--author @me", Reply::ok("not json"));

        let error = fetch(
            &as_runner(&fake),
            "/opt/homebrew/bin/gh",
            &GitHubConfig::default(),
        )
        .await
        .expect_err("malformed");
        assert!(matches!(error, GitHubError::Parse(_)), "{error}");
    }

    #[test]
    fn authentication_failures_are_recognised_from_several_wordings() {
        let failure = |stderr: &str| streamdeck_macos::CommandError::Failed {
            program: "/opt/homebrew/bin/gh".to_string(),
            code: 4,
            stderr: stderr.to_string(),
        };
        assert!(is_auth_failure(&failure("please run: gh auth login")));
        assert!(is_auth_failure(&failure("HTTP 401: Bad credentials")));
        assert!(!is_auth_failure(&failure("connection reset by peer")));
    }
}
