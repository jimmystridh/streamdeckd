//! A scripted command runner for tests.
//!
//! Every service in the daemon takes a [`CommandRunner`], so a test can assert on
//! the exact argument arrays used and inject timeouts, non-zero exits, and
//! malformed output without touching the system.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;

use crate::command::{CommandError, CommandRunner, Output};
use crate::wispr::{WisprAdapter, WisprError};

/// What a scripted invocation should do.
#[derive(Debug, Clone)]
pub enum Reply {
    Stdout(String),
    Failure { code: i32, stderr: String },
    Timeout,
    Missing,
}

impl Reply {
    pub fn ok(stdout: impl Into<String>) -> Self {
        Reply::Stdout(stdout.into())
    }

    pub fn fails(code: i32, stderr: impl Into<String>) -> Self {
        Reply::Failure {
            code,
            stderr: stderr.into(),
        }
    }
}

/// One recorded invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
}

impl Invocation {
    pub fn joined(&self) -> String {
        format!("{} {}", self.program, self.args.join(" "))
    }
}

#[derive(Debug, Default)]
struct State {
    /// Replies keyed by a substring that must appear in the joined command line.
    matchers: Vec<(String, Reply)>,
    /// Replies for a program regardless of arguments.
    programs: HashMap<String, Reply>,
    calls: Vec<Invocation>,
    default: Option<Reply>,
}

/// A `CommandRunner` that answers from a script and records what it was asked.
#[derive(Debug, Default)]
pub struct FakeCommandRunner {
    state: Mutex<State>,
}

impl FakeCommandRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replies when `needle` appears anywhere in the joined command line. Matchers
    /// are checked in registration order, so a specific rule can precede a general
    /// one. Registering the same needle twice replaces the earlier reply, which is
    /// how a test overrides one command inside an otherwise healthy script.
    pub fn on(&self, needle: impl Into<String>, reply: Reply) -> &Self {
        let needle = needle.into();
        let mut state = self.state.lock().expect("fake runner lock");
        match state
            .matchers
            .iter_mut()
            .find(|(existing, _)| *existing == needle)
        {
            Some(entry) => entry.1 = reply,
            None => state.matchers.push((needle, reply)),
        }
        drop(state);
        self
    }

    /// Replies for every invocation of `program`.
    pub fn on_program(&self, program: impl Into<String>, reply: Reply) -> &Self {
        self.state
            .lock()
            .expect("fake runner lock")
            .programs
            .insert(program.into(), reply);
        self
    }

    /// Reply used when nothing else matches. Without one, an unscripted call fails
    /// loudly so a test cannot silently exercise the wrong path.
    pub fn fallback(&self, reply: Reply) -> &Self {
        self.state.lock().expect("fake runner lock").default = Some(reply);
        self
    }

    pub fn calls(&self) -> Vec<Invocation> {
        self.state.lock().expect("fake runner lock").calls.clone()
    }

    pub fn call_count(&self) -> usize {
        self.state.lock().expect("fake runner lock").calls.len()
    }

    /// Whether any recorded invocation contains `needle`.
    pub fn called_with(&self, needle: &str) -> bool {
        self.calls()
            .iter()
            .any(|call| call.joined().contains(needle))
    }

    pub fn reset(&self) {
        self.state.lock().expect("fake runner lock").calls.clear();
    }
}

#[async_trait]
impl CommandRunner for FakeCommandRunner {
    async fn run(
        &self,
        program: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<Output, CommandError> {
        let invocation = Invocation {
            program: program.to_string(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
        };
        let joined = invocation.joined();

        let reply = {
            let mut state = self.state.lock().expect("fake runner lock");
            state.calls.push(invocation);
            state
                .matchers
                .iter()
                .find(|(needle, _)| joined.contains(needle.as_str()))
                .map(|(_, reply)| reply.clone())
                .or_else(|| state.programs.get(program).cloned())
                .or_else(|| state.default.clone())
        };

        match reply {
            Some(Reply::Stdout(stdout)) => Ok(Output {
                stdout,
                stderr: String::new(),
            }),
            Some(Reply::Failure { code, stderr }) => Err(CommandError::Failed {
                program: program.to_string(),
                code,
                stderr,
            }),
            Some(Reply::Timeout) => Err(CommandError::Timeout {
                program: program.to_string(),
                timeout,
            }),
            Some(Reply::Missing) | None => Err(CommandError::Spawn {
                program: program.to_string(),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no scripted reply for this command",
                ),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WisprInvocation {
    HandsFree(bool),
    Microphone(String),
}

#[derive(Debug, Default)]
pub struct FakeWisprAdapter {
    calls: Mutex<Vec<WisprInvocation>>,
}

impl FakeWisprAdapter {
    pub fn calls(&self) -> Vec<WisprInvocation> {
        self.calls.lock().expect("fake Wispr lock").clone()
    }
}

#[async_trait]
impl WisprAdapter for FakeWisprAdapter {
    async fn set_hands_free(&self, enabled: bool) -> Result<(), WisprError> {
        self.calls
            .lock()
            .expect("fake Wispr lock")
            .push(WisprInvocation::HandsFree(enabled));
        Ok(())
    }

    async fn select_microphone(&self, name: &str) -> Result<(), WisprError> {
        self.calls
            .lock()
            .expect("fake Wispr lock")
            .push(WisprInvocation::Microphone(name.to_string()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_matcher_answers_and_the_call_is_recorded() {
        let runner = FakeCommandRunner::new();
        runner.on("-c -t output", Reply::ok("Bose NC 700 Headphones"));

        let output = runner
            .run(
                "/opt/homebrew/bin/SwitchAudioSource",
                &["-c", "-t", "output"],
                Duration::from_secs(1),
            )
            .await
            .expect("scripted");

        assert_eq!(output.trimmed(), "Bose NC 700 Headphones");
        assert_eq!(runner.call_count(), 1);
        assert!(runner.called_with("SwitchAudioSource -c -t output"));
    }

    #[tokio::test]
    async fn registering_the_same_needle_twice_replaces_the_reply() {
        let runner = FakeCommandRunner::new();
        runner.on("status", Reply::ok("first"));
        runner.on("status", Reply::ok("second"));

        let output = runner
            .run("/x", &["status"], Duration::from_secs(1))
            .await
            .expect("scripted");
        assert_eq!(output.trimmed(), "second");
    }

    #[tokio::test]
    async fn matchers_are_checked_in_registration_order() {
        let runner = FakeCommandRunner::new();
        runner
            .on("-a -t input", Reply::ok("MacBook Pro Microphone"))
            .on("-a", Reply::ok("everything else"));

        let inputs = runner
            .run("/x", &["-a", "-t", "input"], Duration::from_secs(1))
            .await
            .expect("scripted");
        assert_eq!(inputs.trimmed(), "MacBook Pro Microphone");

        let outputs = runner
            .run("/x", &["-a", "-t", "output"], Duration::from_secs(1))
            .await
            .expect("scripted");
        assert_eq!(outputs.trimmed(), "everything else");
    }

    #[tokio::test]
    async fn an_unscripted_call_fails_rather_than_returning_empty_output() {
        let runner = FakeCommandRunner::new();
        assert!(runner
            .run("/x", &["surprise"], Duration::from_secs(1))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn failures_and_timeouts_can_be_injected() {
        let runner = FakeCommandRunner::new();
        runner
            .on("boom", Reply::fails(2, "exploded"))
            .on("slow", Reply::Timeout);

        let error = runner
            .run("/x", &["boom"], Duration::from_secs(1))
            .await
            .expect_err("fails");
        assert!(error.to_string().contains("exploded"), "{error}");

        let error = runner
            .run("/x", &["slow"], Duration::from_secs(1))
            .await
            .expect_err("times out");
        assert!(matches!(error, CommandError::Timeout { .. }), "{error}");
    }

    #[tokio::test]
    async fn a_program_level_reply_covers_every_argument_list() {
        let runner = FakeCommandRunner::new();
        runner.on_program("/usr/bin/osascript", Reply::ok("done"));

        for args in [vec!["-e", "a"], vec!["-e", "b", "-e", "c"]] {
            let output = runner
                .run("/usr/bin/osascript", &args, Duration::from_secs(1))
                .await
                .expect("scripted");
            assert_eq!(output.trimmed(), "done");
        }
        assert_eq!(runner.call_count(), 2);
    }

    #[tokio::test]
    async fn resetting_clears_the_recorded_calls_but_keeps_the_script() {
        let runner = FakeCommandRunner::new();
        runner.on("x", Reply::ok("y"));
        let _ = runner.run("/a", &["x"], Duration::from_secs(1)).await;
        runner.reset();

        assert_eq!(runner.call_count(), 0);
        assert!(runner
            .run("/a", &["x"], Duration::from_secs(1))
            .await
            .is_ok());
    }
}
