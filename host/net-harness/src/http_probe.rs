//! Independent HTTP packet peer. Expected requests/responses are not generated
//! by rv2k-apps or rv2k-net; only Ethernet/TCP test packet helpers are shared.
use crate::{
    peer::Observation,
    tcp_probe::{frame_to, packet},
};
use std::collections::VecDeque;
const FIN: u8 = 1;
const SYN: u8 = 2;
const RST: u8 = 4;
const ACK: u8 = 16;
struct Case {
    name: &'static str,
    request: Vec<u8>,
    status: Option<u16>,
    body: Vec<u8>,
    chunk: usize,
    timeout: bool,
    reset: bool,
    eof: bool,
    zero: bool,
    slow: bool,
}
fn normal(name: &'static str, body: Vec<u8>, chunk: usize) -> Case {
    let mut request = format!(
        "POST /echo HTTP/1.1\r\nHost: 192.168.123.50\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    request.extend_from_slice(&body);
    Case {
        name,
        request,
        status: Some(200),
        body,
        chunk,
        timeout: false,
        reset: false,
        eof: false,
        zero: false,
        slow: false,
    }
}
fn bad(name: &'static str, request: &[u8], status: u16) -> Case {
    Case {
        name,
        request: request.to_vec(),
        status: Some(status),
        body: vec![],
        chunk: 64,
        timeout: false,
        reset: false,
        eof: false,
        zero: false,
        slow: false,
    }
}
fn cases() -> Vec<Case> {
    let mut maximum = normal(
        "maximum-binary",
        (0..64).map(|i| (i * 17 + 3) as u8).collect(),
        7,
    );
    maximum.zero = true;
    let mut oversized = normal("oversized", vec![b'x'; 65], 128);
    oversized.status = Some(413);
    oversized.body.clear();
    let mut eof = bad(
        "incomplete-eof",
        b"POST /echo HTTP/1.1\r\nHost: a\r\nContent-Length: 5\r\n\r\nab",
        400,
    );
    eof.eof = true;
    let mut timeout = bad(
        "partial-timeout",
        b"POST /echo HTTP/1.1\r\nHost: a\r\nContent-Length: 5\r\n\r\nab",
        408,
    );
    timeout.timeout = true;
    let mut empty_timeout = bad("empty-timeout", b"", 408);
    empty_timeout.timeout = true;
    let mut slow = bad(
        "slow-progress-timeout",
        b"POST /echo HTTP/1.1\r\nHost: a\r\nContent-Length: 5\r\n\r\nabc",
        408,
    );
    slow.timeout = true;
    slow.slow = true;
    let mut reset = bad("reset-in-header", b"POST /echo HTTP/1.1\r\nHost:", 400);
    reset.reset = true;
    reset.status = None;
    let mut long = b"POST /echo HTTP/1.1\r\nHost: a\r\nX:".to_vec();
    long.extend_from_slice(&[b'a'; 65]);
    long.extend_from_slice(b"\r\nContent-Length: 0\r\n\r\n");
    vec![normal("fragmented-loss",b"hello rv2k\0\r\n".to_vec(),7),normal("empty",vec![],128),maximum,oversized,
        bad("missing-length",b"POST /echo HTTP/1.1\r\nHost: a\r\n\r\n",411),
        bad("duplicate-length",b"POST /echo HTTP/1.1\r\nHost: a\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n",400),
        bad("chunked",b"POST /echo HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\nContent-Length: 0\r\n\r\n0\r\n\r\n",400),
        bad("folded",b"POST /echo HTTP/1.1\r\nHost: a\r\n Content-Length: 0\r\n\r\n",400),
        bad("missing-host",b"POST /echo HTTP/1.1\r\nContent-Length: 0\r\n\r\n",400),
        bad("expect",b"POST /echo HTTP/1.1\r\nHost: a\r\nExpect: 100-continue\r\nContent-Length: 0\r\n\r\n",417),
        bad("long-header",&long,431),eof,timeout,empty_timeout,slow,reset,normal("after-reset",b"reconnected".to_vec(),3)]
}
pub struct Probe {
    pub queue: VecDeque<Vec<u8>>,
    pub inputs: Vec<Vec<u8>>,
    cases: Vec<Case>,
    index: usize,
    waiting: bool,
    done: bool,
    started: bool,
    advance: u32,
    advanced: u32,
    request_baseline: u32,
    client_next: u32,
    server_start: u32,
    server_next: u32,
    request_pos: usize,
    inflight: bool,
    response: Vec<u8>,
    response_complete: bool,
    fin_sent: bool,
    fin_seen: bool,
    timer_armed: bool,
    syn_dropped: bool,
    data_dropped: bool,
    fin_dropped: bool,
    syn_recovered: bool,
    data_recovered: bool,
    fin_recovered: bool,
    zero_started: bool,
    zero_holding: bool,
    persist_seen: bool,
    last_elapsed: u32,
    results: Vec<serde_json::Value>,
    after_release: Option<(u32, u32)>,
    slow_stage: bool,
    window: u16,
}
impl Probe {
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            inputs: vec![],
            cases: cases(),
            index: 0,
            waiting: true,
            done: false,
            started: false,
            advance: 0,
            advanced: 0,
            request_baseline: 0,
            client_next: 0,
            server_start: 0,
            server_next: 0,
            request_pos: 0,
            inflight: false,
            response: vec![],
            response_complete: false,
            fin_sent: false,
            fin_seen: false,
            timer_armed: false,
            syn_dropped: false,
            data_dropped: false,
            fin_dropped: false,
            syn_recovered: false,
            data_recovered: false,
            fin_recovered: false,
            zero_started: false,
            zero_holding: false,
            persist_seen: false,
            last_elapsed: 0,
            results: vec![],
            after_release: None,
            slow_stage: false,
            window: 128,
        }
    }
    fn port(&self) -> u16 {
        52000 + self.index as u16
    }
    fn enqueue(&mut self, seq: u32, flags: u8, window: u16, body: &[u8]) {
        let frame = frame_to(self.port(), 80, seq, self.server_next, flags, window, body);
        self.inputs.push(frame.clone());
        self.queue.push_back(frame);
    }
    pub fn needs_ready(&self) -> bool {
        !self.done
    }
    pub fn done(&self) -> bool {
        self.done
    }
    pub fn observe(&mut self, o: Observation) {
        if self.done {
            return;
        }
        if self.after_release.is_some_and(|(n, _)| o.released >= n) {
            self.advance = self.after_release.take().unwrap().1;
        }
        if self.waiting {
            if o.irqs == 0 || o.listens <= self.index as u32 {
                return;
            }
            if self.index == self.cases.len() {
                self.done = true;
                self.waiting = false;
                return;
            }
            self.request_baseline = o.request_bytes;
            if !self.started {
                self.advance = 6000;
                self.started = true;
            }
            self.client_next = (u32::MAX - 17).wrapping_add(self.index as u32 * 4096);
            self.server_start = 0;
            self.server_next = 0;
            self.request_pos = 0;
            self.inflight = false;
            self.response.clear();
            self.response_complete = false;
            self.fin_sent = false;
            self.fin_seen = false;
            self.timer_armed = false;
            self.slow_stage = false;
            self.zero_started = false;
            self.zero_holding = false;
            self.persist_seen = false;
            self.waiting = false;
            self.enqueue(self.client_next, SYN, 128, &[]);
            self.client_next = self.client_next.wrapping_add(1);
            return;
        }
        self.last_elapsed = o.last_read_ticks;
        let case = &self.cases[self.index];
        if case.slow
            && !self.slow_stage
            && o.accepts > self.index as u32
            && self.request_pos + 1 == case.request.len()
            && !self.inflight
            && o.request_bytes.wrapping_sub(self.request_baseline) >= self.request_pos as u32
        {
            self.slow_stage = true;
            self.advance = 250;
            self.send_request(self.window);
            return;
        }
        if (case.timeout || case.reset)
            && !self.timer_armed
            && o.accepts > self.index as u32
            && self.request_pos == case.request.len()
            && !self.inflight
            && o.request_bytes.wrapping_sub(self.request_baseline) >= case.request.len() as u32
        {
            self.timer_armed = true;
            if case.reset {
                self.enqueue(self.client_next, RST | ACK, 128, &[]);
                self.finish_case();
            } else {
                self.advance = if case.slow { 251 } else { 501 };
            }
        }
    }
    fn send_request(&mut self, window: u16) {
        let case = &self.cases[self.index];
        let limit = case.request.len() - usize::from(case.slow && !self.slow_stage);
        if self.inflight || self.fin_sent || self.request_pos == limit || window == 0 {
            return;
        }
        let case = &self.cases[self.index];
        let count = case
            .chunk
            .min(window as usize)
            .min(limit - self.request_pos);
        let data = case.request[self.request_pos..self.request_pos + count].to_vec();
        self.enqueue(self.client_next, ACK, 128, &data);
        self.client_next = self.client_next.wrapping_add(count as u32);
        self.request_pos += count;
        self.inflight = true;
    }
    fn maybe_fin(&mut self) {
        let case = &self.cases[self.index];
        if !self.fin_sent
            && !self.inflight
            && self.request_pos == case.request.len()
            && (self.response_complete || case.eof)
        {
            self.enqueue(self.client_next, ACK | FIN, 128, &[]);
            self.client_next = self.client_next.wrapping_add(1);
            self.fin_sent = true;
        }
    }
    fn response_ready(&mut self) -> Result<(), String> {
        let Some(end) = self.response.windows(4).position(|b| b == b"\r\n\r\n") else {
            return Ok(());
        };
        let header =
            std::str::from_utf8(&self.response[..end]).map_err(|_| "non-ASCII response")?;
        let mut lines = header.split("\r\n");
        let line = lines.next().ok_or("missing status")?;
        let code = line
            .split_whitespace()
            .nth(1)
            .ok_or("missing code")?
            .parse::<u16>()
            .map_err(|_| "invalid code")?;
        let fields: Vec<_> = lines.filter_map(|s| s.split_once(':')).collect();
        let lengths: Vec<_> = fields
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .collect();
        if lengths.len() != 1 {
            return Err("missing/duplicate response length".into());
        }
        let len = lengths[0]
            .1
            .trim()
            .parse::<usize>()
            .map_err(|_| "invalid response length")?;
        if !fields.iter().any(|(k, v)| {
            k.eq_ignore_ascii_case("connection") && v.trim().eq_ignore_ascii_case("close")
        }) {
            return Err("response is not Connection: close".into());
        }
        let case = &self.cases[self.index];
        if Some(code) != case.status || len != case.body.len() {
            return Err(format!("{}: status/length {code}/{len}", case.name));
        }
        if self.response.len() > end + 4 + len {
            return Err("response has extra bytes".into());
        }
        if self.response.len() == end + 4 + len {
            if self.response[end + 4..] != case.body {
                return Err("HTTP body mismatch".into());
            }
            self.response_complete = true;
        }
        Ok(())
    }
    pub fn receive(&mut self, f: &[u8]) -> Result<(), String> {
        if f.len() < 38 {
            return Err("short HTTP peer frame".into());
        }
        let port = u16::from_be_bytes(f[36..38].try_into().unwrap());
        let p = packet(f, port, 80)?;
        if port != self.port() {
            if port >= 52000 && port < self.port() && p.flags == ACK && p.body.is_empty() {
                return Ok(());
            }
            return Err("HTTP peer received an unexpected tuple".into());
        }
        self.window = p.window;
        if p.flags & RST != 0 {
            return Err("HTTP connection reset by guest".into());
        }
        if p.flags & SYN != 0 {
            if p.flags & (SYN | ACK) != (SYN | ACK) || p.ack != self.client_next {
                return Err("HTTP SYN ACK mismatch".into());
            }
            if self.index == 0 && !self.syn_dropped {
                self.syn_dropped = true;
                self.advance = 100;
                return Ok(());
            }
            if self.index == 0 {
                self.syn_recovered = true;
            }
            self.server_start = p.seq.wrapping_add(1);
            self.server_next = self.server_start;
            self.enqueue(self.client_next, ACK, 128, &[]);
            self.send_request(p.window);
            self.maybe_fin();
            return Ok(());
        }
        if p.ack == self.client_next {
            self.inflight = false;
        }
        if !p.body.is_empty() {
            if p.body.len() > 24 {
                return Err("HTTP peer MSS exceeded".into());
            }
            if self.zero_holding {
                if p.body.len() != 1 || p.seq != self.server_next {
                    return Err("invalid zero-window persist probe".into());
                }
                self.persist_seen = true;
                self.zero_holding = false;
                self.enqueue(self.client_next, ACK, 128, &[]);
                self.after_release = Some((self.inputs.len() as u32, 100));
                return Ok(());
            }
            let offset = p.seq.wrapping_sub(self.server_start) as usize;
            if p.seq == self.server_next {
                self.response.extend_from_slice(p.body);
                self.server_next = self.server_next.wrapping_add(p.body.len() as u32);
            } else if offset + p.body.len() <= self.response.len()
                && self.response[offset..offset + p.body.len()] == *p.body
            {
                if self.index == 0 && self.data_dropped {
                    self.data_recovered = true;
                }
            } else {
                return Err("HTTP response sequence or retransmission differs".into());
            }
            self.response_ready()?;
            if self.index == 0 && !self.data_dropped {
                self.data_dropped = true;
                self.advance = 100;
                return Ok(());
            }
            if self.cases[self.index].zero && !self.zero_started {
                self.zero_started = true;
                self.zero_holding = true;
                self.enqueue(self.client_next, ACK, 0, &[]);
                self.after_release = Some((self.inputs.len() as u32, 100));
                return Ok(());
            }
            self.enqueue(self.client_next, ACK, 128, &[]);
        }
        self.send_request(p.window);
        self.maybe_fin();
        if p.flags & FIN != 0 {
            if !self.fin_sent || !self.response_complete {
                return Err("HTTP guest FIN before complete response".into());
            }
            if !self.fin_seen {
                if p.seq != self.server_next {
                    return Err("HTTP FIN sequence".into());
                }
                self.server_next = self.server_next.wrapping_add(1);
                self.fin_seen = true;
            } else if p.seq.wrapping_add(1) != self.server_next {
                return Err("HTTP FIN repeat sequence".into());
            }
            if self.index == 0 && !self.fin_dropped {
                self.fin_dropped = true;
                self.advance = 100;
                return Ok(());
            }
            if self.index == 0 {
                self.fin_recovered = true;
            }
            self.enqueue(self.client_next, ACK, 128, &[]);
            self.finish_case();
        }
        Ok(())
    }
    fn finish_case(&mut self) {
        let c = &self.cases[self.index];
        self.results.push(serde_json::json!({"name":c.name,"port":self.port(),"status":c.status,"expected_body_hex":hex(&c.body),"request_hex":hex(&c.request),"response_hex":hex(&self.response),"persist_seen":self.persist_seen,"request_timeout":c.timeout,"read_ticks":self.last_elapsed}));
        self.index += 1;
        self.waiting = true;
    }
    pub fn take_advance(&mut self) -> u32 {
        let n = core::mem::take(&mut self.advance);
        self.advanced += n;
        n
    }
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({"done":self.done,"connections":self.index,"cases":self.results,"syn_loss_recovered":self.syn_recovered,"data_ack_loss_recovered":self.data_recovered,"fin_ack_loss_recovered":self.fin_recovered,"advanced_ticks":self.advanced})
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
