//! Human-readable output.
//!
//! Every command also supports `--json`, which prints the daemon's payload
//! verbatim so a soak test can graph it.

use streamdeck_core::control::Response;

use crate::Command;

/// Prints the response and returns the process exit code.
pub fn print(command: &Command, response: &Response, json: bool) -> i32 {
    let (message, data) = match response {
        Response::Ok { message, data, .. } => (message.as_str(), data.as_ref()),
        Response::Error { message, .. } => {
            eprintln!("{message}");
            return 1;
        }
    };

    if json {
        let payload = data
            .cloned()
            .unwrap_or_else(|| serde_json::json!({ "message": message }));
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string())
        );
        return 0;
    }

    match (command, data) {
        (Command::Status, Some(data)) => print_status(data),
        (Command::Devices, Some(data)) => print_devices(data),
        (Command::Doctor, Some(data)) => return print_doctor(data),
        _ => println!("{message}"),
    }
    0
}

fn print_status(data: &serde_json::Value) {
    let string = |key: &str| {
        data.get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("—")
            .to_string()
    };
    let number = |key: &str| {
        data.get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };

    let device = data
        .get("device")
        .and_then(|device| device.get("serial"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("not connected");

    println!("device        {device}");
    println!(
        "page          {}{}",
        string("page"),
        if data
            .get("panel_open")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            " (panel)"
        } else {
            ""
        }
    );
    println!("uptime        {}", duration(number("uptime_seconds")));
    if let Some(resident) = data.get("resident_mib").and_then(serde_json::Value::as_f64) {
        println!("memory        {resident:.1} MiB");
    }
    println!("children      {}", number("child_processes"));
    println!(
        "frames        {} sent, {} skipped, {} KiB",
        number("frames_sent"),
        number("frames_skipped"),
        number("bytes_sent") / 1024
    );
    println!(
        "input         {} presses, {} long presses, {} page switches",
        number("key_presses"),
        number("long_presses"),
        number("page_switches")
    );
    println!(
        "recovery      {} reconnects, {} wakes, {} reloads",
        number("device_reconnects"),
        number("wakes"),
        number("config_reloads")
    );

    if let Some(error) = data
        .get("last_config_error")
        .and_then(serde_json::Value::as_str)
    {
        println!("config error  {error}");
    }

    if let Some(pomodoro) = data.get("pomodoro") {
        let phase = pomodoro
            .get("phase")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("—");
        let status = pomodoro
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("—");
        let remaining = pomodoro
            .get("remaining_seconds")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        print!(
            "pomodoro      {phase} {status}, {}:{:02} left",
            remaining / 60,
            remaining % 60
        );
        if let Some(pending) = pomodoro
            .get("pending_completion")
            .and_then(serde_json::Value::as_str)
        {
            print!(" — {pending} awaiting acknowledgement");
        }
        println!();
    }

    if let Some(integrations) = data
        .get("integrations")
        .and_then(serde_json::Value::as_object)
    {
        println!("integrations");
        for (name, entry) in integrations {
            let failures = entry
                .get("failures")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let stale = entry
                .get("stale")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let mut line = format!(
                "  {name:<16} {} request(s), {failures} failure(s)",
                entry
                    .get("requests")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0)
            );
            if stale {
                line.push_str(" [stale]");
            }
            if let Some(error) = entry.get("last_error").and_then(serde_json::Value::as_str) {
                line.push_str(&format!(" — {error}"));
            }
            println!("{line}");
        }
    }

    if let Some(pending) = data
        .get("pending_deadlines")
        .and_then(serde_json::Value::as_array)
    {
        if pending.is_empty() {
            println!("deadlines     none pending; the daemon is idle");
        } else {
            println!("deadlines");
            for entry in pending {
                println!(
                    "  {:<20} in {}",
                    entry
                        .get("deadline")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("—"),
                    duration(
                        entry
                            .get("in_ms")
                            .and_then(serde_json::Value::as_u64)
                            .unwrap_or(0)
                            / 1_000
                    )
                );
            }
        }
    }
}

fn print_devices(data: &serde_json::Value) {
    let Some(devices) = data.as_array() else {
        println!("no devices");
        return;
    };
    if devices.is_empty() {
        println!("no Stream Deck is connected");
        return;
    }
    for device in devices {
        let serial = device
            .get("serial")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("—");
        let kind = device
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("—");
        let rows = device
            .get("rows")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let columns = device
            .get("columns")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let available = device
            .get("available")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        println!(
            "{serial}  {kind}  {columns}x{rows}  {}",
            if available {
                "available"
            } else {
                "owned by another application"
            }
        );
    }
}

/// Prints the checklist and returns a non-zero exit code if anything failed.
fn print_doctor(data: &serde_json::Value) -> i32 {
    let Some(checks) = data.get("checks").and_then(serde_json::Value::as_array) else {
        println!("no checks were run");
        return 1;
    };

    let mut failed = 0;
    for check in checks {
        let health = check
            .get("health")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("fail");
        let symbol = match health {
            "ok" => "ok  ",
            "warn" => "warn",
            _ => "FAIL",
        };
        if health == "fail" {
            failed += 1;
        }
        println!(
            "[{symbol}] {:<18} {}",
            check
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("—"),
            check
                .get("detail")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
        );
    }

    if failed > 0 {
        eprintln!("\n{failed} check(s) failed");
        1
    } else {
        0
    }
}

/// Compact duration for the status output.
fn duration(seconds: u64) -> String {
    match seconds {
        0 => "now".to_string(),
        seconds if seconds < 60 => format!("{seconds}s"),
        seconds if seconds < 3_600 => format!("{}m{}s", seconds / 60, seconds % 60),
        seconds if seconds < 86_400 => format!("{}h{}m", seconds / 3_600, (seconds % 3_600) / 60),
        seconds => format!("{}d{}h", seconds / 86_400, (seconds % 86_400) / 3_600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_compactly_at_every_scale() {
        assert_eq!(duration(0), "now");
        assert_eq!(duration(45), "45s");
        assert_eq!(duration(95), "1m35s");
        assert_eq!(duration(3_725), "1h2m");
        assert_eq!(duration(90_061), "1d1h");
    }

    #[test]
    fn an_error_response_exits_non_zero() {
        let exit = print(&Command::Status, &Response::error("no device"), false);
        assert_eq!(exit, 1);
    }

    #[test]
    fn an_ok_response_exits_zero() {
        let exit = print(&Command::Reload, &Response::ok("reloaded"), false);
        assert_eq!(exit, 0);
    }

    #[test]
    fn a_failing_doctor_check_exits_non_zero() {
        let response = Response::data(
            "ok",
            serde_json::json!({
                "summary": "fail",
                "checks": [
                    {"name": "device", "health": "fail", "detail": "no Stream Deck is connected"},
                    {"name": "config", "health": "ok", "detail": "valid"}
                ]
            }),
        );
        assert_eq!(print(&Command::Doctor, &response, false), 1);
    }

    #[test]
    fn a_healthy_doctor_run_exits_zero() {
        let response = Response::data(
            "ok",
            serde_json::json!({
                "summary": "ok",
                "checks": [{"name": "config", "health": "ok", "detail": "valid"}]
            }),
        );
        assert_eq!(print(&Command::Doctor, &response, false), 0);
    }

    #[test]
    fn a_doctor_response_with_no_checks_is_treated_as_a_failure() {
        let response = Response::data("ok", serde_json::json!({}));
        assert_eq!(print(&Command::Doctor, &response, false), 1);
    }

    #[test]
    fn json_output_prints_the_payload_for_every_command() {
        let response = Response::data("ok", serde_json::json!({"uptime_seconds": 42}));
        assert_eq!(print(&Command::Status, &response, true), 0);
        // A doctor failure still exits zero in JSON mode: the caller parses it.
        let doctor = Response::data(
            "ok",
            serde_json::json!({"checks": [{"name": "x", "health": "fail", "detail": "y"}]}),
        );
        assert_eq!(print(&Command::Doctor, &doctor, true), 0);
    }

    #[test]
    fn status_output_survives_a_payload_with_missing_fields() {
        let response = Response::data("ok", serde_json::json!({}));
        assert_eq!(print(&Command::Status, &response, false), 0);
    }

    #[test]
    fn device_output_handles_an_empty_list() {
        let response = Response::data("0 device(s)", serde_json::json!([]));
        assert_eq!(print(&Command::Devices, &response, false), 0);
    }
}
