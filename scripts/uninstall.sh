#!/bin/zsh
# Removes streamdeckd for the current user.
#
#   ./scripts/uninstall.sh          remove the agent and binaries, keep config and state
#   ./scripts/uninstall.sh --purge  also remove configuration, state, and logs
#
# The Elgato profile is never touched.
set -eu
setopt PIPE_FAIL

PREFIX="${STREAMDECKD_PREFIX:-$HOME/Library/Application Support/streamdeckd}"
LOGS="${STREAMDECKD_LOGS:-$HOME/Library/Logs/streamdeckd}"
LABEL='io.github.jimmystridh.streamdeckd'
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
PURGE=0

for argument in "$@"; do
  case "$argument" in
    --purge) PURGE=1 ;;
    *)
      print -u2 "uninstall.sh: unknown option $argument"
      exit 2
      ;;
  esac
done

info() { print -- "==> $*"; }

if [[ -x "$PREFIX/bin/streamdeckctl" ]]; then
  info 'Stopping the daemon'
  "$PREFIX/bin/streamdeckctl" stop > /dev/null 2>&1 || true
fi

if [[ -f "$PLIST" ]]; then
  info 'Unloading the LaunchAgent'
  launchctl bootout "gui/$UID/$LABEL" > /dev/null 2>&1 || true
  rm -f "$PLIST"
fi

info 'Removing binaries'
rm -f "$PREFIX/bin/streamdeckd" "$PREFIX/bin/streamdeckctl" "$PREFIX/bin/streamdeck-alert"
rmdir "$PREFIX/bin" 2> /dev/null || true
rm -f "$PREFIX/streamdeckd.sock"

if [[ $PURGE -eq 1 ]]; then
  info 'Removing configuration, state, and logs'
  rm -f "$PREFIX/config.toml" "$PREFIX/state.json" "$PREFIX/state.json.tmp"
  rm -rf "$LOGS"
  rmdir "$PREFIX" 2> /dev/null || true
else
  info "Keeping $PREFIX/config.toml and $PREFIX/state.json"
fi

info 'Checking for leftovers'
if pgrep -qf "$PREFIX/bin/streamdeckd"; then
  print -u2 '    A streamdeckd process is still running; kill it manually.'
else
  info 'No streamdeckd process remains'
fi

info 'Done. The Elgato Stream Deck profile was not modified.'
print -- '    To go back to Elgato: open -a "Elgato Stream Deck"'
