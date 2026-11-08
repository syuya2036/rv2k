"""Prove register-loss detection by omitting one restore in a disposable ELF."""
import json
import re
import struct
import subprocess
from pathlib import Path

logs = Path('docs/logs')
elf = Path('target/coop/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test')
b = bytearray(elf.read_bytes())
text = (logs/'coop-disassembly.txt').read_text()
restore = text.split('<restore_context>:', 1)[1].split('<probe_a>:', 1)[0]
match = re.search(r'^\s*([0-9a-f]+):\s+([0-9a-f]+)\s+lw\s+a5,', restore, re.M)
assert match is not None
pc, opcode = int(match[1], 16), match[2]
assert len(opcode) == 4  # compressed instruction in this pinned build
phoff = struct.unpack_from('<I', b, 28)[0]
ent, count = struct.unpack_from('<HH', b, 42)
for i in range(count):
    kind, off, vma, lma, size, mem, flags, align = struct.unpack_from('<8I', b, phoff+i*ent)
    if kind == 1 and vma <= pc < vma+size:
        pos = off+pc-vma
        assert b[pos:pos+2] == int(opcode, 16).to_bytes(2, 'little')
        b[pos:pos+2] = b'\x01\x00'  # c.nop instead of lw a5, 52(sp)
        break
else:
    raise AssertionError('Instruction is outside ELF load records')
f = Path('target/coop/missing-a5.elf')
f.write_bytes(b)
subprocess.run(['python3', 'scripts/elf-for-remu.py', str(f), str(f)+'.remu'], check=True)
subprocess.run(['vendor/renvo_emu/target/release/remu', 'run', '--target', 'ch32v003', '--elf', str(f)+'.remu', '--max-instructions', '100000', '--result', str(logs/'coop-negative-run.json'), '--bus-log', str(logs/'coop-negative-bus.json'), '--bus-log-region', 'sram'], check=True)
r = json.loads((logs/'coop-negative-run.json').read_text())
bus = json.loads((logs/'coop-negative-bus.json').read_text())
status_addr = None
for line in (logs/'coop-symbols.txt').read_text().splitlines():
    fields = line.split()
    if len(fields)==4 and fields[3]=='COOP_STATUS': status_addr=int(fields[0],16)
assert status_addr is not None
status = [x['value'] for x in bus if x['kind']=='Write' and x['address']==status_addr][-1]
assert r['reason']=='Halted' and status==3, (r['reason'], status)
assert b'COOP OK' not in bytes(r['uart'])
print(f'PASS: negative test detected missing a5 restore at {pc:#x}; COOP_STATUS=3')
