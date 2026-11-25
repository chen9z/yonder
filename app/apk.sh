#!/bin/sh
# Explicit release signing; use --debug for a debug-signed package.
# Increment build-number for each distributed release, independently of Git.
set -eu
export CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
# Panic messages name their source files: this machine's paths stay out.
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--remap-path-prefix=$HOME=~"
cd "$(dirname "$0")"
. ../env.sh
install=false
debug=false
check=false
for arg in "$@"; do
  case "$arg" in
    --install) install=true ;;
    --debug) debug=true ;;
    --check) check=true ;;
    *) echo "Usage: $0 [--debug] [--install] [--check]" >&2; exit 2 ;;
  esac
done
build=${HERDC_BUILD:-$(cat build-number)}
case "$build" in ''|*[!0-9]*|0*) echo 'Build must be a positive decimal integer' >&2; exit 2 ;; esac
if [ "${#build}" -gt 10 ] || [ "$build" -lt 50 ] || [ "$build" -gt 2100000000 ]; then
  echo 'Build must be 50..2100000000 (recovered installed build was 49)' >&2
  exit 2
fi
HERDC_BUILD=$build
HERDC_COMMIT=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)
export HERDC_BUILD HERDC_COMMIT
if $check; then
  echo "Build $build ($HERDC_COMMIT), debug=$debug"
  exit 0
fi
if $debug; then
  nsh package --target android --abi arm64-v8a --build "$build"
else
  keystore=${HERDC_KEYSTORE:-"$HOME/.config/herdc/release.jks"}
  [ -f "$keystore" ] || { echo 'Release keystore is missing; use --debug only for a debug package' >&2; exit 1; }
  if ! NSH_KEYSTORE_PASSWORD=$(security find-generic-password -s herdc-release-keystore -w 2>/dev/null); then
    echo 'Cannot unlock release signing key; no APK was built' >&2
    exit 1
  fi
  export NSH_KEYSTORE_PASSWORD
  nsh package --target android --abi arm64-v8a --build "$build" --keystore "$keystore" --key-alias herdr
fi
if $install; then
  adb install -r .nsh/out/Yonder.apk
fi
