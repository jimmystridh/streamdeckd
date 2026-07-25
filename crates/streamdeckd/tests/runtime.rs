//! End-to-end runtime tests.
//!
//! Everything here drives the real coordinator through the recording device: page
//! navigation, press semantics, the temporary panel, unchanged-frame suppression,
//! Pomodoro durability across a restart, sleep crossing a deadline, and clean
//! shutdown.

mod harness;

use std::sync::Arc;

use chrono::Utc;
use harness::Harness;
use streamdeck_core::model::{KeyPosition, PageId};
use streamdeck_core::pomodoro::{self, Phase, PomodoroState, Status};
use streamdeck_core::state::PersistentState;
use streamdeckd::device::recording::Sent;
use streamdeckd::device::DeviceError;
use streamdeckd::runtime::RuntimeEvent;

#[tokio::test(flavor = "multi_thread")]
async fn starting_paints_every_key_once() {
    let harness = Harness::new(PageId::Home).await;

    let keys = harness.device.keys_sent();
    assert_eq!(keys.len(), 15, "the whole deck should be painted");
    let unique: std::collections::HashSet<_> = keys.iter().collect();
    assert_eq!(unique.len(), 15, "each key exactly once");
    assert!(
        harness.device.flushes() >= 1,
        "buffered images must be flushed to the glass, or nothing ever appears"
    );
    assert!(harness
        .device
        .sent()
        .iter()
        .any(|entry| matches!(entry, Sent::Brightness(60))));
}

#[tokio::test(flavor = "multi_thread")]
async fn repainting_an_unchanged_page_writes_nothing_to_the_device() {
    let mut harness = Harness::new(PageId::Pomodoro).await;
    harness.device.reset();

    // A finished no-op action triggers a full repaint of an unchanged page.
    for _ in 0..3 {
        harness
            .events
            .send(RuntimeEvent::ActionFinished(Default::default()))
            .expect("sent");
    }
    harness.settle().await;

    let writes = harness
        .device
        .sent()
        .into_iter()
        .filter(|entry| matches!(entry, Sent::Key { .. }))
        .count();
    assert_eq!(
        writes, 0,
        "an unchanged deck must produce zero USB writes, got {writes}"
    );
    assert_eq!(
        harness.device.flushes(),
        0,
        "no writes means no flush; an idle repaint stays free"
    );
    assert!(
        harness.runtime.metrics().frames_skipped >= 45,
        "three repaints of fifteen keys should all be skipped, saw {}",
        harness.runtime.metrics().frames_skipped
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pressed_feedback_writes_the_key_down_and_back_up_again() {
    let mut harness = Harness::new(PageId::Pomodoro).await;
    harness.device.reset();

    // A statistics tile changes no state, so the only writes are the pressed
    // treatment appearing and then clearing.
    harness.press(3, 4).await;

    let writes: Vec<KeyPosition> = harness.device.keys_sent();
    assert_eq!(writes, vec![KeyPosition::new(3, 4), KeyPosition::new(3, 4)]);
    assert!(
        harness.device.flushes() >= 2,
        "pressed feedback must be flushed immediately, not left buffered"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_short_press_navigates_and_a_long_press_opens_the_other_page() {
    let mut harness = Harness::new(PageId::Home).await;

    // 2,2 is the GitHub summary: a short press navigates.
    harness.press(2, 2).await;
    assert_eq!(harness.page(), PageId::GitHub);

    // 1,1 on GitHub is Home.
    harness.press(1, 1).await;
    assert_eq!(harness.page(), PageId::Home);

    // 2,3 is the Pomodoro glance: holding it opens the Pomodoro page.
    harness.hold(2, 3).await;
    assert_eq!(harness.page(), PageId::Pomodoro);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_press_does_not_also_fire_the_short_action() {
    let mut harness = Harness::new(PageId::Home).await;

    // A short press on 2,3 toggles the timer; a long press must not.
    harness.hold(2, 3).await;

    assert_eq!(harness.page(), PageId::Pomodoro);
    assert_eq!(
        harness.runtime.state().persistent.pomodoro.status,
        Status::Ready,
        "the timer must not have started"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_armed_affordance_reaches_the_deck_before_the_page_changes() {
    let mut harness = Harness::new(PageId::Home).await;
    harness.device.reset();

    let position = KeyPosition::new(2, 3);
    harness
        .events
        .send(RuntimeEvent::Key(streamdeckd::device::KeyEvent::Down(
            position,
        )))
        .expect("sent");
    harness.settle().await;
    let after_press = harness.device.keys_sent().len();
    assert_eq!(after_press, 1, "only the pressed key repaints immediately");

    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    harness.settle().await;

    // Arming repaints the key again, and only then does the page change.
    let armed_writes = harness
        .device
        .keys_sent()
        .into_iter()
        .filter(|sent| *sent == position)
        .count();
    assert!(
        armed_writes >= 2,
        "the armed key should have been repainted, saw {armed_writes} writes"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_temporary_panel_opens_and_returns_to_the_page_it_came_from() {
    let mut harness = Harness::new(PageId::Home).await;

    // 3,5 on Home is the water tile, which opens the Stensjön panel.
    harness.press(3, 5).await;
    assert_eq!(harness.page(), PageId::Stensjon);
    assert!(harness.runtime.state().navigator.panel_is_open());

    // 1,1 on the panel dismisses it immediately.
    harness.press(1, 1).await;
    assert_eq!(harness.page(), PageId::Home);
    assert!(!harness.runtime.state().navigator.panel_is_open());
}

#[tokio::test(flavor = "multi_thread")]
async fn interacting_with_the_panel_restarts_its_timeout() {
    let mut harness = Harness::new(PageId::Home).await;
    harness.press(3, 5).await;

    let first = harness
        .runtime
        .state()
        .navigator
        .panel_deadline_ms()
        .expect("panel is open");

    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    // A history tile press is a no-op action, but it still touches the panel.
    harness.press(2, 1).await;

    let second = harness
        .runtime
        .state()
        .navigator
        .panel_deadline_ms()
        .expect("panel is still open");
    assert!(second > first, "{second} should be later than {first}");
    assert_eq!(harness.page(), PageId::Stensjon);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pomodoro_press_starts_the_timer_and_persists_it_immediately() {
    let mut harness = Harness::new(PageId::Pomodoro).await;

    // 1,3 is start/pause.
    harness.press(1, 3).await;
    assert_eq!(
        harness.runtime.state().persistent.pomodoro.status,
        Status::Running
    );

    let reloaded = harness
        .store
        .load(PomodoroState::default())
        .expect("state was written");
    assert_eq!(reloaded.pomodoro.status, Status::Running);
    assert!(reloaded.pomodoro.ends_at_ms.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_timer_survives_a_daemon_restart() {
    let mut first = Harness::new(PageId::Pomodoro).await;
    first.press(1, 3).await;
    let ends_at = first
        .runtime
        .state()
        .persistent
        .pomodoro
        .ends_at_ms
        .expect("running");
    let persisted = first.store.load(PomodoroState::default()).expect("loaded");
    drop(first);

    let second = Harness::with_state(PageId::Pomodoro, persisted).await;
    assert_eq!(
        second.runtime.state().persistent.pomodoro.status,
        Status::Running
    );
    assert_eq!(
        second.runtime.state().persistent.pomodoro.ends_at_ms,
        Some(ends_at)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deadline_crossed_while_the_daemon_was_not_running_completes_exactly_once() {
    // A timer that expired an hour ago.
    let mut state = PersistentState::default();
    pomodoro::start_phase(
        &mut state.pomodoro,
        Phase::Focus,
        Utc::now().timestamp_millis() - 60 * 60 * 1_000,
    );

    let harness = Harness::with_state(PageId::Pomodoro, state).await;
    let timer = &harness.runtime.state().persistent.pomodoro;

    assert_eq!(timer.completed_focus_sessions, 1);
    assert_eq!(timer.pending_completion_phase, Some(Phase::Focus));
    assert_eq!(timer.phase, Phase::ShortBreak);
    assert_eq!(timer.status, Status::Ready);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_completion_alert_is_cleared_by_the_next_press_from_the_deck() {
    let mut state = PersistentState::default();
    pomodoro::start_phase(
        &mut state.pomodoro,
        Phase::Focus,
        Utc::now().timestamp_millis() - 60 * 60 * 1_000,
    );

    let mut harness = Harness::with_state(PageId::Pomodoro, state).await;
    assert!(harness
        .runtime
        .state()
        .persistent
        .pomodoro
        .pending_completion_phase
        .is_some());

    // Any interactive press acknowledges the completion.
    harness.press(1, 3).await;
    assert_eq!(
        harness
            .runtime
            .state()
            .persistent
            .pomodoro
            .pending_completion_phase,
        None
    );
    assert!(!harness.runtime.state().alert_flashing);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_completion_plays_the_configured_sound_and_posts_a_notification() {
    let mut state = PersistentState::default();
    pomodoro::start_phase(
        &mut state.pomodoro,
        Phase::Focus,
        Utc::now().timestamp_millis() - 60 * 60 * 1_000,
    );

    let harness = Harness::with_state(PageId::Pomodoro, state).await;
    assert!(
        harness
            .commands
            .called_with("display notification \"Focus complete."),
        "{:?}",
        harness.commands.calls()
    );
    assert!(harness
        .commands
        .called_with("/System/Library/Sounds/Glass.aiff"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wake_reconciles_a_timer_whose_deadline_passed_during_sleep() {
    let mut harness = Harness::new(PageId::Pomodoro).await;
    harness.press(1, 3).await;
    assert_eq!(
        harness.runtime.state().persistent.pomodoro.status,
        Status::Running
    );

    // Move the deadline into the past, as a long sleep would, and wake.
    harness.runtime.state_mut().persistent.pomodoro.ends_at_ms =
        Some(Utc::now().timestamp_millis() - 1_000);
    harness.events.send(RuntimeEvent::SystemWoke).expect("sent");
    harness.settle().await;

    let timer = &harness.runtime.state().persistent.pomodoro;
    assert_eq!(timer.completed_focus_sessions, 1);
    assert_eq!(timer.pending_completion_phase, Some(Phase::Focus));
    assert_eq!(harness.runtime.metrics().wakes, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn duration_keys_add_on_a_short_press_and_subtract_on_a_long_press() {
    let mut harness = Harness::new(PageId::Pomodoro).await;
    assert_eq!(
        harness.runtime.state().persistent.pomodoro.focus_minutes,
        25
    );

    // 3,1 adjusts the focus length by five minutes.
    harness.press(3, 1).await;
    assert_eq!(
        harness.runtime.state().persistent.pomodoro.focus_minutes,
        30
    );

    harness.hold(3, 1).await;
    assert_eq!(
        harness.runtime.state().persistent.pomodoro.focus_minutes,
        25
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mixer_press_selects_the_configured_device_through_the_adapter() {
    let mut harness = Harness::new(PageId::Mixer).await;
    harness.commands.reset();

    // 1,3 is the Bose output.
    harness.press(1, 3).await;
    // Give the spawned task a moment to run and report back.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    harness.settle().await;

    assert!(
        harness
            .commands
            .called_with("-s Bose NC 700 Headphones -t output"),
        "{:?}",
        harness.commands.calls()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_volume_press_reads_the_level_then_sets_the_new_one() {
    let mut harness = Harness::new(PageId::Mixer).await;
    harness.commands.reset();

    // 2,2 is output volume up by ten.
    harness.press(2, 2).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    harness.settle().await;

    assert!(
        harness.commands.called_with("set volume output volume 52"),
        "{:?}",
        harness.commands.calls()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_weather_press_shows_the_detail_card_and_reverts_on_its_own() {
    let mut harness = Harness::new(PageId::Home).await;
    harness.device.reset();

    // 3,3 is the current-weather tile.
    harness.press(3, 3).await;
    assert!(
        matches!(
            harness.runtime.state().weather_detail,
            Some((streamdeck_core::model::WeatherTile::Current, _))
        ),
        "the press should open the detail window"
    );
    assert!(
        !harness.device.keys_sent().is_empty(),
        "the flipped tile must reach the deck"
    );

    // The revert deadline fires six seconds later and repaints on its own.
    harness.settle_for(std::time::Duration::from_secs(7)).await;
    assert_eq!(harness.runtime.state().weather_detail, None);
    assert_eq!(
        harness
            .runtime
            .state()
            .deadlines
            .get(streamdeck_core::deadline::DeadlineId::WeatherDetail),
        None
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn fetched_artwork_is_cached_and_triggers_a_repaint() {
    let mut harness = Harness::new(PageId::Home).await;
    let mut encoded = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        64,
        64,
        image::Rgb([180, 40, 90]),
    ))
    .write_to(
        &mut std::io::Cursor::new(&mut encoded),
        image::ImageFormat::Png,
    )
    .expect("encoded");

    harness
        .events
        .send(RuntimeEvent::ArtworkFetched {
            key: "spotify:track:1".to_string(),
            result: Ok(encoded),
        })
        .expect("sent");
    harness.settle().await;

    assert!(harness.runtime.artwork_cached("spotify:track:1"));

    // Corrupt bytes are rejected without disturbing anything.
    harness
        .events
        .send(RuntimeEvent::ArtworkFetched {
            key: "spotify:track:2".to_string(),
            result: Ok(b"not an image".to_vec()),
        })
        .expect("sent");
    harness.settle().await;
    assert!(!harness.runtime.artwork_cached("spotify:track:2"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blank_key_press_changes_nothing() {
    let mut harness = Harness::new(PageId::Home).await;
    harness.device.reset();
    let before = harness.runtime.state().persistent.clone();

    // 3,1 on Home is intentionally blank.
    harness.press(3, 1).await;

    assert_eq!(harness.page(), PageId::Home);
    assert_eq!(harness.runtime.state().persistent, before);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_disconnect_stops_rendering_and_a_reconnect_repaints_everything() {
    let mut harness = Harness::new(PageId::Pomodoro).await;

    harness
        .events
        .send(RuntimeEvent::DeviceDisconnected)
        .expect("sent");
    harness.settle().await;
    harness.device.reset();

    // While disconnected, presses must not reach the device.
    harness.press(1, 3).await;
    assert!(
        harness.device.keys_sent().is_empty(),
        "nothing should be written while disconnected"
    );
    // Domain state still advances.
    assert_eq!(
        harness.runtime.state().persistent.pomodoro.status,
        Status::Running
    );

    harness
        .runtime
        .attach_device(Arc::clone(&harness.device) as Arc<dyn streamdeckd::device::DeckDevice>);
    harness
        .events
        .send(RuntimeEvent::DeviceReconnected)
        .expect("sent");
    harness.settle().await;

    assert_eq!(
        harness.device.keys_sent().len(),
        15,
        "a reconnect repaints the whole deck"
    );
    assert_eq!(harness.runtime.metrics().device_reconnects, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_that_fails_mid_write_does_not_stop_the_daemon() {
    let mut harness = Harness::new(PageId::Pomodoro).await;
    harness.device.start_failing(DeviceError::Disconnected);

    harness.press(1, 3).await;

    // The press still took effect even though the device refused every write.
    assert_eq!(
        harness.runtime.state().persistent.pomodoro.status,
        Status::Running
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_persists_state_closes_the_device_and_leaves_no_children() {
    let mut harness = Harness::new(PageId::Pomodoro).await;
    harness.press(1, 3).await;
    harness.runtime.shutdown().await;

    assert!(
        harness
            .device
            .sent()
            .iter()
            .any(|entry| matches!(entry, Sent::Closed)),
        "the device must be closed"
    );
    let reloaded = harness
        .store
        .load(PomodoroState::default())
        .expect("loaded");
    assert_eq!(reloaded.pomodoro.status, Status::Running);
    assert_eq!(
        streamdeck_macos::CommandRunner::running(harness.commands.as_ref()),
        0,
        "no child process may outlive the daemon"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_preserves_the_last_frame_unless_configured_to_blank() {
    let mut harness = Harness::new(PageId::Pomodoro).await;
    harness.runtime.shutdown().await;

    assert!(
        !harness
            .device
            .sent()
            .iter()
            .any(|entry| matches!(entry, Sent::Cleared)),
        "blank_on_exit is false in the template, so the frame is preserved"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn every_page_can_be_reached_and_rendered_from_home() {
    let mut harness = Harness::new(PageId::Home).await;

    for (page, presses) in [
        (PageId::Mixer, vec![(1u8, 1u8)]),
        (PageId::GitHub, vec![(2, 2)]),
        (PageId::Stensjon, vec![(3, 5)]),
    ] {
        // Return to Home first.
        while harness.page() != PageId::Home {
            harness.press(1, 1).await;
        }
        harness.device.reset();
        let renders_before = harness.runtime.metrics().renders;
        for (row, column) in presses {
            harness.press(row, column).await;
        }
        assert_eq!(harness.page(), page);
        assert!(
            harness.runtime.metrics().renders >= renders_before + 15,
            "{page} should compose all fifteen keys"
        );
        // Keys whose image is already correct are deliberately not resent, so
        // only the ones that actually changed appear as writes.
        assert!(
            !harness.device.keys_sent().is_empty(),
            "{page} should have changed at least one key"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn navigation_persists_the_page_so_a_restart_returns_to_it() {
    let mut first = Harness::new(PageId::Home).await;
    first.press(2, 2).await;
    assert_eq!(first.page(), PageId::GitHub);
    let persisted = first.store.load(PomodoroState::default()).expect("loaded");
    drop(first);

    let second = Harness::with_state(persisted.active_page, persisted).await;
    assert_eq!(second.page(), PageId::GitHub);
}
