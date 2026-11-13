#!/bin/sh
# Tiny adb UI helpers for runs on a real phone.
#   adbui.sh tap X Y | hold X Y | swipe X1 Y1 X2 Y2 | press TEXT|ID | type TEXT | key KEYCODE | shot NAME | dump | nodes | find TEXT|ID
# Screenshots and the UI dump go to $HERDC_SHOTS (default /tmp/herdc-shots).
set -e
shots=${HERDC_SHOTS:-/tmp/herdc-shots}
mkdir -p "$shots"
cmd=$1; shift
case "$cmd" in
  tap) adb shell input tap "$1" "$2" ;;
  # A finger held still for most of a second: a long press.
  hold) adb shell input swipe "$1" "$2" "$1" "$2" 900 ;;
  swipe) adb shell input swipe "$1" "$2" "$3" "$4" 300 ;;
  # Tap what shows TEXT or ID, or fail: never a blind tap.
  press) at=$("$0" find "$1"); [ -n "$at" ] || { echo "no $1 on screen" >&2; exit 1; }; adb shell input tap $at ;;
  # Quoted for the device's shell; spaces as input's %s.
  type) adb shell "input text '$(printf %s "$1" | sed "s/'/'\\\\''/g; s/ /%s/g")'" ;;
  key) adb shell input keyevent "$@" ;;
  shot) adb exec-out screencap -p > "$shots/$1.png"; sips -Z 1000 "$shots/$1.png" >/dev/null; echo "$shots/$1.png" ;;
  dump|find|nodes)
    adb shell uiautomator dump /sdcard/ui.xml >/dev/null; adb pull -q /sdcard/ui.xml "$shots/ui.xml"
    python3 -I - "$cmd" "$1" "$shots/ui.xml" <<'PY'
import re,sys
s=open(sys.argv[3]).read()
for m in re.finditer(r'<node [^>]*>', s):
    n=m.group(0); g=lambda k:(re.search(k+r'="([^"]*)"',n) or [None,''])[1]
    rid,t,d,b=g('resource-id'),g('text'),g('content-desc'),g('bounds')
    if sys.argv[1]=='find':
        if sys.argv[2] in (rid,t,d):
            x1,y1,x2,y2=map(int,re.findall(r'\d+',b)); print((x1+x2)//2,(y1+y2)//2); break
    elif sys.argv[1]=='nodes':
        # In order, text and label: what a script reads the screen by.
        if t or d: print(t+'\t'+d)
    elif rid or t or d: print(repr(rid),repr(t),repr(d),b)
PY
    ;;
  *) echo "usage: $0 tap|hold|swipe|press|type|key|shot|dump|nodes|find ..." >&2; exit 2 ;;
esac
