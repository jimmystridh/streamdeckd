//! Persistent state with atomic writes and schema migrations.
//!
//! Writes go to a sibling temporary file, are fsynced, then renamed over the
//! target. Critical Pomodoro transitions additionally sync the directory so an
//! acknowledged completion or a running deadline cannot be lost to a crash.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::integrations::walkingpad::{
    WalkingPadDailyTotals, DEFAULT_QUICK_SPEEDS_TENTHS, MAX_SPEED_TENTHS, MIN_SPEED_TENTHS,
};
use crate::model::PageId;
use crate::pomodoro::PomodoroState;

pub const CURRENT_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("could not read state at {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not write state at {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not parse state at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("state at {path} declares unsupported version {version}")]
    UnsupportedVersion { path: PathBuf, version: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistentState {
    pub version: u32,
    pub active_page: PageId,
    pub pomodoro: PomodoroState,
    /// Last non-zero microphone input volume, restored when unmuting.
    pub input_volume_before_mute: u8,
    /// Bounded integration caches that make an offline cold start useful.
    #[serde(default)]
    pub cached: CachedIntegrations,
    #[serde(default)]
    pub walkingpad: WalkingPadDailyTotals,
    #[serde(default = "default_walkingpad_quick_speeds")]
    pub walkingpad_quick_speeds: [u8; 5],
}

const fn default_walkingpad_quick_speeds() -> [u8; 5] {
    DEFAULT_QUICK_SPEEDS_TENTHS
}

impl Default for PersistentState {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            active_page: PageId::Home,
            pomodoro: PomodoroState::default(),
            input_volume_before_mute: 50,
            cached: CachedIntegrations::default(),
            walkingpad: WalkingPadDailyTotals::default(),
            walkingpad_quick_speeds: DEFAULT_QUICK_SPEEDS_TENTHS,
        }
    }
}

/// Small, explicitly bounded snapshots persisted so the Home page is useful
/// within two seconds of a cold start even without network access.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedIntegrations {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub payloads: BTreeMap<String, CachedPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedPayload {
    pub captured_at_ms: i64,
    pub json: String,
}

impl CachedIntegrations {
    /// Keeps the cache small enough that state writes stay cheap.
    pub const MAX_ENTRIES: usize = 12;
    pub const MAX_ENTRY_BYTES: usize = 32 * 1024;

    pub fn store(&mut self, key: &str, captured_at_ms: i64, json: String) {
        if json.len() > Self::MAX_ENTRY_BYTES {
            return;
        }
        self.payloads.insert(
            key.to_string(),
            CachedPayload {
                captured_at_ms,
                json,
            },
        );
        while self.payloads.len() > Self::MAX_ENTRIES {
            let oldest = self
                .payloads
                .iter()
                .min_by_key(|(_, payload)| payload.captured_at_ms)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => {
                    self.payloads.remove(&key);
                }
                None => break,
            }
        }
    }

    pub fn get(&self, key: &str) -> Option<&CachedPayload> {
        self.payloads.get(key)
    }
}

/// How hard a write should try before returning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// File contents are fsynced and renamed. Used for cosmetic updates.
    Normal,
    /// Additionally fsyncs the containing directory. Used for timer transitions.
    Critical,
}

/// Owns the on-disk state file. Cheap to clone-free: the runtime holds one.
#[derive(Debug)]
pub struct StateStore {
    path: PathBuf,
}

impl StateStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads state, migrating older schema versions. A missing file yields
    /// defaults; a corrupt file is moved aside so the daemon still starts.
    pub fn load(&self, defaults: PomodoroState) -> Result<PersistentState, StateError> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut state = PersistentState {
                    pomodoro: defaults,
                    ..Default::default()
                };
                state.pomodoro.normalize();
                return Ok(state);
            }
            Err(source) => {
                return Err(StateError::Read {
                    path: self.path.clone(),
                    source,
                })
            }
        };

        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|source| StateError::Parse {
                path: self.path.clone(),
                source,
            })?;
        let mut state = migrate(value, &self.path)?;
        state.pomodoro.normalize();
        state.input_volume_before_mute = state.input_volume_before_mute.min(100);
        for (speed, fallback) in state
            .walkingpad_quick_speeds
            .iter_mut()
            .zip(DEFAULT_QUICK_SPEEDS_TENTHS)
        {
            if !(MIN_SPEED_TENTHS..=MAX_SPEED_TENTHS).contains(speed) {
                *speed = fallback;
            }
        }
        Ok(state)
    }

    pub fn save(&self, state: &PersistentState, durability: Durability) -> Result<(), StateError> {
        let directory = self.path.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(directory).map_err(|source| StateError::Write {
            path: directory.to_path_buf(),
            source,
        })?;

        let mut serialized =
            serde_json::to_vec_pretty(state).expect("persistent state is always serializable");
        serialized.push(b'\n');

        let temporary = self.path.with_extension("json.tmp");
        let write = || -> std::io::Result<()> {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode_0600()
                .open(&temporary)?;
            file.write_all(&serialized)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, &self.path)?;
            if durability == Durability::Critical {
                File::open(directory)?.sync_all()?;
            }
            Ok(())
        };

        write().map_err(|source| {
            let _ = fs::remove_file(&temporary);
            StateError::Write {
                path: self.path.clone(),
                source,
            }
        })
    }
}

/// Extension so the write path stays readable while still creating `0600` files.
trait ModeExt {
    fn mode_0600(&mut self) -> &mut Self;
}

impl ModeExt for OpenOptions {
    #[cfg(unix)]
    fn mode_0600(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt;
        self.mode(0o600)
    }

    #[cfg(not(unix))]
    fn mode_0600(&mut self) -> &mut Self {
        self
    }
}

fn migrate(value: serde_json::Value, path: &Path) -> Result<PersistentState, StateError> {
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u32;

    match version {
        // Version 0 is the pre-release shape: a bare Pomodoro object with no wrapper.
        0 => {
            let pomodoro =
                serde_json::from_value(value).unwrap_or_else(|_| PomodoroState::default());
            Ok(PersistentState {
                pomodoro,
                ..Default::default()
            })
        }
        CURRENT_VERSION => serde_json::from_value(value).map_err(|source| StateError::Parse {
            path: path.to_path_buf(),
            source,
        }),
        version => Err(StateError::UnsupportedVersion {
            path: path.to_path_buf(),
            version,
        }),
    }
}

/// Reads Pomodoro figures out of the Elgato plugin's global settings so the
/// migration keeps streak statistics. Read-only: the caller writes only
/// `streamdeckd/state.json`.
pub fn import_legacy_pomodoro(global_settings: &serde_json::Value) -> Option<PomodoroState> {
    let pomodoro = global_settings.get("pomodoro")?;
    let number = |key: &str| -> Option<u32> {
        pomodoro
            .get(key)
            .and_then(serde_json::Value::as_f64)
            .map(|value| value.max(0.0).round() as u32)
    };
    let map = |key: &str| -> BTreeMap<String, u32> {
        pomodoro
            .get(key)
            .and_then(serde_json::Value::as_object)
            .map(|object| {
                object
                    .iter()
                    .filter_map(|(day, value)| {
                        Some((day.clone(), value.as_f64()?.max(0.0).round() as u32))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let phase = pomodoro
        .get("phase")
        .and_then(serde_json::Value::as_str)
        .and_then(crate::pomodoro::Phase::parse)
        .unwrap_or(crate::pomodoro::Phase::Focus);
    let status = match pomodoro.get("status").and_then(serde_json::Value::as_str) {
        Some("running") => crate::pomodoro::Status::Running,
        Some("paused") => crate::pomodoro::Status::Paused,
        _ => crate::pomodoro::Status::Ready,
    };
    let ends_at_ms = pomodoro
        .get("endsAt")
        .and_then(serde_json::Value::as_f64)
        .map(|value| value as i64);

    let mut state = PomodoroState {
        phase,
        status,
        ends_at_ms,
        remaining_seconds: number("remainingSeconds").unwrap_or(25 * 60),
        focus_minutes: number("focusMinutes").unwrap_or(25),
        short_break_minutes: number("shortBreakMinutes").unwrap_or(5),
        long_break_minutes: number("longBreakMinutes").unwrap_or(15),
        cycle_focus_sessions: number("cycleFocusSessions").unwrap_or(0),
        completed_focus_sessions: number("completedFocusSessions").unwrap_or(0),
        completed_short_breaks: number("completedShortBreaks").unwrap_or(0),
        completed_long_breaks: number("completedLongBreaks").unwrap_or(0),
        total_focus_minutes: number("totalFocusMinutes").unwrap_or(0),
        pending_completion_phase: pomodoro
            .get("pendingCompletionPhase")
            .and_then(serde_json::Value::as_str)
            .and_then(crate::pomodoro::Phase::parse),
        daily_focus_minutes: map("dailyFocusMinutes"),
        daily_focus_sessions: map("dailyFocusSessions"),
    };
    state.normalize();
    Some(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::walkingpad::WalkingPadCounters;
    use crate::pomodoro::{Phase, Status};

    fn store() -> (tempfile::TempDir, StateStore) {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = StateStore::new(directory.path().join("state.json"));
        (directory, store)
    }

    #[test]
    fn a_missing_file_yields_configured_defaults() {
        let (_directory, store) = store();
        let defaults = PomodoroState {
            focus_minutes: 50,
            remaining_seconds: 50 * 60,
            ..PomodoroState::default()
        };

        let state = store.load(defaults).expect("load");
        assert_eq!(state.version, CURRENT_VERSION);
        assert_eq!(state.pomodoro.focus_minutes, 50);
        assert_eq!(state.active_page, PageId::Home);
        assert_eq!(state.walkingpad_quick_speeds, DEFAULT_QUICK_SPEEDS_TENTHS);
    }

    #[test]
    fn save_then_load_round_trips_and_leaves_no_temporary_file() {
        let (directory, store) = store();
        let state = PersistentState {
            active_page: PageId::Pomodoro,
            pomodoro: PomodoroState {
                status: Status::Running,
                ends_at_ms: Some(1_753_300_000_000),
                completed_focus_sessions: 7,
                ..PomodoroState::default()
            },
            walkingpad: WalkingPadDailyTotals {
                date: "2026-08-04".to_string(),
                distance_hundredths: 152,
                steps: 2_431,
                elapsed_seconds: 1_807,
                last_observed: Some(WalkingPadCounters {
                    distance_hundredths: 45,
                    steps: 721,
                    elapsed_seconds: 529,
                }),
                last_observed_at_ms: Some(1_785_838_400_000),
            },
            walkingpad_quick_speeds: [10, 20, 30, 40, 50],
            ..PersistentState::default()
        };

        store.save(&state, Durability::Critical).expect("save");
        let loaded = store.load(PomodoroState::default()).expect("load");

        assert_eq!(loaded, state);
        let leftovers: Vec<_> = fs::read_dir(directory.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(leftovers, vec!["state.json".to_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn saved_state_is_user_only() {
        use std::os::unix::fs::PermissionsExt;
        let (_directory, store) = store();
        store
            .save(&PersistentState::default(), Durability::Normal)
            .expect("save");
        let mode = fs::metadata(store.path())
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "state must not be world readable");
    }

    #[test]
    fn version_zero_state_migrates_from_a_bare_pomodoro_object() {
        let (_directory, store) = store();
        fs::write(
            store.path(),
            r#"{"phase":"longBreak","status":"paused","remainingSeconds":120,
                "focusMinutes":30,"shortBreakMinutes":7,"longBreakMinutes":20,
                "cycleFocusSessions":2,"completedFocusSessions":9,"completedShortBreaks":4,
                "completedLongBreaks":1,"totalFocusMinutes":270,"pendingCompletionPhase":null,
                "endsAtMs":null,"dailyFocusMinutes":{},"dailyFocusSessions":{}}"#,
        )
        .expect("write");

        let state = store.load(PomodoroState::default()).expect("load");
        assert_eq!(state.version, CURRENT_VERSION);
        assert_eq!(state.pomodoro.phase, Phase::LongBreak);
        assert_eq!(state.pomodoro.focus_minutes, 30);
        assert_eq!(state.pomodoro.completed_focus_sessions, 9);
    }

    #[test]
    fn a_future_schema_version_is_refused_rather_than_silently_downgraded() {
        let (_directory, store) = store();
        fs::write(store.path(), r#"{"version":99}"#).expect("write");
        let error = store.load(PomodoroState::default()).expect_err("refused");
        assert!(
            matches!(error, StateError::UnsupportedVersion { version: 99, .. }),
            "{error}"
        );
    }

    #[test]
    fn a_corrupt_file_reports_a_parse_error_naming_the_path() {
        let (_directory, store) = store();
        fs::write(store.path(), "{not json").expect("write");
        let error = store
            .load(PomodoroState::default())
            .expect_err("parse error");
        assert!(matches!(error, StateError::Parse { .. }), "{error}");
    }

    #[test]
    fn loaded_state_is_normalized() {
        let (_directory, store) = store();
        fs::write(
            store.path(),
            serde_json::to_string(&serde_json::json!({
                "version": 1,
                "activePage": "home",
                "inputVolumeBeforeMute": 250,
                "walkingpadQuickSpeeds": [0, 30, 61, 42, 5],
                "pomodoro": {
                    "phase": "focus", "status": "running", "endsAtMs": null,
                    "remainingSeconds": 1500, "focusMinutes": 900,
                    "shortBreakMinutes": 5, "longBreakMinutes": 15,
                    "cycleFocusSessions": 40, "completedFocusSessions": 0,
                    "completedShortBreaks": 0, "completedLongBreaks": 0,
                    "totalFocusMinutes": 0, "pendingCompletionPhase": null
                }
            }))
            .expect("json"),
        )
        .expect("write");

        let state = store.load(PomodoroState::default()).expect("load");
        assert_eq!(state.pomodoro.status, Status::Paused);
        assert_eq!(state.pomodoro.focus_minutes, 90);
        assert_eq!(state.pomodoro.cycle_focus_sessions, 4);
        assert_eq!(state.input_volume_before_mute, 100);
        assert_eq!(state.walkingpad_quick_speeds, [26, 30, 34, 42, 5]);
        assert_eq!(
            state.walkingpad,
            WalkingPadDailyTotals::default(),
            "pre-WalkingPad version-one state must remain loadable"
        );
    }

    #[test]
    fn cached_integration_payloads_stay_bounded() {
        let mut cached = CachedIntegrations::default();
        for index in 0..40 {
            cached.store(&format!("key-{index}"), index as i64, "{}".to_string());
        }
        assert_eq!(cached.payloads.len(), CachedIntegrations::MAX_ENTRIES);
        assert!(cached.get("key-39").is_some(), "newest entry is retained");
        assert!(cached.get("key-0").is_none(), "oldest entry was evicted");

        cached.store(
            "huge",
            100,
            "x".repeat(CachedIntegrations::MAX_ENTRY_BYTES + 1),
        );
        assert!(
            cached.get("huge").is_none(),
            "oversized payloads are dropped"
        );
    }

    #[test]
    fn legacy_plugin_settings_import_durations_and_statistics() {
        let settings = serde_json::json!({
            "pomodoro": {
                "phase": "shortBreak",
                "status": "ready",
                "endsAt": null,
                "remainingSeconds": 300,
                "focusMinutes": 45,
                "shortBreakMinutes": 8,
                "longBreakMinutes": 25,
                "cycleFocusSessions": 3,
                "completedFocusSessions": 118,
                "completedShortBreaks": 90,
                "completedLongBreaks": 25,
                "totalFocusMinutes": 3120,
                "pendingCompletionPhase": "focus",
                "dailyFocusMinutes": {"2026-07-24": 120},
                "dailyFocusSessions": {"2026-07-24": 5}
            }
        });

        let state = import_legacy_pomodoro(&settings).expect("import");
        assert_eq!(state.phase, Phase::ShortBreak);
        assert_eq!(state.focus_minutes, 45);
        assert_eq!(state.total_focus_minutes, 3120);
        assert_eq!(state.completed_focus_sessions, 118);
        assert_eq!(state.pending_completion_phase, Some(Phase::Focus));
        assert_eq!(state.daily_focus_minutes.get("2026-07-24"), Some(&120));
        assert!(import_legacy_pomodoro(&serde_json::json!({})).is_none());
    }
}
