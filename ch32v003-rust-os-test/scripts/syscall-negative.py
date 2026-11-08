"""Fault injections: invalid task SP, synchronous kernel trap, lost restore."""
import json
import os
import re
import struct
import subprocess
from pathlib import Path

logs = Path('docs/logs')
remu = 'vendor/renvo_emu/target/release/remu'
base = Path('target/syscall/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test')
env = dict(os.environ, RUSTFLAGS='-C link-arg=--defsym=_kernel_stack_size=256 -C link-arg=-Tmemory.x')
sysroot = subprocess.check_output(['rustc', '--print', 'sysroot'], text=True).strip()
host = next(line[6:] for line in subprocess.check_output(['rustc', '-vV'], text=True).splitlines() if line.startswith('host: '))
nm = str(Path(sysroot) / 'lib/rustlib' / host / 'bin/llvm-nm')


def execute(elf, prefix, expected):
    subprocess.run(['python3', 'scripts/elf-for-remu.py', str(elf), str(elf)+'.remu'], check=True)
    run = logs / f'{prefix}-run.json'
    bus_path = Path('target/syscall') / f'{prefix}-bus.json'
    subprocess.run([remu, 'run', '--target', 'ch32v003', '--elf', str(elf)+'.remu',
                    '--max-instructions', '100000', '--result', str(run),
                    '--bus-log', str(bus_path), '--bus-log-region', 'sram'], check=True)
    result = json.loads(run.read_text())
    writes = json.loads(bus_path.read_text())
    symbols = subprocess.check_output([nm, '-S', str(elf)], text=True)
    status_address = next(int(f[0], 16) for line in symbols.splitlines()
                          if len(f := line.split()) == 4 and f[3] == 'SYSCALL_STATUS')
    status = [x['value'] for x in writes if x['kind'] == 'Write' and x['address'] == status_address][-1]
    assert result['reason'] == 'Halted' and status == expected, (prefix, result['reason'], status)
    assert b'SYSCALL OK' not in bytes(result['uart'])
    if prefix == 'syscall-bad-sp':
        addresses = {f[3]: int(f[0], 16) for line in symbols.splitlines() if len(f := line.split()) == 4}
        bottom, top = addresses['_stack_bottom'], addresses['_stack_top']
        # The final trap saved original sp=0 in its temporary kernel frame.
        marker = max(i for i, x in enumerate(writes) if x['kind'] == 'Write'
                     and x['address'] == top - 24 and x['value'] == 0)
        assert all(bottom <= x['address'] < top or x['address'] == status_address
                   for x in writes[marker+1:] if x['kind'] == 'Write')
    return {'case': prefix, 'status': status, 'instructions': result['stats']['instructions']}


cases = []
for feature, expected in [('syscall-bad-sp', 22), ('syscall-kernel-fault', 26)]:
    out = Path('target/syscall') / feature
    subprocess.run(['cargo', 'build', '--locked', '--release', '--features', feature,
                    '--target-dir', str(out)], env=env, check=True)
    elf = out / 'riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test'
    cases.append(execute(elf, feature, expected))

binary = bytearray(base.read_bytes())
text = (logs / 'syscall-disassembly.txt').read_text().split('<rv2k_restore>:', 1)[1]
match = re.search(r'^\s*([0-9a-f]+):\s+([0-9a-f]+)\s+lw\s+t1,', text, re.M)
assert match
pc, opcode = int(match[1], 16), match[2]
width = len(opcode) // 2
phoff = struct.unpack_from('<I', binary, 28)[0]
ent, count = struct.unpack_from('<HH', binary, 42)
for index in range(count):
    kind, off, vma, lma, size, mem, flags, align = struct.unpack_from('<8I', binary, phoff+index*ent)
    if kind == 1 and vma <= pc < vma+size:
        position = off+pc-vma
        assert binary[position:position+width] == int(opcode, 16).to_bytes(width, 'little')
        binary[position:position+width] = b'\x01\x00' * (width // 2)
        break
else:
    raise AssertionError('Restore instruction outside load segments')
elf = Path('target/syscall/missing-t1.elf')
elf.write_bytes(binary)
cases.append(execute(elf, 'syscall-missing-t1', 55))
(logs / 'syscall-negative-summary.json').write_text(json.dumps(cases, indent=2)+'\n')
print('PASS: syscall fault injections:', cases)
