#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p docs/logs
# Keep the baseline and timer binaries independent of this experiment.
cargo build --locked --release --features coop --target-dir target/coop
ELF=target/coop/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test
BIN="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin"
"$BIN/llvm-nm" -S "$ELF" > docs/logs/coop-symbols.txt
"$BIN/llvm-size" "$ELF" | tee docs/logs/coop-size.txt
"$BIN/llvm-readobj" --file-headers --arch-specific --sections "$ELF" > docs/logs/coop-layout.txt
"$BIN/llvm-objdump" -d "$ELF" > docs/logs/coop-disassembly.txt
python3 scripts/elf-for-remu.py "$ELF" "$ELF.remu"
vendor/renvo_emu/target/release/remu run --target ch32v003 --elf "$ELF.remu" --max-instructions 100000 --result docs/logs/coop-run.json --bus-log docs/logs/coop-bus.json --bus-log-region sram --coverage docs/logs/coop-coverage.json
python3 scripts/check-coop.py
