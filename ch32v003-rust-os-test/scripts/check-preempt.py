"""Independent SRAM/trace oracle for forced, non-yielding timer switches."""
import json
from pathlib import Path

LOG = Path('docs/logs/preemption')
r = json.loads((LOG/'probe-run.json').read_text())
symbols = {}
for line in (LOG/'probe-symbols.txt').read_text().splitlines():
    fields = line.split()
    if len(fields) in (3, 4):
        symbols[fields[-1]] = int(fields[0], 16)
ram = bytearray(2048)
bus = json.loads((LOG/'probe-bus.json').read_text())
for access in bus:
    if access['kind'] != 'Write': continue
    start = access['address'] - 0x20000000
    size = {'Byte': 1, 'HalfWord': 2, 'Word': 4}[access['width']]
    if not 0 <= start <= 2048-size: continue
    ram[start:start+size] = access['value'].to_bytes(size, 'little')
def words(name, count=1):
    start = symbols[name] - 0x20000000
    assert 0 <= start <= 2048-count*4
    return [int.from_bytes(ram[i:i+4], 'little') for i in range(start, start+count*4, 4)]
def value(name): return words(name)[0]
assert r['reason'] == 'Halted' and bytes(r['uart']) == b'SERVICES OK\n'
assert value('FAULT_INFO') == 0 and value('SERVICES_STATUS') == 0x600d
assert words('PREEMPT_PROGRESS', 2) == [3, 3]
assert value('PREEMPT_PROBE_STATUS') == 3
assert value('PREEMPT_BLOCKED_CALLS') == 8
assert value('PREEMPT_SWITCHES') >= 8
assert value('PREEMPT_DEFERRED') >= 4
assert value('PREEMPT_MAX_CLOCKS') >= 960000
trace = words('PREEMPT_TRACE', 32)
assert value('PREEMPT_TRACE_LEN') == 8
inside = [0, 0]
boundary_switches = 0
for pos in range(0, len(trace), 4):
    source, pc, cause, dest = trace[pos:pos+4]
    assert source in (0, 1) and dest == 1-source
    assert cause in (0x8000000c, 11), (pc, cause)
    if cause == 11: boundary_switches += 1
    label = 'preempt_probe_' + ('a' if source == 0 else 'b')
    if symbols[label] <= pc < symbols[label+'_end']:
        inside[source] += 1
assert min(inside) >= 2, inside
assert boundary_switches == 1
selected = False
mosi = False
bits = []
packets = []
irq_bit_positions = []
reads = 0
for access in bus:
    address, word = access['address'], access['value']
    if access['kind'] == 'Read' and address == 0x40011008:
        reads += 1
        assert word & 4 == 0
    if access['kind'] != 'Write': continue
    if address == symbols['TIMER_IRQS'] and selected:
        irq_bit_positions.append(len(bits))
    if address in (0x40011010, 0x40011014):
        high = address == 0x40011010
        if word & 2: mosi = high
        if word & 16:
            if high and selected:
                assert len(bits)%8 == 0
                packets.append([sum(int(bits[j+k]) << (7-k) for k in range(8)) for j in range(0,len(bits),8)])
                bits = []
            selected = not high
    if address == 0x40010810 and word & 4:
        assert selected
        bits.append(mosi)
assert packets == [[0xa5, 0x3c]] and reads == 16, (packets, reads)
assert any(0 < n < 16 for n in irq_bit_positions), irq_bit_positions
used = words('STACK_USED', 3)
assert all(80 <= n <= 240 for n in used), used
for address in [symbols['TASK_STACKS'], symbols['TASK_STACKS']+256, symbols['_stack_bottom']]:
    start = address - 0x20000000
    assert ram[start:start+16] == bytes.fromhex('a5a5a5a5')*4
summary = dict(instructions=r['stats']['instructions'], progress=[3, 3],
    forced_switches=value('PREEMPT_SWITCHES'), irqs=value('TIMER_IRQS'),
    deferred_irqs=value('PREEMPT_DEFERRED'), rejected_blocking_calls=8,
    max_disabled_clocks=value('PREEMPT_MAX_CLOCKS'),
    trace_irq_switches_inside_cpu_probes=inside,
    deferred_switches_at_syscall_boundary=boundary_switches,
    spi_mosi_packets=packets, irq_inside_spi_bit_positions=irq_bit_positions,
    stack_used_bytes_tasks_kernel=used, all_guards_intact=True)
(LOG/'probe-summary.json').write_text(json.dumps(summary, indent=2)+'\n')
(LOG/'probe-memory.json').write_text(json.dumps({'ram_hex':ram.hex(),
    'trace_source_pc_cause_dest':[trace[i:i+4] for i in range(0,len(trace),4)],
    'spi_mosi_packets':packets,'irq_inside_spi_bit_positions':irq_bit_positions},indent=2)+'\n')
print('PASS: timer preemption, register/PC sentinels, nesting and guards:', summary)
