//! Claude and Codex usage.
//!
//! Both read an existing credential the user already has, hold the token only for
//! the duration of one request, and never log any part of it.

use std::sync::Arc;

use streamdeck_core::integrations::claude::{self, ClaudeUsage, BETA_HEADER};
use streamdeck_core::integrations::codex::{self, CodexUsage};
use streamdeck_macos::credentials::{self, ClaudeCredential, CredentialError};
use streamdeck_macos::{timeouts as tool_timeouts, CommandRunner};

use super::http::{HttpClient, HttpError};
use super::timeouts;

/// How long to wait for the Keychain. A blocked read means macOS is showing an
/// authorization prompt; the tile reports that rather than stalling forever.
const KEYCHAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

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

pub async fn fetch_claude(
    client: &HttpClient,
    runner: &Arc<dyn CommandRunner>,
    security: &str,
    now_ms: i64,
) -> Result<ClaudeUsage, UsageError> {
    let credential = claude_credential(runner, security, now_ms).await?;
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

/// Resolves the Claude credential, preferring the Security framework and falling
/// back to the `security` CLI.
///
/// The framework read is blocking and can wait indefinitely on an authorization
/// prompt, which an unsigned or newly-signed binary always triggers. It therefore
/// runs on a blocking thread under a timeout; when that expires, `security` — a
/// system tool the user has usually already granted access to — is asked instead.
async fn claude_credential(
    runner: &Arc<dyn CommandRunner>,
    security: &str,
    now_ms: i64,
) -> Result<ClaudeCredential, UsageError> {
    resolve_claude_credential(credentials::claude_credential, runner, security, now_ms).await
}

/// How the Claude payload is read from the Keychain.
///
/// Injected because the machine's Keychain contents and its per-binary access
/// grants are outside this daemon's control. A test that reached the real Keychain
/// would pass or fail depending on the developer's local grants — and did.
type KeychainReader = fn(i64) -> Result<ClaudeCredential, CredentialError>;

async fn resolve_claude_credential(
    read_keychain: KeychainReader,
    runner: &Arc<dyn CommandRunner>,
    security: &str,
    now_ms: i64,
) -> Result<ClaudeCredential, UsageError> {
    let framework = tokio::time::timeout(
        KEYCHAIN_TIMEOUT,
        tokio::task::spawn_blocking(move || read_keychain(now_ms)),
    )
    .await;

    match framework {
        Ok(Ok(Ok(credential))) => return Ok(credential),
        // An expired token is a real answer: falling back cannot improve on it.
        Ok(Ok(Err(CredentialError::ClaudeExpired))) => {
            return Err(UsageError::Credential(CredentialError::ClaudeExpired))
        }
        Ok(Ok(Err(error))) => {
            tracing::debug!(
                component = "claude-usage",
                error = %error,
                "the framework read failed; trying the security CLI"
            );
        }
        Ok(Err(error)) => {
            tracing::warn!(
                component = "claude-usage",
                error = %error,
                "the framework read panicked; trying the security CLI"
            );
        }
        Err(_) => tracing::info!(
            component = "claude-usage",
            "the Keychain did not answer; trying the security CLI"
        ),
    }

    let output = runner
        .run(
            security,
            &credentials::security_cli_arguments(),
            tool_timeouts::LOCAL,
        )
        .await
        .map_err(|_| UsageError::Credential(CredentialError::ClaudeKeychainBlocked))?;

    Ok(credentials::finish_claude(output.trimmed(), now_ms)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use streamdeck_macos::fake::{FakeCommandRunner, Reply};

    const CLAUDE_PAYLOAD: &str =
        r#"{"claudeAiOauth":{"accessToken":"tok","expiresAt":4102444800000}}"#;

    fn runner(reply: Reply) -> Arc<dyn CommandRunner> {
        let fake = Arc::new(FakeCommandRunner::new());
        fake.fallback(reply);
        fake as Arc<dyn CommandRunner>
    }

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

    /// Stands in for a Keychain with no Claude entry.
    fn no_keychain_entry(_now_ms: i64) -> Result<ClaudeCredential, CredentialError> {
        Err(CredentialError::ClaudeMissing(
            "/nowhere/.credentials.json".to_string(),
        ))
    }

    #[tokio::test]
    async fn a_missing_claude_credential_is_reported_rather_than_hanging() {
        // Neither the Keychain nor the CLI can produce one.
        let error = resolve_claude_credential(
            no_keychain_entry,
            &runner(Reply::fails(44, "SecKeychainSearchCopyNext: not found")),
            "/usr/bin/security",
            0,
        )
        .await
        .expect_err("no credential");

        assert!(
            matches!(
                error,
                UsageError::Credential(CredentialError::ClaudeKeychainBlocked)
            ),
            "{error}"
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
    async fn the_security_cli_recovers_the_credential_when_the_framework_is_blocked() {
        // Point HOME at an empty directory so the file fallback finds nothing and
        // the framework path fails fast.
        let directory = tempfile::tempdir().expect("temp dir");
        std::env::set_var("HOME", directory.path());

        let client = HttpClient::new().expect("client");
        let runner = runner(Reply::ok(CLAUDE_PAYLOAD));
        let credential = claude_credential(&runner, "/usr/bin/security", 0)
            .await
            .expect("recovered through the CLI");
        assert_eq!(credential.access_token.expose(), "tok");
        drop(client);
    }

    #[tokio::test]
    async fn an_expired_token_is_reported_rather_than_retried_through_the_cli() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join(".claude");
        std::fs::create_dir_all(&path).expect("dir");
        std::fs::write(path.join(".credentials.json"), CLAUDE_PAYLOAD).expect("write");
        std::env::set_var("HOME", directory.path());

        // The payload expires in 2100, so ask from even later.
        let runner = runner(Reply::fails(1, "should not be called"));
        let error = claude_credential(&runner, "/usr/bin/security", 4_102_444_801_000)
            .await
            .expect_err("expired");
        assert!(
            matches!(
                error,
                UsageError::Credential(CredentialError::ClaudeExpired)
            ),
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
