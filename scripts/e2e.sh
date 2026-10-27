#!/usr/bin/env bash
# Drives the test app through every command and checks each result.
# Usage: scripts/e2e.sh <dir>...   the folders holding gpui-agent and gpui-agent-testapp
set -euo pipefail

[ $# -gt 0 ] || { echo "give the folders holding gpui-agent and gpui-agent-testapp" >&2; exit 2; }
for dir in "$@"; do
  PATH="$(cd "$dir" && pwd):$PATH"
done
export PATH
export GPUI_AGENT_HOME
GPUI_AGENT_HOME=$(mktemp -d)
export GPUI_AGENT_SESSION=e2e
unset DISPLAY WAYLAND_DISPLAY
shots=$(mktemp -d)
passed=0

cleanup() {
  gpui-agent close >/dev/null 2>&1 || true
  gpui-agent --session e2e-second close >/dev/null 2>&1 || true
  rm -rf "$GPUI_AGENT_HOME"
}
trap cleanup EXIT

step() {
  printf '\n$ gpui-agent %s\n' "$*"
}

expect() {
  local pattern=$1
  shift
  step "$@"
  local out
  if ! out=$(gpui-agent "$@" 2>&1); then
    printf '%s\nFAIL: command failed, expected /%s/\n' "$out" "$pattern"
    exit 1
  fi
  printf '%s\n' "$out"
  if ! grep -qE -- "$pattern" <<<"$out"; then
    printf 'FAIL: expected /%s/\n' "$pattern"
    exit 1
  fi
  passed=$((passed + 1))
}

refuse() {
  local pattern=$1
  shift
  step "$@"
  local out
  if out=$(gpui-agent "$@" 2>&1); then
    printf '%s\nFAIL: expected a refusal matching /%s/\n' "$out" "$pattern"
    exit 1
  fi
  printf '%s\n' "$out"
  if ! grep -qE -- "$pattern" <<<"$out"; then
    printf 'FAIL: expected /%s/\n' "$pattern"
    exit 1
  fi
  passed=$((passed + 1))
}

expect 'launched pid [0-9]+' launch -- gpui-agent-testapp
expect 'ok +screenshot' doctor

expect '@e4 button "Increment"' snapshot -i
expect 'textbox "Your name" ctx=TextInput' snapshot -i
expect 'row 7" \[clipped\]' snapshot -i
expect 'footer: a cached view' snapshot

expect 'clicked @e4 button "Increment"' click @e4
expect 'clicked @e4' click @e4
expect '^2$' get probe count
expect 'footer: a cached view' snapshot
expect 'button "Settings"' snapshot -i
expect 'checkbox "Notify me" \[unchecked\]' snapshot -i
expect 'clicked' click id:notify
expect 'checkbox "Notify me" \[checked\]' snapshot -i
expect 'clicked text:Settings button "Settings"' click text:Settings
expect '^1$' get probe settings_opened
expect 'count: 2' get text text:count

expect 'filled @e6 with "Ada"' fill @e6 "Ada"
expect 'typed " Lovelace"' type @e6 " Lovelace"
expect '"Ada Lovelace"' get probe name
expect 'hello, Ada Lovelace' find text hello
expect 'filled' fill @e6 ""
expect '^""$' get probe name
expect 'typed "!"' type --keys @e6 "!"
expect '^"!"$' get probe name
expect 'filled' fill @e6 ""
expect 'filled' fill text:"Your name" "Grace Hopper"
expect 'ready after' wait --text "hello, Grace Hopper" --timeout 3000

expect '@e4 button "Increment"' snapshot -i
expect 'clicked @e7 button "Show overlay"' click @e7
expect 'ready after' wait --text "An overlay" --timeout 3000
refuse 'is covered by overlay' click @e4
expect 'clicked @[0-9e]+ button "Dismiss"' find text Dismiss click
expect 'ready after' wait --gone "An overlay" --timeout 3000
expect 'clicked @e4' click @e4
expect '^3$' get probe count

expect 'row 40" \[hidden\]' find text "row 40"
refuse 'is not visible' click text:"row 40"
expect 'is visible' scrollintoview text:"row 40"
expect 'clicked' click text:"row 40"
expect '^40$' get probe selected
expect 'selected: row 40' get text text:selected
expect 'scrolled up' scroll up 5000 --on id:rows
expect 'row 1"$' find text "row 1"

expect 'clicked' click id:delete
expect 'ready after' wait --text "Delete everything?" --timeout 3000
expect 'This cannot be undone' snapshot
expect 'clicked' find text Cancel click
expect 'ready after' wait --text "status: kept" --timeout 3000
expect 'clicked' click id:delete
expect '\[prompt open\]' snapshot -i
expect 'clicked text:Delete button "Delete"' click text:Delete
expect 'ready after' wait --text "status: deleted" --timeout 3000

expect 'not asked for a file panel' get picks
expect 'clicked' click id:open
expect 'ready after' wait --text "file: open cancelled" --timeout 3000
expect 'open \(files true directories false multiple false\): cancelled, nothing was queued' get picks
expect 'answers /tmp/report.txt' pick /tmp/report.txt
expect 'clicked' click id:open
expect 'ready after' wait --text "file: opened /tmp/report.txt" --timeout 3000
expect 'answers /tmp/out.txt' pick /tmp/out.txt
expect 'clicked' click id:save
expect 'ready after' wait --text "file: saved /tmp/out.txt" --timeout 3000
expect 'save \(in /tmp named notes.txt\): /tmp/out.txt' get picks
expect 'will be cancelled' pick
expect 'clicked' click id:save
expect 'ready after' wait --text "file: save cancelled" --timeout 3000

expect 'testapp::Reset  ctrl-r' actions
expect 'pressed ctrl-i ctrl-i' press ctrl-i ctrl-i
expect '^5$' get probe count
expect 'dispatched testapp::Reset' action testapp::Reset
expect '^0$' get probe count
refuse "Didn't find an action" action nope::Nothing

expect 'ready after' wait --idle
expect '\[4\] @e4 button "Increment"' screenshot --annotate "$shots/annotated.png"
expect '[0-9]{3,}x[0-9]{3,}' screenshot "$shots/plain.png"
expect 'element.png [0-9]{2,3}x[0-9]{2}$' screenshot --element id:increment "$shots/element.png"
expect 'recording window' record start --fps 10
expect 'clicked' click id:increment
expect 'clicked' click id:increment
expect 'waited' wait 700
expect '[0-9]+ frames' record stop "$shots/recording.mp4"
expect 'group "root"' get focus
expect '^16 ' get box @e4

expect '@e4' snapshot -i
step "close and relaunch, then use an old ref"
gpui-agent close >/dev/null
gpui-agent launch -- gpui-agent-testapp >/dev/null
refuse 'no snapshot yet' click @e4

expect 'launched' --session e2e-second launch -- gpui-agent-testapp
expect 'clicked' --session e2e-second click id:increment
expect '^1$' --session e2e-second get probe count
expect '^0$' get probe count

if command -v xvfb-run >/dev/null; then
  GPUI_AGENT=1 xvfb-run -a gpui-agent-testapp >/dev/null 2>&1 &
else
  GPUI_AGENT=1 gpui-agent-testapp >/dev/null 2>&1 &
fi
standalone=$!
for _ in $(seq 1 100); do gpui-agent list | grep -q gpui-agent-testapp && break; sleep 0.2; done
expect 'gpui-agent-testapp pid' list
expect 'attached to gpui-agent-testapp' --session e2e-attach attach gpui-agent-testapp
expect 'clicked' --session e2e-attach click id:increment
expect '^1$' --session e2e-attach get probe count
pkill -P "$standalone" 2>/dev/null || true
kill "$standalone" 2>/dev/null || true

expect 'closed' close
refuse 'no app in session' snapshot

printf '\nall %d checks passed; screenshots in %s\n' "$passed" "$shots"
