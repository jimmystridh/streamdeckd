//! Payload parsers and resolution rules for every integration.
//!
//! Each submodule turns raw text — a CLI's JSON, an HTTP body, a command's stdout
//! — into a typed domain value, with validation and bounds. Nothing here performs
//! I/O, so every parse rule, cap, and rejection is covered by a fixture test.

pub mod application;
pub mod audio;
pub mod ci;
pub mod claude;
pub mod codex;
pub mod departures;
pub mod github;
pub mod lake;
pub mod media;
pub mod meetings;
pub mod spotify;
pub mod system;
pub mod walkingpad;
pub mod weather;

/// Guards against a hostile or broken endpoint returning an unbounded body.
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("{integration} returned malformed JSON: {source}")]
    Json {
        integration: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("{integration} returned an unexpected payload: {detail}")]
    Shape {
        integration: &'static str,
        detail: String,
    },
    #[error("{integration} returned a value outside its valid range: {detail}")]
    Range {
        integration: &'static str,
        detail: String,
    },
    #[error("{integration} response exceeded {MAX_RESPONSE_BYTES} bytes")]
    TooLarge { integration: &'static str },
}

impl ParseError {
    pub fn shape(integration: &'static str, detail: impl Into<String>) -> Self {
        Self::Shape {
            integration,
            detail: detail.into(),
        }
    }

    pub fn range(integration: &'static str, detail: impl Into<String>) -> Self {
        Self::Range {
            integration,
            detail: detail.into(),
        }
    }
}

pub(crate) fn parse_json(
    integration: &'static str,
    body: &str,
) -> Result<serde_json::Value, ParseError> {
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(ParseError::TooLarge { integration });
    }
    serde_json::from_str(body).map_err(|source| ParseError::Json {
        integration,
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_bodies_are_refused_before_parsing() {
        let body = format!("[{}]", "0,".repeat(MAX_RESPONSE_BYTES / 2));
        let error = parse_json("test", &body).expect_err("refused");
        assert!(matches!(error, ParseError::TooLarge { .. }), "{error}");
    }

    #[test]
    fn malformed_json_names_the_integration() {
        let error = parse_json("weather", "{oops").expect_err("refused");
        assert!(error.to_string().contains("weather"), "{error}");
    }
}
