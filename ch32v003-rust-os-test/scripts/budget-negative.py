"""Real linker tests at the SRAM boundary, SRAM overflow and Flash overflow."""
import json
import subprocess
from pathlib import Path
sysroot=subprocess.check_output(['rustc','--print','sysroot'],text=True).strip()
host=next(x[6:] for x in subprocess.check_output(['rustc','-vV'],text=True).splitlines() if x.startswith('host: '))
tools=Path(sysroot)/'lib/rustlib'/host/'bin'
root=Path('target/budget-tests');root.mkdir(parents=True,exist_ok=True)
results=[]
for name,section,kind,size,ok in [('ram-boundary','.bss.budget','global',1792,True),
    ('ram-overflow','.bss.budget','global',1793,False),('flash-overflow','.rodata.budget','constant',16385,False)]:
    source=root/(name+'.ll');obj=root/(name+'.o')
    source.write_text(f'@budget = {kind} [{size} x i8] zeroinitializer, section "{section}", align 4\ndefine void @_start() {{ ret void }}\n')
    subprocess.run([str(tools/'llc'),'-mtriple=riscv32','-filetype=obj',str(source),'-o',str(obj)],check=True)
    result=subprocess.run([str(tools/'rust-lld'),'-flavor','gnu','--defsym=_kernel_stack_size=256','-Tmemory.x',str(obj),'-o',str(root/(name+'.elf'))],capture_output=True,text=True)
    assert (result.returncode==0)==ok,(name,result.stderr)
    if not ok:assert 'RAM exceeds budget' in result.stderr or 'overflow' in result.stderr or 'Flash exceeds' in result.stderr,result.stderr
    Path('docs/logs',name+'-link.log').write_text(result.stderr)
    results.append(dict(case=name,link_succeeded=result.returncode==0))
Path('docs/logs/budget-negative-summary.json').write_text(json.dumps(results,indent=2)+'\n')
print('PASS: real memory.x linker boundaries:',results)
