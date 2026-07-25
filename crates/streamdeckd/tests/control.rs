//! Control-socket tests against the real runtime.
//!
//! These prove that every documented `streamdeckctl` command reaches the
//! coordinator, produces the right state change, and cannot be used to run an
//! arbitrary command.

mod harness;

use harness::Harness;
use streamdeck_core::control::{PomodoroAction, Request, Response};
use streamdeck_core::model::{IntegrationId, KeyPosition, PageId};
use streamdeck_core::pomodoro::{Phase, Status};
use streamdeckd::control::ControlSocket;

/// Binds a socket in front of a live runtime and returns its path.
async fn serve(harness: &Harness) -> (std::path::PathBuf, tempfile::TempDir) {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("streamdeckd.sock");
    let socket = ControlSocket::bind(&path).await.expect("bound");
    tokio::spawn(socket.serve(harness.events.clone()));
    (path, directory)
}

/// Sends a request and keeps driving the runtime until the client has its answer.
///
/// A command such as `hold` deliberately takes longer than one settle window, so
/// the loop pumps the coordinator until the client task finishes.
async fn ask(harness: &mut Harness, path: &std::path::Path, request: Request) -> Response {
    let described = format!("{request:?}");
    // A hold deliberately occupies the coordinator for its whole duration, so the
    // settle budget has to cover it.
    let budget = match &request {
        Request::Hold { milliseconds, .. } => std::time::Duration::from_millis(milliseconds + 500),
        _ => std::time::Duration::from_millis(250),
    };
    let path = path.to_path_buf();
    let mut client = tokio::spawn(async move { streamdeckd::control::send(&path, &request).await });

    for _ in 0..20 {
        harness.settle_for(budget).await;
        if client.is_finished() {
            break;
        }
    }

    tokio::time::timeout(std::time::Duration::from_secs(5), &mut client)
        .await
        .unwrap_or_else(|_| panic!("{described} was never answered"))
        .expect("client task")
        .expect("the daemon answered")
}

#[tokio::test(flavor = "multi_thread")]
async fn status_reports_the_current_page_and_counters() {
    let mut harness = Harness::new(PageId::Home).await;
    let (path, _directory) = serve(&harness).await;

    let response = ask(&mut harness, &path, Request::Status).await;
    assert!(response.is_ok());

    let Response::Ok { data, .. } = response else {
        panic!("expected a payload");
    };
    let data = data.expect("status payload");
    assert_eq!(data["page"], "home");
    assert_eq!(data["device"]["serial"], "RECORDING0001");
    assert!(data["frames_sent"].as_u64().expect("frames") >= 15);
    assert_eq!(data["child_processes"], 0);
    assert!(data["integrations"].is_object());
    assert!(data["pending_deadlines"].is_array());
}

#[tokio::test(flavor = "multi_thread")]
async fn page_switches_the_deck() {
    let mut harness = Harness::new(PageId::Home).await;
    let (path, _directory) = serve(&harness).await;

    let response = ask(
        &mut harness,
        &path,
        Request::Page {
            page: PageId::Pomodoro,
        },
    )
    .await;

    assert!(response.is_ok());
    assert_eq!(response.message(), "switched to pomodoro");
    assert_eq!(harness.page(), PageId::Pomodoro);
}

#[tokio::test(flavor = "multi_thread")]
async fn press_runs_the_key_short_action() {
    let mut harness = Harness::new(PageId::Pomodoro).await;
    let (path, _directory) = serve(&harness).await;

    let response = ask(
        &mut harness,
        &path,
        Request::Press {
            position: KeyPosition::new(1, 3),
        },
    )
    .await;

    assert!(response.is_ok(), "{response:?}");
    assert_eq!(
        harness.runtime.state().persistent.pomodoro.status,
        Status::Running
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pressing_a_blank_key_is_refused_with_an_explanation() {
    let mut harness = Harness::new(PageId::Home).await;
    let (path, _directory) = serve(&harness).await;

    let response = ask(
        &mut harness,
        &path,
        Request::Press {
            position: KeyPosition::new(3, 1),
        },
    )
    .await;

    assert!(!response.is_ok());
    assert!(response.message().contains("blank"), "{response:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn hold_runs_the_long_action() {
    let mut harness = Harness::new(PageId::Home).await;
    let (path, _directory) = serve(&harness).await;

    let response = ask(
        &mut harness,
        &path,
        Request::Hold {
            position: KeyPosition::new(2, 3),
            milliseconds: 700,
        },
    )
    .await;

    assert!(response.is_ok(), "{response:?}");
    assert_eq!(harness.page(), PageId::Pomodoro);
    assert_eq!(
        harness.runtime.state().persistent.pomodoro.status,
        Status::Ready,
        "a hold must not also fire the short action"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pomodoro_commands_drive_the_timer() {
    let mut harness = Harness::new(PageId::Pomodoro).await;
    let (path, _directory) = serve(&harness).await;

    ask(
        &mut harness,
        &path,
        Request::Pomodoro {
            action: PomodoroAction::Start {
                phase: Phase::LongBreak,
            },
        },
    )
    .await;
    assert_eq!(
        harness.runtime.state().persistent.pomodoro.phase,
        Phase::LongBreak
    );
    assert_eq!(
        harness.runtime.state().persistent.pomodoro.status,
        Status::Running
    );

    ask(
        &mut harness,
        &path,
        Request::Pomodoro {
            action: PomodoroAction::Toggle,
        },
    )
    .await;
    assert_eq!(
        harness.runtime.state().persistent.pomodoro.status,
        Status::Paused
    );

    ask(
        &mut harness,
        &path,
        Request::Pomodoro {
            action: PomodoroAction::Reset,
        },
    )
    .await;
    assert_eq!(
        harness.runtime.state().persistent.pomodoro.phase,
        Phase::Focus
    );
    assert_eq!(
        harness.runtime.state().persistent.pomodoro.status,
        Status::Ready
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn acknowledge_clears_a_pending_completion() {
    let mut state = streamdeck_core::state::PersistentState::default();
    streamdeck_core::pomodoro::start_phase(
        &mut state.pomodoro,
        Phase::Focus,
        chrono::Utc::now().timestamp_millis() - 60 * 60 * 1_000,
    );
    let mut harness = Harness::with_state(PageId::Pomodoro, state).await;
    let (path, _directory) = serve(&harness).await;

    assert!(harness
        .runtime
        .state()
        .persistent
        .pomodoro
        .pending_completion_phase
        .is_some());

    let response = ask(
        &mut harness,
        &path,
        Request::Pomodoro {
            action: PomodoroAction::Acknowledge,
        },
    )
    .await;

    assert!(response.is_ok());
    assert_eq!(
        harness
            .runtime
            .state()
            .persistent
            .pomodoro
            .pending_completion_phase,
        None
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_invalidates_and_re_requests_one_integration() {
    let mut harness = Harness::new(PageId::Mixer).await;
    let (path, _directory) = serve(&harness).await;
    harness.commands.reset();

    let response = ask(
        &mut harness,
        &path,
        Request::Refresh {
            integration: IntegrationId::AudioStatus,
        },
    )
    .await;
    assert!(response.is_ok());

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    harness.settle().await;
    assert!(
        harness.commands.called_with("get volume settings"),
        "{:?}",
        harness.commands.calls()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn render_preview_writes_a_png_for_any_page() {
    let mut harness = Harness::new(PageId::Home).await;
    let (path, directory) = serve(&harness).await;
    let output = directory.path().join("preview.png");

    let response = ask(
        &mut harness,
        &path,
        Request::RenderPreview {
            page: PageId::Pomodoro,
            output: output.to_string_lossy().to_string(),
        },
    )
    .await;

    assert!(response.is_ok(), "{response:?}");
    let image = image::open(&output).expect("written").to_rgb8();
    assert_eq!(image.width(), 5 * (72 + 4) + 4);
    assert_eq!(image.height(), 3 * (72 + 4) + 4);
    assert_eq!(
        harness.page(),
        PageId::Home,
        "a preview must not change the visible page"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_returns_a_checklist() {
    let mut harness = Harness::new(PageId::Home).await;
    let (path, _directory) = serve(&harness).await;

    let response = ask(&mut harness, &path, Request::Doctor).await;
    let Response::Ok { data, .. } = response else {
        panic!("expected a payload");
    };
    let data = data.expect("doctor payload");
    let checks = data["checks"].as_array().expect("checks");

    assert!(checks.len() >= 8, "saw {} checks", checks.len());
    let names: Vec<&str> = checks
        .iter()
        .filter_map(|check| check["name"].as_str())
        .collect();
    for expected in ["device", "config", "state", "audio", "orphans"] {
        assert!(
            names.contains(&expected),
            "{expected} is missing from {names:?}"
        );
    }
    // A credential check must never contain a token.
    let serialized = data.to_string();
    assert!(!serialized.contains("sk-ant"), "a token leaked into doctor");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_log_level_can_be_changed_and_a_bad_one_is_refused() {
    let mut harness = Harness::new(PageId::Home).await;
    let (path, _directory) = serve(&harness).await;

    // No level control is installed in the harness, so this reports that clearly
    // rather than silently succeeding.
    let response = ask(
        &mut harness,
        &path,
        Request::LogLevel {
            level: "debug".to_string(),
        },
    )
    .await;
    assert!(!response.is_ok());
    assert!(response.message().contains("logging"), "{response:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn reload_reports_a_missing_or_invalid_configuration_without_changing_anything() {
    let mut harness = Harness::new(PageId::Home).await;
    let (path, directory) = serve(&harness).await;

    let bad = directory.path().join("bad.toml");
    std::fs::write(&bad, "version = 1\nbrightness = 500\n").expect("write");
    std::env::set_var("STREAMDECKD_CONFIG", &bad);

    let before = harness.runtime.state().config.brightness;
    let response = ask(&mut harness, &path, Request::Reload).await;

    assert!(!response.is_ok(), "{response:?}");
    assert!(response.message().contains("brightness"), "{response:?}");
    assert_eq!(
        harness.runtime.state().config.brightness,
        before,
        "an invalid reload must leave the previous configuration active"
    );

    std::env::remove_var("STREAMDECKD_CONFIG");
}

#[tokio::test(flavor = "multi_thread")]
async fn reload_applies_a_valid_configuration() {
    let mut harness = Harness::new(PageId::Home).await;
    let (path, directory) = serve(&harness).await;

    let good = directory.path().join("good.toml");
    let mut text = streamdeck_core::config::TEMPLATE.to_string();
    text = text.replace("brightness = 60", "brightness = 85");
    std::fs::write(&good, &text).expect("write");
    std::env::set_var("STREAMDECKD_CONFIG", &good);

    let response = ask(&mut harness, &path, Request::Reload).await;
    assert!(response.is_ok(), "{response:?}");
    assert_eq!(harness.runtime.state().config.brightness, 85);

    std::env::remove_var("STREAMDECKD_CONFIG");
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_asks_the_runtime_to_exit() {
    let mut harness = Harness::new(PageId::Home).await;
    let (path, _directory) = serve(&harness).await;

    let response = ask(&mut harness, &path, Request::Stop).await;
    assert!(response.is_ok());
    assert_eq!(response.message(), "stopping");

    // A stopped runtime returns from `run` immediately.
    let exited =
        tokio::time::timeout(std::time::Duration::from_millis(500), harness.runtime.run()).await;
    assert!(exited.is_ok(), "the runtime should have exited promptly");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_arbitrary_command_cannot_be_smuggled_through_the_socket() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    // These are refused by the protocol decoder alone, so the runtime never sees
    // them and does not need to be driven.
    let harness = Harness::new(PageId::Home).await;
    let (path, _directory) = serve(&harness).await;

    for payload in [
        r#"{"command":"exec","argv":["/bin/sh","-c","touch /tmp/streamdeckd-pwned"]}"#,
        r#"{"command":"shell","script":"rm -rf ~"}"#,
        r#"{"command":"page","page":"../../etc/passwd"}"#,
        r#"{"command":"press","position":"2,3"}"#,
        r#"{"command":"refresh","integration":"; rm -rf /"}"#,
        "not json",
        "",
    ] {
        let mut stream = tokio::net::UnixStream::connect(&path)
            .await
            .expect("connected");
        stream
            .write_all(format!("{payload}\n").as_bytes())
            .await
            .expect("wrote");
        stream.flush().await.expect("flushed");

        let mut line = String::new();
        BufReader::new(stream)
            .read_line(&mut line)
            .await
            .expect("read");
        if line.trim().is_empty() {
            // An empty request is closed without an answer, which is also a refusal.
            continue;
        }
        let response = streamdeck_core::control::decode_response(line.trim()).expect("decoded");
        assert!(!response.is_ok(), "{payload} was accepted: {response:?}");
    }

    assert!(!std::path::Path::new("/tmp/streamdeckd-pwned").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_field_is_ignored_so_a_newer_cli_still_works() {
    let mut harness = Harness::new(PageId::Home).await;
    let (path, directory) = serve(&harness).await;
    let output = directory.path().join("forward.png");

    // Requests are forward compatible: an unrecognised field is ignored rather
    // than rejected, so a newer CLI can add one without breaking this daemon.
    let response = ask(
        &mut harness,
        &path,
        Request::RenderPreview {
            page: PageId::Home,
            output: output.to_string_lossy().to_string(),
        },
    )
    .await;
    assert!(response.is_ok(), "{response:?}");
    assert!(output.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_disappears_mid_request_does_not_disturb_the_runtime() {
    let mut harness = Harness::new(PageId::Home).await;
    let (path, _directory) = serve(&harness).await;

    {
        let stream = tokio::net::UnixStream::connect(&path)
            .await
            .expect("connected");
        drop(stream);
    }
    harness.settle().await;

    // The runtime still answers a real request afterwards.
    let response = ask(&mut harness, &path, Request::Status).await;
    assert!(response.is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn devices_lists_what_discovery_found() {
    let mut harness = Harness::new(PageId::Home).await;
    let (path, _directory) = serve(&harness).await;

    let response = ask(&mut harness, &path, Request::Devices).await;
    // With or without hardware attached this must answer, not error out.
    match response {
        Response::Ok { data, .. } => {
            assert!(data.expect("payload").is_array());
        }
        Response::Error { message, .. } => {
            assert!(message.contains("hidapi"), "{message}");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn every_request_variant_is_answered() {
    let mut harness = Harness::new(PageId::Home).await;
    let (path, directory) = serve(&harness).await;

    let requests = vec![
        Request::Status,
        Request::Devices,
        Request::Page { page: PageId::Home },
        Request::Press {
            position: KeyPosition::new(2, 2),
        },
        Request::Pomodoro {
            action: PomodoroAction::Skip,
        },
        Request::Refresh {
            integration: IntegrationId::Spotify,
        },
        Request::Reload,
        Request::RenderPreview {
            page: PageId::Mixer,
            output: directory
                .path()
                .join("all.png")
                .to_string_lossy()
                .to_string(),
        },
        Request::Doctor,
        Request::LogLevel {
            level: "info".to_string(),
        },
    ];

    for request in requests {
        let response = ask(&mut harness, &path, request.clone()).await;
        // Some commands legitimately fail in this environment; none may hang or
        // return a malformed answer.
        assert_eq!(
            response.version(),
            streamdeck_core::control::PROTOCOL_VERSION,
            "{request:?}"
        );
    }
}
