#!/bin/zsh
# Soak test for streamdeckd.
#
#   ./scripts/soak.sh [hours]      default 24
#
# Samples `streamdeckctl status --json` every minute into a CSV, and every few
# minutes exercises the daemon: page changes, a running Pomodoro, and forced
# refreshes. Sleep and wake the Mac, and start and stop Spotify, while it runs.
#
# At the end it reports memory growth, task and child counts, render and USB write
# totals, and integration refresh counts against the plan's thresholds.
set -eu
setopt PIPE_FAIL

HOURS=${1:-24}
CTL=${CTL:-$HOME/Library/Application Support/streamdeckd/bin/streamdeckctl}
OUT=${OUT:-$HOME/Library/Logs/streamdeckd/soak-$(date -u +%Y%m%dT%H%M%SZ).csv}
PAGES=(home mixer github spotify pomodoro home)

if [[ ! -x "$CTL" ]]; then
  print -u2 "soak.sh: $CTL is not executable"
  exit 1
fi
if ! "$CTL" status > /dev/null 2>&1; then
  print -u2 'soak.sh: streamdeckd is not running'
  exit 1
fi

print -- "==> Soaking for ${HOURS}h, sampling into $OUT"
print -- 'minute,resident_mib,children,renders,frames_sent,frames_skipped,bytes_sent,wakes,reconnects' > "$OUT"

# Start a long focus phase so the timer runs for the whole soak.
"$CTL" pomodoro start focus > /dev/null

SAMPLES=$((HOURS * 60))
for minute in $(seq 1 "$SAMPLES"); do
  "$CTL" status --json | python3 -c "
import json, sys
data = json.load(sys.stdin)
print(','.join(str(value) for value in [
    $minute,
    round(data.get('resident_mib') or 0, 2),
    data.get('child_processes', 0),
    data.get('renders', 0),
    data.get('frames_sent', 0),
    data.get('frames_skipped', 0),
    data.get('bytes_sent', 0),
    data.get('wakes', 0),
    data.get('device_reconnects', 0),
]))
" >> "$OUT"

  # Every five minutes, move around and force a refresh.
  if (( minute % 5 == 0 )); then
    "$CTL" page "${PAGES[$(( (minute / 5) % ${#PAGES[@]} + 1 ))]}" > /dev/null 2>&1 || true
    "$CTL" refresh github > /dev/null 2>&1 || true
    "$CTL" refresh weather > /dev/null 2>&1 || true
  fi
  # Every thirty minutes, restart the timer so completions and alerts happen.
  if (( minute % 30 == 0 )); then
    "$CTL" pomodoro acknowledge > /dev/null 2>&1 || true
    "$CTL" pomodoro start focus > /dev/null 2>&1 || true
  fi

  sleep 60
done

print -- '==> Soak finished; summarising'
python3 - "$OUT" <<'PY'
import csv, sys

rows = list(csv.DictReader(open(sys.argv[1])))
if len(rows) < 2:
    print('    not enough samples to summarise')
    sys.exit(1)

resident = [float(row['resident_mib']) for row in rows]
first, last, peak = resident[0], resident[-1], max(resident)
growth = (last - first) / first * 100 if first else 0
children = max(int(row['children']) for row in rows)
sent = int(rows[-1]['frames_sent'])
skipped = int(rows[-1]['frames_skipped'])

print(f"    samples          {len(rows)}")
print(f"    resident         {first:.1f} -> {last:.1f} MiB (peak {peak:.1f})")
print(f"    growth           {growth:+.1f}%")
print(f"    max children     {children}")
print(f"    frames           {sent} sent, {skipped} skipped")
print(f"    wakes            {rows[-1]['wakes']}")
print(f"    reconnects       {rows[-1]['device_reconnects']}")

failures = []
if peak > 100:
    failures.append(f'peak resident {peak:.1f} MiB exceeds 100 MiB')
if growth > 10:
    failures.append(f'memory grew {growth:.1f}%, over the 10% limit')
if children > 4:
    failures.append(f'{children} concurrent children is more than expected')
if skipped == 0:
    failures.append('no frames were skipped, so unchanged-frame suppression is not working')

for failure in failures:
    print(f"    FAIL: {failure}")
sys.exit(1 if failures else 0)
PY
