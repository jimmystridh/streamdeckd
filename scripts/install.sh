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
APP_ROOT="${STREAMDECKD_APP_ROOT:-$HOME/Applications}"
LOGS="${STREAMDECKD_LOGS:-$HOME/Library/Logs/streamdeckd}"
AGENTS="$HOME/Library/LaunchAgents"
LABEL='io.github.jimmystridh.streamdeckd'
PLIST="$AGENTS/$LABEL.plist"
APP="$APP_ROOT/streamdeckd.app"
REPO="${0:a:h:h}"
LOAD_AGENT=1

if [[ -n "${STREAMDECKD_CODESIGN_IDENTITY:-}" ]]; then
  CODESIGN_IDENTITY="$STREAMDECKD_CODESIGN_IDENTITY"
else
  CODESIGN_IDENTITY=$(security find-identity -v -p codesigning 2> /dev/null |
    sed -n '1s/.*"\(.*\)"/\1/p' || true)
  [[ -n "$CODESIGN_IDENTITY" ]] || CODESIGN_IDENTITY='-'
fi

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

mkdir -p "$PREFIX/bin" "$APP_ROOT" "$LOGS" "$AGENTS"

# Stop the running daemon before replacing its binary so the socket is released.
if [[ -x "$PREFIX/bin/streamdeckctl" ]]; then
  info 'Stopping the running daemon'
  "$PREFIX/bin/streamdeckctl" stop > /dev/null 2>&1 || true
fi
if [[ $LOAD_AGENT -eq 1 && -f "$PLIST" ]]; then
  launchctl bootout "gui/$UID/$LABEL" > /dev/null 2>&1 || true
fi
if [[ -x "$PREFIX/bin/streamdeckctl" ]]; then
  info 'Waiting for the previous daemon to release its devices'
  for _ in $(seq 1 50); do
    if ! "$PREFIX/bin/streamdeckctl" status > /dev/null 2>&1; then
      break
    fi
    sleep 0.1
  done
  sleep 1
fi

# Keep the previous installation so a failed start can be rolled back.
ROLLBACK=$(mktemp -d)
HAD_APP=0
HAD_PLIST=0
for binary in streamdeckd streamdeckctl streamdeck-alert; do
  if [[ -f "$PREFIX/bin/$binary" ]]; then
    cp "$PREFIX/bin/$binary" "$ROLLBACK/$binary"
    touch "$ROLLBACK/$binary.present"
  fi
done
if [[ -d "$APP" ]]; then
  ditto "$APP" "$ROLLBACK/streamdeckd.app"
  HAD_APP=1
fi
if [[ -f "$PLIST" ]]; then
  cp "$PLIST" "$ROLLBACK/$LABEL.plist"
  HAD_PLIST=1
fi

restore() {
  print -u2 '==> Startup failed; restoring the previous installation'
  for binary in streamdeckd streamdeckctl streamdeck-alert; do
    if [[ -f "$ROLLBACK/$binary.present" ]]; then
      mv "$ROLLBACK/$binary" "$PREFIX/bin/$binary"
    else
      rm -f "$PREFIX/bin/$binary"
    fi
  done
  rm -rf "$APP"
  [[ $HAD_APP -eq 1 ]] && mv "$ROLLBACK/streamdeckd.app" "$APP"
  if [[ $HAD_PLIST -eq 1 ]]; then
    mv "$ROLLBACK/$LABEL.plist" "$PLIST"
  else
    rm -f "$PLIST"
  fi
  rm -rf "$ROLLBACK"
}

info "Installing binaries into $PREFIX/bin"
if [[ "$CODESIGN_IDENTITY" == '-' ]]; then
  info 'No signing identity found; using an ad-hoc signature'
else
  info "Signing binaries with $CODESIGN_IDENTITY"
fi
for binary in streamdeckd streamdeckctl streamdeck-alert; do
  install -m 755 "$REPO/target/release/$binary" "$PREFIX/bin/$binary.new"
  if ! codesign --force --sign "$CODESIGN_IDENTITY" \
    --identifier "io.github.jimmystridh.$binary" \
    "$PREFIX/bin/$binary.new" > /dev/null 2>&1; then
    print -u2 "install.sh: could not codesign $binary"
    restore
    exit 1
  fi
  mv -f "$PREFIX/bin/$binary.new" "$PREFIX/bin/$binary"
done

info "Installing signed application bundle into $APP"
APP_STAGING="$ROLLBACK/streamdeckd.app.new"
mkdir -p "$APP_STAGING/Contents/MacOS"
install -m 755 "$REPO/target/release/streamdeckd" \
  "$APP_STAGING/Contents/MacOS/streamdeckd"
install -m 644 "$REPO/crates/streamdeckd/Info.plist" \
  "$APP_STAGING/Contents/Info.plist"
if ! codesign --force --options runtime --sign "$CODESIGN_IDENTITY" \
  --identifier "$LABEL" --entitlements "$REPO/crates/streamdeckd/streamdeckd.entitlements" \
  "$APP_STAGING" > /dev/null 2>&1; then
  print -u2 'install.sh: could not codesign the application bundle'
  restore
  exit 1
fi
rm -rf "$APP"
mv "$APP_STAGING" "$APP"

if [[ ! -f "$PREFIX/config.toml" ]]; then
  info "Installing the configuration template into $PREFIX/config.toml"
  install -m 600 "$REPO/config/command-center.toml" "$PREFIX/config.toml"
else
  info 'Keeping the existing configuration'
fi

info 'Validating the configuration'
if ! "$PREFIX/bin/streamdeckd" --config "$PREFIX/config.toml" --check; then
  restore
  exit 1
fi

if [[ $LOAD_AGENT -eq 0 ]]; then
  info 'Skipping the LaunchAgent as requested'
  info "Start manually with: '$APP/Contents/MacOS/streamdeckd' --foreground"
  exit 0
fi

info "Writing $PLIST"
sed -e "s|__PREFIX__|$PREFIX|g" -e "s|__APP__|$APP|g" \
  -e "s|__LOGS__|$LOGS|g" \
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
for _ in $(seq 1 60); do
  if "$PREFIX/bin/streamdeckctl" status > /dev/null 2>&1; then
    info 'streamdeckd is running'
    "$PREFIX/bin/streamdeckctl" doctor || true
    rm -rf "$ROLLBACK"
    exit 0
  fi
  sleep 1
done

print -u2 '==> streamdeckd did not become healthy within 60 seconds'
launchctl bootout "gui/$UID/$LABEL" > /dev/null 2>&1 || true
restore
print -u2 "==> Check $LOGS/streamdeckd.log for the reason"
exit 1
