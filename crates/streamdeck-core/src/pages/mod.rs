//! Declarative page and key definitions.
//!
//! Every key on every page is one [`KeyBinding`]: a coordinate, the tile to draw,
//! and the actions its short and long press produce. Nothing in the daemon
//! inspects coordinates to decide what a key does, so adding or moving a key is a
//! table edit rather than a change to the coordinator.

pub mod theme;
pub mod views;

use crate::integrations::github::MetricKind;
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
    Volume(i32),
    ToggleShuffle,
    ToggleRepeat,
    OpenApp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsScope {
    Cycle,
    Breaks,
    Today,
    AllTime,
}

/// What a key shows. The renderer resolves this against the current world view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tile {
    Blank,
    /// Navigation key back to Home.
    HomeButton,
    MixerSummary,
    ClaudeCombined,
    ClaudeFiveHour,
    ClaudeSevenDay,
    CodexUsage,
    SpotifyGlance,
    GitHubSummary,
    PomodoroGlance,
    Meeting(usize),
    WeatherCurrent,
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
        KeyBinding::new(1, 1, Tile::MixerSummary, Navigate(PageId::Mixer)),
        KeyBinding::new(
            1,
            2,
            Tile::ClaudeCombined,
            Refresh(IntegrationId::ClaudeUsage),
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
        KeyBinding::new(
            2,
            1,
            Tile::SpotifyGlance,
            Spotify(SpotifyCommand::PlayPause),
        )
        .with_long(Navigate(PageId::Spotify)),
        KeyBinding::new(2, 2, Tile::GitHubSummary, Navigate(PageId::GitHub)),
        KeyBinding::new(
            2,
            3,
            Tile::PomodoroGlance,
            Pomodoro(PomodoroCommand::Toggle),
        )
        .with_long(Navigate(PageId::Pomodoro)),
        KeyBinding::new(2, 4, Tile::Meeting(0), OpenMeeting(0)),
        KeyBinding::new(2, 5, Tile::Meeting(1), OpenMeeting(1)),
        // 3,1 and 3,2 are intentionally blank.
        KeyBinding::new(
            3,
            3,
            Tile::WeatherCurrent,
            WeatherDetail(WeatherTile::Current),
        ),
        KeyBinding::new(
            3,
            4,
            Tile::WeatherForecast(1),
            WeatherDetail(WeatherTile::Forecast),
        ),
        KeyBinding::new(3, 5, Tile::LakeCurrent, OpenPanel(PageId::Stensjon)),
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
            Tile::AudioMute(Output),
            Audio(AudioCommand::ToggleMute(Output)),
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
            2,
            4,
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
            2,
            5,
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
            1,
            Tile::AudioMute(Input),
            Audio(AudioCommand::ToggleMute(Input)),
        ),
        KeyBinding::new(
            3,
            2,
            Tile::AudioVolume {
                kind: Input,
                delta: -10,
            },
            Audio(AudioCommand::Volume {
                kind: Input,
                delta: -10,
            }),
        ),
        KeyBinding::new(
            3,
            3,
            Tile::AudioVolume {
                kind: Input,
                delta: 10,
            },
            Audio(AudioCommand::Volume {
                kind: Input,
                delta: 10,
            }),
        ),
        // 3,4 is intentionally blank.
        KeyBinding::new(
            3,
            5,
            Tile::MixerSummary,
            Refresh(IntegrationId::AudioStatus),
        ),
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
            Tile::SpotifyControl(SpotifyCommand::ToggleShuffle),
            Spotify(SpotifyCommand::ToggleShuffle),
        ),
        KeyBinding::new(
            2,
            4,
            Tile::SpotifyControl(SpotifyCommand::ToggleRepeat),
            Spotify(SpotifyCommand::ToggleRepeat),
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

/// Integrations a page needs while it is visible. Drives visibility-gated refresh
/// so nothing polls for a key nobody can see.
pub fn required_integrations(id: PageId) -> Vec<IntegrationId> {
    match id {
        PageId::Home => vec![
            IntegrationId::AudioStatus,
            IntegrationId::Meetings,
            IntegrationId::Weather,
            IntegrationId::LakeCurrent,
            IntegrationId::GitHub,
            IntegrationId::ClaudeUsage,
            IntegrationId::CodexUsage,
            IntegrationId::Spotify,
        ],
        PageId::Mixer => vec![IntegrationId::AudioStatus, IntegrationId::AudioInventory],
        PageId::GitHub => vec![IntegrationId::GitHub],
        PageId::Spotify => vec![IntegrationId::Spotify],
        PageId::Stensjon => vec![IntegrationId::LakeCurrent, IntegrationId::LakeHistory],
        PageId::Pomodoro => Vec::new(),
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

        assert_eq!(tile(1, 1), Some(Tile::MixerSummary));
        assert_eq!(tile(1, 2), Some(Tile::ClaudeCombined));
        assert_eq!(tile(1, 3), Some(Tile::CodexUsage));
        assert_eq!(tile(1, 4), Some(Tile::ClaudeFiveHour));
        assert_eq!(tile(1, 5), Some(Tile::ClaudeSevenDay));
        assert_eq!(tile(2, 1), Some(Tile::SpotifyGlance));
        assert_eq!(tile(2, 2), Some(Tile::GitHubSummary));
        assert_eq!(tile(2, 3), Some(Tile::PomodoroGlance));
        assert_eq!(tile(2, 4), Some(Tile::Meeting(0)));
        assert_eq!(tile(2, 5), Some(Tile::Meeting(1)));
        assert_eq!(tile(3, 1), None, "3,1 is intentionally blank");
        assert_eq!(tile(3, 2), None, "3,2 is intentionally blank");
        assert_eq!(tile(3, 3), Some(Tile::WeatherCurrent));
        assert_eq!(tile(3, 4), Some(Tile::WeatherForecast(1)));
        assert_eq!(tile(3, 5), Some(Tile::LakeCurrent));
    }

    #[test]
    fn home_long_presses_open_the_spotify_and_pomodoro_pages() {
        let page = page(PageId::Home);
        let spotify = page
            .binding(KeyPosition::new(2, 1))
            .expect("spotify glance");
        assert_eq!(spotify.short, Action::Spotify(SpotifyCommand::PlayPause));
        assert_eq!(spotify.long, Some(Action::Navigate(PageId::Spotify)));

        let pomodoro = page
            .binding(KeyPosition::new(2, 3))
            .expect("pomodoro glance");
        assert_eq!(pomodoro.short, Action::Pomodoro(PomodoroCommand::Toggle));
        assert_eq!(pomodoro.long, Some(Action::Navigate(PageId::Pomodoro)));
    }

    #[test]
    fn the_home_water_tile_opens_the_temporary_panel() {
        let page = page(PageId::Home);
        let lake = page.binding(KeyPosition::new(3, 5)).expect("lake tile");
        assert_eq!(lake.short, Action::OpenPanel(PageId::Stensjon));
    }

    #[test]
    fn mixer_matches_the_documented_layout_including_its_blank() {
        let page = page(PageId::Mixer);
        let tile = |row, column| {
            page.binding(KeyPosition::new(row, column))
                .map(|key| key.tile)
        };

        assert_eq!(tile(1, 1), Some(Tile::HomeButton));
        assert_eq!(
            tile(1, 4),
            Some(Tile::AudioDevice {
                kind: AudioKind::Output,
                index: 2
            })
        );
        assert_eq!(tile(1, 5), Some(Tile::AudioMute(AudioKind::Output)));
        assert_eq!(
            tile(2, 1),
            Some(Tile::AudioVolume {
                kind: AudioKind::Output,
                delta: -10
            })
        );
        assert_eq!(tile(3, 1), Some(Tile::AudioMute(AudioKind::Input)));
        assert_eq!(tile(3, 4), None, "3,4 is intentionally blank");
        assert_eq!(tile(3, 5), Some(Tile::MixerSummary));
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
            Some(Action::Spotify(SpotifyCommand::ToggleShuffle))
        );
        assert_eq!(
            action(2, 4),
            Some(Action::Spotify(SpotifyCommand::ToggleRepeat))
        );
        assert_eq!(action(2, 5), None, "remaining keys are blank");
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
                (PageId::Home, KeyPosition::new(2, 1)),
                (PageId::Home, KeyPosition::new(2, 3)),
                (PageId::Pomodoro, KeyPosition::new(3, 1)),
                (PageId::Pomodoro, KeyPosition::new(3, 2)),
                (PageId::Pomodoro, KeyPosition::new(3, 3)),
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
        assert!(!required_integrations(PageId::GitHub).contains(&IntegrationId::Spotify));
    }
}
