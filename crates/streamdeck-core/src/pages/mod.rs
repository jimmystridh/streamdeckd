//! Declarative page and key definitions.
//!
//! Every key on every page is one [`KeyBinding`]: a coordinate, the tile to draw,
//! and the actions its short and long press produce. Nothing in the daemon
//! inspects coordinates to decide what a key does, so adding or moving a key is a
//! table edit rather than a change to the coordinator.

pub mod theme;
pub mod views;

use crate::integrations::github::MetricKind;
use crate::integrations::walkingpad::WalkingPadCommand;
use crate::model::{AudioKind, Grid, IntegrationId, KeyPosition, PageId, WeatherTile};
use crate::pomodoro::Phase;

/// What pressing a key asks the runtime to do. Actions are data, so the CLI can
/// synthesise them and tests can assert on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Intentionally blank key.
    None,
    Navigate(PageId),
    /// Show a page as a temporary panel over the current one.
    OpenPanel(PageId),
    /// Close the temporary panel immediately.
    DismissPanel,
    Pomodoro(PomodoroCommand),
    Audio(AudioCommand),
    Spotify(SpotifyCommand),
    Media(MediaCommand),
    Application(ApplicationCommand),
    Dashboard(DashboardCommand),
    Wispr(WisprCommand),
    WalkingPad(WalkingPadCommand),
    OpenGitHubMetric(MetricKind),
    /// Open the URL behind authored-pull-request tile `index`.
    OpenGitHubItem(usize),
    /// Focus an existing Meet window for meeting `index`, else open the PWA.
    OpenMeeting(usize),
    /// Force a refresh of one integration.
    Refresh(IntegrationId),
    /// Force a refresh of both lake feeds, as the Home water tile does.
    RefreshLake,
    /// Show the expanded weather reading on the pressed tile for a few seconds.
    WeatherDetail(WeatherTile),
    /// A read-only tile: acknowledge any pending alert and repaint, nothing else.
    Acknowledge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PomodoroCommand {
    Toggle,
    Skip,
    Reset,
    Start(Phase),
    /// Short press adds `step_minutes`; long press subtracts it.
    Adjust {
        duration: Phase,
        step_minutes: i32,
    },
    Acknowledge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCommand {
    /// Select the `index`-th configured target of this kind.
    Select {
        kind: AudioKind,
        index: usize,
    },
    ToggleMute(AudioKind),
    Volume {
        kind: AudioKind,
        delta: i32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotifyCommand {
    PlayPause,
    Next,
    Previous,
    Seek(i32),
    Volume(i32),
    PlayPlaylist(usize),
    OpenApp,
}

/// Commands sent to whichever application owns the macOS media session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaCommand {
    PlayPause,
    Next,
    Previous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplicationCommand {
    Activate,
    Hide,
    Quit,
    ForceQuit,
    Context(usize),
    Recent(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WisprCommand {
    ToggleHandsFree,
    SelectMicrophone(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureDestination {
    Personal,
    Work,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardCommand {
    QuickCapture(CaptureDestination),
    OpenCiRun,
    OpenDepartureBoard(usize),
    OpenActivityMonitor,
    OpenVpn,
    OpenNetworkSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsScope {
    Cycle,
    Breaks,
    Today,
    AllTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalkingPadMetric {
    Distance,
    Steps,
    Elapsed,
}

/// What a key shows. The renderer resolves this against the current world view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    Blank,
    /// Navigation key back to Home.
    HomeButton,
    DashboardButton,
    QuickCapture,
    MixerSummary,
    CodexFiveHour,
    ClaudeFiveHour,
    ClaudeSevenDay,
    CodexUsage,
    SpotifyGlance,
    MediaGlance,
    CurrentApplication,
    ApplicationControl(ApplicationCommand),
    ApplicationContext(usize),
    ApplicationRecent(usize),
    WisprGlance,
    WisprPickerHeader,
    WisprMicrophone(usize),
    MediaControl(MediaCommand),
    MediaSource,
    GitHubSummary,
    CiRadar,
    MacHealth,
    NetworkVpn,
    DepartureBoard(usize),
    WalkingPadGlance,
    WalkingPadConnection,
    WalkingPadStart,
    WalkingPadStop,
    WalkingPadSpeed,
    WalkingPadSpeedAdjust(WalkingPadCommand),
    WalkingPadQuickSpeed(u8),
    WalkingPadSession(WalkingPadMetric),
    WalkingPadDaily(WalkingPadMetric),
    WalkingPadStatsButton,
    WalkingPadControlsButton,
    PomodoroGlance,
    Meeting(usize),
    WeatherCurrent,
    /// The time-sensitive Home glance: current/today before 17:00, tomorrow after.
    WeatherGlance,
    /// A daily forecast with Today/Tomorrow labels for the weather page.
    WeatherDay(usize),
    WeatherForecast(usize),
    LakeCurrent,
    LakeTrend,
    LakeDay(usize),
    PanelCountdown,
    AudioDevice {
        kind: AudioKind,
        index: usize,
    },
    AudioMute(AudioKind),
    AudioVolume {
        kind: AudioKind,
        delta: i32,
    },
    GitHubMetric(MetricKind),
    GitHubItem(usize),
    GitHubRefresh,
    SpotifyControl(SpotifyCommand),
    SpotifyPlaylist(usize),
    PomodoroTimer,
    PomodoroToggle,
    PomodoroSkip,
    PomodoroReset,
    PomodoroStart(Phase),
    PomodoroStats(StatsScope),
    PomodoroAdjust {
        duration: Phase,
        step_minutes: i32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyBinding {
    pub position: KeyPosition,
    pub tile: Tile,
    pub short: Action,
    pub long: Option<Action>,
}

impl KeyBinding {
    const fn new(row: u8, column: u8, tile: Tile, short: Action) -> Self {
        Self {
            position: KeyPosition::new(row, column),
            tile,
            short,
            long: None,
        }
    }

    const fn with_long(mut self, long: Action) -> Self {
        self.long = Some(long);
        self
    }

    pub const fn has_long_action(&self) -> bool {
        self.long.is_some()
    }
}

/// A page's complete key table.
#[derive(Debug, Clone)]
pub struct Page {
    pub id: PageId,
    pub keys: Vec<KeyBinding>,
}

impl Page {
    pub fn binding(&self, position: KeyPosition) -> Option<&KeyBinding> {
        self.keys.iter().find(|key| key.position == position)
    }
}

/// Builds the key table for a page. Positions absent from the table render as
/// blank keys, which is why every documented blank stays a real, drawn key.
pub fn page(id: PageId) -> Page {
    let keys = match id {
        PageId::Home => home(),
        PageId::Mixer => mixer(),
        PageId::GitHub => github(),
        PageId::Spotify => spotify(),
        PageId::Stensjon => stensjon(),
        PageId::Pomodoro => pomodoro(),
        PageId::Weather => weather(),
        PageId::Media => media(),
        PageId::Wispr => wispr(),
        PageId::Application => application(),
        PageId::Dashboard => dashboard(),
        PageId::WalkingPad => walkingpad(),
        PageId::WalkingPadStats => walkingpad_stats(),
    };
    Page { id, keys }
}

/// The full 15-key table for a page, with blanks filled in.
pub fn full_page(id: PageId, grid: Grid) -> Vec<KeyBinding> {
    let page = page(id);
    grid.positions()
        .map(|position| {
            page.binding(position).copied().unwrap_or(KeyBinding::new(
                position.row,
                position.column,
                Tile::Blank,
                Action::None,
            ))
        })
        .collect()
}

fn home() -> Vec<KeyBinding> {
    use Action::*;
    vec![
        KeyBinding::new(
            1,
            1,
            Tile::WisprGlance,
            Wispr(WisprCommand::ToggleHandsFree),
        )
        .with_long(Navigate(PageId::Wispr)),
        KeyBinding::new(
            1,
            2,
            Tile::CodexFiveHour,
            Refresh(IntegrationId::CodexUsage),
        ),
        KeyBinding::new(1, 3, Tile::CodexUsage, Refresh(IntegrationId::CodexUsage)),
        KeyBinding::new(
            1,
            4,
            Tile::ClaudeFiveHour,
            Refresh(IntegrationId::ClaudeUsage),
        ),
        KeyBinding::new(
            1,
            5,
            Tile::ClaudeSevenDay,
            Refresh(IntegrationId::ClaudeUsage),
        ),
        KeyBinding::new(2, 1, Tile::DashboardButton, Navigate(PageId::Dashboard)),
        KeyBinding::new(
            2,
            2,
            Tile::QuickCapture,
            Dashboard(DashboardCommand::QuickCapture(CaptureDestination::Personal)),
        )
        .with_long(Dashboard(DashboardCommand::QuickCapture(
            CaptureDestination::Work,
        ))),
        KeyBinding::new(
            2,
            3,
            Tile::PomodoroGlance,
            Pomodoro(PomodoroCommand::Toggle),
        )
        .with_long(Navigate(PageId::Pomodoro)),
        KeyBinding::new(2, 4, Tile::Meeting(0), OpenMeeting(0)),
        KeyBinding::new(2, 5, Tile::Meeting(1), OpenMeeting(1)),
        KeyBinding::new(
            3,
            1,
            Tile::CurrentApplication,
            Navigate(PageId::Application),
        ),
        KeyBinding::new(
            3,
            2,
            Tile::SpotifyGlance,
            Spotify(SpotifyCommand::PlayPause),
        )
        .with_long(Navigate(PageId::Spotify)),
        KeyBinding::new(3, 3, Tile::MediaGlance, Media(MediaCommand::PlayPause))
            .with_long(Navigate(PageId::Media)),
        KeyBinding::new(3, 4, Tile::MixerSummary, Navigate(PageId::Mixer)),
        KeyBinding::new(3, 5, Tile::WeatherGlance, Navigate(PageId::Weather)),
    ]
}

fn dashboard() -> Vec<KeyBinding> {
    use Action::*;
    vec![
        KeyBinding::new(1, 1, Tile::HomeButton, Navigate(PageId::Home)),
        KeyBinding::new(1, 2, Tile::GitHubSummary, Navigate(PageId::GitHub)),
        KeyBinding::new(1, 3, Tile::CiRadar, Dashboard(DashboardCommand::OpenCiRun))
            .with_long(Refresh(IntegrationId::CiRadar)),
        KeyBinding::new(
            1,
            4,
            Tile::MacHealth,
            Dashboard(DashboardCommand::OpenActivityMonitor),
        ),
        KeyBinding::new(1, 5, Tile::NetworkVpn, Dashboard(DashboardCommand::OpenVpn))
            .with_long(Dashboard(DashboardCommand::OpenNetworkSettings)),
        KeyBinding::new(
            2,
            1,
            Tile::DepartureBoard(0),
            Dashboard(DashboardCommand::OpenDepartureBoard(0)),
        )
        .with_long(Refresh(IntegrationId::Departures)),
        KeyBinding::new(
            2,
            2,
            Tile::DepartureBoard(1),
            Dashboard(DashboardCommand::OpenDepartureBoard(1)),
        )
        .with_long(Refresh(IntegrationId::Departures)),
        KeyBinding::new(2, 3, Tile::WalkingPadGlance, Navigate(PageId::WalkingPad)),
    ]
}

fn walkingpad() -> Vec<KeyBinding> {
    use Action::*;
    use WalkingPadCommand::{Decrease, Increase, SetSpeed, Start, Stop};
    vec![
        KeyBinding::new(1, 1, Tile::HomeButton, Navigate(PageId::Home)),
        KeyBinding::new(1, 2, Tile::WalkingPadConnection, Acknowledge),
        KeyBinding::new(1, 3, Tile::WalkingPadStart, WalkingPad(Start)),
        KeyBinding::new(1, 4, Tile::WalkingPadStop, WalkingPad(Stop)),
        KeyBinding::new(
            1,
            5,
            Tile::WalkingPadStatsButton,
            Navigate(PageId::WalkingPadStats),
        ),
        KeyBinding::new(
            2,
            1,
            Tile::WalkingPadSpeedAdjust(Decrease),
            WalkingPad(Decrease),
        ),
        KeyBinding::new(2, 2, Tile::WalkingPadSpeed, Acknowledge),
        KeyBinding::new(
            2,
            3,
            Tile::WalkingPadSpeedAdjust(Increase),
            WalkingPad(Increase),
        ),
        KeyBinding::new(
            2,
            4,
            Tile::WalkingPadSession(WalkingPadMetric::Distance),
            Acknowledge,
        ),
        KeyBinding::new(
            2,
            5,
            Tile::WalkingPadSession(WalkingPadMetric::Elapsed),
            Acknowledge,
        ),
        KeyBinding::new(
            3,
            1,
            Tile::WalkingPadQuickSpeed(26),
            WalkingPad(SetSpeed(26)),
        ),
        KeyBinding::new(
            3,
            2,
            Tile::WalkingPadQuickSpeed(30),
            WalkingPad(SetSpeed(30)),
        ),
        KeyBinding::new(
            3,
            3,
            Tile::WalkingPadQuickSpeed(34),
            WalkingPad(SetSpeed(34)),
        ),
        KeyBinding::new(
            3,
            4,
            Tile::WalkingPadQuickSpeed(42),
            WalkingPad(SetSpeed(42)),
        ),
        KeyBinding::new(
            3,
            5,
            Tile::WalkingPadQuickSpeed(45),
            WalkingPad(SetSpeed(45)),
        ),
    ]
}

fn walkingpad_stats() -> Vec<KeyBinding> {
    use Action::*;
    vec![
        KeyBinding::new(1, 1, Tile::HomeButton, Navigate(PageId::Home)),
        KeyBinding::new(
            1,
            2,
            Tile::WalkingPadControlsButton,
            Navigate(PageId::WalkingPad),
        ),
        KeyBinding::new(1, 3, Tile::WalkingPadConnection, Acknowledge),
        KeyBinding::new(1, 4, Tile::WalkingPadSpeed, Acknowledge),
        KeyBinding::new(
            2,
            1,
            Tile::WalkingPadSession(WalkingPadMetric::Distance),
            Acknowledge,
        ),
        KeyBinding::new(
            2,
            2,
            Tile::WalkingPadSession(WalkingPadMetric::Steps),
            Acknowledge,
        ),
        KeyBinding::new(
            2,
            3,
            Tile::WalkingPadSession(WalkingPadMetric::Elapsed),
            Acknowledge,
        ),
        KeyBinding::new(
            3,
            1,
            Tile::WalkingPadDaily(WalkingPadMetric::Distance),
            Acknowledge,
        ),
        KeyBinding::new(
            3,
            2,
            Tile::WalkingPadDaily(WalkingPadMetric::Steps),
            Acknowledge,
        ),
        KeyBinding::new(
            3,
            3,
            Tile::WalkingPadDaily(WalkingPadMetric::Elapsed),
            Acknowledge,
        ),
    ]
}

fn mixer() -> Vec<KeyBinding> {
    use Action::*;
    use AudioKind::{Input, Output};
    vec![
        KeyBinding::new(1, 1, Tile::HomeButton, Navigate(PageId::Home)),
        KeyBinding::new(
            1,
            2,
            Tile::AudioDevice {
                kind: Output,
                index: 0,
            },
            Audio(AudioCommand::Select {
                kind: Output,
                index: 0,
            }),
        ),
        KeyBinding::new(
            1,
            3,
            Tile::AudioDevice {
                kind: Output,
                index: 1,
            },
            Audio(AudioCommand::Select {
                kind: Output,
                index: 1,
            }),
        ),
        KeyBinding::new(
            1,
            4,
            Tile::AudioDevice {
                kind: Output,
                index: 2,
            },
            Audio(AudioCommand::Select {
                kind: Output,
                index: 2,
            }),
        ),
        KeyBinding::new(
            1,
            5,
            Tile::AudioDevice {
                kind: Output,
                index: 3,
            },
            Audio(AudioCommand::Select {
                kind: Output,
                index: 3,
            }),
        ),
        KeyBinding::new(
            2,
            1,
            Tile::AudioVolume {
                kind: Output,
                delta: -10,
            },
            Audio(AudioCommand::Volume {
                kind: Output,
                delta: -10,
            }),
        ),
        KeyBinding::new(
            2,
            2,
            Tile::AudioVolume {
                kind: Output,
                delta: 10,
            },
            Audio(AudioCommand::Volume {
                kind: Output,
                delta: 10,
            }),
        ),
        KeyBinding::new(
            2,
            3,
            Tile::AudioMute(Output),
            Audio(AudioCommand::ToggleMute(Output)),
        ),
        // 2,4 is intentionally blank.
        KeyBinding::new(
            2,
            5,
            Tile::MixerSummary,
            Refresh(IntegrationId::AudioStatus),
        ),
        KeyBinding::new(
            3,
            1,
            Tile::AudioDevice {
                kind: Input,
                index: 0,
            },
            Audio(AudioCommand::Select {
                kind: Input,
                index: 0,
            }),
        ),
        KeyBinding::new(
            3,
            2,
            Tile::AudioDevice {
                kind: Input,
                index: 1,
            },
            Audio(AudioCommand::Select {
                kind: Input,
                index: 1,
            }),
        ),
        KeyBinding::new(
            3,
            3,
            Tile::AudioDevice {
                kind: Input,
                index: 2,
            },
            Audio(AudioCommand::Select {
                kind: Input,
                index: 2,
            }),
        ),
        KeyBinding::new(
            3,
            4,
            Tile::AudioMute(Input),
            Audio(AudioCommand::ToggleMute(Input)),
        ),
        // 3,5 is intentionally blank.
    ]
}

fn github() -> Vec<KeyBinding> {
    use Action::*;
    let mut keys = vec![
        KeyBinding::new(1, 1, Tile::HomeButton, Navigate(PageId::Home)),
        KeyBinding::new(
            1,
            2,
            Tile::GitHubMetric(MetricKind::Reviews),
            OpenGitHubMetric(MetricKind::Reviews),
        ),
        KeyBinding::new(
            1,
            3,
            Tile::GitHubMetric(MetricKind::Prs),
            OpenGitHubMetric(MetricKind::Prs),
        ),
        KeyBinding::new(
            1,
            4,
            Tile::GitHubMetric(MetricKind::Assigned),
            OpenGitHubMetric(MetricKind::Assigned),
        ),
        KeyBinding::new(
            1,
            5,
            Tile::GitHubMetric(MetricKind::Inbox),
            OpenGitHubMetric(MetricKind::Inbox),
        ),
    ];
    for index in 0..crate::integrations::github::ITEM_TILES {
        keys.push(KeyBinding::new(
            2,
            index as u8 + 1,
            Tile::GitHubItem(index),
            OpenGitHubItem(index),
        ));
    }
    keys.push(KeyBinding::new(
        3,
        1,
        Tile::GitHubRefresh,
        Refresh(IntegrationId::GitHub),
    ));
    keys
}

fn spotify() -> Vec<KeyBinding> {
    use Action::*;
    vec![
        KeyBinding::new(1, 1, Tile::HomeButton, Navigate(PageId::Home)),
        KeyBinding::new(
            1,
            2,
            Tile::SpotifyControl(SpotifyCommand::Previous),
            Spotify(SpotifyCommand::Previous),
        ),
        KeyBinding::new(
            1,
            3,
            Tile::SpotifyControl(SpotifyCommand::PlayPause),
            Spotify(SpotifyCommand::PlayPause),
        ),
        KeyBinding::new(
            1,
            4,
            Tile::SpotifyControl(SpotifyCommand::Next),
            Spotify(SpotifyCommand::Next),
        ),
        KeyBinding::new(
            1,
            5,
            Tile::SpotifyControl(SpotifyCommand::OpenApp),
            Spotify(SpotifyCommand::OpenApp),
        ),
        KeyBinding::new(
            2,
            1,
            Tile::SpotifyControl(SpotifyCommand::Volume(-5)),
            Spotify(SpotifyCommand::Volume(-5)),
        ),
        KeyBinding::new(
            2,
            2,
            Tile::SpotifyControl(SpotifyCommand::Volume(5)),
            Spotify(SpotifyCommand::Volume(5)),
        ),
        KeyBinding::new(
            2,
            3,
            Tile::SpotifyControl(SpotifyCommand::Seek(-15)),
            Spotify(SpotifyCommand::Seek(-15)),
        ),
        KeyBinding::new(
            2,
            4,
            Tile::SpotifyControl(SpotifyCommand::Seek(15)),
            Spotify(SpotifyCommand::Seek(15)),
        ),
        KeyBinding::new(
            3,
            1,
            Tile::SpotifyPlaylist(0),
            Spotify(SpotifyCommand::PlayPlaylist(0)),
        ),
        KeyBinding::new(
            3,
            2,
            Tile::SpotifyPlaylist(1),
            Spotify(SpotifyCommand::PlayPlaylist(1)),
        ),
        KeyBinding::new(
            3,
            3,
            Tile::SpotifyPlaylist(2),
            Spotify(SpotifyCommand::PlayPlaylist(2)),
        ),
        KeyBinding::new(
            3,
            4,
            Tile::SpotifyPlaylist(3),
            Spotify(SpotifyCommand::PlayPlaylist(3)),
        ),
        KeyBinding::new(
            3,
            5,
            Tile::SpotifyPlaylist(4),
            Spotify(SpotifyCommand::PlayPlaylist(4)),
        ),
    ]
}

fn stensjon() -> Vec<KeyBinding> {
    use Action::*;
    let mut keys = vec![
        KeyBinding::new(1, 1, Tile::HomeButton, DismissPanel),
        KeyBinding::new(1, 2, Tile::LakeCurrent, RefreshLake),
        KeyBinding::new(1, 3, Tile::LakeTrend, RefreshLake),
        // 1,4 is intentionally blank.
        KeyBinding::new(1, 5, Tile::PanelCountdown, DismissPanel),
    ];
    for index in 0..crate::integrations::lake::HISTORY_DAYS {
        let (row, column) = if index < 5 {
            (2, index as u8 + 1)
        } else {
            (3, index as u8 - 4)
        };
        keys.push(KeyBinding::new(
            row,
            column,
            Tile::LakeDay(index),
            Acknowledge,
        ));
    }
    keys
}

fn pomodoro() -> Vec<KeyBinding> {
    use Action::*;
    use Phase::{Focus, LongBreak, ShortBreak};
    vec![
        KeyBinding::new(1, 1, Tile::HomeButton, Navigate(PageId::Home)),
        KeyBinding::new(1, 2, Tile::PomodoroTimer, Pomodoro(PomodoroCommand::Toggle)),
        KeyBinding::new(
            1,
            3,
            Tile::PomodoroToggle,
            Pomodoro(PomodoroCommand::Toggle),
        ),
        KeyBinding::new(1, 4, Tile::PomodoroSkip, Pomodoro(PomodoroCommand::Skip)),
        KeyBinding::new(1, 5, Tile::PomodoroReset, Pomodoro(PomodoroCommand::Reset)),
        KeyBinding::new(
            2,
            1,
            Tile::PomodoroStart(Focus),
            Pomodoro(PomodoroCommand::Start(Focus)),
        ),
        KeyBinding::new(
            2,
            2,
            Tile::PomodoroStart(ShortBreak),
            Pomodoro(PomodoroCommand::Start(ShortBreak)),
        ),
        KeyBinding::new(
            2,
            3,
            Tile::PomodoroStart(LongBreak),
            Pomodoro(PomodoroCommand::Start(LongBreak)),
        ),
        KeyBinding::new(2, 4, Tile::PomodoroStats(StatsScope::Cycle), Acknowledge),
        KeyBinding::new(2, 5, Tile::PomodoroStats(StatsScope::Breaks), Acknowledge),
        KeyBinding::new(
            3,
            1,
            Tile::PomodoroAdjust {
                duration: Focus,
                step_minutes: 5,
            },
            Pomodoro(PomodoroCommand::Adjust {
                duration: Focus,
                step_minutes: 5,
            }),
        )
        .with_long(Pomodoro(PomodoroCommand::Adjust {
            duration: Focus,
            step_minutes: -5,
        })),
        KeyBinding::new(
            3,
            2,
            Tile::PomodoroAdjust {
                duration: ShortBreak,
                step_minutes: 1,
            },
            Pomodoro(PomodoroCommand::Adjust {
                duration: ShortBreak,
                step_minutes: 1,
            }),
        )
        .with_long(Pomodoro(PomodoroCommand::Adjust {
            duration: ShortBreak,
            step_minutes: -1,
        })),
        KeyBinding::new(
            3,
            3,
            Tile::PomodoroAdjust {
                duration: LongBreak,
                step_minutes: 5,
            },
            Pomodoro(PomodoroCommand::Adjust {
                duration: LongBreak,
                step_minutes: 5,
            }),
        )
        .with_long(Pomodoro(PomodoroCommand::Adjust {
            duration: LongBreak,
            step_minutes: -5,
        })),
        KeyBinding::new(3, 4, Tile::PomodoroStats(StatsScope::Today), Acknowledge),
        KeyBinding::new(3, 5, Tile::PomodoroStats(StatsScope::AllTime), Acknowledge),
    ]
}

fn weather() -> Vec<KeyBinding> {
    use Action::*;
    vec![
        KeyBinding::new(1, 1, Tile::HomeButton, Navigate(PageId::Home)),
        KeyBinding::new(1, 2, Tile::WeatherCurrent, Refresh(IntegrationId::Weather)),
        KeyBinding::new(1, 3, Tile::WeatherDay(0), Refresh(IntegrationId::Weather)),
        KeyBinding::new(1, 4, Tile::WeatherDay(1), Refresh(IntegrationId::Weather)),
        KeyBinding::new(1, 5, Tile::WeatherDay(2), Refresh(IntegrationId::Weather)),
        KeyBinding::new(2, 1, Tile::WeatherDay(3), Refresh(IntegrationId::Weather)),
        KeyBinding::new(2, 2, Tile::WeatherDay(4), Refresh(IntegrationId::Weather)),
        KeyBinding::new(2, 3, Tile::WeatherDay(5), Refresh(IntegrationId::Weather)),
        KeyBinding::new(2, 4, Tile::WeatherDay(6), Refresh(IntegrationId::Weather)),
        // 2,5 is intentionally blank so forecast and water remain distinct rows.
        KeyBinding::new(3, 1, Tile::LakeCurrent, OpenPanel(PageId::Stensjon)),
        KeyBinding::new(3, 2, Tile::LakeTrend, OpenPanel(PageId::Stensjon)),
        KeyBinding::new(3, 3, Tile::LakeDay(0), OpenPanel(PageId::Stensjon)),
        KeyBinding::new(3, 4, Tile::LakeDay(1), OpenPanel(PageId::Stensjon)),
        KeyBinding::new(3, 5, Tile::LakeDay(2), OpenPanel(PageId::Stensjon)),
    ]
}

fn media() -> Vec<KeyBinding> {
    use Action::*;
    use AudioKind::Output;
    vec![
        KeyBinding::new(1, 1, Tile::HomeButton, Navigate(PageId::Home)),
        KeyBinding::new(
            1,
            2,
            Tile::MediaControl(MediaCommand::Previous),
            Media(MediaCommand::Previous),
        ),
        KeyBinding::new(
            1,
            3,
            Tile::MediaControl(MediaCommand::PlayPause),
            Media(MediaCommand::PlayPause),
        ),
        KeyBinding::new(
            1,
            4,
            Tile::MediaControl(MediaCommand::Next),
            Media(MediaCommand::Next),
        ),
        KeyBinding::new(
            1,
            5,
            Tile::MediaSource,
            Refresh(IntegrationId::MediaSession),
        ),
        KeyBinding::new(
            2,
            1,
            Tile::AudioMute(Output),
            Audio(AudioCommand::ToggleMute(Output)),
        ),
        KeyBinding::new(
            2,
            2,
            Tile::AudioVolume {
                kind: Output,
                delta: -10,
            },
            Audio(AudioCommand::Volume {
                kind: Output,
                delta: -10,
            }),
        ),
        KeyBinding::new(
            2,
            3,
            Tile::AudioVolume {
                kind: Output,
                delta: 10,
            },
            Audio(AudioCommand::Volume {
                kind: Output,
                delta: 10,
            }),
        ),
    ]
}

fn wispr() -> Vec<KeyBinding> {
    use Action::*;
    vec![
        KeyBinding::new(1, 1, Tile::HomeButton, Navigate(PageId::Home)),
        KeyBinding::new(1, 3, Tile::WisprPickerHeader, Acknowledge),
        KeyBinding::new(
            2,
            2,
            Tile::WisprMicrophone(0),
            Wispr(WisprCommand::SelectMicrophone(0)),
        ),
        KeyBinding::new(
            2,
            3,
            Tile::WisprMicrophone(1),
            Wispr(WisprCommand::SelectMicrophone(1)),
        ),
        KeyBinding::new(
            2,
            4,
            Tile::WisprMicrophone(2),
            Wispr(WisprCommand::SelectMicrophone(2)),
        ),
    ]
}

fn application() -> Vec<KeyBinding> {
    use Action::*;
    vec![
        KeyBinding::new(1, 1, Tile::HomeButton, Navigate(PageId::Home)),
        KeyBinding::new(
            1,
            2,
            Tile::ApplicationControl(ApplicationCommand::Activate),
            Application(ApplicationCommand::Activate),
        ),
        KeyBinding::new(
            1,
            3,
            Tile::ApplicationControl(ApplicationCommand::Hide),
            Application(ApplicationCommand::Hide),
        ),
        KeyBinding::new(
            1,
            4,
            Tile::ApplicationControl(ApplicationCommand::Quit),
            Acknowledge,
        )
        .with_long(Application(ApplicationCommand::Quit)),
        KeyBinding::new(
            1,
            5,
            Tile::ApplicationControl(ApplicationCommand::ForceQuit),
            Acknowledge,
        )
        .with_long(Application(ApplicationCommand::ForceQuit)),
        KeyBinding::new(
            2,
            1,
            Tile::ApplicationContext(0),
            Application(ApplicationCommand::Context(0)),
        ),
        KeyBinding::new(
            2,
            2,
            Tile::ApplicationContext(1),
            Application(ApplicationCommand::Context(1)),
        ),
        KeyBinding::new(
            2,
            3,
            Tile::ApplicationContext(2),
            Application(ApplicationCommand::Context(2)),
        ),
        KeyBinding::new(
            2,
            4,
            Tile::ApplicationContext(3),
            Application(ApplicationCommand::Context(3)),
        ),
        KeyBinding::new(
            2,
            5,
            Tile::ApplicationContext(4),
            Application(ApplicationCommand::Context(4)),
        ),
        KeyBinding::new(
            3,
            1,
            Tile::ApplicationRecent(0),
            Application(ApplicationCommand::Recent(0)),
        ),
        KeyBinding::new(
            3,
            2,
            Tile::ApplicationRecent(1),
            Application(ApplicationCommand::Recent(1)),
        ),
        KeyBinding::new(
            3,
            3,
            Tile::ApplicationRecent(2),
            Application(ApplicationCommand::Recent(2)),
        ),
        KeyBinding::new(
            3,
            4,
            Tile::ApplicationRecent(3),
            Application(ApplicationCommand::Recent(3)),
        ),
        KeyBinding::new(
            3,
            5,
            Tile::ApplicationRecent(4),
            Application(ApplicationCommand::Recent(4)),
        ),
    ]
}

/// Integrations a page needs while it is visible. Drives visibility-gated refresh
/// so nothing polls for a key nobody can see.
pub fn required_integrations(id: PageId) -> Vec<IntegrationId> {
    match id {
        PageId::Home => vec![
            IntegrationId::FrontmostApplication,
            IntegrationId::AudioStatus,
            IntegrationId::Meetings,
            IntegrationId::Weather,
            IntegrationId::LakeCurrent,
            IntegrationId::ClaudeUsage,
            IntegrationId::CodexUsage,
            IntegrationId::Spotify,
        ],
        PageId::Dashboard => vec![
            IntegrationId::GitHub,
            IntegrationId::CiRadar,
            IntegrationId::MacHealth,
            IntegrationId::NetworkStatus,
            IntegrationId::Departures,
        ],
        // The audio snapshot already contains both status and device inventory.
        PageId::Mixer => vec![IntegrationId::AudioStatus],
        PageId::GitHub => vec![IntegrationId::GitHub],
        PageId::Spotify => vec![IntegrationId::Spotify],
        PageId::Stensjon => vec![IntegrationId::LakeCurrent, IntegrationId::LakeHistory],
        PageId::Pomodoro => Vec::new(),
        PageId::Weather => vec![
            IntegrationId::Weather,
            IntegrationId::LakeCurrent,
            IntegrationId::LakeHistory,
        ],
        PageId::Media => vec![IntegrationId::MediaSession, IntegrationId::AudioStatus],
        PageId::Wispr => Vec::new(),
        PageId::Application => vec![
            IntegrationId::FrontmostApplication,
            IntegrationId::AudioStatus,
            IntegrationId::MediaSession,
            IntegrationId::Meetings,
            IntegrationId::Spotify,
        ],
        PageId::WalkingPad | PageId::WalkingPadStats => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_fits_the_grid_and_has_unique_coordinates() {
        for id in PageId::ALL {
            let page = page(id);
            let mut seen = std::collections::HashSet::new();
            for key in &page.keys {
                assert!(
                    Grid::MK2.index_of(key.position).is_some(),
                    "{id} {} is outside the grid",
                    key.position
                );
                assert!(
                    seen.insert(key.position),
                    "{id} defines {} twice",
                    key.position
                );
            }
        }
    }

    #[test]
    fn a_full_page_always_covers_all_fifteen_keys() {
        for id in PageId::ALL {
            let keys = full_page(id, Grid::MK2);
            assert_eq!(keys.len(), 15, "{id}");
            for (index, key) in keys.iter().enumerate() {
                assert_eq!(Grid::MK2.index_of(key.position), Some(index), "{id}");
            }
        }
    }

    #[test]
    fn home_matches_the_documented_layout() {
        let page = page(PageId::Home);
        let tile = |row, column| {
            page.binding(KeyPosition::new(row, column))
                .map(|key| key.tile)
        };

        assert_eq!(tile(1, 1), Some(Tile::WisprGlance));
        assert_eq!(tile(1, 2), Some(Tile::CodexFiveHour));
        assert_eq!(tile(1, 3), Some(Tile::CodexUsage));
        assert_eq!(tile(1, 4), Some(Tile::ClaudeFiveHour));
        assert_eq!(tile(1, 5), Some(Tile::ClaudeSevenDay));
        assert_eq!(tile(2, 1), Some(Tile::DashboardButton));
        assert_eq!(tile(2, 2), Some(Tile::QuickCapture));
        assert_eq!(tile(2, 3), Some(Tile::PomodoroGlance));
        assert_eq!(tile(2, 4), Some(Tile::Meeting(0)));
        assert_eq!(tile(2, 5), Some(Tile::Meeting(1)));
        assert_eq!(tile(3, 1), Some(Tile::CurrentApplication));
        assert_eq!(tile(3, 2), Some(Tile::SpotifyGlance));
        assert_eq!(tile(3, 3), Some(Tile::MediaGlance));
        assert_eq!(tile(3, 4), Some(Tile::MixerSummary));
        assert_eq!(tile(3, 5), Some(Tile::WeatherGlance));
    }

    #[test]
    fn dashboard_spreads_status_and_departures_across_the_second_screen() {
        let page = page(PageId::Dashboard);
        let tile = |row, column| {
            page.binding(KeyPosition::new(row, column))
                .map(|key| key.tile)
        };

        assert_eq!(tile(1, 1), Some(Tile::HomeButton));
        assert_eq!(tile(1, 2), Some(Tile::GitHubSummary));
        assert_eq!(tile(1, 3), Some(Tile::CiRadar));
        assert_eq!(tile(1, 4), Some(Tile::MacHealth));
        assert_eq!(tile(1, 5), Some(Tile::NetworkVpn));
        assert_eq!(tile(2, 1), Some(Tile::DepartureBoard(0)));
        assert_eq!(tile(2, 2), Some(Tile::DepartureBoard(1)));
        assert_eq!(tile(2, 3), Some(Tile::WalkingPadGlance));
        assert_eq!(tile(3, 1), None);
    }

    #[test]
    fn walkingpad_controls_map_every_motion_button_exactly() {
        let page = page(PageId::WalkingPad);
        let action = |row, column| {
            page.binding(KeyPosition::new(row, column))
                .map(|key| key.short)
        };

        assert_eq!(
            action(1, 3),
            Some(Action::WalkingPad(WalkingPadCommand::Start))
        );
        assert_eq!(
            action(1, 4),
            Some(Action::WalkingPad(WalkingPadCommand::Stop))
        );
        assert_eq!(
            action(2, 1),
            Some(Action::WalkingPad(WalkingPadCommand::Decrease))
        );
        assert_eq!(
            action(2, 3),
            Some(Action::WalkingPad(WalkingPadCommand::Increase))
        );
        for (column, tenths) in [26, 30, 34, 42, 45].into_iter().enumerate() {
            assert_eq!(
                action(3, column as u8 + 1),
                Some(Action::WalkingPad(WalkingPadCommand::SetSpeed(tenths)))
            );
        }
    }

    #[test]
    fn walkingpad_pages_expose_all_session_and_daily_telemetry() {
        let controls = page(PageId::WalkingPad);
        assert!(controls
            .keys
            .iter()
            .any(|key| key.tile == Tile::WalkingPadConnection));
        assert!(controls
            .keys
            .iter()
            .any(|key| key.tile == Tile::WalkingPadSpeed));

        let stats = page(PageId::WalkingPadStats);
        for metric in [
            WalkingPadMetric::Distance,
            WalkingPadMetric::Steps,
            WalkingPadMetric::Elapsed,
        ] {
            assert!(stats
                .keys
                .iter()
                .any(|key| key.tile == Tile::WalkingPadSession(metric)));
            assert!(stats
                .keys
                .iter()
                .any(|key| key.tile == Tile::WalkingPadDaily(metric)));
        }
    }

    #[test]
    fn current_application_opens_from_tile_eleven_and_has_safe_controls() {
        let home = page(PageId::Home);
        assert_eq!(
            home.binding(KeyPosition::new(3, 1))
                .expect("current application")
                .short,
            Action::Navigate(PageId::Application)
        );

        let page = page(PageId::Application);
        assert_eq!(
            page.binding(KeyPosition::new(1, 3)).expect("hide").short,
            Action::Application(ApplicationCommand::Hide)
        );
        let quit = page.binding(KeyPosition::new(1, 4)).expect("quit");
        assert_eq!(quit.short, Action::Acknowledge);
        assert_eq!(
            quit.long,
            Some(Action::Application(ApplicationCommand::Quit))
        );
        let force_quit = page.binding(KeyPosition::new(1, 5)).expect("force quit");
        assert_eq!(force_quit.short, Action::Acknowledge);
        assert_eq!(
            force_quit.long,
            Some(Action::Application(ApplicationCommand::ForceQuit))
        );
        for (column, slot) in (1..=5).zip(0..5) {
            assert_eq!(
                page.binding(KeyPosition::new(2, column))
                    .expect("context action")
                    .short,
                Action::Application(ApplicationCommand::Context(slot))
            );
            assert_eq!(
                page.binding(KeyPosition::new(3, column))
                    .expect("recent application")
                    .short,
                Action::Application(ApplicationCommand::Recent(slot))
            );
        }
    }

    #[test]
    fn home_long_presses_open_spotify_pomodoro_and_media_pages() {
        let page = page(PageId::Home);
        let spotify = page
            .binding(KeyPosition::new(3, 2))
            .expect("spotify glance");
        assert_eq!(spotify.short, Action::Spotify(SpotifyCommand::PlayPause));
        assert_eq!(spotify.long, Some(Action::Navigate(PageId::Spotify)));

        let pomodoro = page
            .binding(KeyPosition::new(2, 3))
            .expect("pomodoro glance");
        assert_eq!(pomodoro.short, Action::Pomodoro(PomodoroCommand::Toggle));
        assert_eq!(pomodoro.long, Some(Action::Navigate(PageId::Pomodoro)));

        let media = page.binding(KeyPosition::new(3, 3)).expect("media key");
        assert_eq!(media.short, Action::Media(MediaCommand::PlayPause));
        assert_eq!(media.long, Some(Action::Navigate(PageId::Media)));
    }

    #[test]
    fn home_slot_one_toggles_wispr_and_holds_for_the_microphone_picker() {
        let key = page(PageId::Home)
            .binding(KeyPosition::new(1, 1))
            .copied()
            .expect("Wispr tile");

        assert_eq!(key.short, Action::Wispr(WisprCommand::ToggleHandsFree));
        assert_eq!(key.long, Some(Action::Navigate(PageId::Wispr)));
    }

    #[test]
    fn wispr_page_offers_three_microphone_choices() {
        let page = page(PageId::Wispr);
        assert_eq!(
            page.binding(KeyPosition::new(1, 1)).expect("home").short,
            Action::Navigate(PageId::Home)
        );
        for (column, index) in [(2, 0), (3, 1), (4, 2)] {
            let key = page
                .binding(KeyPosition::new(2, column))
                .expect("microphone");
            assert_eq!(key.tile, Tile::WisprMicrophone(index));
            assert_eq!(
                key.short,
                Action::Wispr(WisprCommand::SelectMicrophone(index))
            );
        }
    }

    #[test]
    fn the_home_weather_tile_opens_the_weather_page() {
        let page = page(PageId::Home);
        let weather = page.binding(KeyPosition::new(3, 5)).expect("weather tile");
        assert_eq!(weather.short, Action::Navigate(PageId::Weather));
    }

    #[test]
    fn mixer_has_four_outputs_and_no_microphone_gain_controls() {
        let page = page(PageId::Mixer);
        let tile = |row, column| {
            page.binding(KeyPosition::new(row, column))
                .map(|key| key.tile)
        };

        assert_eq!(tile(1, 1), Some(Tile::HomeButton));
        assert_eq!(
            tile(1, 5),
            Some(Tile::AudioDevice {
                kind: AudioKind::Output,
                index: 3
            })
        );
        assert_eq!(tile(2, 3), Some(Tile::AudioMute(AudioKind::Output)));
        assert_eq!(
            tile(2, 1),
            Some(Tile::AudioVolume {
                kind: AudioKind::Output,
                delta: -10
            })
        );
        assert_eq!(
            tile(3, 1),
            Some(Tile::AudioDevice {
                kind: AudioKind::Input,
                index: 0
            })
        );
        assert_eq!(tile(3, 4), Some(Tile::AudioMute(AudioKind::Input)));
        assert_eq!(tile(2, 4), None, "2,4 is intentionally blank");
        assert_eq!(tile(3, 5), None, "3,5 is intentionally blank");
        assert_eq!(tile(2, 5), Some(Tile::MixerSummary));
        assert!(!page.keys.iter().any(|key| {
            matches!(
                key.tile,
                Tile::AudioVolume {
                    kind: AudioKind::Input,
                    ..
                }
            )
        }));
    }

    #[test]
    fn github_exposes_four_metrics_five_items_and_a_refresh() {
        let page = page(PageId::GitHub);
        let metrics: Vec<Tile> = page
            .keys
            .iter()
            .filter(|key| matches!(key.tile, Tile::GitHubMetric(_)))
            .map(|key| key.tile)
            .collect();
        assert_eq!(metrics.len(), 4);

        let items: Vec<Tile> = page
            .keys
            .iter()
            .filter(|key| matches!(key.tile, Tile::GitHubItem(_)))
            .map(|key| key.tile)
            .collect();
        assert_eq!(items.len(), crate::integrations::github::ITEM_TILES);

        assert_eq!(
            page.binding(KeyPosition::new(3, 1)).map(|key| key.tile),
            Some(Tile::GitHubRefresh)
        );
        assert_eq!(page.binding(KeyPosition::new(3, 2)), None);
    }

    #[test]
    fn spotify_matches_the_documented_layout() {
        let page = page(PageId::Spotify);
        let action = |row, column| {
            page.binding(KeyPosition::new(row, column))
                .map(|key| key.short)
        };

        assert_eq!(
            action(1, 2),
            Some(Action::Spotify(SpotifyCommand::Previous))
        );
        assert_eq!(
            action(1, 3),
            Some(Action::Spotify(SpotifyCommand::PlayPause))
        );
        assert_eq!(action(1, 4), Some(Action::Spotify(SpotifyCommand::Next)));
        assert_eq!(action(1, 5), Some(Action::Spotify(SpotifyCommand::OpenApp)));
        assert_eq!(
            action(2, 1),
            Some(Action::Spotify(SpotifyCommand::Volume(-5)))
        );
        assert_eq!(
            action(2, 2),
            Some(Action::Spotify(SpotifyCommand::Volume(5)))
        );
        assert_eq!(
            action(2, 3),
            Some(Action::Spotify(SpotifyCommand::Seek(-15)))
        );
        assert_eq!(
            action(2, 4),
            Some(Action::Spotify(SpotifyCommand::Seek(15)))
        );
        assert_eq!(action(2, 5), None, "remaining keys are blank");
        for index in 0..5 {
            assert_eq!(
                action(3, index + 1),
                Some(Action::Spotify(SpotifyCommand::PlayPlaylist(
                    index as usize
                )))
            );
        }
    }

    #[test]
    fn stensjon_lays_out_seven_history_days_across_two_rows() {
        let page = page(PageId::Stensjon);
        let tile = |row, column| {
            page.binding(KeyPosition::new(row, column))
                .map(|key| key.tile)
        };

        assert_eq!(tile(1, 1), Some(Tile::HomeButton));
        assert_eq!(tile(1, 2), Some(Tile::LakeCurrent));
        assert_eq!(tile(1, 3), Some(Tile::LakeTrend));
        assert_eq!(tile(1, 4), None, "1,4 is intentionally blank");
        assert_eq!(tile(1, 5), Some(Tile::PanelCountdown));
        assert_eq!(tile(2, 1), Some(Tile::LakeDay(0)));
        assert_eq!(tile(2, 5), Some(Tile::LakeDay(4)));
        assert_eq!(tile(3, 1), Some(Tile::LakeDay(5)));
        assert_eq!(tile(3, 2), Some(Tile::LakeDay(6)));
        assert_eq!(tile(3, 3), None, "remaining keys are blank");
    }

    #[test]
    fn weather_combines_the_week_ahead_with_water_readings() {
        let page = page(PageId::Weather);
        let tile = |row, column| {
            page.binding(KeyPosition::new(row, column))
                .map(|key| key.tile)
        };

        assert_eq!(tile(1, 1), Some(Tile::HomeButton));
        assert_eq!(tile(1, 2), Some(Tile::WeatherCurrent));
        assert_eq!(tile(1, 3), Some(Tile::WeatherDay(0)));
        assert_eq!(tile(1, 4), Some(Tile::WeatherDay(1)));
        assert_eq!(tile(2, 4), Some(Tile::WeatherDay(6)));
        assert_eq!(tile(2, 5), None);
        assert_eq!(tile(3, 1), Some(Tile::LakeCurrent));
        assert_eq!(tile(3, 2), Some(Tile::LakeTrend));
        assert_eq!(tile(3, 5), Some(Tile::LakeDay(2)));
    }

    #[test]
    fn media_exposes_transport_owner_and_system_volume() {
        let page = page(PageId::Media);
        let action = |row, column| {
            page.binding(KeyPosition::new(row, column))
                .map(|key| key.short)
        };

        assert_eq!(action(1, 2), Some(Action::Media(MediaCommand::Previous)));
        assert_eq!(action(1, 3), Some(Action::Media(MediaCommand::PlayPause)));
        assert_eq!(action(1, 4), Some(Action::Media(MediaCommand::Next)));
        assert_eq!(
            action(1, 5),
            Some(Action::Refresh(IntegrationId::MediaSession))
        );
        assert_eq!(
            action(2, 1),
            Some(Action::Audio(AudioCommand::ToggleMute(AudioKind::Output)))
        );
        assert_eq!(
            action(2, 2),
            Some(Action::Audio(AudioCommand::Volume {
                kind: AudioKind::Output,
                delta: -10,
            }))
        );
    }

    #[test]
    fn the_panel_home_key_dismisses_rather_than_navigating() {
        let page = page(PageId::Stensjon);
        assert_eq!(
            page.binding(KeyPosition::new(1, 1)).map(|key| key.short),
            Some(Action::DismissPanel)
        );
    }

    #[test]
    fn pomodoro_matches_the_documented_layout() {
        let page = page(PageId::Pomodoro);
        let tile = |row, column| {
            page.binding(KeyPosition::new(row, column))
                .map(|key| key.tile)
        };

        assert_eq!(tile(1, 2), Some(Tile::PomodoroTimer));
        assert_eq!(tile(1, 3), Some(Tile::PomodoroToggle));
        assert_eq!(tile(1, 4), Some(Tile::PomodoroSkip));
        assert_eq!(tile(1, 5), Some(Tile::PomodoroReset));
        assert_eq!(tile(2, 1), Some(Tile::PomodoroStart(Phase::Focus)));
        assert_eq!(tile(2, 2), Some(Tile::PomodoroStart(Phase::ShortBreak)));
        assert_eq!(tile(2, 3), Some(Tile::PomodoroStart(Phase::LongBreak)));
        assert_eq!(tile(2, 4), Some(Tile::PomodoroStats(StatsScope::Cycle)));
        assert_eq!(tile(2, 5), Some(Tile::PomodoroStats(StatsScope::Breaks)));
        assert_eq!(tile(3, 4), Some(Tile::PomodoroStats(StatsScope::Today)));
        assert_eq!(tile(3, 5), Some(Tile::PomodoroStats(StatsScope::AllTime)));
    }

    #[test]
    fn duration_keys_add_on_a_short_press_and_subtract_on_a_long_press() {
        let page = page(PageId::Pomodoro);
        let cases = [
            ((3, 1), Phase::Focus, 5),
            ((3, 2), Phase::ShortBreak, 1),
            ((3, 3), Phase::LongBreak, 5),
        ];

        for ((row, column), duration, step) in cases {
            let key = page
                .binding(KeyPosition::new(row, column))
                .expect("duration key");
            assert_eq!(
                key.short,
                Action::Pomodoro(PomodoroCommand::Adjust {
                    duration,
                    step_minutes: step
                })
            );
            assert_eq!(
                key.long,
                Some(Action::Pomodoro(PomodoroCommand::Adjust {
                    duration,
                    step_minutes: -step
                }))
            );
        }
    }

    #[test]
    fn only_the_keys_that_need_a_long_press_declare_one() {
        let with_long: Vec<(PageId, KeyPosition)> = PageId::ALL
            .into_iter()
            .flat_map(|id| {
                page(id)
                    .keys
                    .into_iter()
                    .filter(|key| key.has_long_action())
                    .map(move |key| (id, key.position))
                    .collect::<Vec<_>>()
            })
            .collect();

        assert_eq!(
            with_long,
            vec![
                (PageId::Home, KeyPosition::new(1, 1)),
                (PageId::Home, KeyPosition::new(2, 2)),
                (PageId::Home, KeyPosition::new(2, 3)),
                (PageId::Home, KeyPosition::new(3, 2)),
                (PageId::Home, KeyPosition::new(3, 3)),
                (PageId::Pomodoro, KeyPosition::new(3, 1)),
                (PageId::Pomodoro, KeyPosition::new(3, 2)),
                (PageId::Pomodoro, KeyPosition::new(3, 3)),
                (PageId::Application, KeyPosition::new(1, 4)),
                (PageId::Application, KeyPosition::new(1, 5)),
                (PageId::Dashboard, KeyPosition::new(1, 3)),
                (PageId::Dashboard, KeyPosition::new(1, 5)),
                (PageId::Dashboard, KeyPosition::new(2, 1)),
                (PageId::Dashboard, KeyPosition::new(2, 2)),
            ]
        );
    }

    #[test]
    fn every_page_can_reach_home() {
        for id in PageId::ALL {
            if id == PageId::Home {
                continue;
            }
            let reaches_home = page(id).keys.iter().any(|key| {
                matches!(
                    key.short,
                    Action::Navigate(PageId::Home) | Action::DismissPanel
                )
            });
            assert!(reaches_home, "{id} has no way back to Home");
        }
    }

    #[test]
    fn visibility_gating_lists_only_what_a_page_shows() {
        assert!(required_integrations(PageId::Pomodoro).is_empty());
        assert_eq!(
            required_integrations(PageId::Spotify),
            vec![IntegrationId::Spotify]
        );
        assert!(required_integrations(PageId::Home).contains(&IntegrationId::Weather));
        assert_eq!(
            required_integrations(PageId::Media),
            vec![IntegrationId::MediaSession, IntegrationId::AudioStatus]
        );
        assert_eq!(
            required_integrations(PageId::Mixer),
            vec![IntegrationId::AudioStatus]
        );
        assert!(required_integrations(PageId::Weather).contains(&IntegrationId::LakeHistory));
        assert!(!required_integrations(PageId::GitHub).contains(&IntegrationId::Spotify));
        assert!(required_integrations(PageId::WalkingPad).is_empty());
        assert!(required_integrations(PageId::WalkingPadStats).is_empty());
    }
}
