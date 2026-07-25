# Asset inventory

Every asset shipped with `streamdeckd`, its origin, and its licence. Nothing here
is Elgato, Spotify, or OpenDeck artwork.

## Fonts

Both fonts are embedded in the `streamdeck-render` binary at compile time via
`include_bytes!`. Only the two families the renderer actually uses are included,
and both are variable fonts, so a weight is a variation setting rather than
another embedded file.

| File | Family | Version | Licence | Source |
|---|---|---|---|---|
| `fonts/Inter.ttf` | Inter (variable, `opsz` + `wght`) | Google Fonts distribution | SIL Open Font License 1.1 | <https://github.com/google/fonts/tree/main/ofl/inter> |
| `fonts/JetBrainsMono.ttf` | JetBrains Mono (variable, `wght`) | Google Fonts distribution | SIL Open Font License 1.1 | <https://github.com/google/fonts/tree/main/ofl/jetbrainsmono> |

Licence texts are shipped alongside them as `fonts/Inter-OFL.txt` and
`fonts/JetBrainsMono-OFL.txt`, as the OFL requires.

**Attribution required by the OFL:**

- Inter — Copyright 2020 The Inter Project Authors
  (<https://github.com/rsms/inter>)
- JetBrains Mono — Copyright 2020 The JetBrains Mono Project Authors
  (<https://github.com/JetBrains/JetBrainsMono>)

Neither font is renamed or modified. Reserved Font Names are respected: the files
keep their upstream family names.

Inter is used for every label and value. JetBrains Mono is used only where digits
must not shift horizontally as they change — the Pomodoro countdown — because its
tabular figures keep the timer from jittering once a second.

## Icons

There are no icon files. Every glyph the renderer draws — play, pause, next,
previous, skip, reset, refresh, check, cross, plus, minus, shuffle, repeat,
repeat-one, home, speaker, speaker-muted, microphone, microphone-muted, calendar,
tomato, GitHub, note, sun, moon, cloud, rain, snow, sleet, thunder, fog, water,
trend-up, trend-down, warning — is authored as vector paths in a unit square in
`crates/streamdeck-render/src/icons.rs`.

This is deliberate. Owning the geometry means:

- no font has to happen to contain a play triangle or a check mark;
- no proprietary plugin artwork is redistributed;
- the weather symbol families map onto shapes this project controls, so MET
  Norway's ~90 `symbol_code` values reduce to eight reviewable icons.

The icons are original work, MIT licensed with the rest of the repository.

## Colours

The palette in `crates/streamdeck-core/src/pages/theme.rs` is original, chosen to
match the previous tiles closely enough that the deck still reads the same at a
glance. Colour values are not copyrightable, and none are taken from a
proprietary brand asset.

## Test fixtures

`tests/fixtures/` holds recorded-shape API payloads used by the parser tests.
They were written for this repository from the observed response *shapes* of the
live endpoints; they contain no real credentials, no real meeting URLs, and no
personal data beyond the two lake identifiers and the Stensjön coordinates, which
are public.

| File | Stands in for |
|---|---|
| `github-search-prs.json` | `gh search prs --json …` |
| `gog-calendar-events.json` | `gog calendar events --json` |
| `met-locationforecast.json` | MET Norway Locationforecast compact |
| `lake-current.json` | Mölndal Energi `getAllCurrent` |
| `lake-historic.json` | Mölndal Energi `getAllHistoric` |
| `claude-usage.json` | `api.anthropic.com/api/oauth/usage` |
| `codex-usage.json` | `chatgpt.com/backend-api/wham/usage` |

## Sounds

None are shipped. The completion alert plays a macOS system sound by name from
`/System/Library/Sounds`, and the configured name is validated to be a bare
alphanumeric identifier before it is used.
