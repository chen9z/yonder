#!/bin/sh
# Yonder.app for this Mac, ad hoc signed (no Developer ID, not notarized).
# The last line printed is the app's absolute path.
set -eu
# Panic messages name their source files: this machine's paths stay out.
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=$HOME=~"
cd "$(dirname "$0")"
. ../env.sh
build=${HERDC_BUILD:-$(cat build-number)}
case "$build" in ''|*[!0-9]*|0*) echo 'Build must be a positive decimal integer' >&2; exit 2 ;; esac
if [ "${#build}" -gt 10 ] || [ "$build" -lt 50 ] || [ "$build" -gt 2100000000 ]; then
  echo 'Build must be 50..2100000000' >&2
  exit 2
fi
HERDC_BUILD=$build
HERDC_COMMIT=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)
export HERDC_BUILD HERDC_COMMIT
out="$PWD/.nsh/macos"
nsh package --target macos --build "$build" -o "$out" >&2
app="$out/Yonder.app"
bin="$app/Contents/MacOS/Yonder"
# The asset table still names each asset's source file by its absolute
# path (nsh's asset! reads CARGO_MANIFEST_DIR, which the remapping leaves
# alone). The app reads its assets from its Resources, never from there:
# the home folder in them is written over at the same length, then the
# app is signed again.
python3 -I - "$bin" "$HOME" <<'EOF'
import sys
path, home = sys.argv[1], sys.argv[2].encode()
data = open(path, 'rb').read()
open(path, 'wb').write(data.replace(home + b'/', b'~' + b'/' * len(home)))
EOF
if grep -rlaF "$HOME/" "$app" >&2; then
  echo "This machine's home folder is still in the app" >&2
  exit 1
fi
codesign --force --sign - "$app" >&2
codesign --verify --deep --strict "$app" >&2
echo "$app"
