//! Credential resolution.
//!
//! Tokens live only in memory and never reach configuration, state, or a log. The
//! wrapper below has no `Debug` or `Display` that exposes the secret, so an
//! accidental interpolation prints a placeholder instead of a bearer token.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use streamdeck_core::integrations::codex::{parse_auth_file, CodexError};

/// The Keychain entry Claude Code writes.
pub const CLAUDE_KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
/// The credential file Claude Code uses when the Keychain is unavailable.
pub const CLAUDE_CREDENTIAL_FILE: &str = "~/.claude/.credentials.json";
/// The Codex credential file.
pub const CODEX_AUTH_FILE: &str = "~/.codex/auth.json";
const CLAUDE_MEMORY_CACHE_VALIDITY_MS: i64 = 30 * 60 * 1_000;
const CLAUDE_KEYCHAIN_RETRY_COOLDOWN_MS: i64 = 6 * 60 * 60 * 1_000;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CredentialFileStamp {
    modified: Option<SystemTime>,
    length: u64,
}

#[derive(Default)]
struct ClaudeCredentialState {
    cached: Option<(ClaudeCredential, i64)>,
    credential_file_stamp: Option<Option<CredentialFileStamp>>,
    keychain_retry_at_ms: Option<i64>,
}

static CLAUDE_CREDENTIAL_STATE: OnceLock<Mutex<ClaudeCredentialState>> = OnceLock::new();

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

/// Reads the Claude Code credential without ever allowing background Keychain UI.
///
/// A fresh in-memory credential is reused for 30 minutes. Otherwise the supported
/// credential file is preferred, followed by a Security.framework query that
/// explicitly skips anything requiring authentication. An inaccessible Keychain
/// item is not retried for six hours.
pub fn claude_credential(now_ms: i64) -> Result<ClaudeCredential, CredentialError> {
    let file = crate::expand_home(CLAUDE_CREDENTIAL_FILE);
    let state =
        CLAUDE_CREDENTIAL_STATE.get_or_init(|| Mutex::new(ClaudeCredentialState::default()));
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .resolve(
            now_ms,
            Path::new(&file),
            || read_keychain_without_ui(CLAUDE_KEYCHAIN_SERVICE),
            || keychain_item_exists(CLAUDE_KEYCHAIN_SERVICE),
        )
}

/// Validates and returns a credential from an already-read payload.
pub fn finish_claude(contents: &str, now_ms: i64) -> Result<ClaudeCredential, CredentialError> {
    let credential = parse_claude_credential(contents)?;
    if credential.is_expired(now_ms) {
        return Err(CredentialError::ClaudeExpired);
    }
    Ok(credential)
}

impl ClaudeCredentialState {
    fn resolve(
        &mut self,
        now_ms: i64,
        file: &Path,
        read_keychain: impl FnOnce() -> Option<String>,
        keychain_item_exists: impl FnOnce() -> bool,
    ) -> Result<ClaudeCredential, CredentialError> {
        let file_stamp = credential_file_stamp(file);
        if self.credential_file_stamp != Some(file_stamp) {
            self.cached = None;
            self.credential_file_stamp = Some(file_stamp);
        }

        if let Some((credential, loaded_at_ms)) = &self.cached {
            let cache_age_ms = now_ms.checked_sub(*loaded_at_ms);
            if cache_age_ms.is_some_and(|age| {
                (0..CLAUDE_MEMORY_CACHE_VALIDITY_MS).contains(&age)
                    && !credential.is_expired(now_ms)
            }) {
                return Ok(credential.clone());
            }
            self.cached = None;
        }

        let mut last_error = match std::fs::read_to_string(file) {
            Ok(contents) => match finish_claude(&contents, now_ms) {
                Ok(credential) => return Ok(self.cache(credential, now_ms)),
                Err(error) => Some(error),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => Some(CredentialError::ClaudeUnreadable(error.to_string())),
        };

        if self
            .keychain_retry_at_ms
            .is_some_and(|retry_at| now_ms < retry_at)
        {
            return Err(last_error
                .unwrap_or_else(|| CredentialError::ClaudeMissing(file.display().to_string())));
        }

        if let Some(contents) = read_keychain() {
            self.keychain_retry_at_ms = None;
            match finish_claude(&contents, now_ms) {
                Ok(credential) => return Ok(self.cache(credential, now_ms)),
                Err(error) => last_error = Some(error),
            }
        } else if keychain_item_exists() {
            self.keychain_retry_at_ms =
                Some(now_ms.saturating_add(CLAUDE_KEYCHAIN_RETRY_COOLDOWN_MS));
        }

        Err(last_error
            .unwrap_or_else(|| CredentialError::ClaudeMissing(file.display().to_string())))
    }

    fn cache(&mut self, credential: ClaudeCredential, now_ms: i64) -> ClaudeCredential {
        self.cached = Some((credential.clone(), now_ms));
        credential
    }
}

fn credential_file_stamp(path: &Path) -> Option<CredentialFileStamp> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(CredentialFileStamp {
        modified: metadata.modified().ok(),
        length: metadata.len(),
    })
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

/// Reads a generic password only when macOS can return it without authentication.
#[cfg(target_os = "macos")]
fn read_keychain_without_ui(service: &str) -> Option<String> {
    use objc2::rc::Retained;
    use objc2_local_authentication::LAContext;
    use security_framework::item::{ItemClass, ItemSearchOptions, SearchResult};

    let context = unsafe { LAContext::new() };
    unsafe { context.setInteractionNotAllowed(true) };

    let mut options = ItemSearchOptions::new();
    options
        .class(ItemClass::generic_password())
        .service(service)
        .load_data(true)
        .skip_authenticated_items(true);
    #[allow(deprecated)]
    unsafe {
        options.authentication_context(Retained::into_raw(context).cast());
    }
    let results = options.search().ok()?;

    results.into_iter().find_map(|result| match result {
        SearchResult::Data(bytes) => String::from_utf8(bytes).ok(),
        _ => None,
    })
}

#[cfg(not(target_os = "macos"))]
fn read_keychain_without_ui(_service: &str) -> Option<String> {
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
    fn the_credential_file_is_preferred_to_keychain_access() {
        let file = tempfile::NamedTempFile::new().expect("credential file");
        std::fs::write(
            file.path(),
            r#"{"claudeAiOauth":{"accessToken":"from-file"}}"#,
        )
        .expect("write credential");
        let mut state = ClaudeCredentialState::default();

        let credential = state
            .resolve(
                1_000,
                file.path(),
                || panic!("the Keychain should not be queried"),
                || panic!("Keychain presence should not be queried"),
            )
            .expect("file credential");

        assert_eq!(credential.access_token.expose(), "from-file");
    }

    #[test]
    fn a_keychain_credential_is_cached_for_thirty_minutes() {
        let directory = tempfile::tempdir().expect("temp dir");
        let missing_file = directory.path().join("credentials.json");
        let mut state = ClaudeCredentialState::default();
        let credential = state
            .resolve(
                1_000,
                &missing_file,
                || Some(r#"{"claudeAiOauth":{"accessToken":"cached"}}"#.to_string()),
                || panic!("a successful read needs no presence query"),
            )
            .expect("Keychain credential");
        assert_eq!(credential.access_token.expose(), "cached");

        let credential = state
            .resolve(
                1_000 + CLAUDE_MEMORY_CACHE_VALIDITY_MS - 1,
                &missing_file,
                || panic!("the fresh cache should avoid the Keychain"),
                || panic!("the fresh cache should avoid the Keychain"),
            )
            .expect("cached credential");
        assert_eq!(credential.access_token.expose(), "cached");
    }

    #[test]
    fn an_inaccessible_keychain_item_has_a_six_hour_retry_cooldown() {
        let directory = tempfile::tempdir().expect("temp dir");
        let missing_file = directory.path().join("credentials.json");
        let mut state = ClaudeCredentialState::default();

        state
            .resolve(1_000, &missing_file, || None, || true)
            .expect_err("inaccessible");

        state
            .resolve(
                1_000 + CLAUDE_KEYCHAIN_RETRY_COOLDOWN_MS - 1,
                &missing_file,
                || panic!("the cooldown should suppress the Keychain read"),
                || panic!("the cooldown should suppress the presence query"),
            )
            .expect_err("still inaccessible");

        let credential = state
            .resolve(
                1_000 + CLAUDE_KEYCHAIN_RETRY_COOLDOWN_MS,
                &missing_file,
                || Some(r#"{"claudeAiOauth":{"accessToken":"available"}}"#.to_string()),
                || panic!("a successful read needs no presence query"),
            )
            .expect("retried after cooldown");
        assert_eq!(credential.access_token.expose(), "available");
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
        let _guard = crate::env_lock();
        std::env::set_var("HOME", "/nonexistent-home");
        let error = codex_credential(Some("   ")).expect_err("missing");
        assert!(error.to_string().contains(".codex/auth.json"), "{error}");
    }
}
