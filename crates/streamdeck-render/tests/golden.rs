//! Golden image suite for the key renderer.
//!
//! Each scenario renders one page from a fixed world view and compares the
//! composed sheet against a stored PNG. Set `UPDATE_GOLDEN=1` to rewrite the
//! stored images; a golden change is expected to be reviewed as part of a diff.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use chrono_tz::Europe::Stockholm;
use streamdeck_core::config::AudioTargetConfig;
use streamdeck_core::integrations::audio::{
    AudioInventory, AudioSnapshot, AudioStatus, AudioTarget,
};
use streamdeck_core::integrations::claude::{ClaudeUsage, UsageWindow};
use streamdeck_core::integrations::codex::{CodexUsage, CodexWindow};
use streamdeck_core::integrations::github::{parse_search, GitHubSnapshot};
use streamdeck_core::integrations::lake::{parse_current, parse_history};
use streamdeck_core::integrations::meetings::Meeting;
use streamdeck_core::integrations::spotify::{parse_status, SpotifyStatus};
use streamdeck_core::integrations::weather::parse_forecast;
use streamdeck_core::model::{Grid, PageId};
use streamdeck_core::pages::views::{render, RenderContext};
use streamdeck_core::pages::{full_page, Tile};
use streamdeck_core::pomodoro::{self, Phase, PomodoroState};
use streamdeck_core::snapshot::{Feed, WorldView};
use streamdeck_render::{Renderer, KEY_SIZE};

const GITHUB: &str = include_str!("../../../tests/fixtures/github-search-prs.json");
const MET: &str = include_str!("../../../tests/fixtures/met-locationforecast.json");
const LAKE_CURRENT: &str = include_str!("../../../tests/fixtures/lake-current.json");
const LAKE_HISTORY: &str = include_str!("../../../tests/fixtures/lake-historic.json");
const STENSJON: &str = "A84041BDC1864B41";

/// Pixel differences this large are treated as a real change. Anti-aliasing can
/// wobble by a hair between platforms; a layout change never does.
const TOLERANCE: u8 = 6;
/// The fraction of pixels allowed to differ by more than `TOLERANCE`.
const MAX_DIFFERING_FRACTION: f64 = 0.002;

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-07-24T10:00:00Z")
        .expect("timestamp")
        .with_timezone(&Utc)
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden")
        .canonicalize()
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden"))
}

fn targets(specs: &[(&str, Option<&str>, Option<&str>)]) -> Vec<AudioTarget> {
    specs
        .iter()
        .map(|(label, exact, pattern)| {
            AudioTarget::from_config(&AudioTargetConfig {
                label: label.to_string(),
                exact: exact.map(str::to_string),
                pattern: pattern.map(str::to_string),
            })
            .expect("valid target")
        })
        .collect()
}

fn output_targets() -> Vec<AudioTarget> {
    targets(&[
        ("MacBook", Some("MacBook Pro Speakers"), None),
        ("Bose", Some("Bose NC 700 Headphones"), None),
        ("USB Home", None, Some("usb")),
    ])
}

fn input_targets() -> Vec<AudioTarget> {
    targets(&[
        ("MacBook Mic", Some("MacBook Pro Microphone"), None),
        ("Bose Mic", Some("Bose NC 700 Headphones"), None),
        ("RØDE Mic", None, Some("røde|rode")),
    ])
}

/// A world view with every integration healthy, matching the fixtures.
fn healthy() -> WorldView {
    let mut world = WorldView::empty(now(), 1_000, Stockholm);
    world.location_name = "Stensjön".to_string();
    world.repository_aliases = vec![("visma.administration.".to_string(), "admin.".to_string())];

    world.audio = Feed::Ready(AudioSnapshot {
        status: Some(AudioStatus {
            current_output: "Bose NC 700 Headphones".to_string(),
            current_input: "MacBook Pro Microphone".to_string(),
            output_volume: 42,
            input_volume: 75,
            output_muted: false,
        }),
        inventory: AudioInventory {
            outputs: vec![
                "MacBook Pro Speakers".to_string(),
                "Bose NC 700 Headphones".to_string(),
            ],
            inputs: vec!["MacBook Pro Microphone".to_string()],
        },
    });

    world.meetings = Feed::Ready(vec![
        Meeting {
            account: "jimmy.stridh@visma.com".to_string(),
            title: "Sprint planning".to_string(),
            start: now() + Duration::minutes(42),
            end: now() + Duration::minutes(102),
            meet_url: "https://meet.google.com/aaa-bbbb-ccc".to_string(),
        },
        Meeting {
            account: "jimmy.stridh@gmail.com".to_string(),
            title: "Architecture review".to_string(),
            start: now() + Duration::hours(26),
            end: now() + Duration::hours(27),
            meet_url: "https://meet.google.com/ddd-eeee-fff".to_string(),
        },
    ]);

    world.weather = Feed::Ready(parse_forecast(MET, "Stensjön", Stockholm).expect("weather"));
    world.lake_current = Feed::Ready(
        parse_current(LAKE_CURRENT, STENSJON, now() + Duration::days(1)).expect("lake"),
    );
    world.lake_history = Feed::Ready(
        parse_history(LAKE_HISTORY, STENSJON, now() + Duration::days(1)).expect("history"),
    );

    let prs = parse_search(GITHUB, 100).expect("prs");
    world.github = Feed::Ready(GitHubSnapshot {
        reviews: prs.iter().take(3).cloned().collect(),
        prs: prs.clone(),
        assigned: prs.iter().take(2).cloned().collect(),
        inbox_count: 17,
        inbox_overflow: false,
        updated_since: "2026-06-24".to_string(),
    });

    world.claude = Feed::Ready(ClaudeUsage {
        five_hour: Some(UsageWindow {
            percent: 12.0,
            resets_at: Some(now() + Duration::minutes(150)),
        }),
        seven_day: Some(UsageWindow {
            percent: 33.0,
            resets_at: Some(now() + Duration::hours(20)),
        }),
    });
    world.codex = Feed::Ready(CodexUsage {
        plan: Some("pro".to_string()),
        primary: Some(CodexWindow {
            percent: 43.0,
            window_seconds: 604_800,
            resets_at: Some(now() + Duration::days(4)),
        }),
        secondary: None,
        limit_reached: false,
    });
    world.spotify = Feed::Ready(
        parse_status(
            "playing\tTruth\tKamasi Washington\tThe Epic\thttps://i.scdn.co/image/abc\tspotify:track:1\t72\ttrue\tall",
        )
        .expect("spotify"),
    );

    world.panel_total_seconds = 10;
    world
}

/// Composes a whole page into one 5x3 sheet with a one-pixel gutter, so a golden
/// review shows the page the way the hardware does.
fn sheet(renderer: &mut Renderer, page: PageId, world: &WorldView) -> image::RgbImage {
    let output = output_targets();
    let input = input_targets();
    let context = RenderContext::new(world).with_audio(&output, &input);
    let grid = Grid::MK2;
    let gutter = 2u32;
    let width = grid.columns as u32 * (KEY_SIZE + gutter) + gutter;
    let height = grid.rows as u32 * (KEY_SIZE + gutter) + gutter;
    let mut canvas = image::RgbImage::from_pixel(width, height, image::Rgb([20, 22, 28]));

    for binding in full_page(page, grid) {
        let mut view = render(binding.tile, &context);
        // Show the long-press affordance wherever a page defines one.
        if binding.has_long_action() && page == PageId::Home && binding.position.column == 3 {
            view.armed = true;
        }
        let key = renderer.render(&view).expect("rendered");
        let image = key.to_image().expect("image");
        let x = gutter + (binding.position.column as u32 - 1) * (KEY_SIZE + gutter);
        let y = gutter + (binding.position.row as u32 - 1) * (KEY_SIZE + gutter);
        image::imageops::replace(&mut canvas, &image, i64::from(x), i64::from(y));
    }
    canvas
}

/// Composes an explicit list of tiles into one row, for state-specific scenarios.
fn strip(renderer: &mut Renderer, tiles: &[Tile], world: &WorldView) -> image::RgbImage {
    let output = output_targets();
    let input = input_targets();
    let context = RenderContext::new(world).with_audio(&output, &input);
    let gutter = 2u32;
    let width = tiles.len() as u32 * (KEY_SIZE + gutter) + gutter;
    let mut canvas =
        image::RgbImage::from_pixel(width, KEY_SIZE + gutter * 2, image::Rgb([20, 22, 28]));

    for (index, tile) in tiles.iter().enumerate() {
        let view = render(*tile, &context);
        let key = renderer.render(&view).expect("rendered");
        let image = key.to_image().expect("image");
        let x = gutter + index as u32 * (KEY_SIZE + gutter);
        image::imageops::replace(&mut canvas, &image, i64::from(x), i64::from(gutter));
    }
    canvas
}

fn check(name: &str, actual: &image::RgbImage) {
    let directory = golden_dir();
    std::fs::create_dir_all(&directory).expect("golden directory");
    let path = directory.join(format!("{name}.png"));

    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        actual.save(&path).expect("write golden");
        return;
    }

    let expected = match image::open(&path) {
        Ok(image) => image.to_rgb8(),
        Err(error) => panic!(
            "missing golden {}: {error}. Run `UPDATE_GOLDEN=1 cargo test -p streamdeck-render` \
             and review the result before committing.",
            path.display()
        ),
    };

    assert_eq!(
        (expected.width(), expected.height()),
        (actual.width(), actual.height()),
        "{name}: golden is a different size"
    );

    let differing = expected
        .as_raw()
        .iter()
        .zip(actual.as_raw())
        .filter(|(before, after)| before.abs_diff(**after) > TOLERANCE)
        .count();
    let fraction = differing as f64 / expected.as_raw().len() as f64;

    if fraction > MAX_DIFFERING_FRACTION {
        let failure = directory.join(format!("{name}.actual.png"));
        actual.save(&failure).expect("write actual");
        panic!(
            "{name}: {:.3}% of channels differ (limit {:.3}%). Wrote {} for comparison.",
            fraction * 100.0,
            MAX_DIFFERING_FRACTION * 100.0,
            failure.display()
        );
    }
}

#[test]
fn home_is_healthy() {
    let mut renderer = Renderer::new().expect("renderer");
    check(
        "home-healthy",
        &sheet(&mut renderer, PageId::Home, &healthy()),
    );
}

#[test]
fn home_is_loading() {
    let mut renderer = Renderer::new().expect("renderer");
    let world = WorldView::empty(now(), 1_000, Stockholm);
    check("home-loading", &sheet(&mut renderer, PageId::Home, &world));
}

#[test]
fn home_is_stale() {
    let mut renderer = Renderer::new().expect("renderer");
    let mut world = healthy();
    world.github = Feed::Stale(world.github.value().cloned().expect("github"));
    world.weather = Feed::Stale(world.weather.value().cloned().expect("weather"));
    world.lake_current = Feed::Stale(world.lake_current.value().copied().expect("lake"));
    world.meetings = Feed::Stale(world.meetings.value().cloned().expect("meetings"));
    check("home-stale", &sheet(&mut renderer, PageId::Home, &world));
}

#[test]
fn home_has_failed() {
    let mut renderer = Renderer::new().expect("renderer");
    let mut world = healthy();
    world.github = Feed::Failed("gh: not authenticated".to_string());
    world.weather = Feed::Failed("met.no timed out".to_string());
    world.lake_current = Feed::Failed("lake api http 503".to_string());
    world.meetings = Feed::Failed("gog: token expired".to_string());
    world.claude = Feed::Failed("keychain entry missing".to_string());
    world.codex = Feed::Failed("codex auth expired".to_string());
    world.spotify = Feed::Failed("automation permission denied".to_string());
    world.audio = Feed::Failed("SwitchAudioSource missing".to_string());
    check("home-error", &sheet(&mut renderer, PageId::Home, &world));
}

#[test]
fn mixer_shows_selected_unavailable_and_ambiguous_devices() {
    let mut renderer = Renderer::new().expect("renderer");
    let mut world = healthy();
    // Two USB matches make the `usb` pattern ambiguous, and the RØDE is absent.
    if let Feed::Ready(snapshot) = &mut world.audio {
        snapshot
            .inventory
            .outputs
            .push("Scarlett 2i2 USB".to_string());
        snapshot
            .inventory
            .outputs
            .push("RØDE NT-USB Mini".to_string());
    }
    check("mixer", &sheet(&mut renderer, PageId::Mixer, &world));
}

#[test]
fn mixer_shows_a_muted_output_and_microphone() {
    let mut renderer = Renderer::new().expect("renderer");
    let mut world = healthy();
    if let Feed::Ready(snapshot) = &mut world.audio {
        if let Some(status) = &mut snapshot.status {
            status.output_muted = true;
            status.input_volume = 0;
        }
    }
    check("mixer-muted", &sheet(&mut renderer, PageId::Mixer, &world));
}

#[test]
fn github_shows_normal_counts() {
    let mut renderer = Renderer::new().expect("renderer");
    check("github", &sheet(&mut renderer, PageId::GitHub, &healthy()));
}

#[test]
fn github_shows_zero_and_capped_counts() {
    let mut renderer = Renderer::new().expect("renderer");
    let mut world = healthy();
    world.github = Feed::Ready(GitHubSnapshot {
        inbox_count: 100,
        inbox_overflow: true,
        updated_since: "2026-06-24".to_string(),
        ..Default::default()
    });
    check(
        "github-edges",
        &sheet(&mut renderer, PageId::GitHub, &world),
    );
}

#[test]
fn spotify_is_playing() {
    let mut renderer = Renderer::new().expect("renderer");
    check(
        "spotify",
        &sheet(&mut renderer, PageId::Spotify, &healthy()),
    );
}

#[test]
fn spotify_is_not_running() {
    let mut renderer = Renderer::new().expect("renderer");
    let mut world = healthy();
    world.spotify = Feed::Ready(SpotifyStatus::not_running());
    check(
        "spotify-not-running",
        &sheet(&mut renderer, PageId::Spotify, &world),
    );
}

#[test]
fn spotify_shows_artwork_when_it_is_cached() {
    let mut renderer = Renderer::new().expect("renderer");
    let mut encoded = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(120, 120, |x, y| {
        image::Rgb([(x * 2) as u8, (y * 2) as u8, 120])
    }))
    .write_to(
        &mut std::io::Cursor::new(&mut encoded),
        image::ImageFormat::Png,
    )
    .expect("encoded");
    renderer
        .cache_artwork("spotify:track:1", &encoded)
        .expect("cached");

    check(
        "spotify-artwork",
        &sheet(&mut renderer, PageId::Spotify, &healthy()),
    );
}

#[test]
fn stensjon_panel_is_healthy() {
    let mut renderer = Renderer::new().expect("renderer");
    let mut world = healthy();
    world.panel_seconds_remaining = Some(7);
    check("stensjon", &sheet(&mut renderer, PageId::Stensjon, &world));
}

#[test]
fn pomodoro_shows_every_phase() {
    let mut renderer = Renderer::new().expect("renderer");
    for (name, phase) in [
        ("focus", Phase::Focus),
        ("short-break", Phase::ShortBreak),
        ("long-break", Phase::LongBreak),
    ] {
        let mut world = healthy();
        let mut state = PomodoroState::default();
        pomodoro::start_phase(&mut state, phase, now().timestamp_millis());
        world.pomodoro = pomodoro::snapshot(&state, now().timestamp_millis() + 120_000, Stockholm);
        check(
            &format!("pomodoro-{name}"),
            &sheet(&mut renderer, PageId::Pomodoro, &world),
        );
    }
}

#[test]
fn pomodoro_shows_paused_and_ready_states() {
    let mut renderer = Renderer::new().expect("renderer");
    let mut world = healthy();
    let mut state = PomodoroState::default();
    pomodoro::toggle(&mut state, now().timestamp_millis(), Stockholm);
    pomodoro::toggle(&mut state, now().timestamp_millis() + 60_000, Stockholm);
    world.pomodoro = pomodoro::snapshot(&state, now().timestamp_millis() + 60_000, Stockholm);
    check(
        "pomodoro-paused",
        &sheet(&mut renderer, PageId::Pomodoro, &world),
    );
}

#[test]
fn pomodoro_shows_the_alert_in_both_flash_states() {
    let mut renderer = Renderer::new().expect("renderer");
    let mut state = PomodoroState::default();
    pomodoro::toggle(&mut state, now().timestamp_millis(), Stockholm);
    pomodoro::reconcile(
        &mut state,
        now().timestamp_millis() + 25 * 60 * 1_000,
        Stockholm,
    );

    for (name, flashing) in [("on", true), ("off", false)] {
        let mut world = healthy();
        world.pomodoro = pomodoro::snapshot(&state, now().timestamp_millis(), Stockholm);
        world.pomodoro_alert_flashing = flashing;
        check(
            &format!("pomodoro-alert-{name}"),
            &sheet(&mut renderer, PageId::Pomodoro, &world),
        );
    }
}

#[test]
fn meeting_tiles_cover_near_ongoing_and_future() {
    let mut renderer = Renderer::new().expect("renderer");
    let mut world = healthy();
    world.meetings = Feed::Ready(vec![
        Meeting {
            account: "a@example.com".to_string(),
            title: "Incident bridge".to_string(),
            start: now() - Duration::minutes(12),
            end: now() + Duration::minutes(30),
            meet_url: "https://meet.google.com/one".to_string(),
        },
        Meeting {
            account: "a@example.com".to_string(),
            title: "Standup".to_string(),
            start: now() + Duration::minutes(3),
            end: now() + Duration::minutes(18),
            meet_url: "https://meet.google.com/two".to_string(),
        },
    ]);
    let ongoing = strip(&mut renderer, &[Tile::Meeting(0), Tile::Meeting(1)], &world);

    let mut later = healthy();
    later.meetings = Feed::Ready(vec![
        Meeting {
            account: "a@example.com".to_string(),
            title: "Retro".to_string(),
            start: now() + Duration::minutes(12),
            end: now() + Duration::minutes(42),
            meet_url: "https://meet.google.com/three".to_string(),
        },
        Meeting {
            account: "a@example.com".to_string(),
            title: "Quarterly business review".to_string(),
            start: now() + Duration::days(3),
            end: now() + Duration::days(3) + Duration::hours(1),
            meet_url: "https://meet.google.com/four".to_string(),
        },
    ]);
    let mut sheet = image::RgbImage::from_pixel(
        ongoing.width(),
        ongoing.height() * 2,
        image::Rgb([20, 22, 28]),
    );
    image::imageops::replace(&mut sheet, &ongoing, 0, 0);
    image::imageops::replace(
        &mut sheet,
        &strip(&mut renderer, &[Tile::Meeting(0), Tile::Meeting(1)], &later),
        0,
        i64::from(ongoing.height()),
    );
    check("meetings", &sheet);
}

#[test]
fn weather_covers_every_symbol_family_and_hard_temperatures() {
    let mut renderer = Renderer::new().expect("renderer");
    let cases: [(&str, f64, f64, f64); 8] = [
        ("clearsky_day", 23.0, 12.0, 0.0),
        ("clearsky_night", -3.0, -9.0, 0.0),
        ("partlycloudy_day", 18.0, 9.0, 0.2),
        ("cloudy", 7.0, 4.0, 0.0),
        ("heavyrainshowers_day", 14.0, 11.0, 12.4),
        ("sleet", 1.0, -1.0, 3.2),
        ("heavysnow", -14.0, -21.0, 8.0),
        ("rainshowersandthunder_day", 26.0, 17.0, 5.5),
    ];

    let gutter = 2u32;
    let mut sheet = image::RgbImage::from_pixel(
        cases.len() as u32 * (KEY_SIZE + gutter) + gutter,
        (KEY_SIZE + gutter) * 2 + gutter,
        image::Rgb([20, 22, 28]),
    );

    for (index, (code, high, low, rain)) in cases.into_iter().enumerate() {
        let body = format!(
            r#"{{"properties":{{"meta":{{"updated_at":"2026-07-24T06:00:00Z"}},"timeseries":[
                {{"time":"2026-07-24T10:00:00Z","data":{{"instant":{{"details":{{
                  "air_temperature":{high},"relative_humidity":72,"wind_speed":4.1,
                  "wind_from_direction":215}}}},
                  "next_1_hours":{{"summary":{{"symbol_code":"{code}"}},
                  "details":{{"precipitation_amount":{rain}}}}}}}}},
                {{"time":"2026-07-24T14:00:00Z","data":{{"instant":{{"details":{{
                  "air_temperature":{low},"relative_humidity":80,"wind_speed":3.0,
                  "wind_from_direction":200}}}},
                  "next_1_hours":{{"summary":{{"symbol_code":"{code}"}},
                  "details":{{"precipitation_amount":0}}}}}}}}
            ]}}}}"#
        );
        let mut world = healthy();
        world.weather = Feed::Ready(parse_forecast(&body, "Stensjön", Stockholm).expect("weather"));
        let context = RenderContext::new(&world);

        for (row, tile) in [Tile::WeatherCurrent, Tile::WeatherForecast(0)]
            .into_iter()
            .enumerate()
        {
            let key = renderer
                .render(&render(tile, &context))
                .expect("rendered")
                .to_image()
                .expect("image");
            image::imageops::replace(
                &mut sheet,
                &key,
                i64::from(gutter + index as u32 * (KEY_SIZE + gutter)),
                i64::from(gutter + row as u32 * (KEY_SIZE + gutter)),
            );
        }
    }
    check("weather-families", &sheet);
}

#[test]
fn weather_detail_cards_show_the_full_reading() {
    let mut renderer = Renderer::new().expect("renderer");
    let mut current = healthy();
    current.weather_detail = Some(streamdeck_core::model::WeatherTile::Current);
    let mut forecast = healthy();
    forecast.weather_detail = Some(streamdeck_core::model::WeatherTile::Forecast);

    let gutter = 2u32;
    let mut sheet = image::RgbImage::from_pixel(
        2 * (KEY_SIZE + gutter) + gutter,
        KEY_SIZE + gutter * 2,
        image::Rgb([20, 22, 28]),
    );
    for (index, (world, tile)) in [
        (&current, Tile::WeatherCurrent),
        (&forecast, Tile::WeatherForecast(1)),
    ]
    .into_iter()
    .enumerate()
    {
        let context = RenderContext::new(world);
        let key = renderer
            .render(&render(tile, &context))
            .expect("rendered")
            .to_image()
            .expect("image");
        image::imageops::replace(
            &mut sheet,
            &key,
            i64::from(gutter + index as u32 * (KEY_SIZE + gutter)),
            i64::from(gutter),
        );
    }
    check("weather-details", &sheet);
}

#[test]
fn a_long_press_armed_key_is_unmistakable() {
    let mut renderer = Renderer::new().expect("renderer");
    let world = healthy();
    let context = RenderContext::new(&world);
    let gutter = 2u32;
    let mut sheet = image::RgbImage::from_pixel(
        2 * (KEY_SIZE + gutter) + gutter,
        KEY_SIZE + gutter * 2,
        image::Rgb([20, 22, 28]),
    );

    for (index, armed) in [false, true].into_iter().enumerate() {
        let mut view = render(Tile::PomodoroGlance, &context);
        view.armed = armed;
        let key = renderer
            .render(&view)
            .expect("rendered")
            .to_image()
            .expect("image");
        image::imageops::replace(
            &mut sheet,
            &key,
            i64::from(gutter + index as u32 * (KEY_SIZE + gutter)),
            i64::from(gutter),
        );
    }
    check("long-press-armed", &sheet);
}

#[test]
fn pressed_feedback_is_visible_on_every_page_style() {
    let mut renderer = Renderer::new().expect("renderer");
    let world = healthy();
    let output = output_targets();
    let input = input_targets();
    let context = RenderContext::new(&world).with_audio(&output, &input);
    let tiles = [
        Tile::HomeButton,
        Tile::PomodoroToggle,
        Tile::WeatherCurrent,
        Tile::LakeCurrent,
        Tile::GitHubMetric(streamdeck_core::integrations::github::MetricKind::Reviews),
    ];

    let gutter = 2u32;
    let mut sheet = image::RgbImage::from_pixel(
        tiles.len() as u32 * (KEY_SIZE + gutter) + gutter,
        (KEY_SIZE + gutter) * 2 + gutter,
        image::Rgb([20, 22, 28]),
    );
    for (index, tile) in tiles.into_iter().enumerate() {
        for (row, pressed) in [false, true].into_iter().enumerate() {
            let mut view = render(tile, &context);
            view.pressed = pressed;
            let key = renderer
                .render(&view)
                .expect("rendered")
                .to_image()
                .expect("image");
            image::imageops::replace(
                &mut sheet,
                &key,
                i64::from(gutter + index as u32 * (KEY_SIZE + gutter)),
                i64::from(gutter + row as u32 * (KEY_SIZE + gutter)),
            );
        }
    }
    check("pressed", &sheet);
}

#[test]
fn usage_tiles_cover_every_severity() {
    let mut renderer = Renderer::new().expect("renderer");
    let gutter = 2u32;
    let percents = [0.0, 12.0, 55.0, 88.0, 100.0];
    let mut sheet = image::RgbImage::from_pixel(
        percents.len() as u32 * (KEY_SIZE + gutter) + gutter,
        (KEY_SIZE + gutter) * 2 + gutter,
        image::Rgb([20, 22, 28]),
    );

    for (index, percent) in percents.into_iter().enumerate() {
        let mut world = healthy();
        world.claude = Feed::Ready(ClaudeUsage {
            five_hour: Some(UsageWindow {
                percent,
                resets_at: Some(now() + Duration::minutes(95)),
            }),
            seven_day: Some(UsageWindow {
                percent: percent / 2.0,
                resets_at: Some(now() + Duration::hours(30)),
            }),
        });
        world.codex = Feed::Ready(CodexUsage {
            plan: Some("pro".to_string()),
            primary: Some(CodexWindow {
                percent,
                window_seconds: 604_800,
                resets_at: Some(now() + Duration::days(2)),
            }),
            secondary: None,
            limit_reached: percent >= 100.0,
        });
        let context = RenderContext::new(&world);

        for (row, tile) in [Tile::ClaudeCombined, Tile::CodexUsage]
            .into_iter()
            .enumerate()
        {
            let key = renderer
                .render(&render(tile, &context))
                .expect("rendered")
                .to_image()
                .expect("image");
            image::imageops::replace(
                &mut sheet,
                &key,
                i64::from(gutter + index as u32 * (KEY_SIZE + gutter)),
                i64::from(gutter + row as u32 * (KEY_SIZE + gutter)),
            );
        }
    }
    check("usage-severities", &sheet);
}
