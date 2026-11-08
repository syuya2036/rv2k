//! Independent shell TCP client; never writes guest CPU or protocol state.
use crate::{
    peer::Observation,
    tcp_probe::{frame_to, packet},
};
use std::collections::VecDeque;
const FIN: u8 = 1;
const SYN: u8 = 2;
const RST: u8 = 4;
const ACK: u8 = 16;
const BANNER: &[u8] = b"rv2k shell\r\n> ";
struct Case {
    name: &'static str,
    input: Vec<u8>,
    chunk: usize,
    prompts: usize,
    quit: bool,
    reset: bool,
    partial: bool,
}
fn cases() -> Vec<Case> {
    let mut errors = vec![b'x'; 65];
    errors.extend_from_slice(b"\nhelp\r\nnope\nup\x01time\nnetstat\nquit\n");
    vec![
        Case {
            name: "fragmented-monitor",
            input: b"ps\nmem\nuptime\nnetstat\n".to_vec(),
            chunk: 1,
            prompts: 5,
            quit: false,
            reset: false,
            partial: false,
        },
        Case {
            name: "coalesced-errors-quit",
            input: errors,
            chunk: 128,
            prompts: 6,
            quit: true,
            reset: false,
            partial: false,
        },
        Case {
            name: "partial-eof",
            input: b"up".to_vec(),
            chunk: 2,
            prompts: 1,
            quit: false,
            reset: false,
            partial: true,
        },
        Case {
            name: "partial-reset",
            input: b"netsta".to_vec(),
            chunk: 6,
            prompts: 1,
            quit: false,
            reset: true,
            partial: true,
        },
        Case {
            name: "after-reset",
            input: b"uptime\nps\n".to_vec(),
            chunk: 128,
            prompts: 3,
            quit: false,
            reset: false,
            partial: false,
        },
    ]
}
pub struct Probe {
    pub queue: VecDeque<Vec<u8>>,
    pub inputs: Vec<Vec<u8>>,
    cases: Vec<Case>,
    index: usize,
    waiting: bool,
    done: bool,
    advance: u32,
    advanced: u32,
    baseline: u32,
    client_next: u32,
    server_start: u32,
    server_next: u32,
    pos: usize,
    inflight: bool,
    response: Vec<u8>,
    fin_sent: bool,
    fin_seen: bool,
    window: u16,
    syn_dropped: bool,
    syn_recovered: bool,
    zero_holding: bool,
    zero_started: bool,
    zero_advanced: bool,
    persist_seen: bool,
    after_release: Option<u32>,
    results: Vec<serde_json::Value>,
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
            advance: 0,
            advanced: 0,
            baseline: 0,
            client_next: 0,
            server_start: 0,
            server_next: 0,
            pos: 0,
            inflight: false,
            response: vec![],
            fin_sent: false,
            fin_seen: false,
            window: 128,
            syn_dropped: false,
            syn_recovered: false,
            zero_holding: false,
            zero_started: false,
            zero_advanced: false,
            persist_seen: false,
            after_release: None,
            results: vec![],
        }
    }
    fn port(&self) -> u16 {
        53000 + self.index as u16
    }
    fn enqueue(&mut self, flags: u8, body: &[u8]) {
        let f = frame_to(
            self.port(),
            23,
            self.client_next,
            self.server_next,
            flags,
            if self.zero_holding { 0 } else { 128 },
            body,
        );
        self.inputs.push(f.clone());
        self.queue.push_back(f);
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
        if self.after_release.is_some_and(|n| o.released >= n) {
            self.after_release = None;
            self.advance = 100;
        }
        if self.waiting {
            if o.irqs == 0 || o.listens <= self.index as u32 {
                return;
            }
            if self.index == self.cases.len() {
                self.done = true;
                return;
            }
            self.baseline = o.request_bytes;
            self.client_next = (u32::MAX - 17).wrapping_add(self.index as u32 * 4096);
            self.server_start = 0;
            self.server_next = 0;
            self.pos = 0;
            self.inflight = false;
            self.response.clear();
            self.fin_sent = false;
            self.fin_seen = false;
            self.zero_holding = false;
            self.zero_started = false;
            self.zero_advanced = false;
            self.waiting = false;
            if self.index == 0 {
                self.advance = 6000;
            }
            self.enqueue(SYN, &[]);
            self.client_next = self.client_next.wrapping_add(1);
            return;
        }
        if o.accepts > self.index as u32
            && o.request_bytes.wrapping_sub(self.baseline) >= self.pos as u32
        {
            if self.zero_holding && !self.zero_advanced && self.pos > 0 {
                self.zero_advanced = true;
                self.advance = 100;
            }
            if self.cases[self.index].partial
                && self.pos == self.cases[self.index].input.len()
                && !self.inflight
                && self.response == BANNER
            {
                if self.cases[self.index].reset {
                    self.enqueue(RST | ACK, &[]);
                    self.finish_case();
                } else {
                    self.maybe_fin(true);
                }
            }
        }
    }
    fn send_input(&mut self) {
        let c = &self.cases[self.index];
        if self.inflight
            || self.fin_sent
            || self.pos == c.input.len()
            || self.window == 0
            || self.response.len() < BANNER.len()
        {
            return;
        }
        let count = c
            .chunk
            .min(self.window as usize)
            .min(c.input.len() - self.pos);
        let data = c.input[self.pos..self.pos + count].to_vec();
        self.enqueue(ACK, &data);
        self.client_next = self.client_next.wrapping_add(count as u32);
        self.pos += count;
        self.inflight = true;
    }
    fn complete(&self) -> bool {
        let c = &self.cases[self.index];
        !c.partial
            && self.response.windows(2).filter(|s| *s == b"> ").count() == c.prompts
            && (!c.quit || self.response.ends_with(b"bye\r\n"))
    }
    fn maybe_fin(&mut self, partial_ready: bool) {
        if !self.fin_sent
            && !self.inflight
            && self.pos == self.cases[self.index].input.len()
            && (self.complete() || partial_ready)
        {
            self.enqueue(FIN | ACK, &[]);
            self.client_next = self.client_next.wrapping_add(1);
            self.fin_sent = true;
        }
    }
    fn finish_case(&mut self) {
        self.results.push(serde_json::json!({"name":self.cases[self.index].name,"input_bytes":self.pos,"response":String::from_utf8_lossy(&self.response),"reset":self.cases[self.index].reset}));
        self.index += 1;
        self.waiting = true;
    }
    pub fn receive(&mut self, f: &[u8]) -> Result<(), String> {
        if f.len() < 38 {
            return Err("short shell frame".into());
        }
        let port = u16::from_be_bytes(f[36..38].try_into().unwrap());
        let p = packet(f, port, 23)?;
        if port != self.port() {
            if port >= 53000 && port < self.port() && p.flags == ACK && p.body.is_empty() {
                return Ok(());
            }
            return Err("unexpected shell tuple".into());
        }
        self.window = p.window;
        if p.flags & RST != 0 {
            return Err("guest reset shell connection".into());
        }
        if p.flags & SYN != 0 {
            if p.flags != SYN | ACK || p.ack != self.client_next {
                return Err("bad shell SYN ACK".into());
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
            self.enqueue(ACK, &[]);
            return Ok(());
        }
        if p.ack == self.client_next {
            self.inflight = false;
        }
        if !p.body.is_empty() {
            if p.body.len() > 24 {
                return Err("shell MSS exceeded".into());
            }
            if self.zero_holding {
                if p.body.len() != 1 || p.seq != self.server_next {
                    return Err("bad shell persist probe".into());
                }
                self.persist_seen = true;
                self.zero_holding = false;
                self.enqueue(ACK, &[]);
                self.after_release = Some(self.inputs.len() as u32);
                return Ok(());
            }
            if p.seq == self.server_next {
                self.response.extend_from_slice(p.body);
                self.server_next = self.server_next.wrapping_add(p.body.len() as u32);
            } else {
                let offset = p.seq.wrapping_sub(self.server_start) as usize;
                if self.response.get(offset..offset + p.body.len()) != Some(p.body) {
                    return Err("shell changed retransmitted bytes".into());
                }
            }
            if self.index == 0 && self.response == BANNER && !self.zero_started {
                self.zero_started = true;
                self.zero_holding = true;
            }
            self.enqueue(ACK, &[]);
        }
        self.send_input();
        self.maybe_fin(false);
        if p.flags & FIN != 0 {
            if !self.fin_sent || (!self.complete() && !self.cases[self.index].partial) {
                return Err("premature shell FIN".into());
            }
            if !self.fin_seen {
                if p.seq.wrapping_add(p.body.len() as u32) != self.server_next {
                    return Err("shell FIN sequence".into());
                }
                self.server_next = self.server_next.wrapping_add(1);
                self.fin_seen = true;
            }
            self.enqueue(ACK, &[]);
            self.finish_case();
        }
        Ok(())
    }
    pub fn take_advance(&mut self) -> u32 {
        let n = std::mem::take(&mut self.advance);
        self.advanced += n;
        n
    }
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({"done":self.done,"connections":self.index,"syn_loss_recovered":self.syn_recovered,"zero_window_persist":self.persist_seen,"advanced_ticks":self.advanced,"cases":self.results})
    }
}
