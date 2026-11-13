#!/bin/sh
# The acceptance run on a real phone, in one go: a test computer (SSH on
# the LAN, an isolated herdr-core behind it, never the user's), the app
# built and installed over the one there (its data kept: the person's
# computers, key and choices), the test computer added beside theirs as
# `e2e-test`, then codex walking through acceptance/walkthrough.md with adb
# and writing what it found. Then the test computer is forgotten and the
# one the app was on opened again. The phone's settings are put back
# however it ends.
#
#   e2e/device.sh [--no-build]
#
# Needs one phone on adb, its Samsung Keyboard (adb's typing goes through
# the IME; a pinyin one composes it), and codex logged in (its auth.json is
# copied into the test computer's home for the run, and goes with it).
set -e
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
. "$root/env.sh"
state=${HERDC_E2E:-/tmp/herdc-e2e/run}
port=${HERDC_E2E_PORT:-2223}
model=${HERDC_E2E_MODEL:-gpt-6.1-sol}
codex=${CODEX:-codex}
keyboard=com.samsung.android.honeyboard/.service.HoneyBoardService
app=dev.yonder.app
export HERDC_SHOTS="$state/shots"
rm -rf "$state"; mkdir -p "$HERDC_SHOTS"
ui="$here/adbui.sh"

[ "$(adb get-state 2>/dev/null)" = device ] || { echo "no phone on adb" >&2; exit 1; }
lan=$(ipconfig getifaddr en0 || ipconfig getifaddr en1)

# The phone as it was, put back on the way out.
ime=$(adb shell settings get secure default_input_method | tr -d '\r')
stay=$(adb shell settings get global stay_on_while_plugged_in | tr -d '\r')
rotate=$(adb shell settings get system accelerometer_rotation | tr -d '\r')
# Still, so the UI can be read (a turning status keeps uiautomator waiting).
scales="window_animation_scale transition_animation_scale animator_duration_scale"
was_scales=$(for k in $scales; do adb shell settings get global "$k" | tr -d '\r'; done | tr '\n' ' ')
computer=""
restore() {
  adb shell ime set "$ime" >/dev/null 2>&1 || true
  adb shell settings put global stay_on_while_plugged_in "$stay" || true
  adb shell settings put system accelerometer_rotation "$rotate" || true
  set -- $was_scales
  for k in $scales; do
    case "$1" in null|"") adb shell settings delete global "$k" >/dev/null || true ;; *) adb shell settings put global "$k" "$1" || true ;; esac
    shift
  done
  adb shell svc wifi enable || true
  [ -n "$computer" ] && kill -INT "$computer" 2>/dev/null || true
}
trap restore EXIT INT TERM

# The test computer, and what the walkthrough expects on it.
cargo build -q --manifest-path "$root/Cargo.toml" -p herdc-testkit --bin herdc-test-computer
"$root/target/debug/herdc-test-computer" --listen "0.0.0.0:$port" > "$state/computer.log" 2>&1 &
computer=$!
until grep -q '^ready' "$state/computer.log"; do sleep 0.2; done
session=$(awk '/^session/ {print $2}' "$state/computer.log")
fingerprint=$(awk '/^fingerprint/ {print $2}' "$state/computer.log")
home="$session/home"
mkdir -p "$home/proj" "$home/.ssh" "$home/.codex"
(
  cd "$home/proj"
  git init -q -b main && git -c user.name=t -c user.email=t@t commit -q --allow-empty -m first
  head -c 4096 /dev/urandom > blob.bin
  seq 1 20000 | sed 's/^/line /' > big.txt
  printf '%06000d\n' 0 | tr 0 x > long.txt
)
printf 'Host vps\n  HostName 10.0.0.7\n  User can\n' > "$home/.ssh/config"
cp "${CODEX_HOME:-$HOME/.codex}/auth.json" "$home/.codex/auth.json"

# The app over the one there, and the computer added as a person adds it,
# beside theirs.
[ "$1" = --no-build ] || "$root/app/apk.sh" > "$state/apk.log" 2>&1
adb install -r "$root/app/.nsh/out/Yonder.apk" >/dev/null
adb shell svc power stayon usb
for k in $scales; do adb shell settings put global "$k" 0; done
adb shell input keyevent KEYCODE_WAKEUP; adb shell input swipe 720 2400 720 800
adb shell ime set "$keyboard" >/dev/null
adb shell am start -W -n "$app/com.longbridge.nsh.AppActivity" >/dev/null
wait_for() { for _ in $(seq 60); do "$ui" dump 2>/dev/null | grep -q "$1" && return; sleep 1; done; echo "never saw $1" >&2; exit 1; }
seen() { "$ui" dump 2>/dev/null | grep -q "$1"; }
# Back to the computers: left from the workspace (Settings) or from home (its menu).
for _ in $(seq 30); do seen "'Switch space'\|'Menu'\|'Add a computer'" && break; sleep 1; done
if seen "'Switch space'"; then
  "$ui" press More; sleep 1; "$ui" press Settings; sleep 1.5; "$ui" press "Leave this computer"
elif seen "'Menu'"; then
  "$ui" press Menu; sleep 1; "$ui" press pick-leave
fi
wait_for "'Add a computer'"
# The computer the app was on: the first under COMPUTERS, opened again at the end.
was=$("$ui" nodes | awk -F '	' '$1 == "COMPUTERS" { after = 1; next } after && $2 != "" && $2 != "Add a computer" { print $2; exit }')
echo "was on: ${was:-none}" > "$state/was.txt"
"$ui" press "Add a computer"; sleep 1
field() { "$ui" press "$1"; sleep 0.5; }
field "display name"; "$ui" type e2e-test
field computer; "$ui" type "$lan"
field user; "$ui" type me
field port; adb shell input keyevent KEYCODE_MOVE_END; for _ in 1 2 3 4 5; do adb shell input keyevent KEYCODE_DEL; done; "$ui" type "$port"
field password; "$ui" type "open sesame"
"$ui" key KEYCODE_BACK; sleep 0.8
"$ui" press Connect; sleep 3
"$ui" dump | grep -q "$fingerprint" || { echo "the phone was shown another host key" >&2; exit 1; }
"$ui" press Trust
wait_for 'No spaces yet'
"$ui" press 'New space'
wait_for "1 · e2e-test"

# codex drives the phone through the walkthrough.
cp "$ui" "$here/acceptance/walkthrough.md" "$state/"
CODEX_HOME="$home/.codex" "$codex" --no-daemon exec -m "$model" -s danger-full-access --skip-git-repo-check --ephemeral \
  -C "$state" -o "$state/walkthrough.result.md" \
  "Follow walkthrough.md in this folder on the phone, step by step, with ./adbui.sh. Screenshots go to $HERDC_SHOTS. At the end, write for each step: Yes or No, and why, with the screenshots' names." \
  > "$state/codex.log" 2>&1
cat "$state/walkthrough.result.md"

# The test computer forgotten, and the person's opened again.
adb shell am start -W -n "$app/com.longbridge.nsh.AppActivity" >/dev/null; sleep 2
if seen "'Switch space'"; then
  "$ui" press More; sleep 1; "$ui" press Settings; sleep 1.5; "$ui" press "Leave this computer"
elif seen "'Menu'"; then
  "$ui" press Menu; sleep 1; "$ui" press pick-leave
fi
wait_for "'Add a computer'"
adb shell input swipe $("$ui" find e2e-test) $("$ui" find e2e-test) 900; sleep 1
"$ui" press pick-forget; sleep 1; "$ui" press Forget; sleep 1
if seen "'e2e-test'"; then echo "e2e-test is still on the phone: forget it by hand" >&2; fi
[ -n "$was" ] && "$ui" press "$was" || echo "open your computer again by hand" >&2
