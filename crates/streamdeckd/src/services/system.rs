//! Cheap local snapshots for Mac health and network/VPN status.

use std::sync::Arc;

use streamdeck_core::integrations::system::{self, MacHealth, NetworkStatus};
use streamdeck_macos::{timeouts, CommandRunner};

#[derive(Debug, thiserror::Error)]
pub enum SystemError {
    #[error(transparent)]
    Command(#[from] streamdeck_macos::CommandError),
    #[error(transparent)]
    Parse(#[from] streamdeck_core::integrations::ParseError),
}

pub async fn health(runner: &Arc<dyn CommandRunner>) -> Result<MacHealth, SystemError> {
    let (battery, memory) = tokio::join!(
        runner.run("/usr/bin/pmset", &["-g", "batt"], timeouts::LOCAL),
        runner.run("/usr/bin/memory_pressure", &["-Q"], timeouts::LOCAL),
    );
    Ok(system::parse_health(&battery?.stdout, &memory?.stdout)?)
}

pub async fn network(
    runner: &Arc<dyn CommandRunner>,
    vpn_name: &str,
) -> Result<NetworkStatus, SystemError> {
    let (network, vpn) = tokio::join!(
        runner.run("/usr/sbin/scutil", &["--nwi"], timeouts::LOCAL),
        runner.run("/usr/sbin/scutil", &["--nc", "list"], timeouts::LOCAL),
    );
    Ok(system::parse_network(
        &network?.stdout,
        &vpn?.stdout,
        vpn_name,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use streamdeck_macos::fake::{FakeCommandRunner, Reply};

    #[tokio::test]
    async fn health_uses_only_local_read_only_commands() {
        let fake = Arc::new(FakeCommandRunner::new());
        fake.on(
            "/usr/bin/pmset -g batt",
            Reply::ok("Now drawing from 'AC Power'\n80%; AC attached; not charging"),
        )
        .on(
            "/usr/bin/memory_pressure -Q",
            Reply::ok("System-wide memory free percentage: 48%"),
        );
        let health = health(&(fake as Arc<dyn CommandRunner>))
            .await
            .expect("health");
        assert_eq!(health.battery_percent, Some(80));
        assert_eq!(health.memory_free_percent, 48);
    }
}
