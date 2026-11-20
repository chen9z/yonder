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
   a small colored square with the space's first letter, the space's name
   and `· ` the computer after it on one line, then `+` and `⋯` (line
   icons, not characters). No keyboard yet, and no dots under the name
   (the space has one tab).
2. Tap the terminal (`./adbui.sh tap 720 1400`); the keyboard comes up
   typing English letters (not composing Chinese), with a key bar above it
   (Esc, Tab, Ctrl, arrows). The keyboard's own layout is not the app's: a
   3×4 keypad or a 中/En key on it is fine. Type `cd ~/proj && ls` then
   `./adbui.sh key KEYCODE_ENTER`. The listing shows the three files. The
   key bar has Enter (↵) at its left end and Hide keyboard (⌄) at its
   right end, which stay put when the keys between are swiped along.
2a. With the keyboard up, press ✎ (Write a message) before ⌄ on the key
   bar: a sheet with a field of several lines and "Paste". Type
   `echo composed` and press Send: the sheet and the keyboard go, and the
   terminal runs it (prints `composed`). Tap the terminal again.
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
   worktrees. Press "Terminal": a second tab opens, and two dots show
   under the space's name (no strip of tabs), the second a short bar (the
   one shown); a note "2/2 · …" shows a moment over the terminal. Tap the
   first dot (`./adbui.sh press` its tab's name, as `dump` says it): the
   bar moves to it and "1/2 · …" shows. Swipe the terminal right to left
   (`./adbui.sh swipe 1200 1400 200 1400`): the second again. Hold the
   second dot: a sheet "Tab" offers Rename, Put away and Close (red);
   press "Close" (only that one: it was opened here). The dots go.
4. More (`⋯`): a sheet of icons (Split right, Split down, Zoom, Rename tab,
   Files, Changes, Worktrees, Put away tab, Settings) and "Close
   pane" in red. Changes: lists `big.txt`, `blob.bin`, `long.txt` as new.
   Open each: `big.txt` starts `@@ -0,0 +1,20000 @@` and its lines are
   numbered; `blob.bin` says "Binary file · no text diff"; `long.txt` wraps
   its one line, which ends "… (line truncated)". The diff's header says
   "1 of 3" (and so on) with ‹ and ›: › goes to the next file. Back
   returns to the list each time.
5. `‹` (Spaces and agents) → the home page: at its top right a search
   (🔍) and Settings (a gear), and `+` on Spaces; no `⋯`. Press Settings,
   then "Add a machine…": "FROM YOUR SSH CONFIG" lists `vps` with `can@10.0.0.7` and
   "Add"; a field `user@host` below. Back, then tap the space to return.
5a. Home's 🔍 (Jump to a terminal): a field over every terminal, each
   row the space's name in bold, what it does after it, its tab under.
   Type `zzz`: "No terminal is called that". Clear it, type the space's
   name: its terminal stays; tap it: the workspace, on that terminal.
6. Settings (in More): its rows sit on rounded cards under small headings
   (COMPUTER, TERMINAL, GENERAL, THIS PHONE'S KEY, ABOUT), each row with a
   line icon, and Leave this computer (red) alone on the last card.
   COMPUTER has the computer (a green dot, "herdr-core" and its version),
   Edit connection and Add a machine…; GENERAL has Main agent
   (None), Language, Appearance (System), Notes (Important only) and Sort
   agents (Priority).
   Press Appearance: System, Light, Dark; press Light: the whole app,
   terminals too, turns light at once; back to System. "Fit terminals to the phone" shows a switch,
   on, and small words under the card say what it does. The font size −
   and + change the size shown; the terminal's text follows when back.
   Put it back as it was. The key is one row, "Copy the public key" over
   `ssh-ed25519 …` and its last letters; pressed, a note "Copied" says
   where the key goes.
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
   why. An agent on this computer is one line (no machine under it), an
   idle one has no status mark, and no row has a pin icon. If one is
   listed: drag its row right by half a row's width and back before
   letting go (`input motionevent` DOWN, MOVEs, UP): a "Pin" in the
   accent shows under it, and let go it slides back, nothing pinned. Drag
   it left by half a row's width and let go: it stays open on two
   buttons, Pin (accent) and Stop (red), nothing asked; scroll the list a
   little: it closes. Don't press Stop. Hold
   it: a sheet titled as its row reads (not a title with `| space`);
   Back. Tap the space under "Spaces"
   to go back.
7a. ‹ to the home page: three tabs at the bottom, "Main" first. Press
   it: "Your main agent", why, and "Choose an agent"; the tabs at the
   bottom are gone, a ‹ at the top left instead. The system's back: on
   "Agents" (the app stays). Press "Main" again, then ‹: back on "Agents",
   hold an agent's row: its sheet offers "Make it the main agent"; press
   it. The first tab shows, called as the agent's space: the space's
   square, its name and program at the top, the agent's terminal, and
   under it a field "Message <name>" with an ↑. Settings → GENERAL → Main
   agent → "No main agent": the first tab says "Your main agent" again.
7b. If a Pi agent is the main one (don't send it anything): its tab shows
   a conversation, not a terminal: the person's words in grey bubbles on
   the right, its replies set as Markdown, tools between them as one line
   ("N steps · bash, read"); press one: it opens on each step; press a
   step: a page with what it ran and gave back. Back. At the top right a
   terminal icon shows the terminal, and then a bubble icon the
   conversation again. Left of the field an image icon; an ↑ on the
   right (a ■ instead while it works and nothing is written: don't press
   it). Scroll up a little: a ↓ shows at the bottom right; press it: back
   at the end. If "Show earlier messages" is at the top, press it: older
   lines show above and the screen stays where it was. Times show between
   lines far apart. No tabs at the bottom; the ‹ at the top left carries
   the count of agents waiting. Drag the lines up (reading on): the header
   and the agents above go, the status bar stays; drag back a little, or
   tap just under the status bar: they come back. Turn the phone sideways
   on "Agents": the tabs at the bottom are lower, without words; turn it
   back.
7c. Still in that conversation (send nothing): the field says "Message
   <name> · / for commands". Type `/`: a list shows above the field,
   /compact first, each with a few words; some with a terminal glyph.
   Type `na`: only /name. Press it: the field says "/name " and the list
   is gone. Clear the field. Hold one of your messages: "Edit", "Copy",
   "Write it again". Press "Edit": its words in the field, above it
   "Editing your message" and an ✕; press the ✕: what was written before
   is back. Only on a throwaway Pi: edit a message and send it; under it
   "Going back to before it…", then the conversation goes on from that
   message, the later ones gone; `/name x` sent says "Named “x”".

End with a table: step, as said (yes/no), what differed, screenshot.
