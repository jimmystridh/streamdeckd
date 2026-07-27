//! Wispr Flow hands-free and microphone control.

use std::fmt::Write;
use std::sync::Arc;

use async_trait::async_trait;

use crate::{timeouts, CommandRunner};

const START_HANDS_FREE: &str = "wispr-flow://start-hands-free";
const STOP_HANDS_FREE: &str = "wispr-flow://stop-hands-free";

#[derive(Debug, thiserror::Error)]
pub enum WisprError {
    #[error(transparent)]
    Command(#[from] crate::CommandError),
}

#[async_trait]
pub trait WisprAdapter: Send + Sync {
    async fn set_hands_free(&self, enabled: bool) -> Result<(), WisprError>;
    async fn select_microphone(&self, name: &str) -> Result<(), WisprError>;
}

pub struct SystemWisprAdapter {
    runner: Arc<dyn CommandRunner>,
    open: String,
}

impl SystemWisprAdapter {
    pub fn new(runner: Arc<dyn CommandRunner>, open: impl Into<String>) -> Self {
        Self {
            runner,
            open: open.into(),
        }
    }

    async fn open_deep_link(&self, url: &str) -> Result<(), WisprError> {
        self.runner
            .run(&self.open, &["-g", url], timeouts::LOCAL)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl WisprAdapter for SystemWisprAdapter {
    async fn set_hands_free(&self, enabled: bool) -> Result<(), WisprError> {
        self.open_deep_link(if enabled {
            START_HANDS_FREE
        } else {
            STOP_HANDS_FREE
        })
        .await
    }

    async fn select_microphone(&self, name: &str) -> Result<(), WisprError> {
        let url = format!(
            "wispr-flow://switch-mic?mic_name={}",
            encode_query_component(name)
        );
        self.open_deep_link(&url).await
    }
}

fn encode_query_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            write!(encoded, "%{byte:02X}").expect("writing to a string cannot fail");
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeCommandRunner, Reply};

    fn adapter(runner: &Arc<FakeCommandRunner>) -> SystemWisprAdapter {
        SystemWisprAdapter::new(
            Arc::clone(runner) as Arc<dyn CommandRunner>,
            "/usr/bin/open",
        )
    }

    #[tokio::test]
    async fn hands_free_uses_wisprs_first_party_deep_links() {
        let runner = Arc::new(FakeCommandRunner::new());
        runner.fallback(Reply::ok(""));
        let adapter = adapter(&runner);

        adapter.set_hands_free(true).await.expect("started");
        adapter.set_hands_free(false).await.expect("stopped");

        assert!(runner.called_with("/usr/bin/open -g wispr-flow://start-hands-free"));
        assert!(runner.called_with("/usr/bin/open -g wispr-flow://stop-hands-free"));
    }

    #[tokio::test]
    async fn microphone_name_is_encoded_into_wisprs_switch_link() {
        let runner = Arc::new(FakeCommandRunner::new());
        runner.fallback(Reply::ok(""));
        let adapter = adapter(&runner);

        adapter
            .select_microphone("RØDE NT-USB (USB)")
            .await
            .expect("selected");

        assert!(runner.called_with(
            "/usr/bin/open -g wispr-flow://switch-mic?mic_name=R%C3%98DE%20NT-USB%20%28USB%29"
        ));
    }
}
