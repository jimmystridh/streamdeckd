//! The control-socket protocol.
//!
//! One newline-delimited JSON request, one newline-delimited JSON response. The
//! request type is a closed enum, so the socket can never be used to run an
//! arbitrary command; adding a capability means adding a variant here.

use serde::{Deserialize, Serialize};

use crate::model::{IntegrationId, KeyPosition, PageId};
use crate::pomodoro::Phase;

/// Bumped when the wire format changes incompatibly. `streamdeckctl` refuses to
/// talk to a daemon that answers with a different version.
pub const PROTOCOL_VERSION: u32 = 1;

/// The longest request the daemon will read, so a stuck client cannot grow memory.
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "kebab-case")]
pub enum Request {
    /// Health, metrics, and current state.
    Status,
    /// Discovered Stream Deck devices and their ownership.
    Devices,
    /// Switch to a page.
    Page {
        page: PageId,
    },
    /// Synthesise a short press.
    Press {
        position: KeyPosition,
    },
    /// Synthesise a press held for `milliseconds`.
    Hold {
        position: KeyPosition,
        milliseconds: u64,
    },
    Pomodoro {
        action: PomodoroAction,
    },
    /// Force one integration to refresh now.
    Refresh {
        integration: IntegrationId,
    },
    /// Re-read and validate the configuration, swapping it in only if valid.
    Reload,
    /// Render a page to a PNG at `output`.
    RenderPreview {
        page: PageId,
        output: String,
    },
    /// Run every health check.
    Doctor,
    /// Temporarily change the log level.
    LogLevel {
        level: String,
    },
    /// Shut down cleanly.
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum PomodoroAction {
    Acknowledge,
    Start { phase: Phase },
    Toggle,
    Skip,
    Reset,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "kebab-case")]
pub enum Response {
    Ok {
        version: u32,
        /// Human-readable confirmation, for the CLI's plain output.
        message: String,
        /// Structured payload for `--json`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
    },
    Error {
        version: u32,
        message: String,
    },
}

impl Response {
    pub fn ok(message: impl Into<String>) -> Self {
        Response::Ok {
            version: PROTOCOL_VERSION,
            message: message.into(),
            data: None,
        }
    }

    pub fn data(message: impl Into<String>, data: serde_json::Value) -> Self {
        Response::Ok {
            version: PROTOCOL_VERSION,
            message: message.into(),
            data: Some(data),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Response::Error {
            version: PROTOCOL_VERSION,
            message: message.into(),
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, Response::Ok { .. })
    }

    pub fn message(&self) -> &str {
        match self {
            Response::Ok { message, .. } | Response::Error { message, .. } => message,
        }
    }

    pub fn version(&self) -> u32 {
        match self {
            Response::Ok { version, .. } | Response::Error { version, .. } => *version,
        }
    }
}

/// Encodes a value as one protocol line.
pub fn encode<T: Serialize>(value: &T) -> String {
    let mut line = serde_json::to_string(value).expect("protocol types are serializable");
    line.push('\n');
    line
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("request exceeded {MAX_REQUEST_BYTES} bytes")]
    TooLarge,
    #[error("malformed request: {0}")]
    Malformed(String),
}

pub fn decode_request(line: &str) -> Result<Request, ProtocolError> {
    if line.len() > MAX_REQUEST_BYTES {
        return Err(ProtocolError::TooLarge);
    }
    serde_json::from_str(line).map_err(|error| ProtocolError::Malformed(error.to_string()))
}

pub fn decode_response(line: &str) -> Result<Response, ProtocolError> {
    serde_json::from_str(line).map_err(|error| ProtocolError::Malformed(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(request: Request) {
        let line = encode(&request);
        assert!(line.ends_with('\n'), "requests are newline delimited");
        let decoded = decode_request(line.trim()).expect("decoded");
        assert_eq!(decoded, request);
    }

    #[test]
    fn every_request_round_trips() {
        round_trip(Request::Status);
        round_trip(Request::Devices);
        round_trip(Request::Page {
            page: PageId::Pomodoro,
        });
        round_trip(Request::Press {
            position: KeyPosition::new(2, 3),
        });
        round_trip(Request::Hold {
            position: KeyPosition::new(2, 3),
            milliseconds: 700,
        });
        round_trip(Request::Pomodoro {
            action: PomodoroAction::Start {
                phase: Phase::Focus,
            },
        });
        round_trip(Request::Pomodoro {
            action: PomodoroAction::Acknowledge,
        });
        round_trip(Request::Refresh {
            integration: IntegrationId::GitHub,
        });
        round_trip(Request::Reload);
        round_trip(Request::RenderPreview {
            page: PageId::Home,
            output: "/tmp/home.png".to_string(),
        });
        round_trip(Request::Doctor);
        round_trip(Request::LogLevel {
            level: "debug".to_string(),
        });
        round_trip(Request::Stop);
    }

    #[test]
    fn the_wire_format_is_tagged_and_readable() {
        let line = encode(&Request::Page {
            page: PageId::Stensjon,
        });
        assert_eq!(line.trim(), r#"{"command":"page","page":"stensjon"}"#);
    }

    #[test]
    fn responses_carry_a_version_and_an_optional_payload() {
        let ok = Response::ok("switched to home");
        assert!(ok.is_ok());
        assert_eq!(ok.version(), PROTOCOL_VERSION);
        assert_eq!(ok.message(), "switched to home");

        let with_data = Response::data("status", serde_json::json!({"uptime_seconds": 42}));
        let line = encode(&with_data);
        let decoded = decode_response(line.trim()).expect("decoded");
        assert_eq!(decoded, with_data);

        let error = Response::error("no device");
        assert!(!error.is_ok());
        assert_eq!(error.message(), "no device");
    }

    #[test]
    fn an_arbitrary_command_string_cannot_be_smuggled_through() {
        for line in [
            r#"{"command":"shell","script":"rm -rf ~"}"#,
            r#"{"command":"page","page":"../../etc"}"#,
            r#"{"command":"press","position":"not a coordinate"}"#,
            "not json at all",
            "",
        ] {
            assert!(decode_request(line).is_err(), "{line}");
        }
    }

    #[test]
    fn an_oversized_request_is_refused_before_parsing() {
        let line = format!(
            r#"{{"command":"page","page":"{}"}}"#,
            "x".repeat(MAX_REQUEST_BYTES)
        );
        assert!(matches!(
            decode_request(&line).expect_err("refused"),
            ProtocolError::TooLarge
        ));
    }

    #[test]
    fn a_key_position_is_validated_by_its_own_deserializer() {
        let line = r#"{"command":"press","position":{"row":2,"column":3}}"#;
        assert_eq!(
            decode_request(line).expect("decoded"),
            Request::Press {
                position: KeyPosition::new(2, 3)
            }
        );
    }
}
