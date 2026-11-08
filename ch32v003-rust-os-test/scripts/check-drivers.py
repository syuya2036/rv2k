"""Check execution, ordering, register probes, private locals and stack budgets."""
import json
from pathlib import Path

p = Path('docs/logs')
r = json.loads((p / 'drivers-run.json').read_text())
bus = json.loads((p / 'drivers-bus.json').read_text())
symbols = {}
for line in (p / 'drivers-symbols.txt').read_text().splitlines():
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
    if not 0 <= start <= len(ram) - width: continue
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
assert words('DRIVER_STATUS') == [1], words('DRIVER_STATUS')
assert words('FAULT_INFO')[0] == 0, words('FAULT_INFO')
assert words('SERVICES_STATUS') == [0x600d], words('SERVICES_STATUS')
assert bytes(r['uart']) == b'UART via API\nSERVICES OK\n', r['uart']
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
# Reconstruct mode-0 MOSI bytes from actual GPIO writes at SCK rising edges.
packets=[]; bits=[]; selected=False; mosi=False; rises=0; reads=0
for access in bus:
    address=access['address']; value=access['value']
    if access['kind']=='Read' and address==0x40011008:
        reads+=1
        assert value & 4 == 0  # this Renvo target has no NIC driving MISO
    if access['kind']!='Write': continue
    if address==0x40010004:
        assert value & (7<<24) == 0 and value & (1<<21) != 0 and value & 4 == 0 and value & (1<<15) == 0
    if address==0x40011400: assert (value>>4)&15==4  # SWIO remains non-driving GPIO
    if address in [0x40011010,0x40011014]:
        high=address==0x40011010
        if value & 2: mosi=high
        if value & 16:
            if high and selected:
                assert len(bits)%8==0
                packets.append([sum(int(bits[j+k])<<(7-k) for k in range(8)) for j in range(0,len(bits),8)])
                bits=[]
            selected=not high
    if address==0x40010810 and value&4:
        assert selected
        bits.append(mosi);rises+=1
assert packets==[[255],[191,3],[159,3],[18,0]],packets
assert rises==reads==56,(rises,reads)
summary = {
    'spi_packets': packets, 'spi_rising_edges': rises, 'nic_attached': False,
    'instructions': r['stats']['instructions'], 'syscalls': words('TRAPS')[0],
    'timer_irqs': words('TIMER_IRQS')[0], 'idle_polls': words('IDLE_POLLS')[0],
    'uptime_ticks': int.from_bytes(data('UPTIME'), 'little'),
    'wait_results': words('WAIT_RESULTS')[0], 'stack_used_bytes_tasks_kernel': usage,
    'static_ram_bytes': symbols['_ebss'][0]-0x20000000,
    'kernel_stack_reserved_bytes': symbols['_stack_top'][0]-symbols['_stack_bottom'][0],
}
(p/'drivers-summary.json').write_text(json.dumps(summary, indent=2)+'\n')
print('PASS: timed sleep/event/IPC and IRQ-enabled idle:', summary)
