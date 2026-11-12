# Terminal scrolling performance — 2026-11-12

Measured on the connected Samsung SM-N975U1 (Note10+), Android 12,
1440×3040 at 60 Hz. This is a terminal-rendering investigation, not a complete
APK performance or memory-leak audit.

## Findings and changes

- A real, running Codex task on installed build 56 missed 31 of 239 frame
  deadlines (12.97%); 30 slow-UI events, GPU P95 2 ms. A separate mixed-language
  output workload reached 84/238 (35.29%). These exploratory samples are not the
  controlled A/B comparison below.
- The terminal regenerated a whole-screen canvas after a scroll. Build 57 uses
  keyed row canvases, retaining native drawing lists for unchanged rows as they
  move. Duplicate rows have distinct occurrence keys. Cursor, palette, size and
  font changes still invalidate the relevant drawings.
- Drag and fling callbacks now publish their snapshot immediately, instead of
  adding the change watcher's next-frame wait to an already frame-driven action.
  Background output remains coalesced; the limiter checks interaction each frame
  rather than sleeping and then waiting for another display frame.
- ASCII run construction no longer allocates a temporary String for every cell.
  Wide characters and combining sequences retain their previous cell placement.

## Controlled device comparison

Both variants used the same temporary package (`dev.yonder.tst`), test data,
USB-forwarded isolated SSH/core session, 55×45 terminal, default font, and
`e2e/perf-output.py`: 1,500 seed lines followed by 20 mixed Chinese/ASCII,
ANSI-colored lines per second. Each sample used `e2e/perf.sh`: six down/up pairs,
600 ms per swipe, roughly 8.2 seconds including injection overhead. Keyboard
was hidden. Battery temperature remained about 25–26°C.

| Metric | Build 56, two runs | Build 57, two runs |
|---|---:|---:|
| Rendered frames | 231 / 231 | 341 / 336 |
| Missed frame deadlines | 28 / 49 | 26 / 19 |
| Missed deadline rate | 12.12% / 21.21% | 7.62% / 5.65% |
| Frame P50 | 13 / 14 ms | 9 / 10 ms |
| Frame P95 | 19 / 20 ms | 18 / 16 ms |
| Frame P99 | 24 / 22 ms | 25 / 23 ms |
| High-input-latency counter | 8 / 4 | 48 / 42 |

Pooled missed-deadline rate: **77/462 = 16.67% → 45/677 = 6.65%**,
about 60% lower. Rendered frames in the same test window increased about 47%;
this is not a claim of constant 60 FPS. P99 did not improve.

An additional reversed-order comparison, with HWUI profiling enabled on both
variants, reproduced the direction: optimized 24/336 (7.14%), old 31/246
(12.60%). That pair is kept separate from the table above.

The first row-cache-only candidate improved P50 to 11 ms but did not reduce the
pooled missed-deadline rate (77/456); the immediate drag/fling update was needed
for the measured improvement. Its raw results are retained as `ab-after-*`.

### Limitations and tradeoffs

The Android high-input-latency counter increased, including in the reversed
comparison (74 vs 3). In Android 12 the corresponding HWUI code also increments
this counter for triple-buffered frames that meet their deadline; it is not a
direct touch-to-photon duration. See the [Android 12 JankTracker implementation](https://android.googlesource.com/platform/frameworks/base/+/android-12.0.0_r1/libs/hwui/JankTracker.cpp).
This run demonstrates smoother rendering, **not improved end-to-end touch
latency**. Buffering remains a follow-up measurement area; this finding is not
hidden by the lower jank rate.

Controlled PSS samples were about 102 MiB before, 104–121 MiB after; a later
optimized sample was 118 MiB. Row reuse trades some memory for rendering work.
These short samples do not establish absence of memory leaks. Terminal scrolling
still advances in terminal lines, not continuous pixel scrolling. Remote TUIs
that handle their own wheel events also depend on remote processing/network time.

## Validation and artifacts

- Core workspace: 119 tests passed. App: 18 unit + 7 integration tests passed.
- After the final scheduling change: terminal interaction integration test and
  app all-target Clippy passed again. Workspace Clippy and diff checks passed.
- Device: mixed Chinese/ASCII text, ANSI color, history scrolling, keyboard
  resize, repeated/blank rows and cursor visually checked. The new Enter key
  executed `touch phone-enter-tested` in the isolated test shell; file existence
  confirmed execution. No fatal exception/panic found in captured final-process
  logcat. This is bounded validation, not an exhaustive UI acceptance run.
- Raw dumps, screenshots and APK backups are in the ignored local directory
  `recovery/performance-2026-11-12/`. `ab-final-*` is the final optimized version;
  `ab-before-*` is baseline. `cross-*` is the extra reversed-order pair.
- Release signing could not read the macOS Keychain item (exit 36). Test packages
  changed only the application ID and signing identity of the corresponding
  Release APK; native library hashes were verified/preserved. The optimized APK
  is debug-signed, not a distributable release-signed update.
- The original `dev.yonder.app` build 56 and its data were not overwritten.
  Temporary test app/service, USB forwarding, input method and HWUI profiling
  settings are cleaned up/restored after testing. Build 57 needs the release
  signing key before an in-place update of the original app.

To repeat, open a disposable terminal running `python3 e2e/perf-output.py`, then
run `e2e/perf.sh /tmp/yonder-sample dev.yonder.app`. Do not run the output fixture
inside an agent's input prompt or use the destructive full-device acceptance
script against an existing user profile.
