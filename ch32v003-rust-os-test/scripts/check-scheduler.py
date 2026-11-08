"""Check execution, ordering, register probes, private locals and stack budgets."""
import json
from pathlib import Path

p = Path('docs/logs')
r = json.loads((p / 'scheduler-run.json').read_text())
bus = json.loads((p / 'scheduler-bus.json').read_text())
symbols = {}
for line in (p / 'scheduler-symbols.txt').read_text().splitlines():
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
assert words('SCHEDULER_STATUS') == [0x600d], words('SCHEDULER_STATUS')
assert bytes(r['uart']) == b'SCHEDULER OK\n', r['uart']
assert words('COUNTERS') == [8, 3], words('COUNTERS')
assert words('EXIT_CODES') == [0, 0xfffffff9]
assert words('SWITCHES') == [23], words('SWITCHES')
usage = words('STACK_USED')
base, size = symbols['TASK_STACKS']
assert size == 512 and base % 16 == 0
for i in range(2):
    offset = base + i*256 - 0x20000000
    assert ram[offset:offset+16] == bytes.fromhex('a5a5a5a5')*4
    assert 64 <= usage[i] <= 240, usage
registers = {x['name']: x['value'] for x in r['cpu']['registers']}
assert registers['mstatus'] & 8 == 0
summary = {
    'instructions': r['stats']['instructions'],
    'counters': words('COUNTERS'), 'exit_codes': [0, -7],
    'selections': words('SWITCHES')[0], 'task_stack_used_bytes': usage,
    'static_ram_bytes': symbols['_ebss'][0]-0x20000000,
    'kernel_stack_reserved_bytes': symbols['_stack_top'][0]-symbols['_stack_bottom'][0],
}
(p/'scheduler-summary.json').write_text(json.dumps(summary, indent=2)+'\n')
print('PASS: return/exit, survivor register probes, stacks:', summary)
