#!/bin/zsh
# One-time, read-only import of Pomodoro figures from the Elgato plugin.
#
#   ./scripts/import-elgato-state.sh [--dry-run]
#
# Reads the Command Center plugin's global settings and writes only
# streamdeckd/state.json. The Elgato profile and plugin settings are never
# modified, and a timestamped copy of the source data is kept.
set -eu
setopt PIPE_FAIL

PREFIX="${STREAMDECKD_PREFIX:-$HOME/Library/Application Support/streamdeckd}"
PLUGIN_UUID='io.github.jimmystridh.command-center'
SETTINGS="$HOME/Library/Application Support/com.elgato.StreamDeck/ProfilesV2"
GLOBAL="$HOME/Library/Application Support/com.elgato.StreamDeck/PluginSettings.json"
DRY_RUN=0

for argument in "$@"; do
  case "$argument" in
    --dry-run) DRY_RUN=1 ;;
    *)
      print -u2 "import-elgato-state.sh: unknown option $argument"
      exit 2
      ;;
  esac
done

info() { print -- "==> $*"; }

if [[ ! -f "$GLOBAL" ]]; then
  print -u2 "import-elgato-state.sh: $GLOBAL does not exist"
  print -u2 '    Nothing to import; streamdeckd will start from its defaults.'
  exit 1
fi

mkdir -p "$PREFIX"
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
BACKUP="$PREFIX/imported-elgato-$STAMP.json"

info "Reading $GLOBAL (read-only)"
python3 - "$GLOBAL" "$PLUGIN_UUID" "$BACKUP" "$PREFIX/state.json" "$DRY_RUN" <<'PY'
import json, sys, pathlib

source, plugin_uuid, backup, target, dry_run = sys.argv[1:6]
dry_run = dry_run == "1"

payload = json.loads(pathlib.Path(source).read_text())
settings = payload.get(plugin_uuid) or payload.get("PluginSettings", {}).get(plugin_uuid)
if not settings:
    print(f"    {plugin_uuid} has no stored settings; nothing to import")
    sys.exit(1)

pomodoro = settings.get("pomodoro")
if not pomodoro:
    print("    no pomodoro section in the plugin settings; nothing to import")
    sys.exit(1)

pathlib.Path(backup).write_text(json.dumps({"pomodoro": pomodoro}, indent=2) + "\n")
print(f"    kept a copy at {backup}")


def bounded(key, low, high, default):
    value = pomodoro.get(key, default)
    try:
        return max(low, min(high, round(float(value))))
    except (TypeError, ValueError):
        return default


def count(key):
    try:
        return max(0, round(float(pomodoro.get(key, 0))))
    except (TypeError, ValueError):
        return 0


def daily(key):
    source_map = pomodoro.get(key) or {}
    result = {}
    for day, value in source_map.items():
        try:
            result[day] = max(0, round(float(value)))
        except (TypeError, ValueError):
            continue
    return result


phases = {"focus": "focus", "shortBreak": "shortBreak", "longBreak": "longBreak"}
phase = phases.get(pomodoro.get("phase"), "focus")
status = pomodoro.get("status") if pomodoro.get("status") in ("ready", "running", "paused") else "ready"
ends_at = pomodoro.get("endsAt")
if status != "running" or not isinstance(ends_at, (int, float)):
    status, ends_at = ("paused" if status == "running" else status), None

state = {
    "version": 1,
    "activePage": "home",
    "inputVolumeBeforeMute": 50,
    "pomodoro": {
        "phase": phase,
        "status": status,
        "endsAtMs": int(ends_at) if ends_at else None,
        "remainingSeconds": max(1, bounded("remainingSeconds", 1, 90 * 60, 25 * 60)),
        "focusMinutes": bounded("focusMinutes", 5, 90, 25),
        "shortBreakMinutes": bounded("shortBreakMinutes", 1, 30, 5),
        "longBreakMinutes": bounded("longBreakMinutes", 5, 60, 15),
        "cycleFocusSessions": bounded("cycleFocusSessions", 0, 4, 0),
        "completedFocusSessions": count("completedFocusSessions"),
        "completedShortBreaks": count("completedShortBreaks"),
        "completedLongBreaks": count("completedLongBreaks"),
        "totalFocusMinutes": count("totalFocusMinutes"),
        "pendingCompletionPhase": phases.get(pomodoro.get("pendingCompletionPhase")),
        "dailyFocusMinutes": daily("dailyFocusMinutes"),
        "dailyFocusSessions": daily("dailyFocusSessions"),
    },
}

summary = state["pomodoro"]
print(
    f"    focus {summary['focusMinutes']}m / break {summary['shortBreakMinutes']}m / "
    f"long {summary['longBreakMinutes']}m"
)
print(
    f"    {summary['completedFocusSessions']} focus session(s), "
    f"{summary['totalFocusMinutes']} focus minute(s) all time"
)

if dry_run:
    print("    --dry-run: nothing written")
    sys.exit(0)

target = pathlib.Path(target)
if target.exists():
    print(f"    {target} already exists; refusing to overwrite it")
    print("    Remove it first if you really want to re-import.")
    sys.exit(1)

temporary = target.with_suffix(".json.tmp")
temporary.write_text(json.dumps(state, indent=2) + "\n")
temporary.chmod(0o600)
temporary.replace(target)
print(f"    wrote {target}")
PY

info 'Done. The Elgato profile and plugin settings were not modified.'
