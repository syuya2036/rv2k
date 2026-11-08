#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
case "${1:-}" in
  "") CHECK_ENC=1 ;;
  --virtual-only) CHECK_ENC=0 ;;
  *) echo 'usage: http.sh [--virtual-only]' >&2; exit 2 ;;
esac
LOG=docs/logs/http
mkdir -p "$LOG" "$LOG/stress"
(cd ..; cargo +stable build --locked --release --manifest-path host/net-harness/Cargo.toml --target-dir "$ROOT/vendor/renvo_emu/target")
export RUSTFLAGS='-C opt-level=z -C target-feature=+relax -C llvm-args=-riscv-enable-global-merge=false -C link-arg=--defsym=_kernel_stack_size=256 -C link-arg=-Tmemory.x -C link-arg=--icf=all -C link-arg=--relax -C link-arg=--relax-gp'
cargo build --locked --release --features http,emulator-stop --target-dir target/http
ELF=target/http/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test
python3 scripts/elf-for-remu.py "$ELF" "$ELF.remu"
vendor/renvo_emu/target/release/rv2k-net-harness "$ELF.remu" probe "$LOG/http-output.pcap" "$LOG/http-run.json" http
python3 scripts/check-http.py "$LOG"
python3 scripts/check-budget.py "$ELF" "$LOG/budget.json"
vendor/renvo_emu/target/release/rv2k-net-harness "$ELF.remu" probe "$LOG/stress/http-output.pcap" "$LOG/stress/http-run.json" http-preempt
python3 scripts/check-http.py "$LOG/stress"
if [ "$CHECK_ENC" = 1 ]; then
  cargo build --locked --release --features http,drivers --target-dir target/http-enc
  python3 scripts/check-budget.py target/http-enc/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test "$LOG/enc-budget.json"
fi
