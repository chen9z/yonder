#!/bin/sh
# On an open terminal, sample the same 12 swipes while its task prints output.
# Coordinates are fractions of the display; never type into the task.
# Usage: e2e/perf.sh OUTPUT_DIRECTORY [PACKAGE]
set -eu
cd "$(dirname "$0")/.."
. ./env.sh
out=${1:?output directory required}
package=${2:-dev.yonder.app}
mkdir -p "$out"
adb get-state >/dev/null
adb shell dumpsys window | grep mCurrentFocus | grep -F "$package/" >/dev/null || { echo 'Open the test terminal first' >&2; exit 1; }
size=$(adb shell wm size | tail -1 | tr -d '\r' | sed 's/.*: //')
w=${size%x*}; h=${size#*x}
x=$((w / 2)); top=$((h * 28 / 100)); bottom=$((h * 71 / 100))
adb shell dumpsys battery > "$out/battery.txt"
adb shell dumpsys gfxinfo "$package" reset > "$out/reset.txt"
adb shell "for i in 1 2 3 4 5 6; do input swipe $x $top $x $bottom 600; input swipe $x $bottom $x $top 600; done"
adb shell dumpsys gfxinfo "$package" framestats > "$out/gfx.txt"
adb shell dumpsys meminfo "$package" > "$out/memory.txt"
adb exec-out screencap -p > "$out/screen.png"
sed -n '1,20p' "$out/gfx.txt"
