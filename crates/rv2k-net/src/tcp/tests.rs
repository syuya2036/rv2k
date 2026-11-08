use super::*;
use rv2k_api::event;
use std::vec::Vec;

const PEER: Endpoint = Endpoint {
    mac: [2, 3, 4, 5, 6, 7],
    ip: [192, 168, 123, 1],
    port: 50000,
};
const IRS: u32 = u32::MAX - 2;
fn segment(seq: u32, ack: u32, flags: u8, len: u16) -> Segment {
    Segment {
        peer: PEER,
        local_port: 80,
        seq,
        ack,
        flags,
        len,
        window: 128,
        mss: 128,
    }
}
struct Harness {
    tcp: Tcp,
    socket: Socket,
    handle: usize,
    now: u32,
    seq: u32,
    ack: u32,
}
impl Harness {
    fn listen() -> Self {
        Self::listen_quiet(0)
    }
    fn listen_quiet(quiet: u32) -> Self {
        let mut socket = Socket::new();
        let handle = socket.listen(80).unwrap();
        let mut tcp = Tcp::new(0xfedcba98, quiet);
        tcp.tick(&mut socket, 0);
        Self {
            tcp,
            socket,
            handle,
            now: 0,
            seq: IRS.wrapping_add(1),
            ack: 0,
        }
    }
    fn input(&mut self, s: Segment, data: &[u8]) {
        self.tcp
            .receive(&mut self.socket, &s, self.now, |skip, out| {
                out.copy_from_slice(&data[skip..skip + out.len()]);
                Ok(())
            })
            .unwrap();
    }
    fn output(&mut self) -> Option<Outbound> {
        let out = self.tcp.next_segment(&mut self.socket, self.now)?;
        self.tcp.sent(&self.socket, &out, self.now);
        Some(out)
    }
    fn connected() -> Self {
        let mut h = Self::listen();
        h.input(segment(IRS, 0, SYN, 0), &[]);
        let syn = h.output().unwrap();
        assert_eq!(syn.flags, SYN | ACK);
        assert_eq!(syn.ack, IRS.wrapping_add(1));
        assert_ne!(syn.seq, 0);
        h.ack = syn.seq.wrapping_add(1);
        h.now += 10;
        h.input(segment(h.seq, h.ack, ACK, 0), &[]);
        assert_eq!(h.tcp.state(), State::Established);
        assert_ne!(h.socket.take_events() & event::NET_CONNECTED, 0);
        h
    }
    fn send(&mut self, bytes: &[u8]) {
        assert_eq!(
            self.socket.send_with(self.handle, bytes.len(), |out| {
                out.copy_from_slice(&bytes[..out.len()]);
                Ok(())
            }),
            Ok(bytes.len().min(128))
        );
    }
    fn read(&mut self, count: usize) -> Vec<u8> {
        let mut result = Vec::new();
        self.socket
            .recv_with(self.handle, count, |bytes| {
                result.extend_from_slice(bytes);
                Ok(())
            })
            .unwrap();
        result
    }
}

#[test]
fn handshake_loss_duplicate_syn_mss_and_quiet_boot() {
    let mut h = Harness::listen_quiet(5);
    h.input(segment(IRS, 0, SYN, 0), &[]);
    assert!(h.output().is_none());
    h.now = 5;
    let mut syn = segment(IRS, 0, SYN, 0);
    syn.mss = 32;
    syn.window = 0;
    h.input(syn, &[]);
    let first = h.output().unwrap();
    h.now = 104;
    assert!(h.output().is_none());
    h.now = 105;
    let retrans = h.output().unwrap();
    assert_eq!(retrans.seq, first.seq);
    assert_eq!(retrans.flags, SYN | ACK);
    h.input(syn, &[]);
    let dup = h.output().unwrap();
    assert_eq!(dup.seq, first.seq);
    h.ack = first.seq.wrapping_add(1);
    h.now += 1;
    h.input(segment(h.seq, h.ack, ACK, 0), &[]);
    assert_eq!(h.tcp.state(), State::Established);
    h.send(&[7; 64]);
    let data = h.output().unwrap();
    assert_eq!(data.len, 32);
    // The retransmitted handshake cannot provide an RTT sample (Karn).
    assert!(h.tcp.rto.ticks() >= 300);
}

#[test]
fn rx_duplicates_overlap_reordering_wrap_and_window_reopening() {
    let mut h = Harness::connected();
    let initial = h.seq;
    h.input(segment(initial.wrapping_add(8), h.ack, ACK, 4), b"late");
    assert_eq!(h.socket.rx_len(), 0);
    assert_eq!(h.output().unwrap().ack, initial);
    h.input(segment(initial, h.ack, ACK, 4), b"abcd");
    h.seq = initial.wrapping_add(4);
    assert_eq!(h.output().unwrap().ack, h.seq);
    assert_eq!(h.socket.rx_len(), 4);
    h.input(segment(initial, h.ack, ACK, 8), b"abcdefgh");
    h.seq = initial.wrapping_add(8);
    assert_eq!(h.output().unwrap().ack, h.seq);
    assert_eq!(h.read(8), b"abcdefgh");
    assert_eq!(h.output().unwrap().window, 128);
    h.input(segment(h.seq, h.ack, ACK, 128), &[0x55; 128]);
    h.seq = h.seq.wrapping_add(128);
    let full = h.output().unwrap();
    assert_eq!(full.window, 0);
    assert_eq!(full.ack, h.seq);
    h.input(segment(h.seq, h.ack, ACK, 1), b"x");
    assert_eq!(h.output().unwrap().ack, h.seq);
    assert_eq!(h.read(64), [0x55; 64]);
    let update = h.output().unwrap();
    assert_eq!(update.window, 64);
    h.input(segment(h.seq, h.ack, ACK, 80), &[0x66; 80]);
    h.seq = h.seq.wrapping_add(64);
    assert_eq!(h.output().unwrap().ack, h.seq);
    let bytes = h.read(128);
    assert_eq!(&bytes[..64], &[0x55; 64]);
    assert_eq!(&bytes[64..], &[0x66; 64]);
}

#[test]
fn tx_partial_ack_retransmits_identical_suffix_and_rejects_future_ack() {
    let mut h = Harness::connected();
    let bytes: [u8; 128] = core::array::from_fn(|i| i as u8);
    h.send(&bytes);
    let data = h.output().unwrap();
    assert_eq!(data.seq, h.ack);
    assert_eq!(data.len, 128);
    let mut other = segment(h.seq, h.ack.wrapping_add(128), ACK, 0);
    other.peer.port += 1;
    h.input(other, &[]);
    assert_eq!(h.socket.tx_len(), 128);
    h.input(segment(h.seq, h.ack.wrapping_add(129), ACK, 0), &[]);
    assert_eq!(h.socket.tx_len(), 128);
    h.output().unwrap();
    h.ack = h.ack.wrapping_add(55);
    h.input(segment(h.seq, h.ack, ACK, 0), &[]);
    assert_eq!(h.socket.tx_len(), 73);
    let deadline = h.tcp.deadline;
    h.now = deadline.wrapping_sub(1);
    assert!(h.output().is_none());
    h.now = deadline;
    let retry = h.output().unwrap();
    assert_eq!(retry.seq, h.ack);
    assert_eq!(retry.len, 73);
    assert_eq!(h.tcp.payload(&h.socket, &retry), &bytes[55..]);
    h.input(segment(h.seq, h.ack, ACK, 0), &[]);
    assert_eq!(h.socket.tx_len(), 73);
    h.ack = h.ack.wrapping_add(73);
    h.input(segment(h.seq, h.ack, ACK, 0), &[]);
    assert_eq!(h.socket.tx_len(), 0);
    assert_eq!(h.tcp.flight, 0);
}

#[test]
fn tx_state_does_not_advance_until_device_commit_and_ack_copy_failure_preserves_rx() {
    let mut h = Harness::connected();
    h.send(b"unchanged");
    let first = h.tcp.next_segment(&mut h.socket, h.now).unwrap();
    h.now += 20;
    let second = h.tcp.next_segment(&mut h.socket, h.now).unwrap();
    assert_eq!(first.seq, second.seq);
    assert_eq!(h.tcp.flight, 0);
    assert_eq!(h.tcp.next, h.ack);
    let s = segment(h.seq, h.ack, ACK, 4);
    assert_eq!(
        h.tcp.receive(&mut h.socket, &s, h.now, |_, out| {
            out.fill(0x99);
            Err(Error::Io)
        }),
        Err(Error::Io)
    );
    assert_eq!(h.socket.rx_len(), 0);
    assert_eq!(h.tcp.receive, h.seq);
}

#[test]
fn persist_keeps_connection_alive_then_resumes_after_window_opens() {
    let mut h = Harness::connected();
    let mut zero = segment(h.seq, h.ack, ACK, 0);
    zero.window = 0;
    h.input(zero, &[]);
    h.send(b"persist");
    for _ in 0..12 {
        h.now = h.tcp.persist_at;
        let probe = h.output().unwrap();
        assert_eq!(probe.len, 1);
        assert_eq!(probe.seq, h.ack);
        assert_eq!(h.tcp.payload(&h.socket, &probe), b"p");
        h.input(zero, &[]);
        assert_eq!(h.tcp.state(), State::Established);
    }
    let mut open = zero;
    open.window = 128;
    h.input(open, &[]);
    h.now = h.tcp.deadline;
    let retrans = h.output().unwrap();
    assert_eq!(retrans.len, 1);
    h.ack = h.ack.wrapping_add(1);
    h.input(segment(h.seq, h.ack, ACK, 0), &[]);
    let rest = h.output().unwrap();
    assert_eq!(rest.len, 6);
    assert_eq!(h.tcp.payload(&h.socket, &rest), b"ersist");
}

#[test]
fn passive_close_drains_rx_eof_fin_loss_and_reconnects_with_new_generation() {
    let mut h = Harness::connected();
    let old_handle = h.handle;
    let old_isn = h.ack;
    h.input(segment(h.seq, h.ack, ACK | FIN, 3), b"bye");
    h.seq = h.seq.wrapping_add(4);
    assert_eq!(h.tcp.state(), State::CloseWait);
    assert_eq!(h.output().unwrap().ack, h.seq);
    assert_eq!(h.read(8), b"bye");
    assert!(h.read(8).is_empty());
    h.output().unwrap(); // window update
    h.send(b"done");
    let data = h.output().unwrap();
    assert_eq!(data.len, 4);
    h.ack = h.ack.wrapping_add(4);
    h.input(segment(h.seq, h.ack, ACK, 0), &[]);
    h.socket.close(h.handle).unwrap();
    let fin = h.output().unwrap();
    assert_eq!(fin.flags, ACK | FIN);
    assert_eq!(h.tcp.state(), State::LastAck);
    h.now = h.tcp.deadline;
    let repeat = h.output().unwrap();
    assert_eq!(repeat.seq, fin.seq);
    assert_eq!(repeat.flags, fin.flags);
    h.ack = h.ack.wrapping_add(1);
    h.input(segment(h.seq, h.ack, ACK, 0), &[]);
    assert_eq!(h.tcp.state(), State::Closed);
    assert_ne!(h.socket.take_events() & event::NET_CLOSED, 0);
    h.handle = h.socket.listen(80).unwrap();
    assert_ne!(h.handle, old_handle);
    h.input(segment(IRS, 0, SYN, 0), &[]);
    let next = h.output().unwrap();
    assert_ne!(next.seq.wrapping_add(1), old_isn);
    assert_eq!(
        h.socket.recv_with(old_handle, 1, |_| Ok(())),
        Err(Error::BadFd)
    );
}

#[test]
fn active_and_simultaneous_close_hold_time_wait_and_repeat_fin_ack() {
    for simultaneous in [false, true] {
        let mut h = Harness::connected();
        h.socket.close(h.handle).unwrap();
        let fin = h.output().unwrap();
        assert_eq!(h.tcp.state(), State::FinWait1);
        if !simultaneous {
            h.ack = fin.seq.wrapping_add(1);
            h.input(segment(h.seq, h.ack, ACK, 0), &[]);
            assert_eq!(h.tcp.state(), State::FinWait2);
        }
        let remote_fin = segment(h.seq, h.ack, ACK | FIN, 0);
        h.input(remote_fin, &[]);
        h.seq = h.seq.wrapping_add(1);
        assert_eq!(h.output().unwrap().ack, h.seq);
        if simultaneous {
            assert_eq!(h.tcp.state(), State::Closing);
            h.ack = fin.seq.wrapping_add(1);
            h.input(segment(h.seq, h.ack, ACK, 0), &[]);
        }
        assert_eq!(h.tcp.state(), State::TimeWait);
        assert_eq!(h.socket.listen(80), Err(Error::Busy));
        h.input(segment(h.seq, h.ack, RST, 0), &[]);
        assert_eq!(h.tcp.state(), State::TimeWait);
        h.now += 100;
        h.input(remote_fin, &[]);
        assert_eq!(h.output().unwrap().ack, h.seq);
        let deadline = h.now.wrapping_add(TIME_WAIT_TICKS);
        h.now = deadline - 1;
        assert!(h.output().is_none());
        h.now = deadline;
        assert!(h.output().is_none());
        assert_eq!(h.tcp.state(), State::Closed);
        assert!(h.socket.listen(80).is_ok());
    }
}

#[test]
fn reset_sequence_validation_timeout_backoff_and_deadline_wrap() {
    let mut h = Harness::connected();
    h.input(segment(h.seq.wrapping_add(1), h.ack, RST, 0), &[]);
    assert_eq!(h.tcp.state(), State::Established);
    assert_eq!(h.output().unwrap().flags, ACK);
    h.input(segment(h.seq, h.ack, RST, 0), &[]);
    assert_eq!(h.tcp.state(), State::Closed);
    let mut h = Harness::connected();
    h.now = u32::MAX - 50;
    h.send(b"lost");
    h.output().unwrap();
    let mut previous = 0;
    for _ in 0..8 {
        let duration = h.tcp.rto.ticks();
        assert!(duration >= previous);
        previous = duration;
        h.now = h.tcp.deadline;
        assert!(h.output().is_some());
    }
    h.now = h.tcp.deadline;
    assert!(h.output().is_none());
    assert_eq!(h.tcp.state(), State::Closed);
    assert_eq!(
        h.socket.send_with(h.handle, 1, |_| Ok(())),
        Err(Error::Timeout)
    );
}

#[test]
fn an_ack_available_at_the_final_timeout_wins_over_connection_failure() {
    let mut h = Harness::connected();
    h.send(b"x");
    let first = h.output().unwrap();
    for _ in 0..8 {
        h.now = h.tcp.deadline;
        h.output().unwrap();
    }
    h.now = h.tcp.deadline;
    h.ack = first.seq.wrapping_add(1);
    h.input(segment(h.seq, h.ack, ACK, 0), &[]);
    assert_eq!(h.socket.tx_len(), 0);
    assert!(h.output().is_none());
    assert_eq!(h.tcp.state(), State::Established);
}

#[test]
fn partial_and_full_tx_ack_cross_the_sequence_wrap_without_releasing_unsent_bytes() {
    let mut h = Harness::listen();
    h.tcp.sequence_seed = (u32::MAX - 10)
        .wrapping_sub(0x10001)
        .wrapping_sub(u32::from_be_bytes(PEER.ip))
        .wrapping_sub(PEER.port as u32);
    h.input(segment(IRS, 0, SYN, 0), &[]);
    let syn = h.output().unwrap();
    assert_eq!(syn.seq, u32::MAX - 10);
    h.ack = syn.seq.wrapping_add(1);
    h.input(segment(h.seq, h.ack, ACK, 0), &[]);
    h.send(&[0x33; 20]);
    let data = h.output().unwrap();
    assert_eq!(data.seq, u32::MAX - 9);
    h.ack = h.ack.wrapping_add(5);
    h.input(segment(h.seq, h.ack, ACK, 0), &[]);
    assert_eq!(h.socket.tx_len(), 15);
    h.ack = h.ack.wrapping_add(15);
    assert_eq!(h.ack, 10);
    h.input(segment(h.seq, h.ack, ACK, 0), &[]);
    assert_eq!(h.socket.tx_len(), 0);
    h.send(b"next");
    let next = h.output().unwrap();
    assert_eq!(next.seq, 10);
    h.input(segment(h.seq, u32::MAX - 9, ACK, 0), &[]);
    assert_eq!(h.socket.tx_len(), 4);
}
