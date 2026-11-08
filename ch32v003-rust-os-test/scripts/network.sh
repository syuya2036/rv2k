#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p docs/logs
ROOT="$PWD"
# Host Cargo must run outside the firmware's build-std/target config.
(cd ..; cargo +stable build --locked --release --manifest-path host/net-harness/Cargo.toml --target-dir "$ROOT/vendor/renvo_emu/target")
(cd ..; cargo +stable test --locked --manifest-path host/net-harness/Cargo.toml --target-dir "$ROOT/vendor/renvo_emu/target")
RUSTFLAGS='-C link-arg=--defsym=_kernel_stack_size=256 -C link-arg=-Tmemory.x' cargo build --locked --release --features net,emulator-stop --target-dir target/network
ELF=target/network/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test
python3 scripts/elf-for-remu.py "$ELF" "$ELF.remu"
python3 scripts/network-fixtures.py generate
vendor/renvo_emu/target/release/rv2k-net-harness "$ELF.remu" docs/logs/network-input.pcap docs/logs/network-output.pcap docs/logs/network-run.json replay
python3 scripts/network-fixtures.py check
python3 scripts/check-budget.py "$ELF" docs/logs/network-budget.json
# The physical NIC must link with the same protocol implementation and limits.
# This is a build gate only; it cannot establish physical timing/stack coverage.
RUSTFLAGS='-C link-arg=--defsym=_kernel_stack_size=256 -C link-arg=-Tmemory.x' cargo build --locked --release --features net,drivers --target-dir target/network-enc
python3 scripts/check-budget.py target/network-enc/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test docs/logs/network-enc-budget.json
