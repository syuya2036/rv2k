"""Independent PCAP/SRAM oracle; Python's HTTP client parses guest responses."""
import http.client
import io
import json
import struct
import sys
from pathlib import Path

LOG=Path(sys.argv[1]) if len(sys.argv)>1 else Path('docs/logs/http')
r=json.loads((LOG/'http-run.json').read_text());symbols=r['symbols'];ram=bytes.fromhex(r['ram_hex'])
def integer(name,size=4):
    start=symbols[name]-0x20000000
    assert 0<=start<=2048-size
    return int.from_bytes(ram[start:start+size],'little')
def checksum(b):
    if len(b)%2:b+=b'\0'
    n=sum(struct.unpack('!'+str(len(b)//2)+'H',b))
    while n>65535:n=(n&65535)+(n>>16)
    return (~n)&65535
def pcap(path):
    b=path.read_bytes();assert b[:24]==struct.pack('<IHHIIII',0xa1b2c3d4,2,4,0,0,65535,1)
    pos=24;result=[]
    while pos<len(b):
        _,_,size,original=struct.unpack_from('<IIII',b,pos);pos+=16
        assert size==original and 14<=size<=1514
        f=b[pos:pos+size];pos+=size;assert len(f)==size
        assert f[12:14]==b'\x08\0' and f[14]>>4==4 and f[23]==6
        ihl=(f[14]&15)*4;total=int.from_bytes(f[16:18],'big')
        assert 20<=ihl<=60 and total>=ihl+20 and 14+total<=len(f)
        assert checksum(f[14:14+ihl])==0
        t=f[14+ihl:14+total];header=(t[12]>>4)*4
        assert 20<=header<=60 and header<=len(t) and t[12]&15==0
        assert checksum(f[26:34]+b'\0\x06'+len(t).to_bytes(2,'big')+t)==0
        src,dst,seq,ack=struct.unpack_from('!HHII',t)
        result.append((src,dst,seq,ack,t[13],int.from_bytes(t[14:16],'big'),t[header:]))
    assert pos==len(b);return result
incoming=pcap(LOG/'http-output.rx.pcap');outgoing=pcap(LOG/'http-output.pcap')
assert r['reason']=='Halted' and r['uart']=='SERVICES OK\n'
assert integer('FAULT_INFO')==0 and integer('SERVICES_STATUS')==0x600d
assert integer('APP_MEM_CHECK')==1 and integer('APP_GP_CHECK')==1
start=symbols['STACK_USED']-0x20000000;used=list(struct.unpack_from('<3I',ram,start))
assert all(80<=n<=240 for n in used),used
for name,delta in [('TASK_STACKS',0),('TASK_STACKS',256),('_stack_bottom',0)]:
    start=symbols[name]+delta-0x20000000
    assert ram[start:start+16]==bytes.fromhex('a5a5a5a5')*4
assert r['injected']==r['released']==len(incoming) and r['transmitted']==len(outgoing)
assert r['rx_overflow']==0 and integer('PREEMPT_STATE')==0
assert integer('PREEMPT_SWITCHES')>0 and integer('PREEMPT_BLOCKED_CALLS')==0
start=symbols['NET_COUNTERS']-0x20000000
assert struct.unpack_from('<6H',ram,start)==(len(incoming),len(outgoing),0,0,0,0)
expected={
 'fragmented-loss':(200,b'hello rv2k\0\r\n'),'empty':(200,b''),
 'maximum-binary':(200,bytes((i*17+3)%256 for i in range(64))),
 'oversized':(413,b''),'missing-length':(411,b''),'duplicate-length':(400,b''),
 'chunked':(400,b''),'folded':(400,b''),'missing-host':(400,b''),'expect':(417,b''),
 'long-header':(431,b''),'incomplete-eof':(400,b''),'partial-timeout':(408,b''),
 'empty-timeout':(408,b''),'slow-progress-timeout':(408,b''),
 'reset-in-header':(None,b''),'after-reset':(200,b'reconnected')}
peer=r['tcp_probe'];cases=peer['cases']
assert peer['done'] and peer['connections']==len(expected)==len(cases)
assert {c['name'] for c in cases}==set(expected)
assert peer['syn_loss_recovered'] and peer['data_ack_loss_recovered'] and peer['fin_ack_loss_recovered']
assert integer('HTTP_ACCEPTS')==len(cases) and integer('HTTP_RESPONSES')==len(cases)-1
assert integer('APP_LISTENS')==len(cases)+1 and integer('APP_BYTES')==88
class Socket:
    def __init__(self,data):self.data=data
    def makefile(self,mode):return io.BytesIO(self.data)
def stream(frames,port,direction):
    packets=[p for p in frames if (p[0] if direction=='in' else p[1])==port]
    syn=[p for p in packets if p[4]&2];assert syn
    base=(syn[0][2]+1)&0xffffffff;data={};duplicates=0
    for p in packets:
        assert p[1 if direction=='in' else 0]==80
        if direction=='out':assert p[4]&4==0 and len(p[6])<=24
        for i,b in enumerate(p[6]):
            offset=((p[2]-base)&0xffffffff)+i
            if offset in data:assert data[offset]==b;duplicates+=1
            data[offset]=b
    assert sorted(data)==list(range(len(data)))
    return bytes(data[i] for i in range(len(data))),packets,duplicates
duplicates=0
for index,c in enumerate(cases):
    assert c['port']==52000+index
    code,body=expected[c['name']]
    request,ins,_=stream(incoming,c['port'],'in')
    response,outs,repeated=stream(outgoing,c['port'],'out');duplicates+=repeated
    assert request==bytes.fromhex(c['request_hex'])
    assert response==bytes.fromhex(c['response_hex'])
    assert c['status']==code and bytes.fromhex(c['expected_body_hex'])==body
    if code is None:
        assert not response and any(p[4]&4 for p in ins)
        continue
    parsed=http.client.HTTPResponse(Socket(response));parsed.begin()
    assert parsed.version==11 and parsed.status==code
    assert len(parsed.headers.get_all('Content-Length'))==1 and parsed.length==len(body)
    assert parsed.getheader('Connection').lower()=='close'
    assert parsed.getheader('Content-Type')=='application/octet-stream'
    assert parsed.getheader('Transfer-Encoding') is None
    assert len(response)==response.index(b'\r\n\r\n')+4+len(body)
    assert parsed.read()==body
    assert any(p[4]&1 for p in ins) and any(p[4]&1 for p in outs)
    if c['request_timeout']:
        # Stress adds one peripheral tick while reading every request frame;
        # these can overshoot the deliberate 501-tick deadline jump. A timer
        # incorrectly renewed by progress would instead wait about 750 ticks.
        injected=sum(bool(p[6]) for p in ins) if r['mode']=='http-preempt' else 0
        assert 500<=c['read_ticks']<=503+injected,c
    if c['name']=='maximum-binary':
        assert c['persist_seen'] and any(p[5]==0 for p in ins)
    if index==0:assert sum(bool(p[4]&2) for p in outs)>=2 and sum(bool(p[4]&1) for p in outs)>=2
assert duplicates>0
if r['mode']=='http-preempt':
    assert r['nic_irq_injections']==len(incoming)
    assert integer('PREEMPT_DEFERRED')>=len(incoming)
summary={'connections':len(cases),'responses':len(cases)-1,'echoed_body_bytes':88,
 'zero_and_64_byte_binary_bodies':True,'fragmented_headers_body':True,
 'explicit_error_statuses':[400,408,411,413,417,431],
 'syn_data_fin_loss_recovered':True,'zero_window_persist_recovered':True,
 'eof_reset_reconnect_and_absolute_timeout':True,'pcap_frames_in_out':[len(incoming),len(outgoing)],
 'instructions':r['instructions'],'stack_used_bytes_tasks_kernel':used,
 'forced_switches':integer('PREEMPT_SWITCHES'),'deferred_irqs':integer('PREEMPT_DEFERRED'),
 'max_disabled_clocks':integer('PREEMPT_MAX_CLOCKS') if 'PREEMPT_MAX_CLOCKS' in symbols else None,'nic_irq_injections':r['nic_irq_injections'],
 'mac_native_http':'UNVERIFIED','physical_enc':'UNVERIFIED'}
(LOG/'http-summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print('PASS: independent HTTP/PCAP/socket/stack oracle:',summary)
