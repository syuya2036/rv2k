"""Independent wire-layout and PCAP-prefix checks for syscall 14 probe copies."""
import json
import struct
import sys
from pathlib import Path

log = Path(sys.argv[1])
run = json.loads((log / 'tcp-run.json').read_text())
ram = bytes.fromhex(run['ram_hex'])
def data(name, size):
    offset = run['symbols'][name] - 0x20000000
    assert 0 <= offset <= 2048 - size
    return ram[offset:offset + size]
assert run['reason'] == 'Halted' and run['uart'] == 'SERVICES OK\n'
assert int.from_bytes(data('FAULT_INFO', 4), 'little') == 0
assert int.from_bytes(data('APP_NET_CHECK', 4), 'little') == 63
assert int.from_bytes(data('APP_NET_RECORDS', 4), 'little') == 3
raw = data('APP_NET_SNAPSHOTS', 96)
snapshots = [struct.unpack_from('<HBB4s4s10H', raw, index * 32) for index in range(3)]
def pcap(name):
    b = (log / name).read_bytes()
    assert b[:4] == b'\xd4\xc3\xb2\xa1'
    pos = 24
    frames = []
    while pos < len(b):
        _, _, size, original = struct.unpack_from('<4I', b, pos)
        assert size == original
        pos += 16
        frames.append(b[pos:pos + size])
        pos += size
    assert pos == len(b)
    return frames
incoming = pcap('tcp-output.rx.pcap')
outgoing = pcap('tcp-output.pcap')
def tcp(frame):
    offset = 14 + (frame[14] & 15) * 4
    return (*struct.unpack_from('!HH', frame, offset), frame[offset + 13])
boot = snapshots[0]
assert boot[0] == 1 and boot[1] == 0 and boot[2] in (0, 1)
assert boot[3] == bytes([192, 168, 123, 50]) and boot[4] == bytes(4)
assert boot[5:] == (0,) * 10
for index, row in enumerate(snapshots[1:]):
    version, state, flags, local, peer, port, remote, rx, tx, drop, bad, frag, io, retries, resets = row
    assert (version, state, flags) == (1, 3, 3)
    assert local == bytes([192, 168, 123, 50]) and peer == bytes([192, 168, 123, 1])
    assert port == 1234 and remote == 50000 + index
    assert (drop, bad, frag, io, resets) == (0, 0, 0, 0, 0)
    assert 2 <= rx <= len(incoming) and 1 <= tx <= len(outgoing)
    assert rx > snapshots[index][7] and tx > snapshots[index][8]
    # At the captured counters' packet prefixes, this tuple has completed both
    # sides of the handshake. The first tuple intentionally loses one SYN-ACK.
    prefix_in = [tcp(f) for f in incoming[:rx]]
    prefix_out = [tcp(f) for f in outgoing[:tx]]
    assert any(p == (remote, 1234, 2) for p in prefix_in)
    assert any(p[0:2] == (remote, 1234) and p[2] & 16 for p in prefix_in)
    synacks = sum(p == (1234, remote, 18) for p in prefix_out)
    assert synacks >= (2 if index == 0 else 1)
    assert retries >= 1
summary = {'syscall': 14, 'wire_bytes': 32, 'version': 1,
           'task_mem_net_callback_copies_survive_yield': True,
           'boot_and_two_connections': True, 'invalid_arguments_and_destinations': 7,
           'states': [s[1] for s in snapshots], 'peer_ports': [s[6] for s in snapshots],
           'rx_counts': [s[7] for s in snapshots], 'tx_counts': [s[8] for s in snapshots],
           'retransmission_counts': [s[13] for s in snapshots],
           'pcap_prefix_handshakes_checked': True,
           'mac_native_tcp': 'UNVERIFIED', 'physical_enc': 'UNVERIFIED'}
(log / 'netinfo-summary.json').write_text(json.dumps(summary, indent=2) + '\n')
print('PASS: public network snapshot ABI and independent packet prefixes:', summary)
