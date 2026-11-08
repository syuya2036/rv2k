#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p docs/logs
# Keep the baseline and timer binaries independent of this experiment.
RUSTFLAGS='-C link-arg=--defsym=_kernel_stack_size=256 -C link-arg=-Tmemory.x' cargo build --locked --release --features drivers,emulator-stop --target-dir target/drivers
ELF=target/drivers/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test
BIN="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin"
"$BIN/llvm-nm" -S "$ELF" > docs/logs/drivers-symbols.txt
"$BIN/llvm-size" "$ELF" | tee docs/logs/drivers-size.txt
"$BIN/llvm-readobj" --file-headers --arch-specific --sections "$ELF" > docs/logs/drivers-layout.txt
"$BIN/llvm-objdump" -d "$ELF" > docs/logs/drivers-disassembly.txt
python3 scripts/elf-for-remu.py "$ELF" "$ELF.remu"
vendor/renvo_emu/target/release/remu run --target ch32v003 --elf "$ELF.remu" --max-instructions 3000000 --result docs/logs/drivers-run.json --bus-log docs/logs/drivers-bus.json --bus-log-region sram --bus-log-region ch32v003.gpioa --bus-log-region ch32v003.gpioc --bus-log-region ch32v003.gpiod --bus-log-region ch32v003.afio --coverage docs/logs/drivers-coverage.json
python3 scripts/check-drivers.py
python3 scripts/check-budget.py "$ELF" docs/logs/drivers-budget.json
