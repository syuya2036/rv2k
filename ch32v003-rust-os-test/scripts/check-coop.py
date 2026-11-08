"""Check execution, ordering, register probes, private locals and stack budgets."""
import json
from pathlib import Path

p = Path('docs/logs')
r = json.loads((p / 'coop-run.json').read_text())
bus = json.loads((p / 'coop-bus.json').read_text())
symbols = {}
for line in (p / 'coop-symbols.txt').read_text().splitlines():
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
assert r['stats']['instructions'] > 0
assert bytes(r['uart']) == b'AB'*8+b'\nCOOP OK\n', r['uart']
assert words('COOP_STATUS') == [0x600d], words('COOP_STATUS')
assert words('COUNTERS') == [8, 8], words('COUNTERS')
assert words('CURRENT_TASK') == [0]
assert words('SWITCHES') == [32], words('SWITCHES')
assert words('TRACE_LEN') == [16]
assert data('TRACE') == b'AB'*8
stack_base, stack_size = symbols['TASK_STACKS']
assert stack_size == 512 and stack_base % 16 == 0
boot_bottom = symbols['_stack_bottom'][0]
assert stack_base + stack_size <= symbols['_ebss'][0] <= boot_bottom
usage = words('STACK_USED')
local_addresses = words('LOCAL_ADDR')
saved_sp = words('SAVED_SP')
for i in range(2):
    lo, hi = stack_base+i*256, stack_base+(i+1)*256
    assert lo+16 <= local_addresses[i] < hi
    assert local_addresses[i] % 4 == 0
    offset = local_addresses[i]-0x20000000
    assert int.from_bytes(ram[offset:offset+4], 'little') == 8
    assert lo+16 <= saved_sp[i] <= hi-64 and saved_sp[i] % 4 == 0
    assert 64 <= usage[i] <= 240, usage
    offset = lo-0x20000000
    assert ram[offset:offset+16] == bytes.fromhex('a5a5a5a5')*4
assert local_addresses[0] != local_addresses[1]
registers = {x['name']: x['value'] for x in r['cpu']['registers']}
assert registers['mstatus'] & 8 == 0  # cooperative run never enables interrupts
assert registers['mtvec'] == 0
summary = {
    'instructions': r['stats']['instructions'],
    'counters': words('COUNTERS'), 'switches': words('SWITCHES')[0],
    'stack_bytes_each': 256, 'stack_watermark_used_bytes': usage,
    'local_addresses': [hex(x) for x in local_addresses],
    'static_ram_bytes': symbols['_ebss'][0]-0x20000000,
    'boot_stack_reserved_bytes': symbols['_stack_top'][0]-boot_bottom,
    'status': hex(words('COOP_STATUS')[0]),
    'uart': bytes(r['uart']).decode(),
}
(p/'coop-summary.json').write_text(json.dumps(summary, indent=2)+'\n')
print(f'PASS: counters=8/8, switches=32, register probes passed; stack watermark A/B={usage} bytes; {summary["instructions"]} instructions')
