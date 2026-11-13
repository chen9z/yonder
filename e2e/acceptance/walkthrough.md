# Walkthrough: a phone already connected to a computer

The Yonder app (`dev.yonder.app`) is open on the phone and connected to a test
computer. Its repo `proj` has new files `blob.bin` (binary), `big.txt`
(20000 lines) and `long.txt` (one 6000-character line); its ssh config
lists a host `vps` (`can@10.0.0.7`).

Drive the phone with `./adbui.sh` only:

- `./adbui.sh shot NAME`: screenshot to `/tmp/herdc-shots/NAME.png`; look at it.
- `./adbui.sh dump`: every labelled view, `id text label bounds`.
- `./adbui.sh press LABEL`: taps the view whose text, label or id is LABEL.
- `./adbui.sh tap X Y`, `./adbui.sh type TEXT`, `./adbui.sh key KEYCODE` (`KEYCODE_BACK`).

Taps are in the phone's pixels (1440×3040, as `dump`'s bounds); screenshots
are scaled to 1000 high, so a point on one is ×3.04 on the phone. Prefer
`press`. `type` sends key events: the phone's keyboard must be one that
passes them on as typed (Samsung Keyboard; a pinyin IME composes them).

Don't leave the app, change phone settings, close panes or spaces (but the
tab step 3b opens), or press
"Add" for a machine. Look after every step; if something isn't as said, note
it with the screenshot's name and go on.

1. The workspace shows a terminal with a shell prompt under a header (the
   space's name, `1 · ` and the computer under it, `‹`, `+`, `⋯`); no
   keyboard yet, and no tab strip (the space has one tab).
2. Tap the terminal (`./adbui.sh tap 720 1400`); the keyboard comes up
   typing English letters (not composing Chinese), with a key bar above it
   (Esc, Tab, Ctrl, arrows). The keyboard's own layout is not the app's: a
   3×4 keypad or a 中/En key on it is fine. Type `cd ~/proj && ls` then
   `./adbui.sh key KEYCODE_ENTER`. The listing shows the three files. The
   key bar has Enter (↵) at its left end and Hide keyboard (⌄) at its
   right end, which stay put when the keys between are swiped along.
3. Hold a finger on the terminal: `adb shell input swipe 720 1400 720 1400 900`.
   A sheet offers "Select text" and "Paste"; press "Select text". A
   "Select text" page shows the terminal's text; press "Copy all"; a
   "Copied" toast shows and the page closes.
3a. Type `echo see https://example.com/pull/7.` and Enter, then hold the
   terminal again: the sheet's first choice is "Open example.com/pull/7"
   (no full stop). Don't press it (it leaves the app): Back.
3b. `+` → "Terminal": a second tab opens, and a strip of two tabs shows
   under the header, the new one selected; a note "2/2 · …" shows a moment
   over the terminal. Tap the first tab on the strip: the header's second
   line starts `1 · ` again. Hold the second tab → "Close" (only that one:
   it was opened here).
4. More (`⋯`): a sheet of icons (Split right, Split down, Zoom, Rename tab,
   Files, Changes, Worktrees, Put away tab, Commands, Settings) and "Close
   pane" in red. Changes: lists `big.txt`, `blob.bin`, `long.txt` as new.
   Open each: `big.txt` starts `@@ -0,0 +1,20000 @@` and its lines are
   numbered; `blob.bin` says "Binary file · no text diff"; `long.txt` wraps
   its one line, which ends "… (line truncated)". The diff's header says
   "1 of 3" (and so on) with ‹ and ›: › goes to the next file. Back
   returns to the list each time.
5. `‹` (Spaces and agents) → the home page; its `⋯` (Menu) → "Add a
   machine…": "FROM YOUR SSH CONFIG" lists `vps` with `can@10.0.0.7` and
   "Add"; a field `user@host` below. Back, then tap the space to return.
6. Settings (in More): the font size − and + change the size shown; the
   terminal's text follows when back. Put it back as it was.
7. Tap the header's title: a sheet "Spaces" lists the space, checked, and
   "All spaces and agents"; press that. On the home page, "Agents" (at the
   bottom) says no agents run or lists them. Tap the space under "Spaces"
   to go back.

End with a table: step, as said (yes/no), what differed, screenshot.
