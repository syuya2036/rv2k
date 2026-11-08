//! Single connection, passive TCP. Payload storage is separate from wire state.
pub mod rto;
pub mod socket;
use crate::due;
use rv2k_api::syscall::Error;
use socket::{Socket, CAPACITY};
#[cfg(test)]
mod tests;

pub const FIN: u8 = 1;
pub const SYN: u8 = 2;
pub const RST: u8 = 4;
pub const PSH: u8 = 8;
pub const ACK: u8 = 16;
pub const TIME_WAIT_TICKS: u32 = 12000;
const FIN_WAIT_TICKS: u32 = 6000;

pub use rv2k_api::net::TcpState as State;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Endpoint {
    pub mac: [u8; 6],
    pub ip: [u8; 4],
    pub port: u16,
}
/// Already checksum-validated TCP header and payload length. The callback passed
/// to receive reads only this packet; it never retains a device or user pointer.
#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub peer: Endpoint,
    pub local_port: u16,
    pub seq: u32,
    pub ack: u32,
    pub window: u16,
    pub flags: u8,
    pub len: u16,
    pub mss: u16,
}
impl Segment {
    pub const fn empty() -> Self {
        Self {
            peer: Endpoint {
                mac: [0; 6],
                ip: [0; 4],
                port: 0,
            },
            local_port: 0,
            seq: 0,
            ack: 0,
            window: 0,
            flags: 0,
            len: 0,
            mss: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SendKind {
    Ack,
    New,
    Repeat,
    Probe,
    Reset,
}
#[derive(Clone, Copy, Debug)]
pub struct Outbound {
    pub peer: Endpoint,
    pub local_port: u16,
    pub seq: u32,
    pub ack: u32,
    pub window: u16,
    pub flags: u8,
    pub len: u16,
    kind: SendKind,
}
impl Outbound {
    pub const fn empty() -> Self {
        Self {
            peer: Endpoint {
                mac: [0; 6],
                ip: [0; 4],
                port: 0,
            },
            local_port: 0,
            seq: 0,
            ack: 0,
            window: 0,
            flags: 0,
            len: 0,
            kind: SendKind::Ack,
        }
    }
    pub fn syn(&self) -> bool {
        self.flags & SYN != 0
    }
}
fn before(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}
fn in_window(seq: u32, start: u32, size: u32) -> bool {
    seq.wrapping_sub(start) < size
}

pub struct Tcp {
    state: State,
    peer: Option<Endpoint>,
    una: u32,
    next: u32,
    receive: u32,
    right: u32,
    window_seq: u32,
    window_ack: u32,
    peer_window: u16,
    mss: u16,
    flight: u16,
    flight_flags: u8,
    sent_at: u32,
    deadline: u32,
    persist_at: u32,
    persist_interval: u32,
    state_deadline: u32,
    quiet_until: u32,
    quiet_done: bool,
    sequence_seed: u32,
    retries: u8,
    retransmitted: bool,
    ack_pending: bool,
    repeat: bool,
    reset: Outbound,
    reset_pending: bool,
    rto: rto::Rto,
    pub retransmissions: u16,
    pub resets: u16,
}
impl Tcp {
    /// A reboot without persistent, monotonically advancing sequence state must
    /// observe one MSL of quiet time. The caller supplies that explicit deadline.
    pub const fn new(seed: u32, quiet_until: u32) -> Self {
        Self {
            state: State::Closed,
            peer: None,
            una: 0,
            next: 0,
            receive: 0,
            right: 0,
            window_seq: 0,
            window_ack: 0,
            peer_window: 0,
            mss: 0,
            flight: 0,
            flight_flags: 0,
            sent_at: 0,
            deadline: 0,
            persist_at: 0,
            persist_interval: 0,
            state_deadline: 0,
            quiet_until,
            quiet_done: false,
            sequence_seed: seed,
            retries: 0,
            retransmitted: false,
            ack_pending: false,
            repeat: false,
            reset: Outbound::empty(),
            reset_pending: false,
            rto: rto::Rto::dormant(),
            retransmissions: 0,
            resets: 0,
        }
    }
    pub fn state(&self) -> State {
        self.state
    }
    /// Configure before starting the listener; no protocol buffers are copied.
    pub fn configure_boot(&mut self, seed: u32, quiet_until: u32) {
        assert!(self.state == State::Closed && self.peer.is_none());
        self.sequence_seed = seed;
        self.quiet_until = quiet_until;
        self.quiet_done = false;
    }
    pub fn peer(&self) -> Option<Endpoint> {
        self.peer
    }
    pub fn peer_window(&self) -> u16 {
        self.peer_window
    }
    pub fn receive_window(&self) -> u16 {
        self.right.wrapping_sub(self.receive).min(128) as u16
    }
    fn finish(&mut self, socket: &mut Socket, error: Error) {
        self.state = State::Closed;
        self.peer = None;
        self.flight = 0;
        self.ack_pending = false;
        self.repeat = false;
        socket.terminal(error);
    }
    fn time_wait(&mut self, now: u32) {
        self.state = State::TimeWait;
        self.state_deadline = now.wrapping_add(TIME_WAIT_TICKS);
    }
    fn matching(&self, segment: &Segment, socket: &Socket) -> bool {
        self.peer
            .is_some_and(|p| p.ip == segment.peer.ip && p.port == segment.peer.port)
            && segment.local_port == socket.port()
    }
    fn reset_for(&mut self, segment: &Segment) {
        if segment.flags & RST != 0 {
            return;
        }
        let has_ack = segment.flags & ACK != 0;
        self.reset = Outbound {
            peer: segment.peer,
            local_port: segment.local_port,
            seq: if has_ack { segment.ack } else { 0 },
            ack: if has_ack {
                0
            } else {
                segment
                    .seq
                    .wrapping_add(segment.len as u32)
                    .wrapping_add((segment.flags & SYN != 0) as u32)
                    .wrapping_add((segment.flags & FIN != 0) as u32)
            },
            window: 0,
            flags: if has_ack { RST } else { RST | ACK },
            len: 0,
            kind: SendKind::Reset,
        };
        self.reset_pending = true;
    }
    fn requests(&mut self, socket: &mut Socket, now: u32) {
        if due(now, self.quiet_until) {
            self.quiet_done = true;
        }
        if self.state == State::Closed && socket.take_listen() {
            self.state = State::Listen;
        }
        if self.state == State::Listen && socket.closing() {
            self.finish(socket, Error::Closed);
        }
        if socket.take_window_update() {
            self.ack_pending = self.peer.is_some();
        }
    }
    pub fn tick(&mut self, socket: &mut Socket, now: u32) {
        self.requests(socket, now);
        if matches!(self.state, State::FinWait2 | State::TimeWait) && due(now, self.state_deadline)
        {
            self.finish(
                socket,
                if self.state == State::TimeWait {
                    Error::Closed
                } else {
                    Error::Timeout
                },
            );
        }
        if self.flight != 0
            && (self.peer_window != 0 || self.flight_flags & SYN != 0)
            && due(now, self.deadline)
            && self.retries >= 8
        {
            self.finish(socket, Error::Timeout);
        }
    }
    #[inline(never)]
    pub fn receive(
        &mut self,
        socket: &mut Socket,
        segment: &Segment,
        now: u32,
        mut copy: impl FnMut(usize, &mut [u8]) -> Result<(), Error>,
    ) -> Result<(), Error> {
        // Process an already available ACK/FIN before deciding that its timeout
        // expired. next_segment/tick applies timers after the RX opportunity.
        self.requests(socket, now);
        if !self.quiet_done {
            return Ok(());
        }
        if matches!(self.state, State::Closed | State::Listen) {
            if self.state == State::Closed || segment.local_port != socket.port() {
                self.reset_for(segment);
                return Ok(());
            }
            if segment.flags & RST != 0 {
                return Ok(());
            }
            if segment.flags & ACK != 0 {
                self.reset_for(segment);
                return Ok(());
            }
            if !socket.listening() || segment.flags & SYN == 0 || segment.flags & FIN != 0 {
                return Ok(());
            }
            // Include tuple and a 4-us clock step; advance independently between
            // connections. This is not a cryptographically unpredictable ISN.
            self.sequence_seed = self
                .sequence_seed
                .wrapping_add(0x10001)
                .wrapping_add(u32::from_be_bytes(segment.peer.ip))
                .wrapping_add(segment.peer.port as u32);
            self.una = self.sequence_seed.wrapping_add(now.wrapping_mul(2500));
            self.next = self.una;
            self.receive = segment.seq.wrapping_add(1);
            self.right = self.receive.wrapping_add(CAPACITY as u32);
            self.peer = Some(segment.peer);
            self.peer_window = segment.window;
            self.mss = segment.mss.clamp(1, 128);
            self.window_seq = segment.seq;
            self.window_ack = 0;
            self.flight = 0;
            self.retries = 0;
            self.retransmitted = false;
            self.rto = rto::Rto::new();
            self.persist_at = now.wrapping_add(rto::INITIAL);
            self.persist_interval = rto::INITIAL;
            self.state = State::SynReceived;
            return Ok(());
        }
        if !self.matching(segment, socket) {
            return Ok(());
        }
        // Preserve the old tuple through TIME_WAIT (RFC 1337 assassination fix).
        if self.state == State::TimeWait && segment.flags & RST != 0 {
            return Ok(());
        }
        if self.state == State::SynReceived
            && segment.flags & SYN != 0
            && segment.flags & (ACK | RST | FIN) == 0
            && segment.seq.wrapping_add(1) == self.receive
        {
            self.repeat = self.flight != 0;
            return Ok(());
        }
        let size = segment.len as u32
            + (segment.flags & SYN != 0) as u32
            + (segment.flags & FIN != 0) as u32;
        let window = self.right.wrapping_sub(self.receive);
        let acceptable = if size == 0 {
            if window == 0 {
                segment.seq == self.receive
            } else {
                in_window(segment.seq, self.receive, window)
            }
        } else {
            window != 0
                && (in_window(segment.seq, self.receive, window)
                    || in_window(segment.seq.wrapping_add(size - 1), self.receive, window)
                    || (before(segment.seq, self.receive)
                        && !before(segment.seq.wrapping_add(size - 1), self.receive)))
        };
        if !acceptable {
            if segment.flags & RST == 0 {
                self.ack_pending = true;
                if self.state == State::TimeWait
                    && segment.flags & FIN != 0
                    && segment.seq.wrapping_add(segment.len as u32).wrapping_add(1) == self.receive
                {
                    self.state_deadline = now.wrapping_add(TIME_WAIT_TICKS);
                }
            }
            return Ok(());
        }
        if segment.flags & RST != 0 {
            if segment.seq == self.receive {
                self.resets = self.resets.saturating_add(1);
                self.finish(socket, Error::Closed);
            } else {
                self.ack_pending = true;
            }
            return Ok(());
        }
        if segment.flags & SYN != 0 {
            self.ack_pending = true;
            return Ok(());
        }
        if segment.flags & ACK == 0 {
            return Ok(());
        }
        if before(self.next, segment.ack) {
            self.ack_pending = true;
            return Ok(());
        }
        if self.state == State::SynReceived {
            if self.flight == 0 || segment.ack != self.next {
                self.reset_for(segment);
                return Ok(());
            }
            self.state = State::Established;
            socket.connected();
        }
        if !before(segment.ack, self.una) {
            if before(self.window_seq, segment.seq)
                || (self.window_seq == segment.seq && !before(segment.ack, self.window_ack))
            {
                let old = self.peer_window;
                self.peer_window = segment.window;
                self.window_seq = segment.seq;
                self.window_ack = segment.ack;
                if old == 0 && self.peer_window != 0 {
                    self.persist_interval = rto::INITIAL;
                    self.deadline = now.wrapping_add(self.rto.ticks());
                } else if old != 0 && self.peer_window == 0 {
                    self.persist_at = now.wrapping_add(rto::INITIAL);
                }
            }
            let acked = segment.ack.wrapping_sub(self.una) as usize;
            if acked != 0 {
                if self.flight_flags & (SYN | FIN) == 0 {
                    socket.acked(acked);
                }
                self.flight -= acked as u16;
                self.una = segment.ack;
                self.retries = 0;
                if self.flight == 0 {
                    if !self.retransmitted {
                        self.rto.sample(now.wrapping_sub(self.sent_at));
                    } else if self.flight_flags & SYN != 0 {
                        self.rto.syn_retransmitted();
                    }
                    self.repeat = false;
                    if self.flight_flags & FIN != 0 {
                        match self.state {
                            State::FinWait1 => {
                                self.state = State::FinWait2;
                                self.state_deadline = now.wrapping_add(FIN_WAIT_TICKS);
                            }
                            State::Closing => self.time_wait(now),
                            State::LastAck => {
                                self.finish(socket, Error::Closed);
                                return Ok(());
                            }
                            _ => {}
                        }
                    }
                }
                self.deadline = now.wrapping_add(self.rto.ticks());
            }
        }
        if self.state == State::TimeWait {
            if size != 0 {
                self.ack_pending = true;
            }
            return Ok(());
        }
        if matches!(
            self.state,
            State::Closing | State::LastAck | State::CloseWait
        ) {
            if size != 0 {
                self.ack_pending = true;
            }
            return Ok(());
        }
        // Keep only the in-order suffix, trimmed to the last advertised edge.
        let skip = self.receive.wrapping_sub(segment.seq) as usize;
        if skip <= segment.len as usize && !before(self.receive, segment.seq) {
            let count = (segment.len as usize - skip).min(window as usize);
            socket.receive_with(count, |out| copy(skip, out))?;
            self.receive = self.receive.wrapping_add(count as u32);
            if count != 0 {
                self.ack_pending = true;
            }
            let fin_at = segment.seq.wrapping_add(segment.len as u32);
            if segment.flags & FIN != 0
                && fin_at == self.receive
                && in_window(fin_at, self.right.wrapping_sub(window), window)
            {
                self.receive = self.receive.wrapping_add(1);
                // FIN occupies sequence space but not the payload buffer.
                self.right = self.right.wrapping_add(1);
                socket.eof();
                self.ack_pending = true;
                match self.state {
                    State::Established => self.state = State::CloseWait,
                    State::FinWait1 => self.state = State::Closing,
                    State::FinWait2 => self.time_wait(now),
                    _ => {}
                }
            }
        }
        if size != 0 {
            self.ack_pending = true;
        }
        Ok(())
    }
    pub fn next_segment(&mut self, socket: &mut Socket, now: u32) -> Option<Outbound> {
        let mut out = Outbound::empty();
        self.prepare(socket, now, &mut out).then_some(out)
    }
    #[inline(never)]
    pub fn prepare(&mut self, socket: &mut Socket, now: u32, out: &mut Outbound) -> bool {
        self.tick(socket, now);
        if !self.quiet_done {
            return false;
        }
        if self.reset_pending {
            *out = self.reset;
            return true;
        }
        let Some(peer) = self.peer else {
            return false;
        };
        *out = Outbound {
            peer,
            local_port: socket.port(),
            seq: self.next,
            ack: self.receive,
            window: (CAPACITY - socket.rx_len()) as u16,
            flags: ACK,
            len: 0,
            kind: SendKind::Ack,
        };
        if self.state == State::SynReceived && self.flight == 0 {
            out.flags |= SYN;
            out.kind = SendKind::New;
        } else if self.flight != 0
            && (self.repeat
                || ((self.peer_window != 0 || self.flight_flags & SYN != 0)
                    && due(now, self.deadline)))
        {
            out.seq = self.una;
            out.flags = self.flight_flags;
            out.kind = SendKind::Repeat;
            if out.flags & (SYN | FIN) == 0 {
                out.len = self.flight.min(self.peer_window);
            }
        } else if self.ack_pending {
            // Window/receive ACKs must not be delayed behind application data.
        } else if self.peer_window == 0
            && (socket.tx_len() != 0 || socket.closing())
            && due(now, self.persist_at)
        {
            out.seq = if self.flight != 0 {
                self.una
            } else {
                self.next
            };
            out.len = (socket.tx_len() != 0) as u16;
            if out.len == 0 {
                out.seq = out.seq.wrapping_sub(1);
            }
            out.kind = SendKind::Probe;
        } else if self.flight == 0 && matches!(self.state, State::Established | State::CloseWait) {
            if socket.tx_len() != 0 && self.peer_window != 0 {
                out.len = socket
                    .tx_len()
                    .min(self.mss as usize)
                    .min(self.peer_window as usize) as u16;
                out.flags |= PSH;
                out.kind = SendKind::New;
            } else if socket.closing() && self.peer_window != 0 {
                out.flags |= FIN;
                out.kind = SendKind::New;
            } else {
                return false;
            }
        } else {
            return false;
        }
        true
    }
    pub fn payload<'a>(&self, socket: &'a Socket, out: &Outbound) -> &'a [u8] {
        socket.tx(out.len as usize)
    }
    /// Commit protocol sequence/timer changes only after the NIC accepted TX.
    #[inline(never)]
    pub fn sent(&mut self, socket: &Socket, out: &Outbound, now: u32) {
        if out.kind == SendKind::Reset {
            self.reset_pending = false;
            return;
        }
        self.right = self.receive.wrapping_add(out.window as u32);
        self.ack_pending = false;
        match out.kind {
            SendKind::New => {
                self.flight =
                    out.len + (out.flags & SYN != 0) as u16 + (out.flags & FIN != 0) as u16;
                self.flight_flags = out.flags;
                self.una = out.seq;
                self.next = out.seq.wrapping_add(self.flight as u32);
                self.sent_at = now;
                self.retransmitted = false;
                self.retries = 0;
                self.deadline = now.wrapping_add(self.rto.ticks());
                if out.flags & FIN != 0 {
                    self.state = if self.state == State::CloseWait {
                        State::LastAck
                    } else {
                        State::FinWait1
                    };
                }
            }
            SendKind::Repeat => {
                self.repeat = false;
                self.retransmissions = self.retransmissions.saturating_add(1);
                self.retransmitted = true;
                self.retries = self.retries.saturating_add(1);
                self.rto.backoff();
                self.deadline = now.wrapping_add(self.rto.ticks());
            }
            SendKind::Probe => {
                if self.flight == 0 && out.len != 0 {
                    self.flight = 1;
                    self.flight_flags = ACK;
                    self.una = out.seq;
                    self.next = out.seq.wrapping_add(1);
                }
                self.retransmitted = true;
                self.persist_interval = (self.persist_interval * 2).min(rto::MAXIMUM);
                self.persist_at = now.wrapping_add(self.persist_interval);
            }
            _ => {}
        }
        let _ = socket; // socket's acknowledged bytes change only in receive().
    }
}
