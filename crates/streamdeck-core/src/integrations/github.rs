//! GitHub search and notification parsing.
//!
//! Preserves the previous behaviour exactly: a 30-day `updated` filter, open
//! items sorted by most recently updated, a 100-item limit, five authored-PR
//! tiles, and an inbox tile that says when the API result was capped.

use serde::{Deserialize, Serialize};

use super::{parse_json, ParseError};

const INTEGRATION: &str = "github";

/// The number of authored pull requests shown as tiles on the GitHub page.
pub const ITEM_TILES: usize = 5;
/// The `per_page=100` notifications request cannot report more than this.
pub const INBOX_CAP: u32 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MetricKind {
    Reviews,
    Prs,
    Assigned,
    Inbox,
}

impl MetricKind {
    pub const fn label(self) -> &'static str {
        match self {
            MetricKind::Reviews => "REVIEWS",
            MetricKind::Prs => "MY PRS",
            MetricKind::Assigned => "ISSUES",
            MetricKind::Inbox => "INBOX",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubItem {
    pub number: u64,
    pub repository_name: String,
    pub repository_name_with_owner: String,
    pub title: String,
    pub updated_at: String,
    pub url: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubSnapshot {
    pub reviews: Vec<GitHubItem>,
    pub prs: Vec<GitHubItem>,
    pub assigned: Vec<GitHubItem>,
    pub inbox_count: u32,
    pub inbox_overflow: bool,
    /// The `YYYY-MM-DD` lower bound of the `updated` filter, echoed into tile URLs.
    pub updated_since: String,
}

impl GitHubSnapshot {
    pub fn count(&self, kind: MetricKind) -> u32 {
        match kind {
            MetricKind::Reviews => self.reviews.len() as u32,
            MetricKind::Prs => self.prs.len() as u32,
            MetricKind::Assigned => self.assigned.len() as u32,
            MetricKind::Inbox => self.inbox_count,
        }
    }

    /// The label shown on a metric tile. The inbox reports `99+` when capped.
    pub fn count_label(&self, kind: MetricKind) -> String {
        if kind == MetricKind::Inbox && self.inbox_overflow {
            return "99+".to_string();
        }
        self.count(kind).to_string()
    }

    /// The GitHub filter a metric tile opens.
    pub fn url(&self, kind: MetricKind) -> String {
        let updated = urlencode(&format!(">={}", self.updated_since));
        match kind {
            MetricKind::Reviews => format!(
                "https://github.com/pulls?q=is%3Aopen+review-requested%3A%40me+updated%3A{updated}"
            ),
            MetricKind::Prs => {
                format!("https://github.com/pulls?q=is%3Aopen+author%3A%40me+updated%3A{updated}")
            }
            MetricKind::Assigned => format!(
                "https://github.com/issues?q=is%3Aopen+assignee%3A%40me+updated%3A{updated}"
            ),
            MetricKind::Inbox => "https://github.com/notifications".to_string(),
        }
    }

    /// The pull request behind item tile `index`, if any.
    pub fn item(&self, index: usize) -> Option<&GitHubItem> {
        self.prs.get(index)
    }
}

/// Parses `gh search prs|issues --json number,repository,title,url,updatedAt`.
///
/// Items are re-sorted newest-first here rather than trusting the CLI's order, and
/// truncated to `limit` so a changed default can never overflow the tiles.
pub fn parse_search(body: &str, limit: usize) -> Result<Vec<GitHubItem>, ParseError> {
    let value = parse_json(INTEGRATION, body)?;
    let array = value
        .as_array()
        .ok_or_else(|| ParseError::shape(INTEGRATION, "search result is not an array"))?;

    let mut items: Vec<GitHubItem> = array
        .iter()
        .filter_map(|entry| {
            let repository = entry.get("repository")?;
            let name_with_owner = repository
                .get("nameWithOwner")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let name = repository
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    name_with_owner
                        .rsplit_once('/')
                        .map(|(_, name)| name.to_string())
                })?;
            let url = entry.get("url")?.as_str()?.to_string();
            if !url.starts_with("https://github.com/") {
                return None;
            }
            Some(GitHubItem {
                number: entry.get("number")?.as_u64()?,
                repository_name: name,
                repository_name_with_owner: name_with_owner,
                title: crate::text::sanitize_single_line(
                    entry
                        .get("title")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default(),
                ),
                updated_at: entry
                    .get("updatedAt")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                url,
            })
        })
        .collect();

    items.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    items.truncate(limit);
    Ok(items)
}

/// Parses the length of `gh api notifications?per_page=100 --jq length`.
pub fn parse_notification_count(stdout: &str) -> Result<(u32, bool), ParseError> {
    let count: u32 = stdout.trim().parse().map_err(|_| {
        ParseError::shape(
            INTEGRATION,
            format!("notification count `{}` is not a number", stdout.trim()),
        )
    })?;
    Ok((count, count >= INBOX_CAP))
}

/// The `YYYY-MM-DD` lower bound for the `updated` filter.
pub fn updated_since(now: chrono::DateTime<chrono::Utc>, days: u32) -> String {
    (now - chrono::Duration::days(i64::from(days)))
        .format("%Y-%m-%d")
        .to_string()
}

fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = include_str!("../../../../tests/fixtures/github-search-prs.json");

    #[test]
    fn a_search_result_parses_into_items() {
        let items = parse_search(SEARCH, 100).expect("parsed");
        assert_eq!(items.len(), 6);

        let newest = &items[0];
        assert_eq!(newest.number, 4821);
        assert_eq!(newest.repository_name, "visma.administration.web");
        assert_eq!(newest.title, "Fix invoice rounding on partial credit notes");
        assert!(newest.url.starts_with("https://github.com/"));
    }

    #[test]
    fn items_are_sorted_newest_first_regardless_of_input_order() {
        let items = parse_search(SEARCH, 100).expect("parsed");
        let timestamps: Vec<&str> = items.iter().map(|item| item.updated_at.as_str()).collect();
        let mut sorted = timestamps.clone();
        sorted.sort_by(|left, right| right.cmp(left));
        assert_eq!(timestamps, sorted);
    }

    #[test]
    fn the_limit_truncates_to_the_configured_cap() {
        let items = parse_search(SEARCH, 3).expect("parsed");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].number, 4821, "the newest items survive truncation");
    }

    #[test]
    fn entries_missing_required_fields_or_with_foreign_urls_are_dropped() {
        let body = r#"[
            {"number": 1, "repository": {"name": "a", "nameWithOwner": "o/a"}, "title": "ok",
             "url": "https://github.com/o/a/pull/1", "updatedAt": "2026-07-01T00:00:00Z"},
            {"number": 2, "repository": {"name": "b", "nameWithOwner": "o/b"}, "title": "phish",
             "url": "https://evil.example/o/b/pull/2", "updatedAt": "2026-07-02T00:00:00Z"},
            {"repository": {"name": "c", "nameWithOwner": "o/c"}, "title": "no number",
             "url": "https://github.com/o/c/pull/3", "updatedAt": "2026-07-03T00:00:00Z"},
            {"number": 4, "title": "no repository", "url": "https://github.com/o/d/pull/4",
             "updatedAt": "2026-07-04T00:00:00Z"}
        ]"#;

        let items = parse_search(body, 100).expect("parsed");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].number, 1);
    }

    #[test]
    fn a_repository_name_can_be_recovered_from_name_with_owner() {
        let body = r#"[{"number": 9, "repository": {"nameWithOwner": "jimmystridh/streamdeckd"},
            "title": "t", "url": "https://github.com/jimmystridh/streamdeckd/pull/9",
            "updatedAt": "2026-07-01T00:00:00Z"}]"#;
        let items = parse_search(body, 10).expect("parsed");
        assert_eq!(items[0].repository_name, "streamdeckd");
    }

    #[test]
    fn item_titles_are_sanitized() {
        let body = r#"[{"number": 1, "repository": {"name": "a", "nameWithOwner": "o/a"},
            "title": "line\none\ttwo", "url": "https://github.com/o/a/pull/1",
            "updatedAt": "2026-07-01T00:00:00Z"}]"#;
        let items = parse_search(body, 10).expect("parsed");
        assert_eq!(items[0].title, "line one two");
    }

    #[test]
    fn a_non_array_search_response_is_rejected() {
        let error = parse_search(r#"{"message":"Bad credentials"}"#, 100).expect_err("rejected");
        assert!(matches!(error, ParseError::Shape { .. }), "{error}");
    }

    #[test]
    fn notification_counts_flag_the_hundred_item_cap() {
        assert_eq!(parse_notification_count("7\n").expect("parsed"), (7, false));
        assert_eq!(parse_notification_count("99").expect("parsed"), (99, false));
        assert_eq!(
            parse_notification_count("100").expect("parsed"),
            (100, true)
        );
        assert!(parse_notification_count("null").is_err());
    }

    #[test]
    fn metric_labels_and_counts_match_the_tiles() {
        let snapshot = GitHubSnapshot {
            reviews: vec![],
            prs: parse_search(SEARCH, 100).expect("parsed"),
            assigned: vec![],
            inbox_count: 100,
            inbox_overflow: true,
            updated_since: "2026-06-24".to_string(),
        };

        assert_eq!(snapshot.count(MetricKind::Reviews), 0);
        assert_eq!(snapshot.count_label(MetricKind::Reviews), "0");
        assert_eq!(snapshot.count(MetricKind::Prs), 6);
        assert_eq!(snapshot.count_label(MetricKind::Inbox), "99+");
        assert_eq!(MetricKind::Assigned.label(), "ISSUES");
    }

    #[test]
    fn metric_urls_carry_the_encoded_updated_filter() {
        let snapshot = GitHubSnapshot {
            updated_since: "2026-06-24".to_string(),
            ..Default::default()
        };

        assert_eq!(
            snapshot.url(MetricKind::Reviews),
            "https://github.com/pulls?q=is%3Aopen+review-requested%3A%40me+updated%3A%3E%3D2026-06-24"
        );
        assert_eq!(
            snapshot.url(MetricKind::Inbox),
            "https://github.com/notifications"
        );
    }

    #[test]
    fn item_tiles_read_the_five_most_recent_authored_pull_requests() {
        let snapshot = GitHubSnapshot {
            prs: parse_search(SEARCH, 100).expect("parsed"),
            ..Default::default()
        };
        for index in 0..ITEM_TILES {
            assert!(snapshot.item(index).is_some(), "tile {index} has an item");
        }
        assert_eq!(snapshot.item(0).expect("first").number, 4821);
    }

    #[test]
    fn the_updated_filter_looks_back_the_configured_number_of_days() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-07-24T12:00:00Z")
            .expect("timestamp")
            .with_timezone(&chrono::Utc);
        assert_eq!(updated_since(now, 30), "2026-06-24");
        assert_eq!(updated_since(now, 1), "2026-07-23");
    }
}
