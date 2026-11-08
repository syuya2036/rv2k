mod device;
mod pcap;
mod tcp_probe;
use device::{Nic, State};
use remu_core::{RunLimits, StopReason};
use remu_image::FirmwareImage;
use remu_machines::{RiscVMachine, TargetId};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    env, fs,
    net::UdpSocket,
    path::Path,
    rc::Rc,
    time::{Duration, Instant},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().collect();
    if args.len() < 6 {
        return Err("usage: net-harness ELF INPUT.pcap OUTPUT.pcap REPORT.json replay | ELF BIND_ADDR OUTPUT.pcap REPORT.json udp PEER_ADDR SECONDS".into());
    }
    if args[5] != "replay" && args[5] != "udp" && args[5] != "tcp" {
        return Err("mode must be replay, udp or tcp".into());
    }
    let image = FirmwareImage::parse(&fs::read(&args[1])?)?;
    let mut machine = RiscVMachine::new(TargetId::Ch32v003)?;
    let state = Rc::new(RefCell::new(State::default()));
    machine.attach_device(
        "rv2k.virtual-ethernet",
        0x50000000,
        4096,
        Box::new(Nic(state.clone())),
    )?;
    machine.load_firmware(&image)?;
    let mut probe = if args[5] == "tcp" {
        Some(tcp_probe::Probe::new())
    } else {
        None
    };
    let mut inputs = if args[5] == "replay" {
        pcap::read(Path::new(&args[2]))?.into_iter()
    } else {
        Vec::new().into_iter()
    };
    let socket = if args[5] == "udp" {
        let s = UdpSocket::bind(&args[2])?;
        s.connect(args.get(6).ok_or("missing UDP peer")?)?;
        s.set_nonblocking(true)?;
        Some(s)
    } else {
        None
    };
    let timeout = if socket.is_some() {
        args.get(7).ok_or("missing duration")?.parse::<u64>()?
    } else {
        60
    };
    let began = Instant::now();
    let mut outputs = Vec::new();
    let mut capture = Vec::new();
    let mut overflow = 0u32;
    let mut exhausted = false;
    let mut stopped = false;
    let mut instructions = 0u64;
    let final_result = loop {
        if let Some(frame) = state.borrow_mut().take_tx() {
            if let Some(probe) = &mut probe {
                probe.receive(&frame)?;
            }
            if let Some(socket) = &socket {
                socket.send(&frame)?;
            }
            outputs.push(frame);
        }
        if let Some(probe) = &mut probe {
            if probe.needs_ready() {
                let mut word = |name: &str| -> Result<u32, Box<dyn std::error::Error>> {
                    let addr = image
                        .symbols
                        .iter()
                        .find(|s| s.name == name)
                        .ok_or("missing probe symbol")?
                        .address;
                    Ok(u32::from_le_bytes(
                        machine.debug_read_memory(addr, 4)?.try_into().unwrap(),
                    ))
                };
                let listens = word("APP_LISTENS")?;
                let irqs = word("TIMER_IRQS")?;
                probe.ready(listens, irqs);
            }
            let ticks = probe.take_advance();
            if ticks != 0 {
                machine.advance_time(ticks as u64 * 240_000)?;
            }
            if state.borrow().empty() {
                if let Some(frame) = probe.queue.pop_front() {
                    state.borrow_mut().inject(frame)?;
                }
            }
            if probe.done() && probe.queue.is_empty() && state.borrow().empty() {
                stopped = true;
            }
        } else if let Some(socket) = &socket {
            let mut packet = [0u8; 1515];
            for _ in 0..16 {
                match socket.recv(&mut packet) {
                    Ok(len) if len <= 1514 => {
                        capture.push(packet[..len].to_vec());
                        if state.borrow_mut().inject(packet[..len].to_vec()).is_err() {
                            overflow += 1;
                        }
                    }
                    Ok(_) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) => return Err(e.into()),
                }
            }
            if began.elapsed() >= Duration::from_secs(timeout) {
                stopped = true;
            }
        } else if state.borrow().empty() && !exhausted {
            if let Some(frame) = inputs.next() {
                state.borrow_mut().inject(frame)?;
            } else {
                exhausted = true;
            }
        }
        if exhausted && state.borrow().empty() {
            stopped = true;
        }
        if stopped {
            state.borrow_mut().stop = true;
        }
        let result = machine.run(
            RunLimits {
                instructions: Some(10000),
                deadline: None,
            },
            None,
        )?;
        instructions += result.stats.instructions;
        if result.reason != StopReason::InstructionLimit {
            break result;
        }
        if (socket.is_none() && instructions > 200_000_000)
            || began.elapsed() > Duration::from_secs(timeout + 10)
        {
            return Err("guest did not terminate within run budget".into());
        }
    };
    if let Some(frame) = state.borrow_mut().take_tx() {
        outputs.push(frame);
    }
    pcap::write(Path::new(&args[3]), &outputs)?;
    if socket.is_some() {
        pcap::write(&Path::new(&args[3]).with_extension("rx.pcap"), &capture)?;
    }
    if let Some(probe) = &probe {
        pcap::write(
            &Path::new(&args[3]).with_extension("rx.pcap"),
            &probe.inputs,
        )?;
    }
    let symbols: BTreeMap<_, _> = image
        .symbols
        .iter()
        .map(|s| (s.name.clone(), s.address))
        .collect();
    let ram = machine.debug_read_memory(0x20000000, 2048)?;
    let report = serde_json::json!({"reason":final_result.reason,"instructions":instructions,"uart":String::from_utf8_lossy(&final_result.uart),
        "injected":state.borrow().received,"released":state.borrow().released,"transmitted":state.borrow().transmitted,
        "symbols":symbols,"ram_hex":ram.iter().map(|b|format!("{b:02x}")).collect::<String>(),"mode":args[5],"rx_overflow":overflow,"tcp_probe":probe.as_ref().map(|p|p.summary())});
    fs::write(&args[4], serde_json::to_vec_pretty(&report)?)?;
    println!(
        "guest {:?}: {} instructions, {} RX, {} TX",
        final_result.reason,
        instructions,
        state.borrow().received,
        state.borrow().transmitted
    );
    if final_result.reason != StopReason::Halted {
        return Err("guest fault".into());
    }
    // EBREAK is also used by fault handling. Halt alone is not a passing guest.
    for (name, expected) in [("FAULT_INFO", 0), ("SERVICES_STATUS", 0x600d)] {
        let address = image
            .symbols
            .iter()
            .find(|s| s.name == name)
            .ok_or("missing guest status")?
            .address;
        let bytes = machine.debug_read_memory(address, 4)?;
        if u32::from_le_bytes(bytes.try_into().unwrap()) != expected {
            return Err(format!("guest failed: {name}").into());
        }
    }
    Ok(())
}
