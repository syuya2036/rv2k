#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
REV=6b0c7221ca32e83903797d2c86a8effdcd62550a
mkdir -p vendor docs/logs
if [ ! -d vendor/renvo_emu/.git ]; then
  git clone https://github.com/tinyrange/renvo_emu.git vendor/renvo_emu
  git -C vendor/renvo_emu checkout "$REV"
fi
[ "$(git -C vendor/renvo_emu rev-parse HEAD)" = "$REV" ] || { echo 'Unexpected Renvo revision' >&2; exit 1; }
# Cargo discovers config from its working directory, even with --manifest-path.
# Build outside this firmware's target/build-std configuration.
SCRATCH=$(mktemp -d)
trap 'rmdir "$SCRATCH"' EXIT
(cd "$SCRATCH"; cargo +stable build --locked --release -p remu-cli --manifest-path "$ROOT/vendor/renvo_emu/Cargo.toml" --target-dir "$ROOT/vendor/renvo_emu/target")
file vendor/renvo_emu/target/release/remu
vendor/renvo_emu/target/release/remu targets > docs/logs/targets.txt
