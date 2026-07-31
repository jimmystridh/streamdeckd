//! Parsed Mac health and network/VPN snapshots.

use super::ParseError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerSource {
    Ac,
    Battery,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacHealth {
    pub battery_percent: Option<u8>,
    pub power_source: PowerSource,
    pub charging: bool,
    pub memory_free_percent: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpnState {
    Connected,
    Disconnected,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkStatus {
    pub connected: bool,
    pub interface: Option<String>,
    pub address: Option<String>,
    pub vpn_name: String,
    pub vpn_state: VpnState,
}

pub fn parse_health(pmset: &str, memory_pressure: &str) -> Result<MacHealth, ParseError> {
    let power_source = if pmset.contains("AC Power") {
        PowerSource::Ac
    } else if pmset.contains("Battery Power") {
        PowerSource::Battery
    } else {
        PowerSource::Unknown
    };
    let battery_percent = pmset
        .split_whitespace()
        .find_map(|token| token.trim_end_matches(';').strip_suffix('%'))
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|value| *value <= 100);
    let charging = pmset
        .lines()
        .any(|line| line.contains("; charging") && !line.contains("not charging"));
    let memory_free_percent = memory_pressure
        .lines()
        .find_map(|line| line.strip_prefix("System-wide memory free percentage:"))
        .and_then(|value| value.trim().trim_end_matches('%').parse::<u8>().ok())
        .filter(|value| *value <= 100)
        .ok_or_else(|| ParseError::shape("mac-health", "memory percentage is missing"))?;

    Ok(MacHealth {
        battery_percent,
        power_source,
        charging,
        memory_free_percent,
    })
}

pub fn parse_network(nwi: &str, vpn_list: &str, wanted_vpn: &str) -> NetworkStatus {
    let connected = nwi.contains("(Reachable)");
    let interface = nwi.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .split_once(" : flags")
            .map(|(name, _)| name.to_string())
    });
    let address = nwi.lines().find_map(|line| {
        line.trim()
            .strip_prefix("address")
            .and_then(|rest| rest.strip_prefix(char::is_whitespace))
            .map(|rest| rest.trim_start_matches(':').trim().to_string())
            .filter(|value| !value.is_empty())
    });
    let vpn_line = vpn_list
        .lines()
        .find(|line| line.contains(&format!("\"{wanted_vpn}\"")));
    let vpn_state = match vpn_line {
        Some(line) if line.contains("(Connected)") => VpnState::Connected,
        Some(_) => VpnState::Disconnected,
        None => VpnState::Unavailable,
    };

    NetworkStatus {
        connected,
        interface,
        address,
        vpn_name: wanted_vpn.to_string(),
        vpn_state,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_command_shapes_parse_into_health() {
        let health = parse_health(
            "Now drawing from 'AC Power'\n -InternalBattery-0\t80%; AC attached; not charging present: true",
            "System-wide memory free percentage: 48%\n",
        )
        .expect("health");
        assert_eq!(health.battery_percent, Some(80));
        assert_eq!(health.power_source, PowerSource::Ac);
        assert!(!health.charging);
        assert_eq!(health.memory_free_percent, 48);
    }

    #[test]
    fn nwi_and_scutil_vpn_output_parse_without_private_apis() {
        let status = parse_network(
            "en0 : flags : 0x5 (IPv4,DNS)\n address : 10.0.1.49\n REACH : flags 0x2 (Reachable)",
            "* (Connected) VPN (io.tailscale.ipn.macsys) \"Tailscale\"",
            "Tailscale",
        );
        assert!(status.connected);
        assert_eq!(status.interface.as_deref(), Some("en0"));
        assert_eq!(status.address.as_deref(), Some("10.0.1.49"));
        assert_eq!(status.vpn_state, VpnState::Connected);
    }
}
