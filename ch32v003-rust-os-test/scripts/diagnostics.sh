#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p docs/logs
# Keep the baseline and timer binaries independent of this experiment.
RUSTFLAGS='-C link-arg=--defsym=_kernel_stack_size=256 -C link-arg=-Tmemory.x' cargo build --locked --release --features diagnostics,emulator-stop --target-dir target/diagnostics
ELF=target/diagnostics/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test
BIN="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin"
"$BIN/llvm-nm" -S "$ELF" > docs/logs/diagnostics-symbols.txt
"$BIN/llvm-size" "$ELF" | tee docs/logs/diagnostics-size.txt
"$BIN/llvm-readobj" --file-headers --arch-specific --sections "$ELF" > docs/logs/diagnostics-layout.txt
"$BIN/llvm-objdump" -d "$ELF" > docs/logs/diagnostics-disassembly.txt
python3 scripts/elf-for-remu.py "$ELF" "$ELF.remu"
vendor/renvo_emu/target/release/remu run --target ch32v003 --elf "$ELF.remu" --max-instructions 3000000 --result docs/logs/diagnostics-run.json --bus-log docs/logs/diagnostics-bus.json --bus-log-region sram --coverage docs/logs/diagnostics-coverage.json
python3 scripts/check-diagnostics.py
python3 scripts/check-budget.py "$ELF" docs/logs/diagnostics-budget.json
