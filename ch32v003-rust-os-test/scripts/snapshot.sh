#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
LOG=docs/logs/snapshot
mkdir -p "$LOG" "$LOG/stress"
(cd ..; cargo +stable build --locked --release --manifest-path host/net-harness/Cargo.toml --target-dir "$ROOT/vendor/renvo_emu/target")
export RUSTFLAGS='-Z inline-mir=yes -C opt-level=z -C llvm-args=-inline-threshold=25 -C target-feature=+relax -C llvm-args=-riscv-enable-global-merge=false -C link-arg=--defsym=_kernel_stack_size=256 -C link-arg=-Tmemory.x -C link-arg=--icf=all -C link-arg=--relax -C link-arg=--relax-gp'
cargo build --locked --release --features netinfo-probe,emulator-stop --target-dir target/netinfo
ELF=target/netinfo/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test
python3 scripts/elf-for-remu.py "$ELF" "$ELF.remu"
vendor/renvo_emu/target/release/rv2k-net-harness "$ELF.remu" probe "$LOG/tcp-output.pcap" "$LOG/tcp-run.json" tcp
python3 scripts/check-tcp.py "$LOG"
python3 scripts/check-netinfo.py "$LOG"
python3 scripts/check-budget.py "$ELF" "$LOG/budget.json"
vendor/renvo_emu/target/release/rv2k-net-harness "$ELF.remu" probe "$LOG/stress/tcp-output.pcap" "$LOG/stress/tcp-run.json" tcp-preempt
python3 scripts/check-tcp.py "$LOG/stress"
python3 scripts/check-netinfo.py "$LOG/stress"
cargo build --locked --release --features tcp,preempt,drivers --target-dir target/preempt-enc
python3 scripts/check-budget.py target/preempt-enc/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test "$LOG/enc-budget.json"
