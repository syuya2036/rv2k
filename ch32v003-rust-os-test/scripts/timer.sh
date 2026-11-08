#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --locked --release --features timer
ELF=target/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test
BIN="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin"
"$BIN/llvm-size" "$ELF" > docs/logs/timer-size.txt
"$BIN/llvm-objdump" -d "$ELF" > docs/logs/timer-disassembly.txt
python3 scripts/elf-for-remu.py "$ELF" target/timer.remu
vendor/renvo_emu/target/release/remu run --target ch32v003 --elf target/timer.remu --max-instructions 10000 --result docs/logs/timer-run.json --bus-log docs/logs/timer-bus.json --bus-log-region sram --interrupt-log docs/logs/timer-interrupts.json
python3 - <<'PY'
import json
from pathlib import Path
p=Path('docs/logs')
r=json.loads((p/'timer-run.json').read_text()); b=json.loads((p/'timer-bus.json').read_text())
assert r['reason']=='Halted'
assert bytes(r['uart'])==b'Rust RV32EC OK\n'
assert any(x['kind']=='Write' and x['address']==0x20000008 and x['value']==1 for x in b)
print('PASS: SysTick handler wrote TICKS=1 and returned; instructions=',r['stats']['instructions'])
PY
