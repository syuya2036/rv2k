"""Check hardware ELF load spans, static SRAM, stacks and allocator absence."""
import json
import struct
import subprocess
import sys
from pathlib import Path

elf=Path(sys.argv[1]); binary=elf.read_bytes()
assert binary[:6]==b'\x7fELF\x01\x01'
assert struct.unpack_from('<H',binary,18)[0]==243
flags=struct.unpack_from('<I',binary,36)[0]
assert flags&9==9 and flags&6==0,hex(flags)
sysroot=subprocess.check_output(['rustc','--print','sysroot'],text=True).strip()
host=next(x[6:] for x in subprocess.check_output(['rustc','-vV'],text=True).splitlines() if x.startswith('host: '))
tools=Path(sysroot)/'lib/rustlib'/host/'bin'
symbol_text=subprocess.check_output([str(tools/'llvm-nm'),'-S',str(elf)],text=True)
symbols={f[3]:int(f[0],16) for line in symbol_text.splitlines() if len(f:=line.split())==4}
assert not any(x in symbol_text for x in ['__rust_alloc','__rust_realloc','__rust_dealloc'])
flash_end=0; ram_end=0x20000000
phoff=struct.unpack_from('<I',binary,28)[0]; ent,count=struct.unpack_from('<HH',binary,42)
for index in range(count):
    kind,off,vma,lma,size,mem,flags,align=struct.unpack_from('<8I',binary,phoff+index*ent)
    if kind!=1:continue
    assert off+size<=len(binary) and size<=mem
    if 0x20000000<=vma<0x20000800:
        assert vma+mem<=0x20000800
        ram_end=max(ram_end,vma+mem)
    if size:
        assert 0<=lma<16384 and lma+size<=16384,(hex(lma),size)
        flash_end=max(flash_end,lma+size)
assert flash_end==symbols['_flash_end']
assert ram_end<=symbols['_ebss']<=symbols['_stack_bottom']
reserved=symbols['_stack_top']-symbols['_stack_bottom']
assert reserved>=256 and reserved%16==0
assert symbols['_stack_top']==0x20000800
report=dict(flash_bytes=flash_end,flash_limit=16384,static_ram_bytes=symbols['_ebss']-0x20000000,
            kernel_stack_reserved_bytes=reserved,total_reserved_ram_bytes=symbols['_ebss']-0x20000000+reserved,ram_limit=2048)
assert report['total_reserved_ram_bytes']<=2048
if len(sys.argv)>2:Path(sys.argv[2]).write_text(json.dumps(report,indent=2)+'\n')
print('PASS: ELF budgets including load alignment and all stacks:',report)
