#!/bin/sh
# Everything that runs without a phone: clippy and the tests of the crates
# and of the app (headless, against isolated cores). `--times N` runs the
# tests N times, to catch the flaky.
set -e
cd "$(dirname "$0")/.."
. ./env.sh
python3 scripts/test-apk.py
times=1
[ "${1:-}" = --times ] && times=$2
cargo clippy -q --all-targets -- -D warnings
(cd app && cargo clippy -q --all-targets -- -D warnings)
i=0
while [ $i -lt "$times" ]; do
  i=$((i + 1))
  echo "tests, run $i of $times"
  cargo test -q
  (cd app && cargo test -q)
done
echo "all good"
