#!/bin/zsh
# Repeatable hardware acceptance test for streamdeckd.
#
# Run this with the physical Stream Deck attached and no other controller
# running. Every step is either automatic or a single question to answer.
#
#   ./scripts/hardware-acceptance.sh
#
set -u
setopt PIPE_FAIL

CTL=${CTL:-./target/release/streamdeckctl}
PASS=0
FAIL=0

step() { printf '\n\033[1m%s\033[0m\n' "$1"; }
pass() { printf '  [ok]   %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf '  [FAIL] %s\n' "$1"; FAIL=$((FAIL + 1)); }

ask() {
  local question=$1
  printf '  %s [y/N] ' "$question"
  read -r answer
  case "$answer" in
    y | Y) pass "$question" ;;
    *) fail "$question" ;;
  esac
}

run() {
  local description=$1
  shift
  if "$@" > /tmp/streamdeckd-acceptance.log 2>&1; then
    pass "$description"
  else
    fail "$description ($(tail -1 /tmp/streamdeckd-acceptance.log))"
  fi
}

step '0. Preconditions'
run 'streamdeckctl is built' test -x "$CTL"
run 'the daemon is reachable' "$CTL" status
"$CTL" doctor || fail 'doctor reported a problem'

step '1. Calibration grid'
for row in 1 2 3; do
  for column in 1 2 3 4 5; do
    printf '  press %s,%s\n' "$row" "$column"
    "$CTL" press "$row,$column" > /dev/null 2>&1
  done
done
ask 'Did every key light up and respond in turn?'

step '2. Page switching'
for page in home mixer github spotify stensjon pomodoro home; do
  run "switch to $page" "$CTL" page "$page"
done
ask 'Did every page render completely and legibly?'

step '3. Short and long press'
run 'short press on the Pomodoro glance' "$CTL" press 2,3
run 'long press on the Pomodoro glance' "$CTL" hold 2,3 --milliseconds 800
ask 'Did the long press show the HOLD affordance before switching page?'
run 'return to Home' "$CTL" page home

step '4. Audio devices'
run 'open the mixer' "$CTL" page mixer
for key in 1,2 1,3 1,4 2,3 2,4 2,5; do
  "$CTL" press "$key" > /dev/null 2>&1
done
ask 'Did available devices switch and unavailable ones stay visible but disabled?'
run 'toggle output mute' "$CTL" press 1,5
run 'toggle output mute back' "$CTL" press 1,5
run 'toggle microphone mute' "$CTL" press 3,1
run 'toggle microphone mute back' "$CTL" press 3,1

step '5. Spotify'
run 'open the Spotify page' "$CTL" page spotify
ask 'Are the transport controls showing the right playback state?'
run 'return to Home' "$CTL" page home

step '6. One-minute Pomodoro'
run 'set a short focus phase' "$CTL" page pomodoro
printf '  Setting focus to 5 minutes and starting it; skip ahead with `%s pomodoro skip`.\n' "$CTL"
run 'start focus' "$CTL" pomodoro start focus
ask 'Is the countdown ticking once a second?'
run 'skip to the next phase' "$CTL" pomodoro skip
run 'acknowledge any pending completion' "$CTL" pomodoro acknowledge
run 'reset the session' "$CTL" pomodoro reset

step '7. USB reconnect'
printf '  Unplug the Stream Deck, wait five seconds, plug it back in.\n'
read -r _
run 'the daemon still answers' "$CTL" status
ask 'Did the deck repaint completely after reconnecting?'

step '8. Sleep and wake'
printf '  Sleep the Mac (Apple menu > Sleep), wake it, then press Return.\n'
read -r _
run 'the daemon still answers' "$CTL" status
ask 'Is the Pomodoro state correct after waking?'

step '9. Resource use'
"$CTL" status --json | python3 -c '
import json, sys
data = json.load(sys.stdin)
resident = data.get("resident_mib") or 0
children = data.get("child_processes", 0)
print(f"  resident {resident:.1f} MiB, {children} child process(es)")
sys.exit(0 if resident <= 80 and children == 0 else 1)
' && pass 'idle resident memory is at most 80 MiB with no children' \
  || fail 'resource thresholds were not met'

step '10. Clean shutdown'
run 'stop the daemon' "$CTL" stop
sleep 2
descendants=$(pgrep -f 'streamdeck-alert|SwitchAudioSource|osascript' | wc -l | tr -d ' ')
if [[ "$descendants" == '0' ]]; then
  pass 'no descendant process remains'
else
  fail "$descendants descendant process(es) remain"
fi

printf '\n\033[1m%d passed, %d failed\033[0m\n' "$PASS" "$FAIL"
exit $((FAIL > 0))
