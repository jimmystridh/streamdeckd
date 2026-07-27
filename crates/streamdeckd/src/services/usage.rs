//! Claude and Codex usage.
//!
//! Both read an existing credential the user already has, hold the token only for
//! the duration of one request, and never log any part of it.

use streamdeck_core::integrations::claude::{self, ClaudeUsage, BETA_HEADER};
use streamdeck_core::integrations::codex::{self, CodexUsage};
use streamdeck_macos::credentials::{self, CredentialError};

use super::http::{HttpClient, HttpError};
use super::timeouts;

#[derive(Debug, thiserror::Error)]
pub enum UsageError {
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error(transparent)]
    Credential(#[from] CredentialError),
    #[error(transparent)]
    Parse(#[from] streamdeck_core::integrations::ParseError),
    #[error(transparent)]
    Codex(#[from] codex::CodexError),
}

pub async fn fetch_claude(client: &HttpClient, now_ms: i64) -> Result<ClaudeUsage, UsageError> {
    let credential = tokio::task::spawn_blocking(move || credentials::claude_credential(now_ms))
        .await
        .map_err(|error| {
            UsageError::Credential(CredentialError::ClaudeUnreadable(error.to_string()))
        })??;
    let authorization = format!("Bearer {}", credential.access_token.expose());

    let response = client
        .get(
            claude::ENDPOINT,
            &[
                ("Accept", "application/json"),
                ("Authorization", &authorization),
                ("anthropic-beta", BETA_HEADER),
            ],
            timeouts::USAGE,
        )
        .await?;
    Ok(claude::parse_usage(&response.body)?)
}

pub async fn fetch_codex(
    client: &HttpClient,
    auth_path: Option<&str>,
) -> Result<CodexUsage, UsageError> {
    let path = auth_path.map(str::to_string);
    let (token, account) =
        tokio::task::spawn_blocking(move || credentials::codex_credential(path.as_deref()))
            .await
            .map_err(|error| {
                UsageError::Credential(CredentialError::ClaudeUnreadable(error.to_string()))
            })??;
    let authorization = format!("Bearer {}", token.expose());

    let mut headers = vec![
        ("Accept", "application/json"),
        ("Authorization", authorization.as_str()),
        ("User-Agent", "streamdeckd/0.1"),
    ];
    if let Some(account) = account.as_deref() {
        headers.push(("chatgpt-account-id", account));
    }

    let response = match client.get(codex::ENDPOINT, &headers, timeouts::USAGE).await {
        Ok(response) => response,
        // A rejected token is an expired login, not a broken endpoint.
        Err(HttpError::Unauthorized { .. }) => {
            return Err(UsageError::Codex(codex::CodexError::Unauthorized))
        }
        Err(error) => return Err(error.into()),
    };
    Ok(codex::parse_usage(&response.body)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_endpoints_and_beta_header_match_the_plan() {
        assert_eq!(
            claude::ENDPOINT,
            "https://api.anthropic.com/api/oauth/usage"
        );
        assert_eq!(BETA_HEADER, "oauth-2025-04-20");
        assert_eq!(
            codex::ENDPOINT,
            "https://chatgpt.com/backend-api/wham/usage"
        );
    }

    #[tokio::test]
    async fn a_missing_codex_credential_names_the_path() {
        let client = HttpClient::new().expect("client");
        let error = fetch_codex(&client, Some("/nonexistent/codex-auth.json"))
            .await
            .expect_err("no credential");
        assert!(
            error.to_string().contains("/nonexistent/codex-auth.json"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn no_error_message_ever_contains_a_bearer_token() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("auth.json");
        std::fs::write(
            &path,
            r#"{"tokens":{"access_token":"unmistakable-secret-value"}}"#,
        )
        .expect("write");

        let client = HttpClient::new().expect("client");
        // The endpoint is unreachable in tests; what matters is the error text.
        if let Err(error) = fetch_codex(&client, Some(&path.to_string_lossy())).await {
            assert!(
                !error.to_string().contains("unmistakable-secret-value"),
                "{error}"
            );
        }
    }
}
