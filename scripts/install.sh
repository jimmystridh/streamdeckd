#!/bin/zsh
# Installs streamdeckd for the current user.
#
#   ./scripts/install.sh            build, install, and load the LaunchAgent
#   ./scripts/install.sh --no-agent install the binaries only
#
# Binaries are replaced atomically, configuration is never overwritten, and state
# is migrated by the daemon itself on first start. Rolls back the binaries if the
# new build does not become healthy.
set -eu
setopt PIPE_FAIL

PREFIX="${STREAMDECKD_PREFIX:-$HOME/Library/Application Support/streamdeckd}"
LOGS="${STREAMDECKD_LOGS:-$HOME/Library/Logs/streamdeckd}"
AGENTS="$HOME/Library/LaunchAgents"
LABEL='io.github.jimmystridh.streamdeckd'
PLIST="$AGENTS/$LABEL.plist"
REPO="${0:a:h:h}"
LOAD_AGENT=1

for argument in "$@"; do
  case "$argument" in
    --no-agent) LOAD_AGENT=0 ;;
    *)
      print -u2 "install.sh: unknown option $argument"
      exit 2
      ;;
  esac
done

info() { print -- "==> $*"; }

info 'Building release binaries'
(cd "$REPO" && cargo build --release --workspace)

mkdir -p "$PREFIX/bin" "$LOGS" "$AGENTS"

# Stop the running daemon before replacing its binary so the socket is released.
if [[ -x "$PREFIX/bin/streamdeckctl" ]]; then
  info 'Stopping the running daemon'
  "$PREFIX/bin/streamdeckctl" stop > /dev/null 2>&1 || true
fi
if [[ $LOAD_AGENT -eq 1 && -f "$PLIST" ]]; then
  launchctl bootout "gui/$UID/$LABEL" > /dev/null 2>&1 || true
fi

# Keep the previous binaries so a failed start can be rolled back.
ROLLBACK=$(mktemp -d)
for binary in streamdeckd streamdeckctl streamdeck-alert; do
  [[ -f "$PREFIX/bin/$binary" ]] && cp "$PREFIX/bin/$binary" "$ROLLBACK/$binary"
done

restore() {
  print -u2 '==> Startup failed; restoring the previous binaries'
  for binary in streamdeckd streamdeckctl streamdeck-alert; do
    [[ -f "$ROLLBACK/$binary" ]] && mv "$ROLLBACK/$binary" "$PREFIX/bin/$binary"
  done
}

info "Installing binaries into $PREFIX/bin"
for binary in streamdeckd streamdeckctl streamdeck-alert; do
  install -m 755 "$REPO/target/release/$binary" "$PREFIX/bin/$binary.new"
  # An ad-hoc signature gives the installed executable a stable macOS identity.
  codesign --force --sign - --identifier "io.github.jimmystridh.$binary" \
    "$PREFIX/bin/$binary.new" > /dev/null 2>&1 ||
    print -u2 "    warning: could not codesign $binary"
  mv -f "$PREFIX/bin/$binary.new" "$PREFIX/bin/$binary"
done

if [[ ! -f "$PREFIX/config.toml" ]]; then
  info "Installing the configuration template into $PREFIX/config.toml"
  install -m 600 "$REPO/config/command-center.toml" "$PREFIX/config.toml"
else
  info 'Keeping the existing configuration'
fi

info 'Validating the configuration'
if ! "$PREFIX/bin/streamdeckd" --check; then
  restore
  exit 1
fi

if [[ $LOAD_AGENT -eq 0 ]]; then
  info 'Skipping the LaunchAgent as requested'
  info "Start manually with: '$PREFIX/bin/streamdeckd' --foreground"
  exit 0
fi

info "Writing $PLIST"
sed -e "s|__PREFIX__|$PREFIX|g" -e "s|__LOGS__|$LOGS|g" \
  "$REPO/config/$LABEL.plist" > "$PLIST.new"

if ! plutil -lint "$PLIST.new" > /dev/null; then
  print -u2 'install.sh: the generated LaunchAgent is not a valid plist'
  rm -f "$PLIST.new"
  restore
  exit 1
fi
mv -f "$PLIST.new" "$PLIST"
chmod 644 "$PLIST"

info 'Checking device ownership'
if pgrep -qf 'Elgato Stream Deck'; then
  print -u2 '    Elgato Stream Deck is running and owns the device.'
  print -u2 '    Quit it before starting streamdeckd, or the daemon will exit.'
fi

info 'Loading the LaunchAgent'
launchctl bootstrap "gui/$UID" "$PLIST"

info 'Waiting for the daemon to become healthy'
for _ in $(seq 1 20); do
  if "$PREFIX/bin/streamdeckctl" status > /dev/null 2>&1; then
    info 'streamdeckd is running'
    "$PREFIX/bin/streamdeckctl" doctor || true
    rm -rf "$ROLLBACK"
    exit 0
  fi
  sleep 1
done

print -u2 '==> streamdeckd did not become healthy within 20 seconds'
launchctl bootout "gui/$UID/$LABEL" > /dev/null 2>&1 || true
restore
print -u2 "==> Check $LOGS/streamdeckd.log for the reason"
exit 1
