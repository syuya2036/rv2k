"""Bounded negative loader tests and GDB Remote Serial Protocol smoke test."""
import json, socket, struct, subprocess, time
from pathlib import Path
p = Path('docs/logs'); remu = 'vendor/renvo_emu/target/release/remu'
elf = Path('target/riscv32ec-unknown-none-elf/release/ch32v003-rust-os-test.remu')
base = elf.read_bytes()
for name, address, size in [('ram-overflow',0x20000000,2049),('flash-overflow',0,16385)]:
    b = bytearray(base)
    off = struct.unpack_from('<I',b,28)[0]
    ent,count = struct.unpack_from('<HH',b,42)
    for i in range(count):
        h = off+i*ent
        if struct.unpack_from('<I',b,h)[0] == 1 and struct.unpack_from('<I',b,h+8)[0] == address:
            struct.pack_into('<I',b,h+20,size)
            break
    f=Path('target')/(name+'.elf');f.write_bytes(b)
    r=subprocess.run([remu,'run','--target','ch32v003','--elf',str(f),'--max-instructions','10000'],capture_output=True,text=True)
    (p/(name+'.log')).write_text(f'exit={r.returncode}\n'+r.stdout+r.stderr)
    assert r.returncode != 0, name
    assert 'Unmapped bus fault' in r.stderr, r.stderr
ready=p/'gdb-ready.json';ready.unlink(missing_ok=True)
with (p/'gdb-console.log').open('w') as log:
    proc=subprocess.Popen([remu,'gdb','--target','ch32v003','--elf',str(elf),'--listen','127.0.0.1:0','--ready',str(ready),'--artifact',str(p/'gdb-session.json')],stdout=log,stderr=log)
    try:
        for _ in range(100):
            if ready.exists():break
            if proc.poll() is not None:raise RuntimeError('GDB server failed')
            time.sleep(.05)
        info=json.loads(ready.read_text());print(info)
        addr=info.get('listen',info.get('address'))
        if addr is None:
            addr=next(v for v in info.values() if isinstance(v,str) and v.startswith('127.0.0.1:'))
        host,port=addr.rsplit(':',1)
        with socket.create_connection((host,int(port)),timeout=5) as s:
            def packet(q):
                data=q.encode();s.sendall(b'$'+data+b'#'+f'{sum(data)%256:02x}'.encode())
                while s.recv(1)!=b'$':pass
                out=b''
                while True:
                    c=s.recv(1)
                    if c==b'#':break
                    if not c:raise RuntimeError('disconnect')
                    out+=c
                checksum=s.recv(2);assert int(checksum,16)==sum(out)%256
                s.sendall(b'+');return out.decode()
            result={q:packet(q) for q in ['?','g','m20000000,8','s','D']}
            assert result['g'] and not result['g'].startswith('E')
            assert result['s'].startswith(('S','T'))
            assert result['D']=='OK'
            (p/'gdb-rsp.json').write_text(json.dumps(result,indent=2)+'\n')
        proc.wait(timeout=5)
    finally:
        if proc.poll() is None:proc.terminate();proc.wait(timeout=5)
print('PASS: RAM/Flash overflow rejected; GDB RSP registers, memory, step, detach')
