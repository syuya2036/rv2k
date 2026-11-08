"""Run real guest faults; verify recorded cause/PC/SP and bounded diagnostics."""
import json
import os
import subprocess
from pathlib import Path

logs = Path('docs/logs')
output = Path('target/diagnostic-faults')
output.mkdir(parents=True, exist_ok=True)
env = dict(os.environ, RUSTFLAGS='-C link-arg=--defsym=_kernel_stack_size=256 -C link-arg=-Tmemory.x')
sysroot = subprocess.check_output(['rustc','--print','sysroot'],text=True).strip()
host = next(x[6:] for x in subprocess.check_output(['rustc','-vV'],text=True).splitlines() if x.startswith('host: '))
nm = Path(sysroot)/'lib/rustlib'/host/'bin/llvm-nm'
results = []
for feature, expected, trap, value in [
    ('fault-irq-canary',2,0x8000000c,1), ('fault-no-debugger',1,0,0), ('fault-canary',2,11,1), ('fault-kernel-canary',2,11,4),
    ('fault-panic',1,0,0), ('fault-illegal',4,2,0xffffffff),
    ('fault-kernel-illegal',5,2,0xffffffff), ('fault-sp',3,11,0),
    ('fault-uart-stuck',1,0,0),
]:
    subprocess.run(['cargo','build','--locked','--release','--features',('fault-panic' if feature=='fault-no-debugger' else feature+',emulator-stop'),'--target-dir',str(output)],env=env,check=True)
    elf = output/'riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test'
    subprocess.run(['python3','scripts/elf-for-remu.py',str(elf),str(elf)+'.remu'],check=True)
    bus_path=output/(feature+'-bus.json'); run_path=logs/(feature+'-run.json')
    subprocess.run(['vendor/renvo_emu/target/release/remu','run','--target','ch32v003','--elf',str(elf)+'.remu',
                    '--max-instructions',('100000' if feature=='fault-no-debugger' else '2000000'),'--result',str(run_path),
                    '--bus-log',str(bus_path),'--bus-log-region','sram'],check=True)
    run=json.loads(run_path.read_text()); bus=json.loads(bus_path.read_text()); ram=bytearray(2048)
    for access in bus:
        if access['kind']!='Write':continue
        offset=access['address']-0x20000000
        width={'Byte':1,'HalfWord':2,'Word':4}[access['width']]
        assert 0<=offset<=2048-width
        ram[offset:offset+width]=access['value'].to_bytes(width,'little')
    symbols={f[3]:int(f[0],16) for line in subprocess.check_output([str(nm),'-S',str(elf)],text=True).splitlines() if len(f:=line.split())==4}
    offset=symbols['FAULT_INFO']-0x20000000
    reason,task,pc,cause,tval,sp=[int.from_bytes(ram[offset+i:offset+i+4],'little') for i in range(0,24,4)]
    assert run['reason']==('InstructionLimit' if feature=='fault-no-debugger' else 'Halted') and reason==expected,(feature,run['reason'],reason)
    assert cause==trap and tval==value,(feature,cause,tval)
    assert task==(0xffffffff if feature=='fault-kernel-illegal' else 0), (feature,task)
    if trap: assert 0<pc<16384,(feature,pc)
    if feature=='fault-no-debugger': assert sum(x['kind']=='Write' and x['address']==symbols['FAULT_INFO'] and x['value']==1 for x in bus)==1
    if feature=='fault-irq-canary': assert symbols['fault_irq_wait'] <= pc < symbols['fault_irq_wait']+8
    if feature=='fault-sp':
        assert sp==0
        marker=max(i for i,x in enumerate(bus) if x['kind']=='Write' and x['address']==symbols['_stack_top']-24 and x['value']==0)
        base=symbols['TASK_STACKS']
        assert all(not base<=x['address']<base+512 for x in bus[marker+1:] if x['kind']=='Write')
    else: assert 0x20000000<=sp<=0x20000800
    uart=bytes(run['uart'])
    if feature=='fault-uart-stuck':assert uart==b''
    else:assert uart.startswith(b'FAULT ') and f'{reason:08x}'.encode() in uart,uart
    results.append(dict(case=feature,reason=reason,task=task,pc=pc,cause=cause,tval=tval,sp=sp,instructions=run['stats']['instructions']))
(logs/'diagnostics-negative-summary.json').write_text(json.dumps(results,indent=2)+'\n')
print('PASS: diagnostic fault injections:',results)
