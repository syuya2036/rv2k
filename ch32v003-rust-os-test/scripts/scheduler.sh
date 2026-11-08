#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p docs/logs
# Keep the baseline and timer binaries independent of this experiment.
cargo build --locked --release --features scheduler --target-dir target/scheduler
ELF=target/scheduler/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test
BIN="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin"
"$BIN/llvm-nm" -S "$ELF" > docs/logs/scheduler-symbols.txt
"$BIN/llvm-size" "$ELF" | tee docs/logs/scheduler-size.txt
"$BIN/llvm-readobj" --file-headers --arch-specific --sections "$ELF" > docs/logs/scheduler-layout.txt
"$BIN/llvm-objdump" -d "$ELF" > docs/logs/scheduler-disassembly.txt
python3 scripts/elf-for-remu.py "$ELF" "$ELF.remu"
vendor/renvo_emu/target/release/remu run --target ch32v003 --elf "$ELF.remu" --max-instructions 100000 --result docs/logs/scheduler-run.json --bus-log docs/logs/scheduler-bus.json --bus-log-region sram --coverage docs/logs/scheduler-coverage.json
python3 scripts/check-scheduler.py
