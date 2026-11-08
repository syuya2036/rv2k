"""Actual guest failures for lost registers, advanced IRQ PC and broken locking."""
import json
import os
import re
import struct
import subprocess
from pathlib import Path

LOG=Path('docs/logs/preemption')
OUT=Path('target/preempt-negative')
OUT.mkdir(parents=True,exist_ok=True)
env=dict(os.environ,RUSTFLAGS='-C opt-level=z -C target-feature=+relax -C llvm-args=-riscv-enable-global-merge=false -C link-arg=--defsym=_kernel_stack_size=256 -C link-arg=-Tmemory.x -C link-arg=--icf=all -C link-arg=--relax -C link-arg=--relax-gp')
symbols={f[-1]:int(f[0],16) for line in (LOG/'probe-symbols.txt').read_text().splitlines() if len(f:=line.split()) in (3,4)}
elf=Path('target/preempt/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test')
binary=bytearray(elf.read_bytes())
restore=(LOG/'probe-disassembly.txt').read_text().split('<rv2k_restore>:',1)[1]
match=re.search(r'^\s*([0-9a-f]+):\s+([0-9a-f]+)\s+lw\s+t1,',restore,re.M)
assert match
pc,opcode=int(match[1],16),match[2]
width=len(opcode)//2
phoff=struct.unpack_from('<I',binary,28)[0]
size,count=struct.unpack_from('<HH',binary,42)
for i in range(count):
    kind,off,vma,lma,length,mem,flags,align=struct.unpack_from('<8I',binary,phoff+i*size)
    if kind==1 and vma<=pc<vma+length:
        offset=off+pc-vma
        assert binary[offset:offset+width]==int(opcode,16).to_bytes(width,'little')
        binary[offset:offset+width]=b'\x01\x00'*(width//2)
        break
else:raise AssertionError('restore instruction missing from ELF load records')
broken=OUT/'missing-t1.elf';broken.write_bytes(binary)
results=[]
for name in ['missing-t1','fault-preempt-pc','fault-preempt-lock']:
    if name=='missing-t1':
        current=broken
    else:
        subprocess.run(['cargo','build','--locked','--release','--features',name+',emulator-stop','--target-dir',str(OUT)],env=env,check=True)
        current=OUT/'riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test'
        sysroot=subprocess.check_output(['rustc','--print','sysroot'],text=True).strip()
        host=next(x[6:] for x in subprocess.check_output(['rustc','-vV'],text=True).splitlines() if x.startswith('host: '))
        nm=Path(sysroot)/'lib/rustlib'/host/'bin/llvm-nm'
        symbols={f[-1]:int(f[0],16) for line in subprocess.check_output([str(nm),'-S',str(current)],text=True).splitlines() if len(f:=line.split()) in (3,4)}
    subprocess.run(['python3','scripts/elf-for-remu.py',str(current),str(current)+'.remu'],check=True)
    run=LOG/(name+'-run.json');bus=OUT/(name+'-bus.json')
    subprocess.run(['vendor/renvo_emu/target/release/remu','run','--target','ch32v003','--elf',str(current)+'.remu','--max-instructions','20000000','--result',str(run),'--bus-log',str(bus),'--bus-log-region','sram'],check=True)
    r=json.loads(run.read_text());ram=bytearray(2048)
    for access in json.loads(bus.read_text()):
        if access['kind']!='Write':continue
        start=access['address']-0x20000000
        width={'Byte':1,'HalfWord':2,'Word':4}[access['width']]
        assert 0<=start<=2048-width
        ram[start:start+width]=access['value'].to_bytes(width,'little')
    def value(name):
        start=symbols[name]-0x20000000
        return int.from_bytes(ram[start:start+4],'little')
    assert r['reason']=='Halted' and value('FAULT_INFO')==1,(name,r['reason'],value('FAULT_INFO'))
    assert value('SERVICES_STATUS')!=0x600d
    assert b'SERVICES OK' not in bytes(r['uart'])
    results.append(dict(case=name,detected_panic=True,instructions=r['stats']['instructions']))
(LOG/'negative-summary.json').write_text(json.dumps(results,indent=2)+'\n')
print('PASS: preemption faults detected:',results)
