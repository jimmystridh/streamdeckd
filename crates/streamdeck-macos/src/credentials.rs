//! Credential resolution.
//!
//! Tokens live only in memory and never reach configuration, state, or a log. The
//! wrapper below has no `Debug` or `Display` that exposes the secret, so an
//! accidental interpolation prints a placeholder instead of a bearer token.

use std::fmt;
use std::path::{Path, PathBuf};

use streamdeck_core::integrations::codex::{parse_auth_file, CodexError};

/// The Keychain entry Claude Code writes.
pub const CLAUDE_KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
/// The credential file Claude Code uses when the Keychain is unavailable.
pub const CLAUDE_CREDENTIAL_FILE: &str = "~/.claude/.credentials.json";
/// The Codex credential file.
pub const CODEX_AUTH_FILE: &str = "~/.codex/auth.json";

/// A bearer token that cannot be printed by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The only way to read the secret. Named so a review notices it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("no Claude Code credential found in the Keychain or at {0}")]
    ClaudeMissing(String),
    #[error("the Claude Code credential has expired; run `claude` to sign in again")]
    ClaudeExpired,
    #[error("the Claude Code credential could not be read: {0}")]
    ClaudeUnreadable(String),
    #[error(
        "reading the Claude Code Keychain entry timed out; macOS is probably asking \
         for authorization. Grant streamdeckd access once, or codesign the binary."
    )]
    ClaudeKeychainBlocked,
    #[error("no Codex credential found at {0}")]
    CodexMissing(String),
    #[error(transparent)]
    Codex(#[from] CodexError),
}

/// A Claude Code OAuth credential, with only what the usage tile needs.
#[derive(Debug, Clone)]
pub struct ClaudeCredential {
    pub access_token: Secret,
    /// Milliseconds since the epoch, or `None` when the payload omits it.
    pub expires_at_ms: Option<i64>,
}

impl ClaudeCredential {
    pub fn is_expired(&self, now_ms: i64) -> bool {
        self.expires_at_ms
            .is_some_and(|expires_at| expires_at <= now_ms)
    }
}

/// Reads the OAuth section out of a Claude Code credential payload.
pub fn parse_claude_credential(contents: &str) -> Result<ClaudeCredential, CredentialError> {
    let value: serde_json::Value = serde_json::from_str(contents)
        .map_err(|error| CredentialError::ClaudeUnreadable(error.to_string()))?;
    let oauth = value.get("claudeAiOauth").ok_or_else(|| {
        CredentialError::ClaudeUnreadable("payload has no `claudeAiOauth` section".to_string())
    })?;
    let access_token = oauth
        .get("accessToken")
        .and_then(serde_json::Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            CredentialError::ClaudeUnreadable("payload has no access token".to_string())
        })?;

    Ok(ClaudeCredential {
        access_token: Secret::new(access_token),
        expires_at_ms: oauth
            .get("expiresAt")
            .and_then(serde_json::Value::as_f64)
            .map(|value| value as i64),
    })
}

/// Reads the Claude Code credential: Keychain first, then the supported file.
///
/// Blocking. The Keychain read can wait on an authorization prompt, so callers run
/// this off the async runtime and under a timeout.
pub fn claude_credential(now_ms: i64) -> Result<ClaudeCredential, CredentialError> {
    let file = crate::expand_home(CLAUDE_CREDENTIAL_FILE);
    let contents = read_keychain(CLAUDE_KEYCHAIN_SERVICE)
        .or_else(|| std::fs::read_to_string(&file).ok())
        .ok_or_else(|| CredentialError::ClaudeMissing(file.clone()))?;
    finish_claude(&contents, now_ms)
}

/// Validates and returns a credential from an already-read payload.
///
/// Used by both the Keychain path and the `security` fallback so expiry handling
/// and error reporting are identical.
pub fn finish_claude(contents: &str, now_ms: i64) -> Result<ClaudeCredential, CredentialError> {
    let credential = parse_claude_credential(contents)?;
    if credential.is_expired(now_ms) {
        return Err(CredentialError::ClaudeExpired);
    }
    Ok(credential)
}

/// The arguments that read the Claude Code entry through the `security` CLI.
///
/// A fallback for when the framework read is blocked on an authorization prompt:
/// `security` is a system tool the user has typically already granted access to,
/// so this recovers the tile without another dialog. Returns the raw JSON on stdout.
pub fn security_cli_arguments() -> [&'static str; 4] {
    ["find-generic-password", "-s", CLAUDE_KEYCHAIN_SERVICE, "-w"]
}

/// Reads the Codex credential from the configured override or its default path.
pub fn codex_credential(
    override_path: Option<&str>,
) -> Result<(Secret, Option<String>), CredentialError> {
    let path = PathBuf::from(crate::expand_home(
        override_path
            .filter(|path| !path.trim().is_empty())
            .unwrap_or(CODEX_AUTH_FILE),
    ));
    let contents = std::fs::read_to_string(&path)
        .map_err(|_| CredentialError::CodexMissing(path.display().to_string()))?;
    let (token, account) = parse_auth_file(&contents)?;
    Ok((Secret::new(token), account))
}

/// Reports whether a credential exists, without reading or displaying it.
///
/// Deliberately does not load the item's data: reading a Keychain secret can put
/// up an authorization prompt, and a health check must never block on one.
pub fn claude_credential_present() -> bool {
    keychain_item_exists(CLAUDE_KEYCHAIN_SERVICE)
        || Path::new(&crate::expand_home(CLAUDE_CREDENTIAL_FILE)).exists()
}

pub fn codex_credential_present(override_path: Option<&str>) -> bool {
    Path::new(&crate::expand_home(
        override_path
            .filter(|path| !path.trim().is_empty())
            .unwrap_or(CODEX_AUTH_FILE),
    ))
    .exists()
}

/// Reads a generic password by service name.
///
/// Blocking, and potentially *very* blocking: macOS puts up an authorization
/// prompt when the calling binary is not on the item's access-control list, which
/// an unsigned build never is. Callers must run this off the async runtime and
/// under a timeout.
#[cfg(target_os = "macos")]
fn read_keychain(service: &str) -> Option<String> {
    use security_framework::item::{ItemClass, ItemSearchOptions, SearchResult};

    // Search by service only. Claude Code stores the entry under the macOS short
    // username, which this daemon must not assume or hard-code.
    let results = ItemSearchOptions::new()
        .class(ItemClass::generic_password())
        .service(service)
        .load_data(true)
        .search()
        .ok()?;

    results.into_iter().find_map(|result| match result {
        SearchResult::Data(bytes) => String::from_utf8(bytes).ok(),
        _ => None,
    })
}

#[cfg(not(target_os = "macos"))]
fn read_keychain(_service: &str) -> Option<String> {
    None
}

/// Whether a Keychain item exists.
///
/// Asks only for attributes, never the data. Requesting the secret is what puts up
/// the authorization prompt, so a presence check that avoids it stays fast even for
/// a binary that has not been granted access.
#[cfg(target_os = "macos")]
pub fn keychain_item_exists(service: &str) -> bool {
    use security_framework::item::{ItemClass, ItemSearchOptions};

    ItemSearchOptions::new()
        .class(ItemClass::generic_password())
        .service(service)
        .load_attributes(true)
        .load_data(false)
        .search()
        .is_ok_and(|results| !results.is_empty())
}

#[cfg(not(target_os = "macos"))]
pub fn keychain_item_exists(_service: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE: &str = r#"{
        "mcpOAuth": {},
        "claudeAiOauth": {
            "accessToken": "sk-ant-oat-super-secret",
            "refreshToken": "sk-ant-ort-also-secret",
            "expiresAt": 1784941859948,
            "scopes": ["user:inference"],
            "subscriptionType": "team"
        }
    }"#;

    #[cfg(target_os = "macos")]
    #[test]
    fn presence_can_be_checked_without_loading_the_secret() {
        // This must not prompt and must not block, so it is safe in `doctor`.
        let _ = keychain_item_exists(CLAUDE_KEYCHAIN_SERVICE);
        assert!(!keychain_item_exists(
            "streamdeckd-service-that-does-not-exist"
        ));
    }

    #[test]
    fn a_secret_never_prints_itself() {
        let secret = Secret::new("sk-ant-oat-super-secret");
        assert_eq!(format!("{secret}"), "<redacted>");
        assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
        assert!(!format!("{secret:?}").contains("super-secret"));
        assert_eq!(secret.expose(), "sk-ant-oat-super-secret");
    }

    #[test]
    fn the_claude_credential_payload_yields_a_token_and_an_expiry() {
        let credential = parse_claude_credential(CLAUDE).expect("parsed");
        assert_eq!(credential.access_token.expose(), "sk-ant-oat-super-secret");
        assert_eq!(credential.expires_at_ms, Some(1784941859948));
    }

    #[test]
    fn the_security_cli_arguments_ask_only_for_this_one_item() {
        let arguments = security_cli_arguments();
        assert_eq!(arguments[0], "find-generic-password");
        assert_eq!(arguments[2], CLAUDE_KEYCHAIN_SERVICE);
        assert!(
            arguments.contains(&"-w"),
            "the password itself is requested"
        );
        assert!(
            !arguments.iter().any(|argument| argument.contains("..")),
            "no path traversal is possible"
        );
    }

    #[test]
    fn finishing_from_a_payload_applies_the_same_expiry_rule() {
        let fresh = finish_claude(CLAUDE, 1_784_941_859_947).expect("not expired");
        assert_eq!(fresh.access_token.expose(), "sk-ant-oat-super-secret");

        let error = finish_claude(CLAUDE, 1_784_941_859_948).expect_err("expired");
        assert!(matches!(error, CredentialError::ClaudeExpired), "{error}");
    }

    #[test]
    fn expiry_is_evaluated_against_the_supplied_clock() {
        let credential = parse_claude_credential(CLAUDE).expect("parsed");
        assert!(!credential.is_expired(1_784_941_859_947));
        assert!(credential.is_expired(1_784_941_859_948));
        assert!(credential.is_expired(1_784_941_859_949));
    }

    #[test]
    fn a_credential_without_an_expiry_is_never_treated_as_expired() {
        let credential =
            parse_claude_credential(r#"{"claudeAiOauth":{"accessToken":"t"}}"#).expect("parsed");
        assert_eq!(credential.expires_at_ms, None);
        assert!(!credential.is_expired(i64::MAX));
    }

    #[test]
    fn malformed_claude_payloads_are_rejected_without_leaking_anything() {
        for contents in [
            "{not json",
            r#"{"somethingElse":{}}"#,
            r#"{"claudeAiOauth":{}}"#,
            r#"{"claudeAiOauth":{"accessToken":""}}"#,
        ] {
            let error = parse_claude_credential(contents).expect_err("rejected");
            assert!(
                !error.to_string().contains("accessToken\":\""),
                "{contents}"
            );
        }
    }

    #[test]
    fn error_messages_name_the_path_but_never_the_token() {
        let error =
            CredentialError::ClaudeMissing("/Users/tester/.claude/.credentials.json".into());
        assert!(error.to_string().contains(".credentials.json"));

        let error = CredentialError::ClaudeUnreadable("payload has no access token".into());
        assert!(!error.to_string().contains("sk-ant"));
    }

    #[test]
    fn the_codex_credential_reads_from_an_explicit_override() {
        let directory =
            std::env::temp_dir().join(format!("streamdeckd-codex-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("auth.json");
        std::fs::write(
            &path,
            r#"{"tokens":{"access_token":"codex-secret","account_id":"user-abc"}}"#,
        )
        .expect("write");

        let (token, account) = codex_credential(Some(&path.to_string_lossy())).expect("read");
        assert_eq!(token.expose(), "codex-secret");
        assert_eq!(account.as_deref(), Some("user-abc"));
        assert!(codex_credential_present(Some(&path.to_string_lossy())));

        std::fs::remove_dir_all(&directory).expect("cleanup");
    }

    #[test]
    fn a_missing_codex_credential_names_the_path_it_looked_at() {
        let error =
            codex_credential(Some("/nonexistent/streamdeckd/auth.json")).expect_err("missing");
        assert!(
            error
                .to_string()
                .contains("/nonexistent/streamdeckd/auth.json"),
            "{error}"
        );
        assert!(!codex_credential_present(Some("/nonexistent/x.json")));
    }

    #[test]
    fn an_empty_override_falls_back_to_the_default_path() {
        std::env::set_var("HOME", "/nonexistent-home");
        let error = codex_credential(Some("   ")).expect_err("missing");
        assert!(error.to_string().contains(".codex/auth.json"), "{error}");
    }
}
