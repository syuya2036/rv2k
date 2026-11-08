#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p docs/logs
cargo build --release --locked
BIN="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin"
ELF=target/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test
"$BIN/llvm-readobj" --file-headers --arch-specific --sections "$ELF" > docs/logs/elf-layout.txt
"$BIN/llvm-size" "$ELF" | tee docs/logs/size.txt
"$BIN/llvm-objdump" -d "$ELF" > docs/logs/disassembly.txt
# Materialize PT_LOAD LMA as extra Flash segments for Renvo.
python3 scripts/elf-for-remu.py "$ELF" "$ELF.remu"
