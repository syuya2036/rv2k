"""Check execution, ordering, register probes, private locals and stack budgets."""
import json
from pathlib import Path

p = Path('docs/logs')
r = json.loads((p / 'syscall-run.json').read_text())
bus = json.loads((p / 'syscall-bus.json').read_text())
symbols = {}
for line in (p / 'syscall-symbols.txt').read_text().splitlines():
    fields = line.split()
    if len(fields) == 4:
        address, size, kind, name = fields
        symbols[name] = (int(address, 16), int(size, 16))

# Replay SRAM writes; do not assume any symbol's link-time address.
ram = bytearray(2048)
for access in bus:
    if access['kind'] != 'Write':
        continue
    start = access['address'] - 0x20000000
    width = {'Byte': 1, 'HalfWord': 2, 'Word': 4}[access['width']]
    assert 0 <= start <= len(ram) - width, access
    ram[start:start+width] = access['value'].to_bytes(width, 'little')

def data(name):
    address, size = symbols[name]
    start = address - 0x20000000
    assert 0 <= start <= len(ram)-size
    return ram[start:start+size]

def words(name):
    v = data(name)
    return [int.from_bytes(v[i:i+4], 'little') for i in range(0, len(v), 4)]

assert r['reason'] == 'Halted', r['reason']
assert words('SYSCALL_STATUS') == [0x600d], words('SYSCALL_STATUS')
assert bytes(r['uart']) == b'SYSCALL OK\n', r['uart']
assert words('COUNTERS') == [8, 3], words('COUNTERS')
assert words('CALL_MASK') == [(1 << 18) - 1], words('CALL_MASK')
assert words('LAST_CAUSE') == [11]
assert words('TRAPS')[0] >= 50
usage = words('STACK_USED')
base, size = symbols['TASK_STACKS']
assert size == 512 and base % 16 == 0
for i in range(2):
    offset = base + i*256 - 0x20000000
    assert ram[offset:offset+16] == bytes.fromhex('a5a5a5a5')*4
    assert 80 <= usage[i] <= 240, usage
bottom = symbols['_stack_bottom'][0]
assert symbols['_stack_top'][0] - bottom == 256
assert 80 <= usage[2] <= 240, usage
assert ram[bottom-0x20000000:bottom-0x20000000+16] == bytes.fromhex('a5a5a5a5')*4
registers = {x['name']: x['value'] for x in r['cpu']['registers']}
assert registers['mstatus'] & 8 == 0
summary = {
    'instructions': r['stats']['instructions'], 'traps': words('TRAPS')[0],
    'counters': words('COUNTERS'), 'call_mask': hex(words('CALL_MASK')[0]),
    'stack_used_bytes_tasks_kernel': usage,
    'static_ram_bytes': symbols['_ebss'][0]-0x20000000,
    'kernel_stack_reserved_bytes': 256,
}
(p/'syscall-summary.json').write_text(json.dumps(summary, indent=2)+'\n')
print('PASS: RV32E ecall dispatch, pointers, register probes, return/exit:', summary)
