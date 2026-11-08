"""Retain hardware ELF, add PT_LOAD records for initialized Flash LMAs."""
import struct, sys
from pathlib import Path
b = bytearray(Path(sys.argv[1]).read_bytes())
assert b[:6] == b'\x7fELF\x01\x01'
off = struct.unpack_from('<I', b, 28)[0]
ent, count = struct.unpack_from('<HH', b, 42)
assert ent == 32
headers = [bytes(b[off+i*ent:off+(i+1)*ent]) for i in range(count)]
extra = []
for h in headers:
    kind, pos, vma, lma, size, mem, flags, align = struct.unpack('<8I', h)
    if kind == 1 and vma != lma and size:
        extra.append(struct.pack('<8I', 1, pos, lma, lma, size, size, 4, 1))
while len(b) % 4: b.append(0)
struct.pack_into('<I', b, 28, len(b))
struct.pack_into('<H', b, 44, count+len(extra))
b.extend(b''.join(headers+extra))
Path(sys.argv[2]).write_bytes(b)
