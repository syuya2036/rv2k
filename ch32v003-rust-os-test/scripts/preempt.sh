#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
LOG=docs/logs/preemption
mkdir -p "$LOG"
(cd ..; cargo +stable build --locked --release --manifest-path host/net-harness/Cargo.toml --target-dir "$ROOT/vendor/renvo_emu/target")
export RUSTFLAGS='-C opt-level=z -C target-feature=+relax -C llvm-args=-riscv-enable-global-merge=false -C link-arg=--defsym=_kernel_stack_size=256 -C link-arg=-Tmemory.x -C link-arg=--icf=all -C link-arg=--relax -C link-arg=--relax-gp'
BIN="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin"
cargo build --locked --release --features preempt-probe,emulator-stop --target-dir target/preempt
ELF=target/preempt/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test
"$BIN/llvm-nm" -S "$ELF" > "$LOG/probe-symbols.txt"
"$BIN/llvm-objdump" -d "$ELF" > "$LOG/probe-disassembly.txt"
python3 scripts/elf-for-remu.py "$ELF" "$ELF.remu"
vendor/renvo_emu/target/release/remu run --target ch32v003 --elf "$ELF.remu" --max-instructions 20000000 --result "$LOG/probe-run.json" --bus-log "$LOG/probe-bus.json" --bus-log-region sram --bus-log-region ch32v003.gpioa --bus-log-region ch32v003.gpioc --bus-log-region ch32v003.gpiod --bus-log-region ch32v003.afio
python3 scripts/check-preempt.py
python3 scripts/check-budget.py "$ELF" "$LOG/probe-budget.json"
python3 scripts/preempt-negative.py
cargo build --locked --release --features tcp,preempt-timing,emulator-stop --target-dir target/preempt-tcp
ELF=target/preempt-tcp/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test
python3 scripts/elf-for-remu.py "$ELF" "$ELF.remu"
mkdir -p "$LOG/baseline"
vendor/renvo_emu/target/release/rv2k-net-harness "$ELF.remu" probe "$LOG/baseline/tcp-output.pcap" "$LOG/baseline/tcp-run.json" tcp
python3 scripts/check-tcp.py "$LOG/baseline"
vendor/renvo_emu/target/release/rv2k-net-harness "$ELF.remu" probe "$LOG/tcp-output.pcap" "$LOG/tcp-run.json" tcp-preempt
python3 scripts/check-tcp.py "$LOG"
python3 scripts/check-budget.py "$ELF" "$LOG/tcp-budget.json"
cargo build --locked --release --features tcp,preempt,drivers --target-dir target/preempt-enc
python3 scripts/check-budget.py target/preempt-enc/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test "$LOG/enc-budget.json"
