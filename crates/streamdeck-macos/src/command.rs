//! Bounded subprocess execution.
//!
//! Every external tool is invoked with an argument array — never a shell string —
//! under a timeout, with stdout and stderr captured, and with the child killed if
//! the future is dropped. A live counter of running children lets the daemon
//! assert it leaves nothing behind.

use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("`{program}` is not an absolute path")]
    NotAbsolute { program: String },
    #[error("could not start `{program}`: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`{program}` timed out after {}ms", timeout.as_millis())]
    Timeout { program: String, timeout: Duration },
    #[error("`{program}` exited with {code}: {stderr}")]
    Failed {
        program: String,
        code: i32,
        stderr: String,
    },
    #[error("`{program}` produced more than {limit} bytes")]
    TooMuchOutput { program: String, limit: usize },
}

/// The result of a completed command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn trimmed(&self) -> &str {
        self.stdout.trim()
    }
}

/// Runs external tools. Injected so every service can be tested without spawning
/// a process.
#[async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(
        &self,
        program: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<Output, CommandError>;

    /// How many children this runner currently has running.
    fn running(&self) -> usize {
        0
    }
}

const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

/// The real runner. Refuses relative program paths so nothing is ever resolved
/// through `PATH`, and never passes a string to a shell.
#[derive(Debug, Clone, Default)]
pub struct SystemCommandRunner {
    running: Arc<AtomicUsize>,
}

impl SystemCommandRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Shared counter, so `streamdeckctl status` can report live child processes.
    pub fn running_counter(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.running)
    }
}

/// Decrements the live-child counter however the future ends, including on drop.
struct ChildGuard(Arc<AtomicUsize>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

#[async_trait]
impl CommandRunner for SystemCommandRunner {
    async fn run(
        &self,
        program: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<Output, CommandError> {
        if !Path::new(program).is_absolute() {
            return Err(CommandError::NotAbsolute {
                program: program.to_string(),
            });
        }

        self.running.fetch_add(1, Ordering::Relaxed);
        let _guard = ChildGuard(Arc::clone(&self.running));

        let mut command = tokio::process::Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Without this a dropped future would leave the child running.
            .kill_on_drop(true);

        let child = command.spawn().map_err(|source| CommandError::Spawn {
            program: program.to_string(),
            source,
        })?;

        let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(result) => result.map_err(|source| CommandError::Spawn {
                program: program.to_string(),
                source,
            })?,
            Err(_) => {
                return Err(CommandError::Timeout {
                    program: program.to_string(),
                    timeout,
                })
            }
        };

        if output.stdout.len() > MAX_OUTPUT_BYTES {
            return Err(CommandError::TooMuchOutput {
                program: program.to_string(),
                limit: MAX_OUTPUT_BYTES,
            });
        }

        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if !output.status.success() {
            return Err(CommandError::Failed {
                program: program.to_string(),
                code: output.status.code().unwrap_or(-1),
                stderr: truncate(&stderr, 512),
            });
        }

        Ok(Output {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr,
        })
    }

    fn running(&self) -> usize {
        self.running.load(Ordering::Relaxed)
    }
}

fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.trim().to_string();
    }
    value
        .chars()
        .take(limit)
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runner() -> SystemCommandRunner {
        SystemCommandRunner::new()
    }

    #[tokio::test]
    async fn a_successful_command_returns_its_output() {
        let output = runner()
            .run("/bin/echo", &["hello", "deck"], Duration::from_secs(5))
            .await
            .expect("echo succeeds");
        assert_eq!(output.trimmed(), "hello deck");
    }

    #[tokio::test]
    async fn arguments_are_passed_as_an_array_and_never_reach_a_shell() {
        let output = runner()
            .run(
                "/bin/echo",
                &["$(touch /tmp/streamdeckd-should-not-exist); rm -rf ~"],
                Duration::from_secs(5),
            )
            .await
            .expect("echo succeeds");

        assert_eq!(
            output.trimmed(),
            "$(touch /tmp/streamdeckd-should-not-exist); rm -rf ~",
            "the argument was interpreted instead of passed through"
        );
        assert!(!Path::new("/tmp/streamdeckd-should-not-exist").exists());
    }

    #[tokio::test]
    async fn a_relative_program_is_refused_so_path_is_never_consulted() {
        let error = runner()
            .run("echo", &["hi"], Duration::from_secs(1))
            .await
            .expect_err("refused");
        assert!(matches!(error, CommandError::NotAbsolute { .. }), "{error}");
    }

    #[tokio::test]
    async fn a_missing_program_reports_a_spawn_error_naming_it() {
        let error = runner()
            .run(
                "/usr/bin/definitely-not-installed",
                &[],
                Duration::from_secs(1),
            )
            .await
            .expect_err("refused");
        assert!(
            error.to_string().contains("definitely-not-installed"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_non_zero_exit_captures_stderr() {
        let error = runner()
            .run(
                "/bin/sh",
                &["-c", "echo boom >&2; exit 3"],
                Duration::from_secs(5),
            )
            .await
            .expect_err("fails");
        match error {
            CommandError::Failed { code, stderr, .. } => {
                assert_eq!(code, 3);
                assert_eq!(stderr, "boom");
            }
            other => panic!("expected a failure, got {other}"),
        }
    }

    #[tokio::test]
    async fn a_slow_command_times_out_and_is_killed() {
        let runner = runner();
        let error = runner
            .run("/bin/sleep", &["30"], Duration::from_millis(150))
            .await
            .expect_err("times out");
        assert!(matches!(error, CommandError::Timeout { .. }), "{error}");

        // The killed child must not still be counted as running.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(runner.running(), 0);
    }

    #[tokio::test]
    async fn the_live_child_count_returns_to_zero_after_every_outcome() {
        let runner = runner();
        let _ = runner
            .run("/bin/echo", &["ok"], Duration::from_secs(5))
            .await;
        let _ = runner
            .run("/bin/sh", &["-c", "exit 1"], Duration::from_secs(5))
            .await;
        let _ = runner.run("relative", &[], Duration::from_secs(5)).await;

        assert_eq!(runner.running(), 0);
    }

    #[tokio::test]
    async fn dropping_the_future_does_not_leave_an_orphan() {
        let runner = runner();
        {
            let future = runner.run("/bin/sleep", &["30"], Duration::from_secs(30));
            // Poll once so the child actually spawns, then drop it.
            let mut future = Box::pin(future);
            let _ = tokio::time::timeout(Duration::from_millis(80), &mut future).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(runner.running(), 0);
    }

    #[test]
    fn long_stderr_is_truncated_before_it_reaches_a_log() {
        let long = "x".repeat(4_000);
        assert_eq!(truncate(&long, 512).len(), 512);
        assert_eq!(truncate("  short  ", 512), "short");
    }
}
