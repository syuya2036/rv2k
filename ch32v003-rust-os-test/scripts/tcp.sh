#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
mkdir -p docs/logs
(cd ..; cargo +stable build --locked --release --manifest-path host/net-harness/Cargo.toml --target-dir "$ROOT/vendor/renvo_emu/target")
export RUSTFLAGS='-C opt-level=z -C target-feature=+relax -C link-arg=--defsym=_kernel_stack_size=256 -C link-arg=-Tmemory.x -C link-arg=--icf=all -C link-arg=--relax'
BIN="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin"
cargo build --locked --release --features compact-probe,emulator-stop --target-dir target/compact
COMPACT=target/compact/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test
"$BIN/llvm-nm" -S "$COMPACT" > docs/logs/compact-symbols.txt
python3 scripts/elf-for-remu.py "$COMPACT" "$COMPACT.remu"
vendor/renvo_emu/target/release/remu run --target ch32v003 --elf "$COMPACT.remu" --max-instructions 3000000 --result docs/logs/compact-run.json --bus-log docs/logs/compact-bus.json --bus-log-region sram
python3 scripts/check-diagnostics.py compact
python3 scripts/check-budget.py "$COMPACT" docs/logs/compact-budget.json
cargo build --locked --release --features tcp,emulator-stop --target-dir target/tcp
ELF=target/tcp/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test
python3 scripts/elf-for-remu.py "$ELF" "$ELF.remu"
vendor/renvo_emu/target/release/rv2k-net-harness "$ELF.remu" probe docs/logs/tcp-output.pcap docs/logs/tcp-run.json tcp
python3 scripts/check-tcp.py
python3 scripts/check-budget.py "$ELF" docs/logs/tcp-budget.json
cargo build --locked --release --features tcp,drivers --target-dir target/tcp-enc
python3 scripts/check-budget.py target/tcp-enc/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test docs/logs/tcp-enc-budget.json
