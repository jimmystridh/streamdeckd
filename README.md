# streamdeckd

A headless macOS daemon that drives a 5×3 Stream Deck MK.2 directly over HID.

It replaces the Elgato Stream Deck application and its plugin host for one
specific setup — the Command Center layout — with a single Rust process that owns
the device, renders every key natively, and runs all state, scheduling, and
integration logic itself.

| Runtime | Resident memory | Processes |
|---|---:|---:|
| Elgato Stream Deck with the Command Center profile | ~1,232 MB | 13 |
| OpenDeck with copied plugins | ~501 MB | 7 |
| `streamdeckd` | target ≤80 MiB | 1 |

It deliberately does *not* implement the Stream Deck plugin SDK, load
`.sdPlugin` bundles, or ship a graphical profile editor. Those are what make the
alternatives expensive.

## Layout

Six pages, coordinates as `row,column`, one-based.

**Home** — mixer summary, Codex 5-hour and overall usage, Claude 5-hour and
7-day usage, Spotify glance
(hold for the Spotify page), GitHub summary, Pomodoro glance (hold for the
Pomodoro page), the next two meetings, current and tomorrow's Stensjön weather,
and the lake water temperature (opens a temporary panel).

**Mixer** — three output devices, output mute, output volume ±10, three input
devices, microphone mute, input gain ±10, and a mixer summary.

**GitHub** — review requests, authored pull requests, assigned issues, the
notification inbox, the five most recently updated authored pull requests, and a
force refresh.

**Spotify** — previous, play/pause with artwork, next, open Spotify, volume ±5,
shuffle, repeat.

**Stensjön** — current water temperature, seven-day trend, an auto-close
countdown, and seven days of history. Shown as a temporary panel that returns to
Home after ten seconds; any interaction restarts the timeout.

**Pomodoro** — timer, start/pause, skip, reset, start focus/short break/long
break, cycle and break statistics, three duration controls, and today's and
all-time focus statistics.

Blank keys are intentional and are drawn as real, blank keys.

## Install

```sh
./scripts/install.sh              # build, install, load the LaunchAgent
./scripts/install.sh --no-agent   # binaries only, start it yourself
```

The installer builds release binaries, replaces them atomically, copies the
configuration template only if no configuration exists, validates the generated
LaunchAgent with `plutil`, and rolls the binaries back if the new build does not
become healthy within twenty seconds.

Installed files:

```text
~/Library/Application Support/streamdeckd/bin/{streamdeckd,streamdeckctl,streamdeck-alert}
~/Library/Application Support/streamdeckd/config.toml
~/Library/Application Support/streamdeckd/state.json
~/Library/Application Support/streamdeckd/streamdeckd.sock   (0600)
~/Library/LaunchAgents/io.github.jimmystridh.streamdeckd.plist
~/Library/Logs/streamdeckd/
```

The daemon will not start while another application owns the device. Quit Elgato
Stream Deck or OpenDeck first; `streamdeckd` never kills them for you.

## Migrating from the Elgato plugin

```sh
./scripts/import-elgato-state.sh --dry-run   # show what would be imported
./scripts/import-elgato-state.sh             # write streamdeckd/state.json
```

This reads the plugin's global settings read-only, keeps a timestamped copy of
what it read, and writes only `streamdeckd/state.json`. It refuses to overwrite
an existing state file. Your Elgato profile is never modified.

## Control

```sh
streamdeckctl status                 # uptime, memory, page, counters, integration health
streamdeckctl status --json          # the same as a JSON payload
streamdeckctl devices                # connected decks and who owns them
streamdeckctl page home
streamdeckctl press 2,3
streamdeckctl hold 2,3 --milliseconds 700
streamdeckctl pomodoro acknowledge
streamdeckctl pomodoro start focus
streamdeckctl refresh github
streamdeckctl reload                 # transactional: invalid config changes nothing
streamdeckctl render-preview --page home --output /tmp/home.png
streamdeckctl doctor
streamdeckctl log-level debug
streamdeckctl stop
```

The socket is a closed protocol: every command is an enum variant. There is no
way to pass a command string, and nothing reaches a shell.

## Development

The default development target is a PNG, not the hardware, so you never have to
fight the Elgato app for the device:

```sh
cargo run -p streamdeckd -- --preview /tmp/deck.png --foreground
```

```sh
cargo test --workspace                                  # everything
cargo test -p streamdeck-core                           # domain rules, no I/O
UPDATE_GOLDEN=1 cargo test -p streamdeck-render         # rewrite the golden images
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo audit
```

Golden images live in `tests/golden/`. A change there shows up as an image diff
and is expected to be reviewed, not rubber-stamped.

For a hardware session:

```sh
osascript -e 'quit app "Elgato Stream Deck"'
cargo run --release -p streamdeckd -- --foreground
./scripts/hardware-acceptance.sh
```

## Architecture

```text
crates/streamdeck-core     domain model: config, state, pomodoro, pages, parsers
crates/streamdeck-render   tiny-skia renderer, embedded fonts, project-owned icons
crates/streamdeck-macos    audio, Spotify, notifications, Meet, credentials
crates/streamdeckd         device I/O, services, coordinator, control socket
crates/streamdeckctl       the CLI
crates/streamdeck-alert    the AppKit completion alert
```

`streamdeck-core` depends on no HID library, no macOS framework, and no HTTP
client, so every layout rule, timer transition, and payload parser is tested
without hardware or network access.

Three device implementations sit behind one `DeckDevice` trait: the real HID
device, a recording device for tests, and a preview device that writes a composed
PNG.

One deadline queue serves every timed behaviour — Pomodoro completion, visible
countdowns, long-press arming, panel dismissal, integration refresh, retry
backoff, alert repetition — so the process sleeps when nothing is due. Integration
refresh is visibility-gated: nothing polls for a key nobody can see.

## Secrets

Tokens are never written to configuration, state, or a log.

- GitHub uses the authenticated `gh` CLI.
- Google Calendar uses the authenticated `gog` CLI.
- Claude usage reads the existing Claude Code Keychain entry, then its credential
  file.
- Codex usage reads `~/.codex/auth.json`.

Bearer tokens are held in a `Secret` wrapper whose `Debug` and `Display` print
`<redacted>`, so an accidental interpolation cannot leak one. Meeting URLs are
validated against `meet.google.com` before being handed to the system, and
artwork is only fetched from Spotify's own image hosts.

### The Claude Keychain prompt

The first time a given `streamdeckd` binary reads the Claude Code Keychain entry,
macOS asks you to authorize it, because an unsigned binary is not on that item's
access-control list. Choose **Always Allow** once and the Claude tiles work from
then on. Until you do, they show a timeout — the read runs on a blocking thread
under a five-second limit, so a pending prompt never stalls the daemon.

Codesigning the release binaries avoids re-prompting after every rebuild:

```sh
codesign --force --sign - --identifier io.github.jimmystridh.streamdeckd \
  target/release/streamdeckd
```

An ad-hoc signature (`--sign -`) is enough for a stable identity on one machine.

## Rolling back to Elgato

```sh
streamdeckctl stop
launchctl bootout gui/$UID ~/Library/LaunchAgents/io.github.jimmystridh.streamdeckd.plist
open -a "Elgato Stream Deck"
```

Nothing needs to be regenerated: the Elgato profile is untouched throughout.

## Licensing

`streamdeckd` is MIT licensed. It is a clean implementation and contains no
OpenDeck (GPL-3.0) code and no Elgato or Spotify plugin artwork.

Third-party assets are inventoried in [`assets/ASSETS.md`](assets/ASSETS.md).
Both embedded fonts are SIL Open Font License 1.1; every icon is authored in
`crates/streamdeck-render/src/icons.rs` as vector paths.

`cargo audit` reports no vulnerabilities. It does flag `ttf-parser` as
unmaintained (RUSTSEC-2026-0192); it is used only to read glyph outlines out of
the two fonts embedded at compile time, so it never parses untrusted input.
