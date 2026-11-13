# Walkthrough: a phone already connected to a computer

The Yonder app (`dev.yonder.app`) is open on the phone and connected to a test
computer. Its repo `proj` has new files `blob.bin` (binary), `big.txt`
(20000 lines) and `long.txt` (one 6000-character line); its ssh config
lists a host `vps` (`can@10.0.0.7`).

Drive the phone with `./adbui.sh` only:

- `./adbui.sh shot NAME`: screenshot to `/tmp/herdc-shots/NAME.png`; look at it.
- `./adbui.sh dump`: every labelled view, `id text label bounds`.
- `./adbui.sh press LABEL`: taps the view whose text, label or id is LABEL.
- `./adbui.sh hold X Y`: a long press; `./adbui.sh swipe X1 Y1 X2 Y2`.
- `./adbui.sh tap X Y`, `./adbui.sh type TEXT`, `./adbui.sh key KEYCODE` (`KEYCODE_BACK`).

Taps are in the phone's pixels (1440×3040, as `dump`'s bounds); screenshots
are scaled to 1000 high, so a point on one is ×3.04 on the phone. Prefer
`press`. `type` sends key events: the phone's keyboard must be one that
passes them on as typed (Samsung Keyboard; a pinyin IME composes them).

Don't leave the app, change phone settings, close panes or spaces (but the
tab step 3b opens), press "Add" for a machine, or press "Leave", "Forget"
or anything in red.

The app's menus are its own sheets from the bottom (not the system's): a
row each, an icon before its words, what loses something in red, the
current choice checked. `press` finds a row by its words. Look after every step; if something isn't as said, note
it with the screenshot's name and go on.

1. The workspace shows a terminal with a shell prompt under a header: `‹`,
   a small colored square with the space's first letter, the space's name,
   `1 · ` and the computer under it, then `+` and `⋯` (line icons, not
   characters). No keyboard yet, and no tab strip (the space has one tab).
2. Tap the terminal (`./adbui.sh tap 720 1400`); the keyboard comes up
   typing English letters (not composing Chinese), with a key bar above it
   (Esc, Tab, Ctrl, arrows). The keyboard's own layout is not the app's: a
   3×4 keypad or a 中/En key on it is fine. Type `cd ~/proj && ls` then
   `./adbui.sh key KEYCODE_ENTER`. The listing shows the three files. The
   key bar has Enter (↵) at its left end and Hide keyboard (⌄) at its
   right end, which stay put when the keys between are swiped along.
3. With the keyboard still up, hold the terminal: `./adbui.sh hold 720 1400`.
   The keyboard goes, so the sheet is all in sight.
   A sheet titled "Terminal" offers "Select text" and "Paste", each with
   an icon; press "Select text". A
   "Select text" page shows the terminal's text; press "Copy all"; a
   "Copied" toast shows and the page closes.
3a. Type `echo see https://example.com/pull/7.` and Enter, then hold the
   terminal again: the sheet's first choice is "Open example.com/pull/7"
   (no full stop). Don't press it (it leaves the app): Back.
3c. Type `echo look at ./big.txt:20` and Enter, then hold the terminal:
   the sheet offers "Open big.txt:20" with a file icon; press it. A page
   "big.txt" shows the file with line 20 (`line 20`) marked, near the
   middle of the screen. Back returns to the terminal.
3b. `+` (New): a sheet "New" offers "Terminal" (a `>_` icon) and the
   agents installed (their own icons), then "New space" and the
   worktrees. Press "Terminal": a second tab opens, and a strip of two
   tabs shows under the header (no line between the header and the
   strip; a thin one under the strip, over the terminal), the new one
   selected; the header's second line now says only the computer; a note
   "2/2 · …" shows a moment over the terminal. Tap the first tab on the
   strip: it is selected. Hold the second tab: a sheet "Tab" offers
   Rename, Put away and Close (red); press "Close" (only that one: it was
   opened here). The strip goes, and the header's second line starts
   `1 · ` again.
4. More (`⋯`): a sheet of icons (Split right, Split down, Zoom, Rename tab,
   Files, Changes, Worktrees, Put away tab, Commands, Settings) and "Close
   pane" in red. Changes: lists `big.txt`, `blob.bin`, `long.txt` as new.
   Open each: `big.txt` starts `@@ -0,0 +1,20000 @@` and its lines are
   numbered; `blob.bin` says "Binary file · no text diff"; `long.txt` wraps
   its one line, which ends "… (line truncated)". The diff's header says
   "1 of 3" (and so on) with ‹ and ›: › goes to the next file. Back
   returns to the list each time.
5. `‹` (Spaces and agents) → the home page; its `⋯` (Menu): a sheet with
   Commands, Add a machine…, Edit connection, Settings and Leave (red).
   Press "Add a machine…": "FROM YOUR SSH CONFIG" lists `vps` with `can@10.0.0.7` and
   "Add"; a field `user@host` below. Back, then tap the space to return.
6. Settings (in More): its rows sit on rounded cards under small headings
   (TERMINAL, COMPUTER, THIS PHONE'S KEY, ABOUT), each row with a line
   icon. The computer's row has a green dot. "Fit terminals to the phone"
   shows a switch, on. The font size − and + change the size shown; the
   terminal's text follows when back. Put it back as it was. "Copy" under
   the key shows a note "Copied" saying where the key goes.
6a. Settings → Key bar: at the bottom, under YOUR OWN, a field ("Words it
   types, like /compact"), a + and "Then press Enter", on. Type
   `echo own-key` in the field and press + (once, with the keyboard
   still up): "echo own-key" and a small Enter icon join the keys on the
   bar at the list's end. Back to the terminal, tap it: the key bar ends
   with "echo own-key ↵" (swipe the bar along). Press it: the terminal
   runs it and prints `own-key`. Then Settings → Key bar again and take
   it off (its ✕): the Key bar row in Settings says 17 keys again.
7. Tap the header's title: a sheet "Spaces" lists the space with its
   colored square, checked (a ✓ at the row's end), and "All spaces and
   agents"; press that. On the home page, "Agents" (at the
   bottom) lists the agents, or with none says "No agent status yet" and
   why. Tap the space under "Spaces"
   to go back.

End with a table: step, as said (yes/no), what differed, screenshot.
