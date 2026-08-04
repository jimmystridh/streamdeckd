//! Tile-to-view mapping: the only place a tile decides what it says.
//!
//! Every function here is pure over a [`WorldView`], so the golden image suite and
//! the CLI preview render exactly what the hardware does.

use crate::config::{SpotifyPlaylistConfig, WisprMicrophoneConfig};
use crate::integrations::application::{
    ApplicationInfo, ApplicationKind, ContextAction, CustomApplicationAction,
};
use crate::integrations::audio::{AudioTarget, Resolution};
use crate::integrations::claude::UsageSeverity;
use crate::integrations::github::MetricKind;
use crate::integrations::lake::LakeReading;
use crate::integrations::media::MediaStatus;
use crate::integrations::spotify::{PlayerState, SpotifyStatus};
use crate::integrations::system::{PowerSource, VpnState};
use crate::integrations::walkingpad::{
    request_for, WalkingPadCommand, WalkingPadConnection, WalkingPadRequest, WalkingPadTelemetry,
};
use crate::integrations::weather::{SymbolFamily, WeatherDay, WeatherSnapshot, WeatherSymbol};
use crate::model::{AudioKind, WeatherTile};
use crate::pomodoro::{Phase, Status};
use crate::snapshot::{Feed, WorldView};
use crate::text::{
    compact_device_label, compass_direction, device_family, ellipsize, format_duration_minutes,
    format_focus_time, format_lake_temperature, format_precipitation, format_temperature,
    format_timer, short_repository, upper_short,
};
use crate::view::{Color, Icon, KeyStatus, KeyView, TextRun, Weight};

use super::theme;
use super::{ApplicationCommand, MediaCommand, SpotifyCommand, StatsScope, Tile, WalkingPadMetric};
use chrono::Timelike;

/// Configuration-derived inputs the tiles need but the world view does not carry.
pub struct RenderContext<'a> {
    pub world: &'a WorldView,
    pub audio_output: &'a [AudioTarget],
    pub audio_input: &'a [AudioTarget],
    pub spotify_playlists: &'a [SpotifyPlaylistConfig],
    pub wispr_microphones: &'a [WisprMicrophoneConfig],
}

impl<'a> RenderContext<'a> {
    pub fn new(world: &'a WorldView) -> Self {
        Self {
            world,
            audio_output: &[],
            audio_input: &[],
            spotify_playlists: &[],
            wispr_microphones: &[],
        }
    }

    pub fn with_audio(mut self, output: &'a [AudioTarget], input: &'a [AudioTarget]) -> Self {
        self.audio_output = output;
        self.audio_input = input;
        self
    }

    pub fn with_spotify_playlists(mut self, playlists: &'a [SpotifyPlaylistConfig]) -> Self {
        self.spotify_playlists = playlists;
        self
    }

    pub fn with_wispr_microphones(mut self, microphones: &'a [WisprMicrophoneConfig]) -> Self {
        self.wispr_microphones = microphones;
        self
    }

    fn targets(&self, kind: AudioKind) -> &[AudioTarget] {
        match kind {
            AudioKind::Output => self.audio_output,
            AudioKind::Input => self.audio_input,
        }
    }
}

pub fn render(tile: Tile, context: &RenderContext<'_>) -> KeyView {
    let world = context.world;
    match tile {
        Tile::Blank => KeyView::blank(),
        Tile::HomeButton => KeyView::solid(theme::NAVIGATION)
            .glyph(Icon::Home)
            .header("HOME")
            .footer("BACK"),
        Tile::DashboardButton => KeyView::solid(theme::NAVIGATION)
            .glyph(Icon::Dashboard)
            .header("DASHBOARD 2")
            .footer("MORE"),
        Tile::QuickCapture => KeyView::solid(theme::QUICK_CAPTURE)
            .glyph(Icon::Capture)
            .header("QUICK CAPTURE")
            .footer("AUTO · HOLD WORK"),
        Tile::MixerSummary => mixer_summary(world),
        Tile::CodexFiveHour => codex_five_hour(world),
        Tile::ClaudeFiveHour => claude_window(world, false),
        Tile::ClaudeSevenDay => claude_window(world, true),
        Tile::CodexUsage => codex_usage(world),
        Tile::SpotifyGlance => spotify_glance(world),
        Tile::MediaGlance => media_glance(),
        Tile::CurrentApplication => current_application(world, "TAP FOR ACTIONS"),
        Tile::ApplicationControl(command) => application_control(command),
        Tile::ApplicationContext(slot) => application_context(context, slot),
        Tile::ApplicationRecent(slot) => application_recent(world, slot),
        Tile::WisprGlance => wispr_glance(world),
        Tile::WisprPickerHeader => wispr_picker_header(),
        Tile::WisprMicrophone(index) => wispr_microphone(context, index),
        Tile::MediaControl(command) => media_control(command),
        Tile::MediaSource => media_source(world),
        Tile::GitHubSummary => github_summary(world),
        Tile::CiRadar => ci_radar(world),
        Tile::MacHealth => mac_health(world),
        Tile::NetworkVpn => network_vpn(world),
        Tile::DepartureBoard(index) => departure_board(world, index),
        Tile::WalkingPadGlance => walkingpad_glance(world),
        Tile::WalkingPadConnection => walkingpad_connection(world),
        Tile::WalkingPadStart => walkingpad_start(world),
        Tile::WalkingPadStop => walkingpad_stop(world),
        Tile::WalkingPadSpeed => walkingpad_speed(world),
        Tile::WalkingPadSpeedAdjust(command) => walkingpad_speed_adjust(world, command),
        Tile::WalkingPadQuickSpeed(tenths) => walkingpad_quick_speed(world, tenths),
        Tile::WalkingPadSession(metric) => walkingpad_session(world, metric),
        Tile::WalkingPadDaily(metric) => walkingpad_daily(world, metric),
        Tile::WalkingPadStatsButton => KeyView::solid(theme::NAVIGATION)
            .glyph(Icon::Activity)
            .header("TODAY")
            .footer("SESSION & TOTALS"),
        Tile::WalkingPadControlsButton => KeyView::solid(theme::NAVIGATION)
            .glyph(Icon::WalkingPad)
            .header("CONTROLS")
            .footer("WALKINGPAD"),
        Tile::PomodoroGlance => pomodoro_timer(world, "TAP START · HOLD"),
        Tile::Meeting(index) => meeting(world, index),
        Tile::WeatherCurrent => weather_current(world),
        Tile::WeatherGlance => weather_glance(world),
        Tile::WeatherDay(offset) => weather_day(world, offset),
        Tile::WeatherForecast(offset) => weather_forecast(world, offset),
        Tile::LakeCurrent => lake_current(world),
        Tile::LakeTrend => lake_trend(world),
        Tile::LakeDay(index) => lake_day(world, index),
        Tile::PanelCountdown => panel_countdown(world),
        Tile::AudioDevice { kind, index } => audio_device(context, kind, index),
        Tile::AudioMute(kind) => audio_mute(world, kind),
        Tile::AudioVolume { kind, delta } => audio_volume(kind, delta),
        Tile::GitHubMetric(kind) => github_metric(world, kind),
        Tile::GitHubItem(index) => github_item(world, index),
        Tile::GitHubRefresh => KeyView::solid(theme::NAVIGATION)
            .glyph(Icon::Refresh)
            .header("GITHUB")
            .footer("FORCE REFRESH"),
        Tile::SpotifyControl(command) => spotify_control(world, command),
        Tile::SpotifyPlaylist(index) => {
            spotify_playlist(world, context.spotify_playlists.get(index))
        }
        Tile::PomodoroTimer => pomodoro_timer(world, "TAP TOGGLE"),
        Tile::PomodoroToggle => pomodoro_toggle(world),
        Tile::PomodoroSkip => KeyView::solid(theme::SURFACE_RAISED)
            .glyph(Icon::Skip)
            .header("SKIP TIMER")
            .footer("QUEUE NEXT PHASE"),
        Tile::PomodoroReset => KeyView::solid(Color::hex(0x991b1b))
            .glyph(Icon::Reset)
            .header("RESET")
            .footer("SESSION CYCLE"),
        Tile::PomodoroStart(phase) => pomodoro_start(world, phase),
        Tile::PomodoroStats(scope) => pomodoro_stats(world, scope),
        Tile::PomodoroAdjust {
            duration,
            step_minutes,
        } => pomodoro_adjust(world, duration, step_minutes),
    }
}

fn mixer_summary(world: &WorldView) -> KeyView {
    let Some(snapshot) = world.audio.value() else {
        return offline_or_loading(&world.audio, "MIXER", "AUDIO OFFLINE");
    };
    let Some(status) = &snapshot.status else {
        return offline_or_loading(&world.audio, "MIXER", "AUDIO OFFLINE");
    };

    let output_state = if status.output_muted {
        "OFF".to_string()
    } else {
        format!("{}%", status.output_volume)
    };
    let input_state = if status.input_volume == 0 {
        "OFF".to_string()
    } else {
        "ON".to_string()
    };

    KeyView::solid(theme::MIXER)
        .header("MIXER")
        .glyph(Icon::Speaker)
        .rows(vec![
            (
                device_family(&status.current_output).to_string(),
                output_state,
            ),
            (
                format!("MIC {}", device_family(&status.current_input)),
                input_state,
            ),
        ])
        .status(world.audio.status())
}

/// The Codex five-hour tile. The endpoint omits this window when it is not
/// currently applicable, so a missing window is quiet rather than an error.
fn codex_five_hour(world: &WorldView) -> KeyView {
    let Some(usage) = world.codex.value() else {
        return offline_or_loading(&world.codex, "CODEX", "USAGE OFFLINE");
    };
    let Some(window) = usage.five_hour() else {
        return KeyView::solid(theme::DISABLED)
            .header("CODEX")
            .header_right("5H")
            .value("—", 34.0)
            .footer("NO DATA")
            .status(KeyStatus::Disabled);
    };
    let (warning, critical) = world.usage_thresholds;
    let severity = UsageSeverity::of(window.percent, warning, critical);

    usage_tile(
        "CODEX",
        "5H",
        window.percent,
        &window.reset_label(world.now),
        theme::usage(severity, theme::CODEX),
        world.codex.status(),
    )
}

fn claude_window(world: &WorldView, seven_day: bool) -> KeyView {
    let header = "CLAUDE";
    let Some(usage) = world.claude.value() else {
        return offline_or_loading(&world.claude, header, "USAGE OFFLINE");
    };
    let window = if seven_day {
        usage.seven_day
    } else {
        usage.five_hour
    };
    let Some(window) = window else {
        return KeyView::solid(theme::DISABLED)
            .header(header)
            .value("—", 34.0)
            .footer("NO DATA")
            .status(KeyStatus::Disabled);
    };
    let (warning, critical) = world.usage_thresholds;
    let severity = UsageSeverity::of(window.percent, warning, critical);

    usage_tile(
        header,
        if seven_day { "7D" } else { "5H" },
        window.percent,
        &window.reset_label(world.now),
        theme::usage(severity, theme::CLAUDE),
        world.claude.status(),
    )
}

fn codex_usage(world: &WorldView) -> KeyView {
    let Some(usage) = world.codex.value() else {
        return offline_or_loading(&world.codex, "CODEX", "USAGE OFFLINE");
    };
    let Some(window) = usage.binding() else {
        return KeyView::error("CODEX", "NO WINDOW");
    };
    let (warning, critical) = world.usage_thresholds;
    let severity = if usage.limit_reached {
        UsageSeverity::Critical
    } else {
        UsageSeverity::of(window.percent, warning, critical)
    };

    let mut view = usage_tile(
        "CODEX",
        &window.window_label(),
        window.percent,
        &window.reset_label(world.now),
        theme::usage(severity, theme::CODEX),
        world.codex.status(),
    );
    if usage.limit_reached {
        view.status = KeyStatus::Alert;
        view.footer_center = Some(TextRun::new("LIMIT REACHED", 14.0, Weight::Black));
    }
    view
}

fn usage_tile(
    header: &str,
    window_label: &str,
    percent: f64,
    reset_label: &str,
    background: Color,
    status: KeyStatus,
) -> KeyView {
    KeyView::solid(background)
        .header(header)
        .header_right(window_label)
        .value(format!("{}%", percent.round() as i64), 34.0)
        .progress(
            (percent / 100.0) as f32,
            background.darken(0.45),
            theme::FILL,
        )
        .footer(format!("RESET {reset_label}"))
        .status(status)
}

fn spotify_glance(world: &WorldView) -> KeyView {
    let Some(status) = world.spotify.value() else {
        return KeyView::solid(theme::SURFACE_RAISED)
            .header("SPOTIFY")
            .glyph(Icon::Note)
            .footer("HOLD: CONTROLS")
            .status(world.spotify.status());
    };

    let background = if status.is_playing() {
        theme::SPOTIFY
    } else {
        theme::SURFACE_RAISED
    };
    let mut view = KeyView::solid(background)
        .header(status.glance_label(13))
        .glyph(if status.is_playing() {
            Icon::Pause
        } else {
            Icon::Play
        })
        .footer("HOLD: CONTROLS")
        .status(if status.is_available() {
            world.spotify.status()
        } else {
            KeyStatus::Disabled
        });
    if !status.artist.is_empty() {
        view = view.subvalue(ellipsize(&status.artist, 16));
    }
    if let Some(track_id) = &status.track_id {
        if status.artwork_url.is_some() {
            view = view.artwork(track_id.clone());
        }
    }
    view
}

fn media_glance() -> KeyView {
    KeyView::solid(theme::MEDIA)
        .header("MEDIA")
        .glyph(Icon::PlayPause)
        .footer("HOLD: CONTROLS")
}

fn current_application(world: &WorldView, footer: &str) -> KeyView {
    let Some(application) = world.application.value() else {
        return offline_or_loading(&world.application, "CURRENT APP", "APP UNAVAILABLE");
    };
    KeyView::solid(application_color(application))
        .header("CURRENT APP")
        .value(upper_short(&application.name, 20), 21.0)
        .footer(footer)
        .status(world.application.status())
}

fn application_color(application: &ApplicationInfo) -> Color {
    match application.kind() {
        ApplicationKind::Spotify => theme::SPOTIFY,
        ApplicationKind::Wispr => theme::WISPR,
        ApplicationKind::Meet => theme::MEETING_NEXT,
        ApplicationKind::Ghostty => Color::hex(0x6941c6),
        ApplicationKind::Chrome | ApplicationKind::Browser => theme::APPLICATION,
        ApplicationKind::Finder => Color::hex(0x1479c9),
        ApplicationKind::Slack => Color::hex(0x4a154b),
        ApplicationKind::Other => theme::SURFACE_RAISED,
    }
}

fn application_control(command: ApplicationCommand) -> KeyView {
    match command {
        ApplicationCommand::Activate => KeyView::solid(theme::APPLICATION)
            .header("ALL WINDOWS")
            .glyph(Icon::Application)
            .footer("BRING FORWARD"),
        ApplicationCommand::Hide => KeyView::solid(theme::SURFACE_RAISED)
            .header("HIDE APP")
            .glyph(Icon::Minus)
            .footer("KEEP RUNNING"),
        ApplicationCommand::Quit => KeyView::solid(theme::CRITICAL)
            .header("QUIT APP")
            .glyph(Icon::Cross)
            .footer("HOLD TO QUIT"),
        ApplicationCommand::ForceQuit => KeyView::solid(theme::CRITICAL.darken(0.22))
            .header("FORCE QUIT")
            .glyph(Icon::Warning)
            .footer("HOLD TO FORCE"),
        ApplicationCommand::Context(_) | ApplicationCommand::Recent(_) => KeyView::blank(),
    }
}

fn application_context(context: &RenderContext<'_>, slot: usize) -> KeyView {
    let world = context.world;
    let Some(application) = world.application.value() else {
        return offline_or_loading(&world.application, "APP ACTION", "APP UNAVAILABLE");
    };

    match application.context_action(slot) {
        ContextAction::SpotifyPrevious => spotify_control(world, SpotifyCommand::Previous),
        ContextAction::SpotifyPlayPause => spotify_control(world, SpotifyCommand::PlayPause),
        ContextAction::SpotifyNext => spotify_control(world, SpotifyCommand::Next),
        ContextAction::SpotifySeek(delta) => spotify_control(world, SpotifyCommand::Seek(delta)),
        ContextAction::WisprToggle => wispr_glance(world),
        ContextAction::WisprMicrophone(index) => wispr_microphone(context, index),
        ContextAction::OpenMeeting(index) => meeting(world, index),
        ContextAction::Navigate(crate::model::PageId::Wispr) => KeyView::solid(theme::WISPR)
            .header("MIC PICKER")
            .glyph(Icon::Microphone)
            .footer("OPEN CONTROLS"),
        ContextAction::Navigate(crate::model::PageId::Media) => KeyView::solid(theme::MEDIA)
            .header("MEDIA")
            .glyph(Icon::PlayPause)
            .footer("OPEN CONTROLS"),
        ContextAction::Navigate(crate::model::PageId::Mixer) => mixer_summary(world),
        ContextAction::Custom(action) => custom_application_action(action),
        ContextAction::Navigate(_) | ContextAction::None => KeyView::blank(),
    }
}

fn custom_application_action(action: CustomApplicationAction) -> KeyView {
    use CustomApplicationAction::*;

    let (header, icon, footer) = match action {
        GhosttyNewWindow => ("NEW WINDOW", Icon::Application, "GHOSTTY"),
        GhosttyNewTab => ("NEW TAB", Icon::Plus, "GHOSTTY"),
        GhosttySplitRight => ("SPLIT RIGHT", Icon::Application, "GHOSTTY"),
        GhosttySplitDown => ("SPLIT DOWN", Icon::Application, "GHOSTTY"),
        GhosttyToggleSplitZoom => ("ZOOM SPLIT", Icon::Application, "TOGGLE"),
        ChromeNewTab => ("NEW TAB", Icon::Plus, "CHROME"),
        ChromeIncognito => ("INCOGNITO", Icon::Application, "NEW WINDOW"),
        ChromeDownloads => ("DOWNLOADS", Icon::Application, "OPEN PAGE"),
        ChromeHistory => ("HISTORY", Icon::Application, "OPEN PAGE"),
        ChromeExtensions => ("EXTENSIONS", Icon::Application, "OPEN PAGE"),
        FinderNewWindow => ("NEW WINDOW", Icon::Application, "FINDER"),
        FinderHome => ("HOME FOLDER", Icon::Home, "OPEN"),
        FinderDownloads => ("DOWNLOADS", Icon::Application, "OPEN"),
        FinderApplications => ("APPLICATIONS", Icon::Application, "OPEN"),
        FinderAirDrop => ("AIRDROP", Icon::Application, "OPEN"),
        SlackNewMessage => ("NEW MESSAGE", Icon::Message, "COMPOSE"),
        SlackSearch => ("SEARCH", Icon::Search, "SLACK"),
        SlackActivity => ("ACTIVITY", Icon::Activity, "OPEN VIEW"),
        SlackThreads => ("THREADS", Icon::Thread, "OPEN VIEW"),
        SlackDirectMessages => ("DIRECT MSGS", Icon::Message, "BROWSE"),
    };
    let color = match action {
        SlackNewMessage | SlackSearch | SlackActivity | SlackThreads | SlackDirectMessages => {
            Color::hex(0x4a154b)
        }
        _ => theme::APPLICATION,
    };
    KeyView::solid(color)
        .header(header)
        .glyph(icon)
        .footer(footer)
}

fn application_recent(world: &WorldView, slot: usize) -> KeyView {
    let Some(application) = world.recent_applications.get(slot) else {
        return KeyView::solid(theme::SURFACE_SUNKEN)
            .header("RECENT APP")
            .glyph(Icon::Application)
            .footer("NONE YET")
            .status(KeyStatus::Disabled);
    };
    KeyView::solid(application_color(application))
        .header("RECENT APP")
        .value(upper_short(&application.name, 18), 20.0)
        .footer("SWITCH")
}

fn wispr_glance(world: &WorldView) -> KeyView {
    if world.wispr_hands_free {
        return KeyView::solid(theme::LIVE)
            .header("WISPR FLOW")
            .glyph(Icon::Microphone)
            .footer("TAP TO STOP")
            .status(KeyStatus::Selected);
    }
    KeyView::solid(theme::WISPR)
        .header("WISPR FLOW")
        .glyph(Icon::Microphone)
        .footer("TAP START · HOLD MICS")
}

fn wispr_picker_header() -> KeyView {
    KeyView::solid(theme::WISPR)
        .header("WISPR FLOW")
        .glyph(Icon::Microphone)
        .footer("CHOOSE MICROPHONE")
}

fn wispr_microphone(context: &RenderContext<'_>, index: usize) -> KeyView {
    let Some(microphone) = context.wispr_microphones.get(index) else {
        return KeyView::blank();
    };
    KeyView::solid(theme::SURFACE_RAISED)
        .header(upper_short(&microphone.label, 20))
        .glyph(Icon::Microphone)
        .footer("SELECT")
}

fn media_control(command: MediaCommand) -> KeyView {
    match command {
        MediaCommand::Previous => KeyView::solid(theme::SURFACE_RAISED)
            .header("PREVIOUS")
            .glyph(Icon::Previous)
            .footer("SYSTEM MEDIA"),
        MediaCommand::PlayPause => KeyView::solid(theme::MEDIA)
            .header("PLAY / PAUSE")
            .glyph(Icon::PlayPause)
            .footer("SYSTEM MEDIA"),
        MediaCommand::Next => KeyView::solid(theme::SURFACE_RAISED)
            .header("NEXT")
            .glyph(Icon::Next)
            .footer("SYSTEM MEDIA"),
    }
}

fn media_source(world: &WorldView) -> KeyView {
    let Some(status): Option<&MediaStatus> = world.media.value() else {
        return offline_or_loading(&world.media, "MEDIA OWNER", "SESSION OFFLINE");
    };
    if !status.is_active() {
        return KeyView::solid(theme::SURFACE_SUNKEN)
            .header("MEDIA OWNER")
            .glyph(Icon::Note)
            .footer("NO ACTIVE PLAYER")
            .status(KeyStatus::Disabled);
    }

    let source = status.display_source().unwrap_or("MEDIA");
    let mut view = KeyView::solid(theme::MEDIA)
        .header("MEDIA OWNER")
        .value(upper_short(source, 13), 22.0)
        .status(world.media.status());
    if let Some(title) = &status.title {
        view = view.footer(ellipsize(title, 20));
    } else if let Some(application) = &status.application {
        view = view.footer(ellipsize(application, 20));
    }
    view
}

fn github_summary(world: &WorldView) -> KeyView {
    let Some(snapshot) = world.github.value() else {
        return offline_or_loading(&world.github, "GITHUB", "GH OFFLINE");
    };
    let reviews = snapshot.count(MetricKind::Reviews);
    let background = if reviews > 0 {
        theme::GITHUB_ACTIVE
    } else {
        theme::SURFACE_RAISED
    };

    KeyView::solid(background)
        .header("GITHUB")
        .glyph(Icon::GitHub)
        .rows(vec![
            ("REV".to_string(), reviews.to_string()),
            (
                "PR".to_string(),
                snapshot.count(MetricKind::Prs).to_string(),
            ),
            (
                "ISS".to_string(),
                snapshot.count(MetricKind::Assigned).to_string(),
            ),
        ])
        .status(world.github.status())
}

fn ci_radar(world: &WorldView) -> KeyView {
    let Some(snapshot) = world.ci.value() else {
        return offline_or_loading(&world.ci, "CI RADAR", "ACTIONS OFFLINE");
    };
    let failures = snapshot.failures();
    let running = snapshot.running();
    let background = if failures > 0 {
        theme::ERROR
    } else if running > 0 {
        theme::WARNING
    } else {
        theme::LIVE
    };

    KeyView::solid(background)
        .header("CI RADAR")
        .rows(vec![
            ("FAIL".to_string(), failures.to_string()),
            ("RUN".to_string(), running.to_string()),
            ("OK".to_string(), snapshot.successes().to_string()),
        ])
        .status(world.ci.status())
}

fn mac_health(world: &WorldView) -> KeyView {
    let Some(health) = world.mac_health.value() else {
        return offline_or_loading(&world.mac_health, "MAC HEALTH", "SYSTEM OFFLINE");
    };
    let battery = health
        .battery_percent
        .map_or_else(|| "—".to_string(), |percent| format!("{percent}%"));
    let power = if health.charging {
        "CHG"
    } else {
        match health.power_source {
            PowerSource::Ac => "AC",
            PowerSource::Battery => "BATT",
            PowerSource::Unknown => "—",
        }
    };
    let background = if health.memory_free_percent < 15
        || (health.power_source == PowerSource::Battery
            && health.battery_percent.is_some_and(|percent| percent < 15))
    {
        theme::WARNING
    } else {
        theme::MAC_HEALTH
    };

    KeyView::solid(background)
        .header("MAC HEALTH")
        .rows(vec![
            ("BAT".to_string(), battery),
            (
                "MEM FREE".to_string(),
                format!("{}%", health.memory_free_percent),
            ),
            ("POWER".to_string(), power.to_string()),
        ])
        .status(world.mac_health.status())
}

fn network_vpn(world: &WorldView) -> KeyView {
    let Some(network) = world.network.value() else {
        return offline_or_loading(&world.network, "NETWORK", "NETWORK OFFLINE");
    };
    let vpn = match network.vpn_state {
        VpnState::Connected => "ON",
        VpnState::Disconnected => "OFF",
        VpnState::Unavailable => "—",
    };
    let background = if !network.connected {
        theme::ERROR
    } else if network.vpn_state == VpnState::Connected {
        theme::LIVE
    } else {
        theme::NETWORK
    };

    KeyView::solid(background)
        .header("NETWORK / VPN")
        .rows(vec![
            (
                "NET".to_string(),
                if network.connected {
                    "ONLINE"
                } else {
                    "OFFLINE"
                }
                .to_string(),
            ),
            (
                "LINK".to_string(),
                network.interface.as_deref().unwrap_or("—").to_uppercase(),
            ),
            (upper_short(&network.vpn_name, 10), vpn.to_string()),
        ])
        .status(world.network.status())
}

fn departure_board(world: &WorldView, index: usize) -> KeyView {
    let Some(board) = world.departures.value() else {
        return offline_or_loading(&world.departures, "DEPARTURES", "TRANSIT OFFLINE");
    };
    let Some(stop) = board.stops.get(index) else {
        return KeyView::solid(theme::DISABLED)
            .header("DEPARTURES")
            .glyph(Icon::Bus)
            .footer("STOP NOT CONFIGURED")
            .status(KeyStatus::Disabled);
    };
    let rows = ["NÄSTA", "DÄREFTER"]
        .into_iter()
        .enumerate()
        .map(|(index, label)| {
            let countdown = stop.departures.get(index).map_or_else(
                || "—".to_string(),
                |departure| {
                    if departure.cancelled {
                        "X".to_string()
                    } else {
                        departure.countdown(world.now)
                    }
                },
            );
            (label.to_string(), countdown)
        })
        .collect();

    let mut view = KeyView::solid(theme::VASTTRAFIK)
        .header(upper_short(&stop.label, 16))
        .rows(rows)
        .status(world.departures.status());
    if let Some(line) = &stop.line {
        view = view.header_right(upper_short(line, 8));
    }
    view
}

fn meeting(world: &WorldView, index: usize) -> KeyView {
    let ordinal = (index + 1).to_string();
    let Some(meetings) = world.meetings.value() else {
        let mut view = offline_or_loading(&world.meetings, "MEETING", "CALENDAR OFFLINE");
        view.badge = Some(crate::view::Badge {
            text: ordinal,
            background: theme::CODEX,
        });
        return view;
    };

    let Some(meeting) = meetings.get(index) else {
        return KeyView::solid(theme::SURFACE_RAISED)
            .header(if index == 0 {
                "NO UPCOMING"
            } else {
                "NO LATER"
            })
            .art(Icon::Calendar)
            .value("—", 26.0)
            .footer("MEETING")
            .badge(ordinal, theme::CODEX)
            .status(KeyStatus::Disabled);
    };

    let urgency = meeting.urgency(world.now, world.timezone);
    KeyView::solid(theme::meeting(urgency, index == 0))
        .header(ellipsize(&meeting.title, 13))
        .art(Icon::Calendar)
        .value(meeting.start_label(world.timezone), 24.0)
        .subvalue(meeting.status_label(world.now, world.timezone))
        .badge(ordinal, theme::CODEX)
        .status(world.meetings.status())
}

fn weather_current(world: &WorldView) -> KeyView {
    let Some(snapshot) = world.weather.value() else {
        return offline_or_loading(
            &world.weather,
            &upper_short(&world.location_name, 13),
            "WEATHER OFFLINE",
        );
    };
    if world.weather_detail == Some(WeatherTile::Current) {
        return weather_current_detail(world, snapshot);
    }
    let current = snapshot.current;
    let today = snapshot.today();
    let (top, bottom) = theme::sky(current.symbol);

    let mut view = KeyView {
        background: crate::view::Background::Diagonal { top, bottom },
        ..Default::default()
    }
    .header(upper_short(&snapshot.location, 13))
    .header_right("MET.NO")
    .art(weather_icon(current.symbol))
    .value(format_temperature(current.temperature), 34.0)
    .status(world.weather.status());

    if let Some(today) = today {
        view = view.footers(
            format!("H {}", format_temperature(today.high)),
            format!("L {}", format_temperature(today.low)),
        );
    }
    view
}

fn weather_glance(world: &WorldView) -> KeyView {
    if world.now.with_timezone(&world.timezone).hour() >= 17 {
        weather_day(world, 1)
    } else {
        weather_current(world).header("TODAY")
    }
}

fn weather_day(world: &WorldView, offset: usize) -> KeyView {
    let Some(snapshot) = world.weather.value() else {
        return offline_or_loading(&world.weather, "FORECAST", "WEATHER OFFLINE");
    };
    let Some(day) = snapshot.day(offset) else {
        return KeyView::error("FORECAST", "NO DAY");
    };
    let (top, bottom) = theme::sky(day.symbol);
    let header = match offset {
        0 => "TODAY".to_string(),
        1 => "TOMORROW".to_string(),
        _ => weekday_label(day, world),
    };

    KeyView {
        background: crate::view::Background::Diagonal { top, bottom },
        ..Default::default()
    }
    .header(header)
    .header_right("MET.NO")
    .art(weather_icon(day.symbol))
    .value(
        format!(
            "{}/{}",
            format_temperature(day.high),
            format_temperature(day.low)
        ),
        23.0,
    )
    .footers(
        format_precipitation(day.precipitation),
        date_label(day, world),
    )
    .status(world.weather.status())
}

/// The expanded reading a press reveals: humidity, wind, rain, and today's range.
fn weather_current_detail(world: &WorldView, snapshot: &WeatherSnapshot) -> KeyView {
    let current = snapshot.current;
    let (top, bottom) = theme::sky(current.symbol);
    let range = snapshot
        .today()
        .map(|day| {
            format!(
                "{}–{}",
                format_temperature(day.low),
                format_temperature(day.high)
            )
        })
        .unwrap_or_else(|| "—".to_string());

    KeyView {
        background: crate::view::Background::Diagonal { top, bottom },
        ..Default::default()
    }
    .header(upper_short(&snapshot.location, 11))
    .header_right("NOW")
    .rows(vec![
        (
            "HUM".to_string(),
            format!("{}%", current.humidity.round() as i64),
        ),
        (
            "WIND".to_string(),
            format!(
                "{} {:.0} m/s",
                compass_direction(current.wind_direction),
                current.wind_speed
            ),
        ),
        (
            "RAIN".to_string(),
            format!("{:.1} mm", current.precipitation),
        ),
        ("RANGE".to_string(), range),
    ])
    .footer("MET NORWAY")
    .status(world.weather.status())
}

fn weather_forecast(world: &WorldView, offset: usize) -> KeyView {
    let Some(snapshot) = world.weather.value() else {
        return offline_or_loading(&world.weather, "FORECAST", "WEATHER OFFLINE");
    };
    let Some(day) = snapshot.day(offset) else {
        return KeyView::error("FORECAST", "NO DAY");
    };
    if world.weather_detail == Some(WeatherTile::Forecast) {
        return weather_forecast_detail(world, day);
    }
    let (top, bottom) = theme::sky(day.symbol);

    KeyView {
        background: crate::view::Background::Diagonal { top, bottom },
        ..Default::default()
    }
    .header(weekday_label(day, world))
    .header_right("MET.NO")
    .art(weather_icon(day.symbol))
    .value(
        format!(
            "{}/{}",
            format_temperature(day.high),
            format_temperature(day.low)
        ),
        23.0,
    )
    .footers(
        format_precipitation(day.precipitation),
        date_label(day, world),
    )
    .status(world.weather.status())
}

/// The expanded forecast a press reveals: sky, high, low, and rain for the day.
fn weather_forecast_detail(world: &WorldView, day: &WeatherDay) -> KeyView {
    let (top, bottom) = theme::sky(day.symbol);

    KeyView {
        background: crate::view::Background::Diagonal { top, bottom },
        ..Default::default()
    }
    .header(weekday_label(day, world))
    .header_right(date_label(day, world))
    .rows(vec![
        ("SKY".to_string(), day.symbol.condition_label().to_string()),
        ("HIGH".to_string(), format_temperature(day.high)),
        ("LOW".to_string(), format_temperature(day.low)),
        ("RAIN".to_string(), format!("{:.1} mm", day.precipitation)),
    ])
    .footer("MET NORWAY")
    .status(world.weather.status())
}

fn weekday_label(day: &WeatherDay, world: &WorldView) -> String {
    day.representative
        .with_timezone(&world.timezone)
        .format("%a")
        .to_string()
        .to_uppercase()
}

fn date_label(day: &WeatherDay, world: &WorldView) -> String {
    day.representative
        .with_timezone(&world.timezone)
        .format("%-d %b")
        .to_string()
        .to_uppercase()
}

fn weather_icon(symbol: WeatherSymbol) -> Icon {
    match symbol.family {
        SymbolFamily::Clear if symbol.night => Icon::Moon,
        SymbolFamily::Clear => Icon::Sun,
        SymbolFamily::PartlyCloudy | SymbolFamily::Cloudy => Icon::Cloud,
        SymbolFamily::Rain => Icon::Rain,
        SymbolFamily::Sleet => Icon::Sleet,
        SymbolFamily::Snow => Icon::Snow,
        SymbolFamily::Thunder => Icon::Thunder,
        SymbolFamily::Fog => Icon::Fog,
    }
}

fn lake_current(world: &WorldView) -> KeyView {
    let Some(reading) = world.lake_current.value() else {
        return water_tile_placeholder(world, &world.lake_current);
    };
    water_tile(
        reading,
        &upper_short(&world.location_name, 13),
        Some(
            reading
                .measured_at
                .with_timezone(&world.timezone)
                .format("%H:%M")
                .to_string(),
        ),
        world.lake_current.status(),
    )
}

fn water_tile(
    reading: &LakeReading,
    header: &str,
    footer: Option<String>,
    status: KeyStatus,
) -> KeyView {
    let color = theme::water(reading.temperature);
    let mut view = KeyView {
        background: crate::view::Background::Vertical {
            top: color,
            bottom: Color::hex(0x082f49),
        },
        ..Default::default()
    }
    .header(header)
    .art(Icon::Water)
    .value(format_lake_temperature(reading.temperature), 36.0)
    .status(status);
    if let Some(footer) = footer {
        view = view.footer(footer);
    }
    view
}

fn water_tile_placeholder<T>(world: &WorldView, feed: &Feed<T>) -> KeyView {
    offline_or_loading(
        feed,
        &upper_short(&world.location_name, 13),
        "WATER OFFLINE",
    )
}

fn lake_trend(world: &WorldView) -> KeyView {
    let Some(history) = world.lake_history.value() else {
        return offline_or_loading(&world.lake_history, "7 DAYS", "HISTORY OFFLINE");
    };
    let Some(trend) = history.trend() else {
        return KeyView::solid(theme::SURFACE_RAISED)
            .header("7 DAYS")
            .value("—", 30.0)
            .footer("NO TREND")
            .status(KeyStatus::Disabled);
    };

    let rising = trend >= 0.0;
    KeyView::solid(if rising {
        theme::WARNING
    } else {
        Color::hex(0x0369a1)
    })
    .header("7 DAYS")
    .art(if rising {
        Icon::TrendUp
    } else {
        Icon::TrendDown
    })
    .value(
        format!("{}{trend:.1}°", if rising { "+" } else { "" }),
        28.0,
    )
    .footer("TREND")
    .status(world.lake_history.status())
}

fn lake_day(world: &WorldView, index: usize) -> KeyView {
    let Some(history) = world.lake_history.value() else {
        return offline_or_loading(&world.lake_history, "HISTORY", "HISTORY OFFLINE");
    };
    let Some(reading) = history.day(index) else {
        return KeyView::solid(theme::SURFACE_SUNKEN)
            .header("—")
            .value("—", 30.0)
            .footer("NO DATA")
            .status(KeyStatus::Disabled);
    };

    let local = reading.measured_at.with_timezone(&world.timezone);
    water_tile(
        reading,
        &local.format("%a").to_string().to_uppercase(),
        Some(local.format("%-d %b").to_string().to_uppercase()),
        world.lake_history.status(),
    )
}

fn panel_countdown(world: &WorldView) -> KeyView {
    let remaining = world.panel_seconds_remaining.unwrap_or(0);
    let total = world.panel_total_seconds.max(1);

    KeyView::solid(theme::SURFACE_RAISED)
        .header("AUTO CLOSE")
        .value(remaining.to_string(), 34.0)
        .progress(
            (remaining as f32 / total as f32).clamp(0.0, 1.0),
            theme::TRACK,
            theme::FILL,
        )
        .footer("TAP FOR HOME")
}

fn audio_device(context: &RenderContext<'_>, kind: AudioKind, index: usize) -> KeyView {
    let world = context.world;
    let targets = context.targets(kind);
    let Some(target) = targets.get(index) else {
        return KeyView::blank();
    };
    let label = compact_device_label(&target.label, 13);
    let badge = if kind == AudioKind::Output {
        "OUT"
    } else {
        "MIC"
    };

    let Some(snapshot) = world.audio.value() else {
        return KeyView::solid(theme::DISABLED)
            .header(label)
            .header_right(badge)
            .glyph(device_icon(kind, false))
            .footer("AUDIO OFFLINE")
            .status(world.audio.status());
    };

    let resolution = target.resolve(snapshot.inventory.devices(kind));
    let current = snapshot.status.as_ref().map(|status| status.current(kind));
    let selected =
        matches!(&resolution, Resolution::Available(name) if Some(name.as_str()) == current);

    let (background, status, footer) = match &resolution {
        Resolution::Available(_) if selected => (theme::SELECTED, KeyStatus::Selected, "ACTIVE"),
        Resolution::Available(_) => (theme::SURFACE_RAISED, KeyStatus::Ok, "TAP TO SELECT"),
        Resolution::Ambiguous(_) => (theme::WARNING, KeyStatus::Ambiguous, "CHECK NAME"),
        Resolution::Unavailable => (theme::DISABLED, KeyStatus::Disabled, "OFFLINE"),
    };

    KeyView::solid(background)
        .header(label)
        .header_right(badge)
        .glyph(device_icon(kind, selected))
        .footer(footer)
        .status(status)
}

fn device_icon(kind: AudioKind, _selected: bool) -> Icon {
    match kind {
        AudioKind::Output => Icon::Speaker,
        AudioKind::Input => Icon::Microphone,
    }
}

fn audio_mute(world: &WorldView, kind: AudioKind) -> KeyView {
    let label = if kind == AudioKind::Output {
        "OUTPUT"
    } else {
        "MIC"
    };
    let Some(status) = world
        .audio
        .value()
        .and_then(|snapshot| snapshot.status.clone())
    else {
        return KeyView::solid(theme::DISABLED)
            .header(label)
            .glyph(device_icon(kind, false))
            .footer("AUDIO OFFLINE")
            .status(world.audio.status());
    };

    let muted = status.is_muted(kind);
    KeyView::solid(if muted { theme::MUTED } else { theme::LIVE })
        .header(label)
        .glyph(match (kind, muted) {
            (AudioKind::Output, true) => Icon::SpeakerMuted,
            (AudioKind::Output, false) => Icon::Speaker,
            (AudioKind::Input, true) => Icon::MicrophoneMuted,
            (AudioKind::Input, false) => Icon::Microphone,
        })
        .value(if muted { "OFF" } else { "ON" }, 26.0)
        .footer(if muted { "MUTED" } else { "LIVE" })
        .status(if muted {
            KeyStatus::Alert
        } else {
            KeyStatus::Selected
        })
}

fn audio_volume(kind: AudioKind, delta: i32) -> KeyView {
    let label = if kind == AudioKind::Output {
        "VOLUME"
    } else {
        "MIC GAIN"
    };
    KeyView::solid(theme::SURFACE_RAISED)
        .header(label)
        .glyph(if delta > 0 { Icon::Plus } else { Icon::Minus })
        .footer(format!("{}{}", if delta > 0 { "+" } else { "" }, delta))
}

fn github_metric(world: &WorldView, kind: MetricKind) -> KeyView {
    let Some(snapshot) = world.github.value() else {
        return offline_or_loading(&world.github, kind.label(), "GH OFFLINE");
    };
    let count = snapshot.count(kind);
    let background = if kind == MetricKind::Reviews && count > 0 {
        theme::GITHUB_ACTIVE
    } else {
        theme::GITHUB
    };

    let mut view = KeyView::solid(background)
        .header(kind.label())
        .glyph(Icon::GitHub)
        .value(snapshot.count_label(kind), 34.0)
        .status(world.github.status());
    if kind == MetricKind::Inbox && snapshot.inbox_overflow {
        view = view.footer("CAPPED AT 100");
    }
    view
}

fn github_item(world: &WorldView, index: usize) -> KeyView {
    let Some(snapshot) = world.github.value() else {
        return offline_or_loading(&world.github, "PR", "GH OFFLINE");
    };
    let Some(item) = snapshot.item(index) else {
        return KeyView::solid(theme::SURFACE_SUNKEN)
            .header("NO ITEM")
            .glyph(Icon::GitHub)
            .status(KeyStatus::Disabled);
    };

    KeyView::solid(theme::GITHUB_ITEM)
        .header(short_repository(
            &item.repository_name,
            &world.repository_aliases,
            13,
        ))
        .value(format!("#{}", item.number), 24.0)
        .footer(ellipsize(&item.title, 20))
        .status(world.github.status())
}

fn spotify_control(world: &WorldView, command: SpotifyCommand) -> KeyView {
    let status = world.spotify.value();
    let available = status.is_some_and(SpotifyStatus::is_available);
    let base = if available {
        theme::SURFACE_RAISED
    } else {
        theme::DISABLED
    };
    let key_status = if available {
        KeyStatus::Ok
    } else {
        KeyStatus::Disabled
    };

    match command {
        SpotifyCommand::Previous => KeyView::solid(base)
            .header("PREVIOUS")
            .glyph(Icon::Previous)
            .status(key_status),
        SpotifyCommand::Next => KeyView::solid(base)
            .header("NEXT")
            .glyph(Icon::Next)
            .status(key_status),
        SpotifyCommand::PlayPause => spotify_play_pause(world, status, key_status),
        SpotifyCommand::Seek(delta) => KeyView::solid(base)
            .header(if delta < 0 { "BACK" } else { "FORWARD" })
            .glyph(if delta < 0 {
                Icon::Previous
            } else {
                Icon::Next
            })
            .value(format!("{}s", delta.unsigned_abs()), 24.0)
            .status(key_status),
        SpotifyCommand::OpenApp => KeyView::solid(theme::SPOTIFY)
            .header("SPOTIFY")
            .glyph(Icon::Note)
            .footer("OPEN APP"),
        SpotifyCommand::Volume(delta) => KeyView::solid(base)
            .header("VOLUME")
            .glyph(if delta > 0 { Icon::Plus } else { Icon::Minus })
            .value(
                status
                    .filter(|_| available)
                    .map(|status| format!("{}%", status.volume))
                    .unwrap_or_else(|| "—".to_string()),
                22.0,
            )
            .footer(format!("{}{}", if delta > 0 { "+" } else { "" }, delta))
            .status(key_status),
        SpotifyCommand::PlayPlaylist(_) => KeyView::blank(),
    }
}

fn spotify_playlist(world: &WorldView, playlist: Option<&SpotifyPlaylistConfig>) -> KeyView {
    let available = world
        .spotify
        .value()
        .is_some_and(SpotifyStatus::is_available);
    let Some(playlist) = playlist else {
        return KeyView::solid(theme::DISABLED)
            .header("PLAYLIST")
            .glyph(Icon::Note)
            .footer("NOT SET")
            .status(KeyStatus::Disabled);
    };

    KeyView::solid(if available {
        theme::SPOTIFY
    } else {
        theme::DISABLED
    })
    .header(upper_short(&playlist.label, 13))
    .glyph(Icon::Note)
    .footer("PLAY PLAYLIST")
    .status(if available {
        KeyStatus::Ok
    } else {
        KeyStatus::Disabled
    })
}

fn spotify_play_pause(
    world: &WorldView,
    status: Option<&SpotifyStatus>,
    key_status: KeyStatus,
) -> KeyView {
    let Some(status) = status else {
        return KeyView::solid(theme::DISABLED)
            .header("SPOTIFY")
            .glyph(Icon::Note)
            .status(world.spotify.status());
    };
    if status.state == PlayerState::NotRunning {
        return KeyView::solid(theme::DISABLED)
            .header("SPOTIFY")
            .glyph(Icon::Note)
            .footer("NOT RUNNING")
            .status(KeyStatus::Disabled);
    }

    let mut view = KeyView::solid(if status.is_playing() {
        theme::SPOTIFY
    } else {
        theme::SURFACE_RAISED
    })
    .header(ellipsize(&status.track, 13))
    .glyph(if status.is_playing() {
        Icon::Pause
    } else {
        Icon::Play
    })
    .footer(ellipsize(&status.artist, 20))
    .status(key_status);
    if let Some(track_id) = &status.track_id {
        if status.artwork_url.is_some() {
            view = view.artwork(track_id.clone());
        }
    }
    view
}

fn pomodoro_timer(world: &WorldView, hint: &str) -> KeyView {
    if let Some(phase) = world.pomodoro.pending_completion_phase {
        return pomodoro_completion(phase, world.pomodoro_alert_flashing);
    }
    let snapshot = &world.pomodoro;
    let status_label = match snapshot.status {
        Status::Running => "RUNNING",
        Status::Paused => "PAUSED",
        Status::Ready => "READY",
    };
    let hint = if snapshot.status == Status::Running {
        hint.replace("TAP START", "TAP PAUSE")
    } else {
        hint.to_string()
    };

    KeyView::solid(theme::phase(snapshot.phase))
        .header(snapshot.phase.label())
        .mono_value(format_timer(snapshot.remaining_seconds), 28.0)
        .subvalue(status_label)
        .progress(
            snapshot.progress(),
            theme::phase(snapshot.phase).darken(0.4),
            theme::FILL,
        )
        .footer(hint)
}

fn pomodoro_completion(phase: Phase, flashing: bool) -> KeyView {
    let focus_finished = phase == Phase::Focus;
    let background = if flashing {
        theme::ALERT
    } else if focus_finished {
        theme::CRITICAL
    } else {
        theme::SHORT_BREAK
    };

    KeyView::solid(background)
        .header(if focus_finished {
            "FOCUS DONE"
        } else {
            "BREAK DONE"
        })
        .glyph(Icon::Check)
        .subvalue(if focus_finished {
            "BREAK READY"
        } else {
            "FOCUS READY"
        })
        .footer("PRESS TO CONTINUE")
        .status(KeyStatus::Alert)
}

fn pomodoro_toggle(world: &WorldView) -> KeyView {
    if let Some(phase) = world.pomodoro.pending_completion_phase {
        return pomodoro_completion(phase, world.pomodoro_alert_flashing);
    }
    let snapshot = &world.pomodoro;
    let (header, glyph) = match snapshot.status {
        Status::Running => ("PAUSE", Icon::Pause),
        Status::Paused => ("RESUME", Icon::Play),
        Status::Ready => ("START", Icon::Play),
    };

    KeyView::solid(theme::phase(snapshot.phase))
        .header(header)
        .glyph(glyph)
        .footer(snapshot.phase.label())
}

fn pomodoro_start(world: &WorldView, phase: Phase) -> KeyView {
    let minutes = match phase {
        Phase::Focus => world.pomodoro.focus_minutes,
        Phase::ShortBreak => world.pomodoro.short_break_minutes,
        Phase::LongBreak => world.pomodoro.long_break_minutes,
    };
    KeyView::solid(theme::phase(phase))
        .header(format!("START {}", phase.label()))
        .value(format!("{minutes}m"), 30.0)
        .footer("START NOW")
}

fn pomodoro_adjust(world: &WorldView, duration: Phase, step_minutes: i32) -> KeyView {
    let minutes = match duration {
        Phase::Focus => world.pomodoro.focus_minutes,
        Phase::ShortBreak => world.pomodoro.short_break_minutes,
        Phase::LongBreak => world.pomodoro.long_break_minutes,
    };
    let header = match duration {
        Phase::Focus => "FOCUS LENGTH",
        Phase::ShortBreak => "BREAK LENGTH",
        Phase::LongBreak => "LONG LENGTH",
    };

    KeyView::solid(theme::phase(duration))
        .header(header)
        .value(format!("{minutes}m"), 30.0)
        .footer(format!(
            "TAP {}m · HOLD {}m",
            wrapped_duration(duration, minutes, step_minutes),
            wrapped_duration(duration, minutes, -step_minutes)
        ))
}

/// Previews what an adjustment would produce, including wraparound, so the tile
/// footer can promise the value the press will actually set.
pub fn wrapped_duration(duration: Phase, current: u32, delta: i32) -> u32 {
    let (minimum, maximum) = duration.bounds();
    if delta > 0 && current >= maximum {
        return minimum;
    }
    if delta < 0 && current <= minimum {
        return maximum;
    }
    (i64::from(current) + i64::from(delta)).clamp(minimum as i64, maximum as i64) as u32
}

fn pomodoro_stats(world: &WorldView, scope: StatsScope) -> KeyView {
    let snapshot = &world.pomodoro;
    match scope {
        StatsScope::Cycle => KeyView::solid(Color::hex(0x6d28d9))
            .header("CURRENT CYCLE")
            .value(
                format!(
                    "{}/{}",
                    snapshot.cycle_focus_sessions,
                    crate::pomodoro::LONG_BREAK_EVERY
                ),
                30.0,
            )
            .footer("FOCUS SESSIONS"),
        StatsScope::Breaks => KeyView::solid(theme::SHORT_BREAK)
            .header("BREAKS DONE")
            .value(
                format!(
                    "{}/{}",
                    snapshot.completed_short_breaks, snapshot.completed_long_breaks
                ),
                28.0,
            )
            .footer("SHORT / LONG"),
        StatsScope::Today => KeyView::solid(Color::hex(0x0369a1))
            .header("TODAY")
            .value(snapshot.today_focus_sessions.to_string(), 34.0)
            .footer(format!("{} FOCUS MIN", snapshot.today_focus_minutes)),
        StatsScope::AllTime => KeyView::solid(Color::hex(0x1e3a8a))
            .header("ALL TIME")
            .value(snapshot.completed_focus_sessions.to_string(), 34.0)
            .footer(format_focus_time(snapshot.total_focus_minutes)),
    }
}

/// A loading tile while the first fetch is in flight, an error tile once it has
/// failed with nothing cached.
fn offline_or_loading<T>(feed: &Feed<T>, header: &str, detail: &str) -> KeyView {
    match feed {
        Feed::Loading => KeyView::loading(header),
        _ => KeyView::error(header, detail),
    }
}

/// Countdown label for the next meeting, used by the CLI status output.
pub fn meeting_countdown(world: &WorldView, index: usize) -> Option<String> {
    let meeting = world.meetings.value()?.get(index)?;
    let minutes = (meeting.start - world.now).num_minutes().max(0) as u32;
    Some(format_duration_minutes(minutes))
}

/// Exposed for the status command: which weather snapshot the tiles are showing.
pub fn weather_summary(snapshot: &WeatherSnapshot) -> String {
    format!(
        "{} {} ({})",
        snapshot.location,
        format_temperature(snapshot.current.temperature),
        snapshot.current.symbol.condition_label()
    )
}

fn walkingpad_glance(world: &WorldView) -> KeyView {
    let mut view = KeyView::solid(theme::WALKINGPAD)
        .header("WALKINGPAD")
        .glyph(Icon::WalkingPad);
    if let Some(pending) = &world.walkingpad.pending {
        return match pending.request {
            WalkingPadRequest::Start => view
                .value("STARTING", 18.0)
                .footer("RESUMING TARGET")
                .status(KeyStatus::Selected),
            WalkingPadRequest::Stop => view
                .value("STOPPING", 18.0)
                .footer("HALT REQUESTED")
                .status(KeyStatus::Alert),
            WalkingPadRequest::SetSpeed(tenths) => view
                .value(format_speed(tenths), 28.0)
                .subvalue("KM/H")
                .footer("SETTING SPEED")
                .status(KeyStatus::Selected),
        };
    }
    if world
        .walkingpad
        .has_fresh_status(world.now.timestamp_millis())
    {
        if let Some(status) = &world.walkingpad.telemetry {
            if status.is_moving() {
                return view
                    .value(format_speed(status.speed_tenths), 30.0)
                    .subvalue("KM/H")
                    .footer(format!(
                        "TODAY {}",
                        format_distance(world.walkingpad_daily.distance_hundredths)
                    ));
            }
            return view.value("READY", 22.0).footer("TAP FOR CONTROLS");
        }
    }
    let (label, status) = walkingpad_connection_label(world);
    view = view.value(label, 17.0).status(status);
    if let Some(age) = stale_age(world) {
        view = view.footer(age);
    }
    view
}

fn walkingpad_connection(world: &WorldView) -> KeyView {
    let (label, status) = walkingpad_connection_label(world);
    let mut view = KeyView::solid(theme::WALKINGPAD)
        .header("BELT STATE")
        .glyph(Icon::WalkingPad)
        .value(label, 18.0)
        .status(status);

    if let Some(pending) = &world.walkingpad.pending {
        view = match pending.request {
            WalkingPadRequest::Start => view
                .value("STARTING", 18.0)
                .footer("RESUMING TARGET")
                .status(KeyStatus::Selected),
            WalkingPadRequest::Stop => view
                .value("STOPPING", 18.0)
                .footer("HALT REQUESTED")
                .status(KeyStatus::Alert),
            WalkingPadRequest::SetSpeed(_) => view.footer("SETTING SPEED"),
        };
    } else if let Some(age) = stale_age(world) {
        view = view.footer(age);
    } else if let Some(error) = &world.walkingpad.connection_error {
        view = view.footer(upper_short(error, 20));
    } else if let Some(status) = &world.walkingpad.telemetry {
        view = view.footer(match status.mode {
            crate::integrations::walkingpad::WalkingPadMode::Automatic => "AUTOMATIC MODE",
            crate::integrations::walkingpad::WalkingPadMode::Manual => "MANUAL MODE",
            crate::integrations::walkingpad::WalkingPadMode::Standby => "STANDBY MODE",
        });
    }
    view
}

fn walkingpad_start(world: &WorldView) -> KeyView {
    let command = WalkingPadCommand::Start;
    let standby = world
        .walkingpad
        .telemetry
        .as_ref()
        .is_some_and(WalkingPadTelemetry::is_standby);
    if let Some(error) = walkingpad_feedback(world, command) {
        return KeyView::solid(theme::ERROR)
            .glyph(Icon::Cross)
            .header("START FAILED")
            .footer(error)
            .status(KeyStatus::Error);
    }
    if world
        .walkingpad
        .pending
        .as_ref()
        .is_some_and(|pending| pending.request == WalkingPadRequest::Start)
    {
        return KeyView::solid(theme::WALKINGPAD_START)
            .glyph(Icon::Play)
            .header(if standby { "WAKING" } else { "STARTING" })
            .value(if standby { "WAKE" } else { "RESUME" }, 22.0)
            .footer(if standby {
                "MANUAL THEN START"
            } else {
                "DEVICE START SPEED"
            })
            .status(KeyStatus::Selected);
    }

    let fresh = world
        .walkingpad
        .has_fresh_status(world.now.timestamp_millis());
    let stopped = world
        .walkingpad
        .telemetry
        .as_ref()
        .is_some_and(|status| !status.is_moving());
    let enabled = fresh && stopped && world.walkingpad.pending.is_none();
    KeyView::solid(theme::WALKINGPAD_START)
        .glyph(Icon::Play)
        .header("START")
        .value(if standby { "WAKE" } else { "RESUME" }, 22.0)
        .footer(if enabled {
            if standby {
                "THEN START"
            } else {
                "DEVICE START SPEED"
            }
        } else if !fresh {
            "WAIT FOR STATUS"
        } else if !stopped {
            "ALREADY MOVING"
        } else {
            "BUSY"
        })
        .status(if enabled {
            KeyStatus::Ok
        } else {
            KeyStatus::Disabled
        })
}

fn walkingpad_stop(world: &WorldView) -> KeyView {
    let command = WalkingPadCommand::Stop;
    if let Some(error) = walkingpad_feedback(world, command) {
        return KeyView::solid(theme::WALKINGPAD_STOP)
            .glyph(Icon::Warning)
            .header("STOP FAILED")
            .footer(error)
            .status(KeyStatus::Error);
    }
    if world
        .walkingpad
        .pending
        .as_ref()
        .is_some_and(|pending| pending.request == WalkingPadRequest::Stop)
    {
        return KeyView::solid(theme::WALKINGPAD_STOP)
            .glyph(Icon::Cross)
            .header("STOPPING")
            .footer("HALT REQUESTED")
            .status(KeyStatus::Alert);
    }
    let connected = world.walkingpad.connection == WalkingPadConnection::Connected;
    let fresh = world
        .walkingpad
        .has_fresh_status(world.now.timestamp_millis());
    let moving = world
        .walkingpad
        .telemetry
        .as_ref()
        .is_some_and(WalkingPadTelemetry::is_moving);
    let another_command_pending = world.walkingpad.pending.is_some();
    let enabled = connected && (another_command_pending || !fresh || moving);
    KeyView::solid(theme::WALKINGPAD_STOP)
        .glyph(Icon::Cross)
        .header("STOP")
        .value(if enabled { "HALT" } else { "STOPPED" }, 22.0)
        .footer(if another_command_pending {
            "CANCEL COMMAND"
        } else if moving {
            "STAY READY"
        } else if connected && fresh {
            "NO COMMAND SENT"
        } else if connected {
            "STATUS STALE"
        } else {
            "DISCONNECTED"
        })
        .status(if enabled {
            KeyStatus::Ok
        } else {
            KeyStatus::Disabled
        })
}

fn walkingpad_speed(world: &WorldView) -> KeyView {
    let speed = world
        .walkingpad
        .telemetry
        .as_ref()
        .map(|status| status.speed_tenths);
    let Some(speed) = speed else {
        return KeyView::solid(theme::DISABLED)
            .header("CURRENT SPEED")
            .value("—", 34.0)
            .footer("NO STATUS")
            .status(KeyStatus::Disabled);
    };
    let status = if world.walkingpad.pending.is_some() {
        KeyStatus::Selected
    } else {
        walkingpad_telemetry_status(world)
    };
    KeyView::solid(theme::WALKINGPAD)
        .header("CURRENT SPEED")
        .value(format_speed(speed), 34.0)
        .subvalue("KM/H")
        .footer(if speed == 0 { "STOPPED" } else { "LIVE" })
        .status(status)
}

fn walkingpad_speed_adjust(world: &WorldView, command: WalkingPadCommand) -> KeyView {
    let increasing = command == WalkingPadCommand::Increase;
    if let Some(error) = walkingpad_feedback(world, command) {
        return KeyView::solid(theme::ERROR)
            .glyph(Icon::Cross)
            .header(if increasing { "SPEED UP" } else { "SPEED DOWN" })
            .footer(error)
            .status(KeyStatus::Error);
    }
    let current = world
        .walkingpad
        .telemetry
        .as_ref()
        .map(WalkingPadTelemetry::control_speed_tenths);
    let target = request_for(command, current)
        .ok()
        .flatten()
        .and_then(|request| match request {
            WalkingPadRequest::SetSpeed(tenths) => Some(tenths),
            _ => None,
        });
    let enabled = world
        .walkingpad
        .has_fresh_status(world.now.timestamp_millis())
        && world.walkingpad.pending.is_none()
        && target.is_some();
    KeyView::solid(theme::WALKINGPAD)
        .glyph(if increasing { Icon::Plus } else { Icon::Minus })
        .header(if increasing { "SPEED UP" } else { "SPEED DOWN" })
        .value(if increasing { "+0.2" } else { "−0.2" }, 27.0)
        .footer(
            target
                .map(|tenths| format!("NEXT {} KM/H", format_speed(tenths)))
                .unwrap_or_else(|| speed_control_hint(world, command)),
        )
        .status(if enabled {
            KeyStatus::Ok
        } else {
            KeyStatus::Disabled
        })
}

fn walkingpad_quick_speed(world: &WorldView, tenths: u8) -> KeyView {
    let command = WalkingPadCommand::SetSpeed(tenths);
    if let Some(error) = walkingpad_feedback(world, command) {
        return KeyView::solid(theme::ERROR)
            .glyph(Icon::Cross)
            .header("SET SPEED")
            .value(format_speed(tenths), 30.0)
            .footer(error)
            .status(KeyStatus::Error);
    }
    let selected = world
        .walkingpad
        .telemetry
        .as_ref()
        .is_some_and(|status| status.target_speed_tenths == tenths);
    let pending = world
        .walkingpad
        .pending
        .as_ref()
        .is_some_and(|pending| pending.request == WalkingPadRequest::SetSpeed(tenths));
    let enabled = world
        .walkingpad
        .has_fresh_status(world.now.timestamp_millis())
        && world
            .walkingpad
            .telemetry
            .as_ref()
            .is_some_and(|status| status.is_moving())
        && (world.walkingpad.pending.is_none() || pending);
    KeyView::solid(if selected || pending {
        theme::SELECTED
    } else {
        theme::WALKINGPAD
    })
    .header("SET SPEED")
    .value(format_speed(tenths), 31.0)
    .footer(if enabled { "KM/H" } else { "START FIRST" })
    .status(if pending {
        KeyStatus::Alert
    } else if selected {
        KeyStatus::Selected
    } else if enabled {
        KeyStatus::Ok
    } else {
        KeyStatus::Disabled
    })
}

fn walkingpad_session(world: &WorldView, metric: WalkingPadMetric) -> KeyView {
    let Some(telemetry) = &world.walkingpad.telemetry else {
        return KeyView::solid(theme::DISABLED)
            .header("SESSION")
            .value("—", 32.0)
            .footer(metric_label(metric))
            .status(KeyStatus::Disabled);
    };
    let (value, size) = match metric {
        WalkingPadMetric::Distance => (
            format_distance(u64::from(telemetry.counters.distance_hundredths)),
            25.0,
        ),
        WalkingPadMetric::Steps => (telemetry.counters.steps.to_string(), 29.0),
        WalkingPadMetric::Elapsed => (
            format_elapsed(u64::from(telemetry.counters.elapsed_seconds)),
            22.0,
        ),
    };
    KeyView::solid(theme::WALKINGPAD)
        .header("SESSION")
        .value(value, size)
        .footer(metric_label(metric))
        .status(walkingpad_telemetry_status(world))
}

fn walkingpad_daily(world: &WorldView, metric: WalkingPadMetric) -> KeyView {
    let totals = &world.walkingpad_daily;
    let (value, size) = match metric {
        WalkingPadMetric::Distance => (format_distance(totals.distance_hundredths), 25.0),
        WalkingPadMetric::Steps => (totals.steps.to_string(), 29.0),
        WalkingPadMetric::Elapsed => (format_elapsed(totals.elapsed_seconds), 22.0),
    };
    KeyView::solid(theme::WALKINGPAD)
        .header("TODAY")
        .value(value, size)
        .footer(metric_label(metric))
        .status(
            if world
                .walkingpad
                .has_fresh_status(world.now.timestamp_millis())
            {
                KeyStatus::Ok
            } else {
                KeyStatus::Stale
            },
        )
}

fn walkingpad_connection_label(world: &WorldView) -> (&'static str, KeyStatus) {
    match world.walkingpad.connection {
        WalkingPadConnection::Locked => ("LOCKED", KeyStatus::Error),
        WalkingPadConnection::Connecting => ("CONNECTING", KeyStatus::Loading),
        WalkingPadConnection::Disconnected => ("DISCONNECTED", KeyStatus::Disabled),
        WalkingPadConnection::Connected
            if !world
                .walkingpad
                .has_fresh_status(world.now.timestamp_millis()) =>
        {
            ("STALE", KeyStatus::Stale)
        }
        WalkingPadConnection::Connected => match world.walkingpad.telemetry.as_ref() {
            Some(status) if status.is_moving() => ("WALKING", KeyStatus::Selected),
            Some(_) => ("READY", KeyStatus::Ok),
            None => ("CONNECTED", KeyStatus::Loading),
        },
    }
}

fn walkingpad_telemetry_status(world: &WorldView) -> KeyStatus {
    if world
        .walkingpad
        .has_fresh_status(world.now.timestamp_millis())
    {
        KeyStatus::Ok
    } else if world.walkingpad.telemetry.is_some() {
        KeyStatus::Stale
    } else {
        KeyStatus::Disabled
    }
}

fn walkingpad_feedback(world: &WorldView, command: WalkingPadCommand) -> Option<String> {
    world.walkingpad.feedback.as_ref().and_then(|feedback| {
        (feedback.source == command).then(|| upper_short(&feedback.message, 20))
    })
}

fn stale_age(world: &WorldView) -> Option<String> {
    let age = world
        .walkingpad
        .status_age_ms(world.now.timestamp_millis())?;
    (age > crate::integrations::walkingpad::STATUS_STALE_AFTER_MS)
        .then(|| format!("STALE {}S", (age + 999) / 1_000))
}

fn speed_control_hint(world: &WorldView, command: WalkingPadCommand) -> String {
    let speed = world
        .walkingpad
        .telemetry
        .as_ref()
        .map(|status| status.speed_tenths);
    match (command, speed) {
        (_, Some(0)) => "START FIRST".to_string(),
        (WalkingPadCommand::Decrease, Some(5)) => "MIN 0.5 KM/H".to_string(),
        (WalkingPadCommand::Increase, Some(60)) => "MAX 6.0 KM/H".to_string(),
        _ => "WAIT FOR STATUS".to_string(),
    }
}

fn metric_label(metric: WalkingPadMetric) -> &'static str {
    match metric {
        WalkingPadMetric::Distance => "DISTANCE",
        WalkingPadMetric::Steps => "STEPS",
        WalkingPadMetric::Elapsed => "WALKING TIME",
    }
}

fn format_speed(tenths: u8) -> String {
    format!("{}.{:01}", tenths / 10, tenths % 10)
}

fn format_distance(hundredths: u64) -> String {
    format!("{}.{:02} KM", hundredths / 100, hundredths % 100)
}

fn format_elapsed(seconds: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3_600,
        (seconds / 60) % 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AudioTargetConfig;
    use crate::integrations::application::ApplicationInfo;
    use crate::integrations::audio::{AudioInventory, AudioSnapshot, AudioStatus};
    use crate::integrations::claude::{ClaudeUsage, UsageWindow};
    use crate::integrations::codex::{CodexUsage, CodexWindow};
    use crate::integrations::github::{parse_search, GitHubSnapshot};
    use crate::integrations::lake::{parse_history, LakeHistory};
    use crate::integrations::meetings::Meeting;
    use crate::integrations::walkingpad::{
        WalkingPadCounters, WalkingPadDailyTotals, WalkingPadMode, WalkingPadState,
        WalkingPadTelemetry,
    };
    use crate::integrations::weather::parse_forecast;
    use crate::pomodoro::{self, PomodoroState};
    use chrono::{DateTime, Utc};
    use chrono_tz::Europe::Stockholm;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-24T10:00:00Z")
            .expect("timestamp")
            .with_timezone(&Utc)
    }

    fn world() -> WorldView {
        WorldView::empty(now(), 1_000, Stockholm)
    }

    fn view(tile: Tile, world: &WorldView) -> KeyView {
        render(tile, &RenderContext::new(world))
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

    fn audio_snapshot(current_output: &str, output_muted: bool, input_volume: u8) -> AudioSnapshot {
        AudioSnapshot {
            status: Some(AudioStatus {
                current_output: current_output.to_string(),
                current_input: "MacBook Pro Microphone".to_string(),
                output_volume: 42,
                input_volume,
                output_muted,
            }),
            inventory: AudioInventory {
                outputs: vec![
                    "MacBook Pro Speakers".to_string(),
                    "Bose NC 700 Headphones".to_string(),
                ],
                inputs: vec!["MacBook Pro Microphone".to_string()],
            },
        }
    }

    #[test]
    fn a_blank_tile_is_a_real_drawn_key_with_no_content() {
        let view = view(Tile::Blank, &world());
        assert!(view.value.is_none());
        assert!(view.header.is_none());
        assert!(view.glyph.is_none());
    }

    #[test]
    fn a_loading_feed_produces_a_loading_tile_and_a_failed_feed_an_error_tile() {
        let mut world = world();
        assert_eq!(view(Tile::GitHubSummary, &world).status, KeyStatus::Loading);

        world.github = Feed::Failed("gh: not authenticated".to_string());
        let view = view(Tile::GitHubSummary, &world);
        assert_eq!(view.status, KeyStatus::Error);
        assert_eq!(view.glyph, Some(Icon::Warning));
    }

    #[test]
    fn stale_data_is_shown_with_a_stale_status_rather_than_an_error() {
        let mut world = world();
        world.github = Feed::Stale(GitHubSnapshot {
            prs: parse_search(
                include_str!("../../../../tests/fixtures/github-search-prs.json"),
                100,
            )
            .expect("parsed"),
            ..Default::default()
        });

        let view = view(Tile::GitHubSummary, &world);
        assert_eq!(view.status, KeyStatus::Stale);
        assert_eq!(view.rows[1], ("PR".to_string(), "6".to_string()));
    }

    #[test]
    fn the_mixer_summary_reports_devices_without_microphone_gain() {
        let mut world = world();
        world.audio = Feed::Ready(audio_snapshot("Bose NC 700 Headphones", false, 75));

        let view = view(Tile::MixerSummary, &world);
        assert_eq!(view.rows[0], ("BOSE".to_string(), "42%".to_string()));
        assert_eq!(view.rows[1], ("MIC MAC".to_string(), "ON".to_string()));
    }

    #[test]
    fn the_mixer_summary_recognizes_airpods() {
        let mut world = world();
        world.audio = Feed::Ready(audio_snapshot("Jimmy’s AirPods - Find My", false, 75));

        let view = view(Tile::MixerSummary, &world);
        assert_eq!(view.rows[0].0, "AIRPODS");
    }

    #[test]
    fn a_muted_output_and_a_zero_gain_microphone_both_read_off() {
        let mut world = world();
        world.audio = Feed::Ready(audio_snapshot("MacBook Pro Speakers", true, 0));

        let view = view(Tile::MixerSummary, &world);
        assert_eq!(view.rows[0].1, "OFF");
        assert_eq!(view.rows[1].1, "OFF");
    }

    #[test]
    fn a_selected_device_tile_reads_active_and_an_absent_one_stays_visible_but_disabled() {
        let mut world = world();
        world.audio = Feed::Ready(audio_snapshot("Bose NC 700 Headphones", false, 50));
        let output = targets(&[
            ("MacBook", Some("MacBook Pro Speakers"), None),
            ("Bose", Some("Bose NC 700 Headphones"), None),
            ("USB Home", Some("USB audio CODEC"), None),
            ("AirPods", Some("Jimmy’s AirPods - Find My"), None),
        ]);
        let context = RenderContext::new(&world).with_audio(&output, &[]);

        let bose = render(
            Tile::AudioDevice {
                kind: AudioKind::Output,
                index: 1,
            },
            &context,
        );
        assert_eq!(bose.status, KeyStatus::Selected);
        assert_eq!(bose.footer_center.expect("footer").text, "ACTIVE");

        let macbook = render(
            Tile::AudioDevice {
                kind: AudioKind::Output,
                index: 0,
            },
            &context,
        );
        assert_eq!(macbook.status, KeyStatus::Ok);

        let usb = render(
            Tile::AudioDevice {
                kind: AudioKind::Output,
                index: 2,
            },
            &context,
        );
        assert_eq!(usb.status, KeyStatus::Disabled);
        assert_eq!(usb.footer_center.expect("footer").text, "OFFLINE");
        assert!(usb.header.is_some(), "an absent device keeps its label");
    }

    #[test]
    fn an_ambiguous_device_tile_asks_the_user_to_check_the_name() {
        let mut world = world();
        let mut snapshot = audio_snapshot("MacBook Pro Speakers", false, 50);
        snapshot.inventory.outputs = vec!["Scarlett USB".to_string(), "RØDE USB".to_string()];
        world.audio = Feed::Ready(snapshot);
        let output = targets(&[("USB Home", None, Some("usb"))]);
        let context = RenderContext::new(&world).with_audio(&output, &[]);

        let view = render(
            Tile::AudioDevice {
                kind: AudioKind::Output,
                index: 0,
            },
            &context,
        );
        assert_eq!(view.status, KeyStatus::Ambiguous);
        assert_eq!(view.footer_center.expect("footer").text, "CHECK NAME");
    }

    #[test]
    fn a_device_tile_with_no_configured_target_renders_blank() {
        let world = world();
        let view = render(
            Tile::AudioDevice {
                kind: AudioKind::Output,
                index: 7,
            },
            &RenderContext::new(&world),
        );
        assert!(view.header.is_none());
    }

    #[test]
    fn mute_tiles_show_their_state_for_both_kinds() {
        let mut world = world();
        world.audio = Feed::Ready(audio_snapshot("MacBook Pro Speakers", true, 0));

        let output = view(Tile::AudioMute(AudioKind::Output), &world);
        assert_eq!(output.value.expect("value").text, "OFF");
        assert_eq!(output.glyph, Some(Icon::SpeakerMuted));
        assert_eq!(output.status, KeyStatus::Alert);

        let input = view(Tile::AudioMute(AudioKind::Input), &world);
        assert_eq!(input.glyph, Some(Icon::MicrophoneMuted));

        world.audio = Feed::Ready(audio_snapshot("MacBook Pro Speakers", false, 60));
        let live = view(Tile::AudioMute(AudioKind::Output), &world);
        assert_eq!(live.value.expect("value").text, "ON");
        assert_eq!(live.status, KeyStatus::Selected);
    }

    #[test]
    fn volume_tiles_label_their_direction_and_kind() {
        let world = world();
        let up = view(
            Tile::AudioVolume {
                kind: AudioKind::Output,
                delta: 10,
            },
            &world,
        );
        assert_eq!(up.header.expect("header").text, "VOLUME");
        assert_eq!(up.glyph, Some(Icon::Plus));
        assert_eq!(up.footer_center.expect("footer").text, "+10");

        let down = view(
            Tile::AudioVolume {
                kind: AudioKind::Input,
                delta: -10,
            },
            &world,
        );
        assert_eq!(down.header.expect("header").text, "MIC GAIN");
        assert_eq!(down.glyph, Some(Icon::Minus));
        assert_eq!(down.footer_center.expect("footer").text, "-10");
    }

    #[test]
    fn usage_tiles_report_percentage_reset_and_severity() {
        let mut world = world();
        world.claude = Feed::Ready(ClaudeUsage {
            five_hour: Some(UsageWindow {
                percent: 91.0,
                resets_at: Some(now() + chrono::Duration::minutes(90)),
            }),
            seven_day: Some(UsageWindow {
                percent: 33.0,
                resets_at: None,
            }),
        });

        let five = view(Tile::ClaudeFiveHour, &world);
        assert_eq!(five.value.expect("value").text, "91%");
        assert_eq!(five.header_right.expect("window").text, "5H");
        assert_eq!(five.footer_center.expect("footer").text, "RESET 1H 30M");
        assert_eq!(five.background.representative(), theme::CRITICAL);

        let seven = view(Tile::ClaudeSevenDay, &world);
        assert_eq!(seven.header.expect("header").text, "CLAUDE");
        assert_eq!(seven.header_right.expect("window").text, "7D");
        assert_eq!(seven.value.expect("value").text, "33%");
        assert_eq!(seven.footer_center.expect("footer").text, "RESET —");
    }

    #[test]
    fn a_missing_claude_window_renders_disabled_rather_than_wrong() {
        let mut world = world();
        world.claude = Feed::Ready(ClaudeUsage {
            five_hour: Some(UsageWindow {
                percent: 10.0,
                resets_at: None,
            }),
            seven_day: None,
        });

        let view = view(Tile::ClaudeSevenDay, &world);
        assert_eq!(view.status, KeyStatus::Disabled);
        assert_eq!(view.value.expect("value").text, "—");
    }

    #[test]
    fn the_codex_five_hour_tile_shows_its_window_or_a_quiet_placeholder() {
        let mut world = world();
        // The live payload has been seen with only the weekly window.
        world.codex = Feed::Ready(CodexUsage {
            plan: Some("pro".to_string()),
            primary: Some(CodexWindow {
                percent: 44.0,
                window_seconds: 604_800,
                resets_at: None,
            }),
            secondary: None,
            limit_reached: false,
        });
        let missing = view(Tile::CodexFiveHour, &world);
        assert_eq!(missing.status, KeyStatus::Disabled);
        assert_eq!(missing.value.expect("value").text, "—");
        assert_eq!(missing.header_right.expect("window").text, "5H");

        world.codex = Feed::Ready(CodexUsage {
            plan: Some("pro".to_string()),
            primary: Some(CodexWindow {
                percent: 44.0,
                window_seconds: 604_800,
                resets_at: None,
            }),
            secondary: Some(CodexWindow {
                percent: 61.0,
                window_seconds: 18_000,
                resets_at: Some(now() + chrono::Duration::minutes(150)),
            }),
            limit_reached: false,
        });
        let present = view(Tile::CodexFiveHour, &world);
        assert_eq!(present.header.expect("header").text, "CODEX");
        assert_eq!(present.header_right.expect("window").text, "5H");
        assert_eq!(present.value.expect("value").text, "61%");
        assert_eq!(present.footer_center.expect("footer").text, "RESET 2H 30M");
        assert_eq!(present.background.representative(), theme::WARNING);
    }

    #[test]
    fn a_reached_codex_limit_becomes_an_alert() {
        let mut world = world();
        world.codex = Feed::Ready(CodexUsage {
            plan: Some("pro".to_string()),
            primary: Some(CodexWindow {
                percent: 100.0,
                window_seconds: 604_800,
                resets_at: None,
            }),
            secondary: None,
            limit_reached: true,
        });

        let view = view(Tile::CodexUsage, &world);
        assert_eq!(view.status, KeyStatus::Alert);
        assert_eq!(view.footer_center.expect("footer").text, "LIMIT REACHED");
        assert_eq!(view.header_right.expect("window").text, "7D");
    }

    #[test]
    fn meeting_tiles_show_title_time_and_countdown() {
        let mut world = world();
        world.meetings = Feed::Ready(vec![
            Meeting {
                account: "a@example.com".to_string(),
                title: "Sprint planning".to_string(),
                start: now() + chrono::Duration::minutes(42),
                end: now() + chrono::Duration::minutes(102),
                meet_url: "https://meet.google.com/aaa-bbbb-ccc".to_string(),
            },
            Meeting {
                account: "b@example.com".to_string(),
                title: "Architecture review with a very long name".to_string(),
                start: now() + chrono::Duration::hours(26),
                end: now() + chrono::Duration::hours(27),
                meet_url: "https://meet.google.com/ddd-eeee-fff".to_string(),
            },
        ]);

        let next = view(Tile::Meeting(0), &world);
        assert_eq!(next.header.expect("title").text, "Sprint plann…");
        assert_eq!(next.value.expect("time").text, "12:42");
        assert_eq!(next.subvalue.expect("status").text, "IN 42M");
        assert_eq!(next.badge.expect("badge").text, "1");
        assert_eq!(next.background.representative(), theme::MEETING_NEXT);

        let following = view(Tile::Meeting(1), &world);
        assert_eq!(following.subvalue.expect("status").text, "TOMORROW");
        assert_eq!(following.badge.expect("badge").text, "2");
        assert_eq!(
            following.background.representative(),
            theme::MEETING_FOLLOWING
        );
    }

    #[test]
    fn an_ongoing_meeting_turns_green_and_says_now() {
        let mut world = world();
        world.meetings = Feed::Ready(vec![Meeting {
            account: "a@example.com".to_string(),
            title: "Incident bridge".to_string(),
            start: now() - chrono::Duration::minutes(10),
            end: now() + chrono::Duration::minutes(20),
            meet_url: "https://meet.google.com/aaa-bbbb-ccc".to_string(),
        }]);

        let view = view(Tile::Meeting(0), &world);
        assert_eq!(view.subvalue.expect("status").text, "NOW");
        assert_eq!(view.background.representative(), theme::MEETING_NOW);
    }

    #[test]
    fn an_empty_meeting_list_still_renders_both_tiles() {
        let mut world = world();
        world.meetings = Feed::Ready(Vec::new());

        let first = view(Tile::Meeting(0), &world);
        assert_eq!(first.header.expect("header").text, "NO UPCOMING");
        assert_eq!(first.status, KeyStatus::Disabled);

        let second = view(Tile::Meeting(1), &world);
        assert_eq!(second.header.expect("header").text, "NO LATER");
    }

    #[test]
    fn weather_tiles_reserve_separate_icon_and_value_regions() {
        let mut world = world();
        world.weather = Feed::Ready(
            parse_forecast(
                include_str!("../../../../tests/fixtures/met-locationforecast.json"),
                "Stensjön",
                Stockholm,
            )
            .expect("parsed"),
        );

        let current = view(Tile::WeatherCurrent, &world);
        assert_eq!(current.header.expect("header").text, "STENSJÖN");
        assert_eq!(current.value.expect("value").text, "19°");
        assert_eq!(current.art, Some(Icon::Cloud));
        assert_eq!(current.footer_left.expect("high").text, "H 23°");
        assert_eq!(current.footer_right.expect("low").text, "L 15°");

        let forecast = view(Tile::WeatherForecast(1), &world);
        assert_eq!(forecast.value.expect("value").text, "21°/11°");
        assert_eq!(forecast.header.expect("weekday").text, "SAT");
        assert_eq!(forecast.footer_left.expect("rain").text, "DRY");
    }

    #[test]
    fn home_weather_changes_from_current_conditions_to_tomorrow_at_seventeen() {
        let mut world = world();
        world.weather = Feed::Ready(
            parse_forecast(
                include_str!("../../../../tests/fixtures/met-locationforecast.json"),
                "Stensjön",
                Stockholm,
            )
            .expect("parsed"),
        );

        let before = view(Tile::WeatherGlance, &world);
        assert_eq!(before.header.expect("day").text, "TODAY");
        assert_eq!(before.value.expect("temperature").text, "19°");

        world.now = DateTime::parse_from_rfc3339("2026-07-24T15:00:00Z")
            .expect("timestamp")
            .with_timezone(&Utc);
        let after = view(Tile::WeatherGlance, &world);
        assert_eq!(after.header.expect("day").text, "TOMORROW");
        assert_eq!(after.value.expect("range").text, "21°/11°");
    }

    #[test]
    fn weather_page_days_name_today_and_tomorrow_explicitly() {
        let mut world = world();
        world.weather = Feed::Ready(
            parse_forecast(
                include_str!("../../../../tests/fixtures/met-locationforecast.json"),
                "Stensjön",
                Stockholm,
            )
            .expect("parsed"),
        );

        assert_eq!(
            view(Tile::WeatherDay(0), &world)
                .header
                .expect("today")
                .text,
            "TODAY"
        );
        assert_eq!(
            view(Tile::WeatherDay(1), &world)
                .header
                .expect("tomorrow")
                .text,
            "TOMORROW"
        );
    }

    #[test]
    fn media_owner_prefers_the_resolved_service_over_its_browser() {
        let mut world = world();
        world.media = Feed::Ready(MediaStatus {
            application: Some("Google Chrome".to_string()),
            source: Some("YouTube".to_string()),
            title: Some("Deep Work Music".to_string()),
        });

        let view = view(Tile::MediaSource, &world);
        assert_eq!(view.value.expect("owner").text, "YOUTUBE");
        assert_eq!(view.footer_center.expect("title").text, "Deep Work Music");
    }

    #[test]
    fn current_application_and_its_context_actions_are_readable() {
        let mut world = world();
        world.application = Feed::Ready(ApplicationInfo {
            name: "Google Chrome".to_string(),
            bundle_id: Some("com.google.Chrome".to_string()),
            pid: 42,
        });

        let glance = view(Tile::CurrentApplication, &world);
        assert_eq!(glance.value.expect("application").text, "GOOGLE CHROME");
        assert_eq!(
            glance.footer_center.expect("affordance").text,
            "TAP FOR ACTIONS"
        );

        let incognito = view(Tile::ApplicationContext(1), &world);
        assert_eq!(incognito.glyph, Some(Icon::Application));
        assert_eq!(incognito.footer_center.expect("context").text, "NEW WINDOW");

        world.recent_applications.push(ApplicationInfo {
            name: "Ghostty".to_string(),
            bundle_id: Some("com.mitchellh.ghostty".to_string()),
            pid: 99,
        });
        let recent = view(Tile::ApplicationRecent(0), &world);
        assert_eq!(recent.value.expect("recent application").text, "GHOSTTY");
    }

    #[test]
    fn a_pressed_weather_tile_shows_the_full_reading() {
        let mut world = world();
        world.weather = Feed::Ready(
            parse_forecast(
                include_str!("../../../../tests/fixtures/met-locationforecast.json"),
                "Stensjön",
                Stockholm,
            )
            .expect("parsed"),
        );
        world.weather_detail = Some(WeatherTile::Current);

        let detail = view(Tile::WeatherCurrent, &world);
        assert_eq!(detail.header_right.expect("badge").text, "NOW");
        assert_eq!(
            detail.rows,
            vec![
                ("HUM".to_string(), "71%".to_string()),
                ("WIND".to_string(), "SW 4 m/s".to_string()),
                ("RAIN".to_string(), "0.4 mm".to_string()),
                ("RANGE".to_string(), "15°–23°".to_string()),
            ]
        );
        assert!(detail.value.is_none(), "the rows replace the big value");

        // Only the pressed tile flips; the forecast tile stays compact.
        let forecast = view(Tile::WeatherForecast(1), &world);
        assert!(forecast.rows.is_empty());

        world.weather_detail = Some(WeatherTile::Forecast);
        let forecast_detail = view(Tile::WeatherForecast(1), &world);
        assert_eq!(
            forecast_detail.rows,
            vec![
                ("SKY".to_string(), "CLEAR".to_string()),
                ("HIGH".to_string(), "21°".to_string()),
                ("LOW".to_string(), "11°".to_string()),
                ("RAIN".to_string(), "0.0 mm".to_string()),
            ]
        );
        let current = view(Tile::WeatherCurrent, &world);
        assert!(current.rows.is_empty());
    }

    #[test]
    fn a_night_symbol_selects_the_moon_and_a_night_sky() {
        let mut world = world();
        let body = r#"{"properties":{"meta":{"updated_at":"2026-07-24T22:00:00Z"},"timeseries":[
            {"time":"2026-07-24T22:00:00Z","data":{"instant":{"details":{"air_temperature":11.0,
              "relative_humidity":90,"wind_speed":1.0,"wind_from_direction":0}},
              "next_1_hours":{"summary":{"symbol_code":"clearsky_night"},"details":{"precipitation_amount":0}}}}
        ]}}"#;
        world.weather = Feed::Ready(parse_forecast(body, "Stensjön", Stockholm).expect("parsed"));

        let view = view(Tile::WeatherCurrent, &world);
        assert_eq!(view.art, Some(Icon::Moon));
    }

    #[test]
    fn the_water_tile_colours_by_temperature_band_and_shows_the_reading_time() {
        let mut world = world();
        world.lake_current = Feed::Ready(LakeReading {
            measured_at: now() - chrono::Duration::minutes(30),
            temperature: 21.3,
        });

        let view = view(Tile::LakeCurrent, &world);
        assert_eq!(view.value.expect("value").text, "21.3°");
        assert_eq!(view.footer_center.expect("footer").text, "11:30");
        assert_eq!(view.art, Some(Icon::Water));
    }

    #[test]
    fn the_seven_day_trend_signs_its_change_and_picks_a_direction_icon() {
        let mut world = world();
        world.lake_history = Feed::Ready(
            parse_history(
                include_str!("../../../../tests/fixtures/lake-historic.json"),
                "A84041BDC1864B41",
                now() + chrono::Duration::days(1),
            )
            .expect("parsed"),
        );

        let rising = view(Tile::LakeTrend, &world);
        assert_eq!(rising.value.expect("value").text, "+3.1°");
        assert_eq!(rising.art, Some(Icon::TrendUp));

        world.lake_history = Feed::Ready(LakeHistory {
            days: vec![
                LakeReading {
                    measured_at: now(),
                    temperature: 15.0,
                },
                LakeReading {
                    measured_at: now() - chrono::Duration::days(6),
                    temperature: 18.0,
                },
            ],
        });
        let falling = view(Tile::LakeTrend, &world);
        assert_eq!(falling.value.expect("value").text, "-3.0°");
        assert_eq!(falling.art, Some(Icon::TrendDown));
    }

    #[test]
    fn history_day_tiles_beyond_the_available_data_render_disabled() {
        let mut world = world();
        world.lake_history = Feed::Ready(LakeHistory {
            days: vec![LakeReading {
                measured_at: now(),
                temperature: 20.0,
            }],
        });

        assert_eq!(view(Tile::LakeDay(0), &world).status, KeyStatus::Ok);
        assert_eq!(view(Tile::LakeDay(3), &world).status, KeyStatus::Disabled);
    }

    #[test]
    fn the_panel_countdown_tracks_the_remaining_seconds() {
        let mut world = world();
        world.panel_total_seconds = 10;
        world.panel_seconds_remaining = Some(4);

        let view = view(Tile::PanelCountdown, &world);
        assert_eq!(view.value.expect("value").text, "4");
        assert!((view.progress.expect("progress").fraction - 0.4).abs() < 1e-6);
    }

    #[test]
    fn github_metric_tiles_show_counts_and_flag_the_capped_inbox() {
        let mut world = world();
        world.github = Feed::Ready(GitHubSnapshot {
            reviews: parse_search(
                include_str!("../../../../tests/fixtures/github-search-prs.json"),
                2,
            )
            .expect("parsed"),
            inbox_count: 100,
            inbox_overflow: true,
            updated_since: "2026-06-24".to_string(),
            ..Default::default()
        });

        let reviews = view(Tile::GitHubMetric(MetricKind::Reviews), &world);
        assert_eq!(reviews.value.expect("value").text, "2");
        assert_eq!(reviews.background.representative(), theme::GITHUB_ACTIVE);

        let inbox = view(Tile::GitHubMetric(MetricKind::Inbox), &world);
        assert_eq!(inbox.value.expect("value").text, "99+");
        assert_eq!(inbox.footer_center.expect("footer").text, "CAPPED AT 100");
    }

    #[test]
    fn a_zero_review_count_uses_the_quiet_colour() {
        let mut world = world();
        world.github = Feed::Ready(GitHubSnapshot::default());
        let reviews = view(Tile::GitHubMetric(MetricKind::Reviews), &world);
        assert_eq!(reviews.value.expect("value").text, "0");
        assert_eq!(reviews.background.representative(), theme::GITHUB);
    }

    #[test]
    fn github_item_tiles_shorten_the_repository_name() {
        let mut world = world();
        world.repository_aliases =
            vec![("visma.administration.".to_string(), "admin.".to_string())];
        world.github = Feed::Ready(GitHubSnapshot {
            prs: parse_search(
                include_str!("../../../../tests/fixtures/github-search-prs.json"),
                100,
            )
            .expect("parsed"),
            ..Default::default()
        });

        let first = view(Tile::GitHubItem(0), &world);
        assert_eq!(first.header.expect("header").text, "admin.web");
        assert_eq!(first.value.expect("value").text, "#4821");

        let empty = view(Tile::GitHubItem(9), &world);
        assert_eq!(empty.status, KeyStatus::Disabled);
        assert_eq!(empty.header.expect("header").text, "NO ITEM");
    }

    #[test]
    fn spotify_controls_are_visible_but_disabled_when_spotify_is_not_running() {
        let mut world = world();
        world.spotify = Feed::Ready(SpotifyStatus::not_running());

        for command in [
            SpotifyCommand::Previous,
            SpotifyCommand::Next,
            SpotifyCommand::PlayPause,
            SpotifyCommand::Volume(5),
            SpotifyCommand::Seek(-15),
            SpotifyCommand::Seek(15),
        ] {
            let view = view(Tile::SpotifyControl(command), &world);
            assert_eq!(view.status, KeyStatus::Disabled, "{command:?}");
        }

        // Opening the app is the one control that still works.
        let open = view(Tile::SpotifyControl(SpotifyCommand::OpenApp), &world);
        assert_eq!(open.status, KeyStatus::Ok);
    }

    #[test]
    fn a_playing_track_selects_the_pause_glyph_and_requests_artwork() {
        let mut world = world();
        world.spotify = Feed::Ready(
            crate::integrations::spotify::parse_status(
                "playing\tTruth\tKamasi Washington\tThe Epic\thttps://i.scdn.co/image/abc\tspotify:track:1\t72\ttrue\tone",
            )
            .expect("parsed"),
        );

        let play = view(Tile::SpotifyControl(SpotifyCommand::PlayPause), &world);
        assert_eq!(play.glyph, Some(Icon::Pause));
        assert_eq!(play.artwork.as_deref(), Some("spotify:track:1"));
        assert_eq!(play.background.representative(), theme::SPOTIFY);

        let seek = view(Tile::SpotifyControl(SpotifyCommand::Seek(-15)), &world);
        assert_eq!(seek.header.expect("header").text, "BACK");
        assert_eq!(seek.value.expect("value").text, "15s");

        let volume = view(Tile::SpotifyControl(SpotifyCommand::Volume(-5)), &world);
        assert_eq!(volume.value.expect("value").text, "72%");
    }

    #[test]
    fn a_paused_track_offers_play_and_configured_playlists_are_named() {
        let mut world = world();
        world.spotify = Feed::Ready(
            crate::integrations::spotify::parse_status(
                "paused\tTruth\tKamasi\tThe Epic\t\tspotify:track:1\t50\tfalse\toff",
            )
            .expect("parsed"),
        );
        assert_eq!(
            view(Tile::SpotifyControl(SpotifyCommand::PlayPause), &world).glyph,
            Some(Icon::Play)
        );
        assert_eq!(view(Tile::SpotifyGlance, &world).glyph, Some(Icon::Play));

        let playlists = [SpotifyPlaylistConfig {
            label: "Deep Focus".to_string(),
            uri: "spotify:playlist:1jnizfcJFNGVeJgmp7ngK9".to_string(),
        }];
        let playlist = render(
            Tile::SpotifyPlaylist(0),
            &RenderContext::new(&world).with_spotify_playlists(&playlists),
        );
        assert_eq!(playlist.header.expect("header").text, "DEEP FOCUS");
        assert_eq!(
            playlist.footer_center.expect("footer").text,
            "PLAY PLAYLIST"
        );
    }

    #[test]
    fn a_track_without_artwork_does_not_request_an_image() {
        let mut world = world();
        world.spotify = Feed::Ready(
            crate::integrations::spotify::parse_status(
                "playing\tTruth\tKamasi\tThe Epic\t\tspotify:track:1\t50\tfalse\toff",
            )
            .expect("parsed"),
        );
        assert_eq!(
            view(Tile::SpotifyControl(SpotifyCommand::PlayPause), &world).artwork,
            None
        );
    }

    #[test]
    fn wispr_tile_distinguishes_idle_and_hands_free() {
        let mut world = world();

        let idle = view(Tile::WisprGlance, &world);
        assert_eq!(idle.background.representative(), theme::WISPR);
        assert_eq!(idle.glyph, Some(Icon::Microphone));
        assert_eq!(
            idle.footer_center.expect("footer").text,
            "TAP START · HOLD MICS"
        );

        world.wispr_hands_free = true;
        let hands_free = view(Tile::WisprGlance, &world);
        assert_eq!(hands_free.status, KeyStatus::Selected);
        assert_eq!(
            hands_free.footer_center.expect("footer").text,
            "TAP TO STOP"
        );
    }

    #[test]
    fn wispr_microphone_tile_uses_the_configured_label() {
        let world = world();
        let microphones = vec![crate::config::WisprMicrophoneConfig {
            label: "RØDE".to_string(),
            name: "RODE NT-USB".to_string(),
        }];
        let selected = render(
            Tile::WisprMicrophone(0),
            &RenderContext::new(&world).with_wispr_microphones(&microphones),
        );
        assert_eq!(selected.header.expect("header").text, "RØDE");
        assert_eq!(selected.footer_center.expect("footer").text, "SELECT");

        assert_eq!(
            render(
                Tile::WisprMicrophone(1),
                &RenderContext::new(&world).with_wispr_microphones(&microphones),
            ),
            KeyView::blank()
        );
    }

    #[test]
    fn the_pomodoro_timer_shows_a_monospaced_countdown_and_progress_ring() {
        let mut world = world();
        let mut state = PomodoroState::default();
        pomodoro::toggle(&mut state, now().timestamp_millis(), Stockholm);
        world.pomodoro =
            pomodoro::snapshot(&state, now().timestamp_millis() + 5 * 60 * 1_000, Stockholm);

        let view = view(Tile::PomodoroTimer, &world);
        let value = view.value.expect("value");
        assert_eq!(value.text, "20:00");
        assert_eq!(value.family, crate::view::FontFamily::Mono);
        assert_eq!(view.subvalue.expect("status").text, "RUNNING");
        assert!((view.progress.expect("progress").fraction - 0.8).abs() < 1e-3);
        assert_eq!(view.header.expect("phase").text, "FOCUS");
    }

    #[test]
    fn a_pending_completion_turns_the_timer_tiles_into_an_alert() {
        let mut world = world();
        let mut state = PomodoroState::default();
        pomodoro::toggle(&mut state, now().timestamp_millis(), Stockholm);
        pomodoro::reconcile(
            &mut state,
            now().timestamp_millis() + 25 * 60 * 1_000,
            Stockholm,
        );
        world.pomodoro = pomodoro::snapshot(&state, now().timestamp_millis(), Stockholm);
        world.pomodoro_alert_flashing = true;

        for tile in [
            Tile::PomodoroTimer,
            Tile::PomodoroToggle,
            Tile::PomodoroGlance,
        ] {
            let view = view(tile, &world);
            assert_eq!(view.status, KeyStatus::Alert, "{tile:?}");
            assert_eq!(view.glyph, Some(Icon::Check), "{tile:?}");
            assert_eq!(view.background.representative(), theme::ALERT, "{tile:?}");
        }

        world.pomodoro_alert_flashing = false;
        let steady = view(Tile::PomodoroTimer, &world);
        assert_eq!(steady.background.representative(), theme::CRITICAL);
        assert_eq!(steady.header.expect("header").text, "FOCUS DONE");
    }

    #[test]
    fn the_toggle_tile_reads_start_pause_and_resume() {
        let mut world = world();
        let mut state = PomodoroState::default();

        world.pomodoro = pomodoro::snapshot(&state, now().timestamp_millis(), Stockholm);
        assert_eq!(
            view(Tile::PomodoroToggle, &world).header.expect("h").text,
            "START"
        );

        pomodoro::toggle(&mut state, now().timestamp_millis(), Stockholm);
        world.pomodoro = pomodoro::snapshot(&state, now().timestamp_millis(), Stockholm);
        assert_eq!(
            view(Tile::PomodoroToggle, &world).header.expect("h").text,
            "PAUSE"
        );

        pomodoro::toggle(&mut state, now().timestamp_millis() + 1_000, Stockholm);
        world.pomodoro = pomodoro::snapshot(&state, now().timestamp_millis(), Stockholm);
        assert_eq!(
            view(Tile::PomodoroToggle, &world).header.expect("h").text,
            "RESUME"
        );
    }

    #[test]
    fn start_tiles_show_the_configured_phase_length() {
        let world = world();
        assert_eq!(
            view(Tile::PomodoroStart(Phase::Focus), &world)
                .value
                .expect("value")
                .text,
            "25m"
        );
        assert_eq!(
            view(Tile::PomodoroStart(Phase::LongBreak), &world)
                .value
                .expect("value")
                .text,
            "15m"
        );
    }

    #[test]
    fn duration_tiles_promise_the_values_a_press_will_set_including_wraparound() {
        let mut world = world();
        world.pomodoro.focus_minutes = 90;

        let view = view(
            Tile::PomodoroAdjust {
                duration: Phase::Focus,
                step_minutes: 5,
            },
            &world,
        );
        assert_eq!(view.value.expect("value").text, "90m");
        assert_eq!(
            view.footer_center.expect("footer").text,
            "TAP 5m · HOLD 85m"
        );
    }

    #[test]
    fn wrapped_duration_matches_the_state_machine_at_the_bounds() {
        assert_eq!(wrapped_duration(Phase::Focus, 90, 5), 5);
        assert_eq!(wrapped_duration(Phase::Focus, 5, -5), 90);
        assert_eq!(wrapped_duration(Phase::Focus, 25, 5), 30);
        assert_eq!(wrapped_duration(Phase::ShortBreak, 30, 1), 1);
        assert_eq!(wrapped_duration(Phase::LongBreak, 60, 5), 5);
    }

    #[test]
    fn statistics_tiles_render_every_scope() {
        let mut world = world();
        world.pomodoro.cycle_focus_sessions = 3;
        world.pomodoro.completed_short_breaks = 9;
        world.pomodoro.completed_long_breaks = 2;
        world.pomodoro.today_focus_sessions = 5;
        world.pomodoro.today_focus_minutes = 125;
        world.pomodoro.completed_focus_sessions = 118;
        world.pomodoro.total_focus_minutes = 3_120;

        assert_eq!(
            view(Tile::PomodoroStats(StatsScope::Cycle), &world)
                .value
                .expect("value")
                .text,
            "3/4"
        );
        assert_eq!(
            view(Tile::PomodoroStats(StatsScope::Breaks), &world)
                .value
                .expect("value")
                .text,
            "9/2"
        );
        assert_eq!(
            view(Tile::PomodoroStats(StatsScope::Today), &world)
                .footer_center
                .expect("footer")
                .text,
            "125 FOCUS MIN"
        );
        assert_eq!(
            view(Tile::PomodoroStats(StatsScope::AllTime), &world)
                .footer_center
                .expect("footer")
                .text,
            "52 FOCUS HOURS"
        );
    }

    #[test]
    fn walkingpad_tiles_distinguish_connected_stale_and_disconnected_state() {
        let mut world = world();
        world.walkingpad = WalkingPadState {
            connection: WalkingPadConnection::Connected,
            telemetry: Some(WalkingPadTelemetry {
                counters: WalkingPadCounters {
                    distance_hundredths: 152,
                    steps: 2_345,
                    elapsed_seconds: 3_661,
                },
                speed_tenths: 34,
                target_speed_tenths: 34,
                belt_state: 1,
                mode: WalkingPadMode::Manual,
            }),
            last_status_at_ms: Some(world.now.timestamp_millis()),
            ..WalkingPadState::default()
        };

        let connected = view(Tile::WalkingPadConnection, &world);
        assert_eq!(connected.value.expect("state").text, "WALKING");
        assert_eq!(connected.status, KeyStatus::Selected);
        assert_eq!(
            view(Tile::WalkingPadSpeed, &world)
                .value
                .expect("speed")
                .text,
            "3.4"
        );
        assert_eq!(
            view(Tile::WalkingPadQuickSpeed(34), &world).status,
            KeyStatus::Selected
        );

        world.walkingpad.last_status_at_ms = Some(world.now.timestamp_millis() - 5_000);
        let stale = view(Tile::WalkingPadConnection, &world);
        assert_eq!(stale.value.expect("state").text, "STALE");
        assert_eq!(stale.status, KeyStatus::Stale);
        assert_eq!(
            view(
                Tile::WalkingPadSpeedAdjust(WalkingPadCommand::Increase),
                &world,
            )
            .status,
            KeyStatus::Disabled
        );

        world.walkingpad.connection = WalkingPadConnection::Disconnected;
        let disconnected = view(Tile::WalkingPadConnection, &world);
        assert_eq!(disconnected.value.expect("state").text, "DISCONNECTED");
        assert_eq!(disconnected.status, KeyStatus::Disabled);
    }

    #[test]
    fn standby_offers_wake_without_sending_another_stop() {
        let mut world = world();
        world.walkingpad = WalkingPadState {
            connection: WalkingPadConnection::Connected,
            telemetry: Some(WalkingPadTelemetry {
                counters: WalkingPadCounters::default(),
                speed_tenths: 0,
                target_speed_tenths: 0,
                belt_state: 5,
                mode: WalkingPadMode::Standby,
            }),
            last_status_at_ms: Some(world.now.timestamp_millis()),
            ..WalkingPadState::default()
        };

        let start = view(Tile::WalkingPadStart, &world);
        assert_eq!(start.value.expect("start label").text, "WAKE");
        assert_eq!(
            start.footer_center.expect("start footer").text,
            "THEN START"
        );
        assert_eq!(start.status, KeyStatus::Ok);

        let stop = view(Tile::WalkingPadStop, &world);
        assert_eq!(stop.value.expect("stop label").text, "STOPPED");
        assert_eq!(
            stop.footer_center.expect("stop footer").text,
            "NO COMMAND SENT"
        );
        assert_eq!(stop.status, KeyStatus::Disabled);
    }

    #[test]
    fn walkingpad_metrics_use_exact_requested_formats() {
        let mut world = world();
        world.walkingpad = WalkingPadState {
            connection: WalkingPadConnection::Connected,
            telemetry: Some(WalkingPadTelemetry {
                counters: WalkingPadCounters {
                    distance_hundredths: 152,
                    steps: 2_345,
                    elapsed_seconds: 3_661,
                },
                speed_tenths: 34,
                target_speed_tenths: 34,
                belt_state: 1,
                mode: WalkingPadMode::Manual,
            }),
            last_status_at_ms: Some(world.now.timestamp_millis()),
            ..WalkingPadState::default()
        };
        world.walkingpad_daily = WalkingPadDailyTotals {
            date: "2026-07-24".to_string(),
            distance_hundredths: 987,
            steps: 12_345,
            elapsed_seconds: 7_322,
            ..WalkingPadDailyTotals::default()
        };

        assert_eq!(
            view(Tile::WalkingPadSession(WalkingPadMetric::Distance), &world,)
                .value
                .expect("distance")
                .text,
            "1.52 KM"
        );
        assert_eq!(
            view(Tile::WalkingPadSession(WalkingPadMetric::Steps), &world)
                .value
                .expect("steps")
                .text,
            "2345"
        );
        assert_eq!(
            view(Tile::WalkingPadSession(WalkingPadMetric::Elapsed), &world,)
                .value
                .expect("elapsed")
                .text,
            "01:01:01"
        );
        assert_eq!(
            view(Tile::WalkingPadDaily(WalkingPadMetric::Distance), &world)
                .value
                .expect("distance")
                .text,
            "9.87 KM"
        );
        assert_eq!(
            view(Tile::WalkingPadDaily(WalkingPadMetric::Elapsed), &world)
                .value
                .expect("elapsed")
                .text,
            "02:02:02"
        );
    }

    #[test]
    fn every_tile_on_every_page_renders_without_data() {
        let world = world();
        let output = targets(&[
            ("MacBook", Some("MacBook Pro Speakers"), None),
            ("Bose", Some("Bose NC 700 Headphones"), None),
            ("USB Home", Some("USB audio CODEC"), None),
            ("AirPods", Some("Jimmy’s AirPods - Find My"), None),
        ]);
        let input = targets(&[
            ("MacBook Mic", Some("MacBook Pro Microphone"), None),
            ("Bose Mic", Some("Bose NC 700 Headphones"), None),
            ("RØDE Mic", None, Some("røde|rode")),
        ]);
        let wispr = crate::config::WisprConfig::default();
        let context = RenderContext::new(&world)
            .with_audio(&output, &input)
            .with_wispr_microphones(&wispr.microphones);
        for id in crate::model::PageId::ALL {
            for key in super::super::full_page(id, crate::model::Grid::MK2) {
                let view = render(key.tile, &context);
                // A tile must always produce something visible or be deliberately blank.
                let has_content = view.value.is_some()
                    || view.glyph.is_some()
                    || view.header.is_some()
                    || !view.rows.is_empty();
                assert!(
                    has_content || key.tile == Tile::Blank,
                    "{id} {:?} rendered nothing",
                    key.tile
                );
            }
        }
    }
}
