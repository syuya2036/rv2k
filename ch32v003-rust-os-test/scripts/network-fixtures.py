"""Independent Ethernet fixtures and PCAP oracle for the RV32EC protocol task."""
import json
import struct
import sys
from pathlib import Path

LOG = Path('docs/logs')
MAC = bytes.fromhex('020000000001')
HOST = bytes.fromhex('02aabbccddee')
IP = bytes([192, 168, 123, 50])
HOST_IP = bytes([192, 168, 123, 1])


def checksum(b):
    b = b + bytes(len(b) % 2)
    s = sum(struct.unpack('!' + 'H' * (len(b) // 2), b))
    while s >> 16:
        s = (s & 65535) + (s >> 16)
    return (~s) & 65535


def write_pcap(path, frames):
    b = struct.pack('<IHHIIII', 0xa1b2c3d4, 2, 4, 0, 0, 65535, 1)
    for i, f in enumerate(frames):
        b += struct.pack('<IIII', i, 0, len(f), len(f)) + f
    path.write_bytes(b)


def read_pcap(path):
    b = path.read_bytes()
    assert b[:24] == struct.pack('<IHHIIII', 0xa1b2c3d4, 2, 4, 0, 0, 65535, 1)
    out = []
    pos = 24
    while pos < len(b):
        _, _, length, original = struct.unpack_from('<IIII', b, pos)
        assert length == original
        out.append(b[pos+16:pos+16+length])
        pos += 16 + length
    assert pos == len(b)
    return out


def arp():
    return (b'\xff'*6 + HOST + b'\x08\x06' +
            struct.pack('!HHBBH', 1, 0x800, 6, 4, 1) + HOST + HOST_IP + bytes(6) + IP).ljust(60, b'\0')


def echo(size, options=b''):
    body = bytes((i * 13 + size) % 256 for i in range(size))
    icmp = struct.pack('!BBHHH', 8, 0, 0, 0x1234, size) + body
    icmp = icmp[:2] + struct.pack('!H', checksum(icmp)) + icmp[4:]
    ip = bytearray(struct.pack('!BBHHHBBH4s4s', 0x45+len(options)//4, 0,
        20+len(options)+len(icmp), size, 0, 64, 1, 0, HOST_IP, IP) + options)
    struct.pack_into('!H', ip, 10, checksum(bytes(ip)))
    return (MAC + HOST + b'\x08\x00' + bytes(ip) + icmp).ljust(60, b'\0')


def fixtures():
    valid = [arp()] + [echo(n) for n in [0, 1, 55, 56, 57, 63, 64, 65, 1472]]
    valid += [echo(57, b'\x01\x01\0\0'), echo(57, bytes(40))]
    bad = [bytes(13)]
    for offset, value in [(0, 0), (14, 0x44), (22, 0), (24, 1), (42, 1), (20, 0x20)]:
        f = bytearray(echo(3))
        f[offset] = value
        if offset in [20, 22]:
            f[24:26] = bytes(2)
            f[24:26] = struct.pack('!H', checksum(bytes(f[14:34])))
        bad.append(bytes(f))
    bad += [echo(3, b'\x07\x01\0\0'), echo(3)[:40]]
    return valid, bad


def check():
    valid, bad = fixtures()
    out = read_pcap(LOG/'network-output.pcap')
    assert len(out) == len(valid), len(out)
    for original, reply in zip(valid, out):
        assert reply[:6] == HOST and reply[6:12] == MAC
        if original[12:14] == b'\x08\x06':
            assert reply[12:22] == bytes.fromhex('08060001080006040002')
            assert reply[22:42] == MAC + IP + HOST + HOST_IP
        else:
            ihl = (original[14] & 15)*4
            length = int.from_bytes(original[16:18], 'big')-ihl
            assert reply[12:16] == b'\x08\0\x45\0'
            assert reply[26:34] == IP+HOST_IP
            assert int.from_bytes(reply[16:18], 'big') == length+20
            assert checksum(reply[14:34]) == checksum(reply[34:34+length]) == 0
            assert reply[34:36] == bytes(2)
            assert reply[38:34+length] == original[14+ihl+4:14+ihl+length]
    r = json.loads((LOG/'network-run.json').read_text())
    ram = bytes.fromhex(r['ram_hex'])
    symbols = r['symbols']

    def data(name, size):
        offset = symbols[name]-0x20000000
        assert 0 <= offset <= 2048-size
        return ram[offset:offset+size]

    def word(name):
        return int.from_bytes(data(name, 4), 'little')

    assert r['reason'] == 'Halted' and r['uart'] == 'SERVICES OK\n', r
    assert word('FAULT_INFO') == 0 and word('SERVICES_STATUS') == 0x600d
    usage = struct.unpack('<III', data('STACK_USED', 12))
    assert all(80 <= n <= 240 for n in usage), usage
    for address in [symbols['TASK_STACKS'], symbols['TASK_STACKS']+256, symbols['_stack_bottom']]:
        offset = address-0x20000000
        assert ram[offset:offset+16] == bytes.fromhex('a5a5a5a5')*4
    counters = struct.unpack('<6H', data('NET_COUNTERS', 12))
    assert counters[0:3] == (len(valid)+len(bad), len(valid), len(bad)), counters
    assert counters[5] == 0
    assert r['injected'] == r['released'] == len(valid)+len(bad)
    assert r['transmitted'] == len(valid)
    assert word('TIMER_IRQS') > 0 and word('TRAPS') > 2
    summary = dict(instructions=r['instructions'], frames_in=r['injected'], frames_out=len(out),
        stats_rx_tx_drop_bad_checksum_fragment_io=counters, stack_used_bytes_tasks_kernel=usage,
        static_ram_bytes=symbols['_ebss']-0x20000000, kernel_stack_reserved_bytes=256,
        syscalls=word('TRAPS'), timer_irqs=word('TIMER_IRQS'),
        mac_live_ping='UNVERIFIED', physical_enc28j60='UNVERIFIED')
    (LOG/'network-summary.json').write_text(json.dumps(summary, indent=2)+'\n')
    print('PASS: RV32EC ARP/IPv4/ICMP PCAP, stack guards and OS waits:', summary)


if sys.argv[1:] == ['generate']:
    LOG.mkdir(exist_ok=True, parents=True)
    valid, bad = fixtures()
    write_pcap(LOG/'network-input.pcap', valid+bad)
elif sys.argv[1:] == ['check']:
    check()
else:
    raise SystemExit('usage: network-fixtures.py generate|check')
