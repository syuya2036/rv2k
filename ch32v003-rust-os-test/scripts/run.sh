#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
REMU=vendor/renvo_emu/target/release/remu
ELF=target/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test.remu
"$REMU" run --target ch32v003 --elf "$ELF" --max-instructions 10000 --result docs/logs/run.json --bus-log docs/logs/bus.json --bus-log-region sram --interrupt-log docs/logs/interrupts.json
python3 scripts/check.py
