"""Independent PCAP, guest memory, stack and echo oracle for the TCP probe."""
import json
import struct
from pathlib import Path

LOG=Path('docs/logs')
r=json.loads((LOG/'tcp-run.json').read_text())
ram=bytes.fromhex(r['ram_hex']);symbols=r['symbols']

def data(name,size):
    offset=symbols[name]-0x20000000
    assert 0<=offset<=2048-size
    return ram[offset:offset+size]

def integer(name,size=4):
    return int.from_bytes(data(name,size),'little')

def pcap(path):
    b=path.read_bytes()
    assert b[:24]==struct.pack('<IHHIIII',0xa1b2c3d4,2,4,0,0,65535,1)
    pos=24;frames=[]
    while pos<len(b):
        _,_,size,original=struct.unpack_from('<IIII',b,pos)
        assert size==original and 14<=size<=1514
        pos+=16;frames.append(b[pos:pos+size]);pos+=size
    assert pos==len(b)
    return frames

def checksum(b):
    if len(b)%2:b+=b'\0'
    total=sum(struct.unpack('!'+'H'*(len(b)//2),b))
    while total>65535:total=(total&65535)+(total>>16)
    return (~total)&65535

def tcp(f):
    assert f[12:14]==b'\x08\0' and f[23]==6
    ihl=(f[14]&15)*4;total=int.from_bytes(f[16:18],'big')
    assert 20<=ihl<=60 and total>=ihl+20 and total+14<=len(f)
    assert checksum(f[14:14+ihl])==0
    t=f[14+ihl:14+total];size=(t[12]>>4)*4
    assert 20<=size<=60 and size<=len(t)
    pseudo=f[26:34]+b'\0\x06'+len(t).to_bytes(2,'big')+t
    assert checksum(pseudo)==0
    source,dest,seq,ack=struct.unpack_from('!HHII',t)
    return source,dest,seq,ack,t[13],t[size:]

assert r['reason']=='Halted' and r['uart']=='SERVICES OK\n'
assert integer('FAULT_INFO')==0 and integer('SERVICES_STATUS')==0x600d
usage=struct.unpack('<III',data('STACK_USED',12))
assert all(80<=used<=240 for used in usage),usage
for address in [symbols['TASK_STACKS'],symbols['TASK_STACKS']+256,symbols['_stack_bottom']]:
    offset=address-0x20000000
    assert ram[offset:offset+16]==bytes.fromhex('a5a5a5a5')*4
assert integer('APP_BYTES')==193 and integer('APP_LISTENS')==3
assert integer('APP_MEM_CHECK')==1
assert integer('APP_GP_CHECK')==1
assert integer('UPTIME',8)>=6500 and integer('TIMER_IRQS')>0
probe=r['tcp_probe']
assert probe['done'] and probe['connections']==2 and probe['echoed_bytes']==193
assert probe['syn_loss_recovered'] and probe['data_ack_loss_recovered'] and probe['fin_ack_loss_recovered']
assert probe['advanced_ticks']==6500
incoming=pcap(LOG/'tcp-output.rx.pcap');outgoing=pcap(LOG/'tcp-output.pcap')
assert r['injected']==r['released']==len(incoming)
assert r['transmitted']==len(outgoing) and r['rx_overflow']==0
counters=struct.unpack('<6H',data('NET_COUNTERS',12))
assert counters==(len(incoming),len(outgoing),0,0,0,0),counters
for f in incoming:tcp(f)
syn={};payload={50000:{},50001:{}};syn_count={50000:0,50001:0};fin_count={50000:0,50001:0};duplicates=0
for f in outgoing:
    source,port,seq,ack,flags,body=tcp(f)
    assert source==1234 and port in payload and flags&4==0
    if flags&2:
        assert flags&16
        if port in syn:assert syn[port]==(seq+1)&0xffffffff
        syn[port]=(seq+1)&0xffffffff;syn_count[port]+=1
    if body:
        assert len(body)<=24
        offset=(seq-syn[port])&0xffffffff
        for i,b in enumerate(body):
            if offset+i in payload[port]:assert payload[port][offset+i]==b;duplicates+=1
            payload[port][offset+i]=b
    if flags&1:fin_count[port]+=1
for port,size in [(50000,65),(50001,128)]:
    assert sorted(payload[port])==list(range(size))
    assert bytes(payload[port][i] for i in range(size))==bytes((i*17+3)%256 for i in range(size))
assert syn_count[50000]>=2 and fin_count[50000]>=2 and duplicates>0
summary={'host_packet_peer':'PASS (synthetic peer, not native Mac TCP)',
    'public_mem_snapshot_checked':True,
    'gp_preserved_across_timer_irqs_and_syscall':True,
    'instructions':r['instructions'],'frames_in':len(incoming),'frames_out':len(outgoing),
    'connections':2,'echoed_bytes':193,'syn_data_fin_loss_recovered':True,
    'out_of_order_and_duplicate_input':True,'sequence_wrap':True,'negotiated_peer_mss':24,
    'stack_used_bytes_tasks_kernel':usage,'syscalls':integer('TRAPS'),
    'timer_irqs':integer('TIMER_IRQS'),'uptime_ticks':integer('UPTIME',8),
    'simulated_clock_advance_ticks':6500,'mac_native_tcp':'UNVERIFIED','physical_enc':'UNVERIFIED'}
(LOG/'tcp-summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print('PASS: guest TCP echo/socket calls, PCAP checksums, loss and stack guards:',summary)
