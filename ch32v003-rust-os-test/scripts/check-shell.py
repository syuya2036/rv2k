"""Independent shell wire, public snapshot, and stack oracle (no app imports)."""
import json
import re
import struct
import sys
from pathlib import Path

log = Path(sys.argv[1])
r = json.loads((log / 'tcp-run.json').read_text())
budget = json.loads(Path(sys.argv[2]).read_text())
ram = bytes.fromhex(r['ram_hex'])
symbols = r['symbols']
def data(name, size):
    offset = symbols[name] - 0x20000000
    assert 0 <= offset <= 2048 - size
    return ram[offset:offset + size]
def integer(name, size=4):
    return int.from_bytes(data(name, size), 'little')
def pcap(name):
    b = (log / name).read_bytes()
    assert b[:24] == struct.pack('<IHHIIII', 0xa1b2c3d4, 2, 4, 0, 0, 65535, 1)
    pos, frames = 24, []
    while pos < len(b):
        _, _, size, original = struct.unpack_from('<4I', b, pos)
        assert size == original and 54 <= size <= 1514
        pos += 16
        frames.append(b[pos:pos + size])
        pos += size
    assert pos == len(b)
    return frames
def checksum(b):
    if len(b) & 1:
        b += b'\0'
    total = sum(struct.unpack('!' + 'H' * (len(b) // 2), b))
    while total > 65535:
        total = (total & 65535) + (total >> 16)
    return (~total) & 65535

def packets(frames, outgoing):
    streams = {53000 + i: {} for i in range(5)}
    starts, syns, fins, resets = {}, {}, set(), set()
    duplicates = 0
    for f in frames:
        assert f[12:14] == b'\x08\0' and f[23] == 6
        ihl, total = (f[14] & 15) * 4, int.from_bytes(f[16:18], 'big')
        assert 20 <= ihl <= 60 and ihl + 20 <= total <= len(f) - 14
        assert checksum(f[14:14 + ihl]) == 0
        guest, host = bytes([192, 168, 123, 50]), bytes([192, 168, 123, 1])
        assert f[26:34] == (guest + host if outgoing else host + guest)
        t = f[14 + ihl:14 + total]
        size = (t[12] >> 4) * 4
        assert 20 <= size <= 60 and size <= len(t)
        assert checksum(f[26:34] + b'\0\x06' + len(t).to_bytes(2, 'big') + t) == 0
        source, dest, seq, ack = struct.unpack_from('!HHII', t)
        port = dest if outgoing else source
        assert (source if outgoing else dest) == 23 and port in streams
        flags, body = t[13], t[size:]
        if flags & 2:
            assert not body and flags == (18 if outgoing else 2)
            start = (seq + 1) & 0xffffffff
            if port in starts:
                assert starts[port] == start
            starts[port] = start
            syns[port] = syns.get(port, 0) + 1
        if flags & 1:
            fins.add(port)
        if flags & 4:
            resets.add(port)
        if body:
            assert len(body) <= (24 if outgoing else 128)
            offset = (seq - starts[port]) & 0xffffffff
            assert offset < 4096
            for i, value in enumerate(body):
                if offset + i in streams[port]:
                    assert streams[port][offset + i] == value
                    duplicates += 1
                streams[port][offset + i] = value
    result = []
    for port, stream in streams.items():
        assert sorted(stream) == list(range(len(stream)))
        result.append(bytes(stream[i] for i in range(len(stream))))
    assert set(starts) == set(streams)
    assert fins == {53000, 53001, 53002, 53004}
    assert resets == (set() if outgoing else {53003})
    return result, syns, duplicates

assert r['reason'] == 'Halted' and r['uart'] == 'SERVICES OK\n'
assert integer('FAULT_INFO') == 0 and integer('SERVICES_STATUS') == 0x600d
used = struct.unpack('<3I', data('STACK_USED', 12))
assert all(80 <= n <= 240 for n in used), used
for address in [symbols['TASK_STACKS'], symbols['TASK_STACKS'] + 256, symbols['_stack_bottom']]:
    offset = address - 0x20000000
    assert ram[offset:offset + 16] == bytes([0xa5]) * 16
assert integer('APP_LISTENS') == 6 and integer('APP_SHELL_ACCEPTS') == 5
assert integer('APP_SHELL_COMMANDS') == 12
assert integer('UPTIME', 8) >= 6200 and integer('TIMER_IRQS') > 0
incoming, outgoing = pcap('tcp-output.rx.pcap'), pcap('tcp-output.pcap')
assert r['injected'] == r['released'] == len(incoming)
assert r['transmitted'] == len(outgoing) and r['rx_overflow'] == 0
assert struct.unpack('<6H', data('NET_COUNTERS', 12)) == (len(incoming), len(outgoing), 0, 0, 0, 0)
requests, _, _ = packets(incoming, False)
responses, syns, duplicates = packets(outgoing, True)
expected = [b'ps\nmem\nuptime\nnetstat\n', b'x' * 65 + b'\nhelp\r\nnope\nup\x01time\nnetstat\nquit\n', b'up', b'netsta', b'uptime\nps\n']
assert requests == expected
assert integer('APP_SHELL_INPUT_BYTES') == sum(map(len, expected))
assert syns[53000] >= 2 and duplicates > 0
banner = b'rv2k shell\r\n> '
assert all(s.startswith(banner) for s in responses)
assert responses[2] == responses[3] == banner
assert [s.count(b'> ') for s in responses] == [5, 6, 1, 1, 3]
texts = [s.decode('ascii') for s in responses]
ps = r'pid=0 state=(?:READY|BLOCKED) exit=0\r\npid=1 state=RUNNING exit=0\r\n'
assert re.fullmatch(ps, texts[0].split('> ')[1])
assert re.fullmatch(ps, texts[4].split('> ')[2])
m = re.fullmatch(r'ram=(\d+)/2048 flash=(\d+)/16384\r\nstatic=(\d+) kernel=256 task=256\r\nstack0=(\d+) stack1=(\d+) kernel=(\d+) guards=0\r\n', texts[0].split('> ')[2])
assert m
reserved, flash, static, *seen_used = map(int, m.groups())
assert (reserved, flash, static) == (budget['total_reserved_ram_bytes'], budget['flash_bytes'], budget['static_ram_bytes'])
assert all(80 <= a <= b <= 240 for a, b in zip(seen_used, used))
ticks = []
for text, part in [(texts[0], 3), (texts[4], 1)]:
    match = re.fullmatch(r'ticks=(\d+) tick_ms=10\r\n', text.split('> ')[part])
    assert match
    ticks.append(int(match[1]))
assert 6200 <= ticks[0] <= ticks[1] <= integer('UPTIME', 8)
for index, part in [(0, 4), (1, 5)]:
    value = texts[index].split('> ')[part]
    pattern = (r'net state=ESTABLISHED flags=3\r\nlocal=192\.168\.123\.50:23 peer=192\.168\.123\.1:' + str(53000 + index) +
               r'\r\nrx=(\d+) tx=(\d+) drop=0 csum=0\r\nfrag=0 io=0 retrans=(\d+) reset=0\r\n')
    match = re.fullmatch(pattern, value)
    assert match, value
    rx, tx, retry = map(int, match.groups())
    assert 2 <= rx <= len(incoming) and 2 <= tx <= len(outgoing) and retry >= 2
    port = 53000 + index
    def tuple_flags(frame):
        offset = 14 + (frame[14] & 15) * 4
        return (*struct.unpack_from('!HH', frame, offset), frame[offset + 13])
    assert any(tuple_flags(f) == (port, 23, 2) for f in incoming[:rx])
    assert any(tuple_flags(f) == (23, port, 18) for f in outgoing[:tx])
    assert any(tuple_flags(f)[:2] == (port, 23) and tuple_flags(f)[2] & 16 for f in incoming[:rx])
assert texts[1].split('> ')[1:5] == ['error: line too long\r\n', 'ps mem uptime netstat help quit\r\n', 'error: unknown command\r\n', 'error: invalid input\r\n']
assert texts[1].endswith('> bye\r\n')
peer = r['tcp_probe']
assert peer['done'] and peer['connections'] == 5 and peer['syn_loss_recovered'] and peer['zero_window_persist']
assert peer['advanced_ticks'] == 6200
assert [case['response'].encode() for case in peer['cases']] == responses
assert integer('PREEMPT_STATE') == integer('PREEMPT_BLOCKED_CALLS') == 0
# The normal run must switch on a timer. Stress delivers each IRQ while
# T0 holds its borrow; T1 may be blocked, so a forced switch is not guaranteed.
if r['mode'] == 'shell':
    assert integer('PREEMPT_SWITCHES') > 0
if r['mode'] == 'shell-preempt':
    assert r['nic_irq_injections'] == len(incoming)
assert integer('PREEMPT_DEFERRED') >= r['nic_irq_injections']
summary = dict(host_packet_peer='PASS (synthetic peer, not native Mac TCP)', connections=5,
               all_four_commands_via_public_api=True, split_and_coalesced_input=True,
               oversized_invalid_unknown_recovery=True, partial_eof_and_reset_discard=True,
               clean_reconnect=True, syn_loss_and_zero_window_persist=True,
               input_bytes=sum(map(len, expected)), commands=12, frames_in=len(incoming), frames_out=len(outgoing),
               instructions=r['instructions'], stack_used_bytes_tasks_kernel=used, all_guards_intact=True,
               uptime_command_ticks=ticks, nic_irq_injections=r['nic_irq_injections'],
               forced_switches=integer('PREEMPT_SWITCHES'), deferred_irqs=integer('PREEMPT_DEFERRED'),
               max_disabled_clocks=None, mac_native_tcp='UNVERIFIED', physical_enc='UNVERIFIED')
(log / 'shell-summary.json').write_text(json.dumps(summary, indent=2) + '\n')
print('PASS: shell TCP commands, public snapshots, input recovery, reconnect and guards:', summary)
