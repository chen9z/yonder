#!/bin/sh
# Yonder's APK, for 64-bit phones (arm64-v8a: every phone of the last years),
# signed with the release key when there is one: the keystore in
# ~/.config/herdc/release.jks, its password in the macOS keychain
# (herdc-release-keystore). Without them, the debug key signs.
# Writes .nsh/out/Yonder.apk; `--install` also puts it on the adb phone.
# Built with fat LTO in one codegen unit: a sixth smaller, a few minutes longer.
# Its build number is the commits so far, so each release is above the last.
set -e
export CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
cd "$(dirname "$0")"
. ../env.sh
install=false
[ "$1" = --install ] && install=true
build=$(git rev-list --count HEAD)
# Said in Settings, About.
export HERDC_BUILD=$build HERDC_COMMIT=$(git rev-parse --short HEAD)
keystore="$HOME/.config/herdc/release.jks"
if [ -f "$keystore" ] && NSH_KEYSTORE_PASSWORD=$(security find-generic-password -s herdc-release-keystore -w 2>/dev/null); then
  export NSH_KEYSTORE_PASSWORD
  nsh package --target android --abi arm64-v8a --build "$build" --keystore "$keystore" --key-alias herdr
else
  nsh package --target android --abi arm64-v8a --build "$build"
fi
if $install; then
  adb install -r .nsh/out/Yonder.apk
fi
