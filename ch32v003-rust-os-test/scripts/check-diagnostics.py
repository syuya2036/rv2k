"""Check execution, ordering, register probes, private locals and stack budgets."""
import json
import sys
from pathlib import Path

p = Path('docs/logs')
prefix = sys.argv[1] if len(sys.argv)>1 else 'diagnostics'
r = json.loads((p / f'{prefix}-run.json').read_text())
bus = json.loads((p / f'{prefix}-bus.json').read_text())
symbols = {}
for line in (p / f'{prefix}-symbols.txt').read_text().splitlines():
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
assert words('FAULT_INFO')[0] == 0, words('FAULT_INFO')
if prefix == 'compact':
    assert words('COMPACT_STATUS') == [0xc0]
assert words('SERVICES_STATUS') == [0x600d], words('SERVICES_STATUS')
assert bytes(r['uart']) == b'SERVICES OK\n', r['uart']
assert words('COUNTERS') == [1, 1]
assert words('WAIT_RESULTS') == [127], words('WAIT_RESULTS')
assert words('TIMER_IRQS')[0] > 0
assert words('IDLE_POLLS')[0] > 0
assert int.from_bytes(data('UPTIME'), 'little') >= 4
usage = words('STACK_USED')
assert all(80 <= used <= 240 for used in usage), usage
base, size = symbols['TASK_STACKS']
for offset in [base, base+256, symbols['_stack_bottom'][0]]:
    offset -= 0x20000000
    assert ram[offset:offset+16] == bytes.fromhex('a5a5a5a5')*4
summary = {
    'instructions': r['stats']['instructions'], 'syscalls': words('TRAPS')[0],
    'timer_irqs': words('TIMER_IRQS')[0], 'idle_polls': words('IDLE_POLLS')[0],
    'uptime_ticks': int.from_bytes(data('UPTIME'), 'little'),
    'wait_results': words('WAIT_RESULTS')[0], 'stack_used_bytes_tasks_kernel': usage,
    'static_ram_bytes': symbols['_ebss'][0]-0x20000000,
    'kernel_stack_reserved_bytes': symbols['_stack_top'][0]-symbols['_stack_bottom'][0],
}
if prefix == 'compact':
    summary['compact_memory_status'] = words('COMPACT_STATUS')[0]
(p/f'{prefix}-summary.json').write_text(json.dumps(summary, indent=2)+'\n')
print('PASS: timed sleep/event/IPC and IRQ-enabled idle:', summary)
