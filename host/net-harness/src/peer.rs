use crate::{http_probe, shell_probe, tcp_probe};
use std::collections::VecDeque;
pub struct Observation {
    pub listens: u32,
    pub irqs: u32,
    pub accepts: u32,
    pub request_bytes: u32,
    pub last_read_ticks: u32,
    pub released: u32,
}
pub enum Peer {
    Tcp(tcp_probe::Probe),
    Shell(shell_probe::Probe),
    Http(http_probe::Probe),
}
impl Peer {
    pub fn is_http(&self) -> bool {
        matches!(self, Self::Http(_))
    }
    pub fn is_shell(&self) -> bool {
        matches!(self, Self::Shell(_))
    }
    pub fn needs_ready(&self) -> bool {
        match self {
            Self::Tcp(p) => p.needs_ready(),
            Self::Shell(p) => p.needs_ready(),
            Self::Http(p) => p.needs_ready(),
        }
    }
    pub fn observe(&mut self, o: Observation) {
        match self {
            Self::Tcp(p) => p.ready(o.listens, o.irqs),
            Self::Shell(p) => p.observe(o),
            Self::Http(p) => p.observe(o),
        }
    }
    pub fn take_advance(&mut self) -> u32 {
        match self {
            Self::Tcp(p) => p.take_advance(),
            Self::Shell(p) => p.take_advance(),
            Self::Http(p) => p.take_advance(),
        }
    }
    pub fn queue(&mut self) -> &mut VecDeque<Vec<u8>> {
        match self {
            Self::Tcp(p) => &mut p.queue,
            Self::Shell(p) => &mut p.queue,
            Self::Http(p) => &mut p.queue,
        }
    }
    pub fn inputs(&self) -> &[Vec<u8>] {
        match self {
            Self::Tcp(p) => &p.inputs,
            Self::Shell(p) => &p.inputs,
            Self::Http(p) => &p.inputs,
        }
    }
    pub fn receive(&mut self, f: &[u8]) -> Result<(), String> {
        match self {
            Self::Tcp(p) => p.receive(f),
            Self::Shell(p) => p.receive(f),
            Self::Http(p) => p.receive(f),
        }
    }
    pub fn done(&self) -> bool {
        match self {
            Self::Tcp(p) => p.done(),
            Self::Shell(p) => p.done(),
            Self::Http(p) => p.done(),
        }
    }
    pub fn summary(&self) -> serde_json::Value {
        match self {
            Self::Tcp(p) => p.summary(),
            Self::Shell(p) => p.summary(),
            Self::Http(p) => p.summary(),
        }
    }
}
