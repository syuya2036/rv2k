#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p docs/logs
# Keep the baseline and timer binaries independent of this experiment.
RUSTFLAGS='-C link-arg=--defsym=_kernel_stack_size=256 -C link-arg=-Tmemory.x' cargo build --locked --release --features services --target-dir target/services
ELF=target/services/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test
BIN="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin"
"$BIN/llvm-nm" -S "$ELF" > docs/logs/services-symbols.txt
"$BIN/llvm-size" "$ELF" | tee docs/logs/services-size.txt
"$BIN/llvm-readobj" --file-headers --arch-specific --sections "$ELF" > docs/logs/services-layout.txt
"$BIN/llvm-objdump" -d "$ELF" > docs/logs/services-disassembly.txt
python3 scripts/elf-for-remu.py "$ELF" "$ELF.remu"
vendor/renvo_emu/target/release/remu run --target ch32v003 --elf "$ELF.remu" --max-instructions 3000000 --result docs/logs/services-run.json --bus-log docs/logs/services-bus.json --bus-log-region sram --coverage docs/logs/services-coverage.json
python3 scripts/check-services.py
