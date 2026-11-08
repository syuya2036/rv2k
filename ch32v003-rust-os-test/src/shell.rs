//! Plain TCP shell. Snapshot acquisition and I/O use only public OS APIs.
use core::ptr::{addr_of, addr_of_mut};
use rv2k_api::{
    event, mem, net,
    socket::{self, Handle},
    syscall::Error,
    task, time, TaskId,
};
use rv2k_apps::{
    monitor,
    shell::{Command, Line},
};
struct Application {
    input: Line,
    output: monitor::Line,
}
static mut APP: Application = Application {
    input: Line::new(),
    output: monitor::Line::new(),
};
#[cfg(feature = "emulator-stop")]
#[no_mangle]
static mut APP_LISTENS: u32 = 0;
#[cfg(feature = "emulator-stop")]
#[no_mangle]
static mut APP_SHELL_ACCEPTS: u32 = 0;
#[cfg(feature = "emulator-stop")]
#[no_mangle]
static mut APP_SHELL_INPUT_BYTES: u32 = 0;
#[cfg(feature = "emulator-stop")]
#[no_mangle]
static mut APP_SHELL_COMMANDS: u32 = 0;
pub fn region() -> (usize, usize) {
    let start = addr_of!(APP) as usize;
    (start, start + core::mem::size_of::<Application>())
}
#[inline(never)]
fn now() -> u32 {
    time::uptime().unwrap_or(0) as u32
}
fn remaining(deadline: u32) -> u32 {
    let left = deadline.wrapping_sub(now());
    if left > 500 {
        0
    } else {
        left
    }
}
struct Sender {
    handle: Handle,
    deadline: u32,
}
impl Sender {
    fn new(handle: Handle) -> Self {
        Self {
            handle,
            deadline: now().wrapping_add(500),
        }
    }
    #[inline(never)]
    fn send(&self, mut bytes: &[u8]) -> bool {
        while !bytes.is_empty() {
            let left = remaining(self.deadline);
            if left == 0 {
                return false;
            }
            match socket::send(self.handle, bytes) {
                Ok(0) => return false,
                Ok(n) => bytes = &bytes[n..],
                Err(Error::Again) => {
                    if event::wait_until(event::NET_TX_READY | event::NET_CLOSED, self.deadline)
                        .is_err()
                    {
                        return false;
                    }
                }
                Err(_) => return false,
            }
        }
        true
    }
}
#[inline(never)]
fn ps(sender: &Sender, out: &mut monitor::Line) -> bool {
    for id in [TaskId::Network, TaskId::Application] {
        match task::with_info(id, |info| {
            out.task(info);
            sender.send(out.bytes())
        }) {
            Ok(true) => {}
            Ok(false) => return false,
            Err(_) => return sender.send(b"error: snapshot\r\n"),
        }
    }
    true
}
#[inline(never)]
fn memory(sender: &Sender, out: &mut monitor::Line) -> bool {
    match mem::with_info(|info| {
        for part in 0..3 {
            out.memory(info, part);
            if !sender.send(out.bytes()) {
                return false;
            }
        }
        true
    }) {
        Ok(result) => result,
        Err(_) => sender.send(b"error: snapshot\r\n"),
    }
}
#[inline(never)]
fn uptime(sender: &Sender, out: &mut monitor::Line) -> bool {
    let value = match time::uptime() {
        Ok(value) => value,
        Err(_) => return sender.send(b"error: snapshot\r\n"),
    };
    out.uptime(value);
    sender.send(out.bytes())
}
#[inline(never)]
fn netstat(sender: &Sender, out: &mut monitor::Line) -> bool {
    match net::with_info(|info| {
        for part in 0..4 {
            out.network(info, part);
            if !sender.send(out.bytes()) {
                return false;
            }
        }
        true
    }) {
        Ok(result) => result,
        Err(_) => sender.send(b"error: snapshot\r\n"),
    }
}
#[inline(never)]
fn command(sender: &Sender, out: &mut monitor::Line, command: Command) -> bool {
    match command {
        Command::Ps => ps(sender, out),
        Command::Mem => memory(sender, out),
        Command::Uptime => uptime(sender, out),
        Command::Netstat => netstat(sender, out),
        Command::Help => sender.send(b"ps mem uptime netstat help quit\r\n"),
        Command::Quit => sender.send(b"bye\r\n"),
        Command::Empty => true,
        Command::Unknown => sender.send(b"error: unknown command\r\n"),
        Command::TooLong => sender.send(b"error: line too long\r\n"),
        Command::InvalidInput => sender.send(b"error: invalid input\r\n"),
    }
}
#[inline(never)]
fn finish(handle: Handle) {
    // Permit peer-first close, draining bytes after quit without executing them.
    let deadline = now().wrapping_add(100);
    let mut byte = [0u8; 1];
    loop {
        let left = remaining(deadline);
        if left == 0 {
            break;
        }
        match socket::recv(handle, &mut byte) {
            Ok(0) => break,
            Ok(_) => {}
            Err(Error::Again) => {
                if event::wait_until(event::NET_RX | event::NET_CLOSED, deadline).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let _ = socket::close(handle);
}
#[inline(never)]
fn connection(handle: Handle) {
    let app = unsafe { &mut *addr_of_mut!(APP) };
    app.input.reset();
    if !Sender::new(handle).send(b"rv2k shell\r\n> ") {
        return;
    }
    let mut byte = [0u8; 1];
    loop {
        match socket::recv(handle, &mut byte) {
            Ok(0) => break,
            Ok(_) => {
                #[cfg(feature = "emulator-stop")]
                unsafe {
                    core::ptr::write_volatile(
                        addr_of_mut!(APP_SHELL_INPUT_BYTES),
                        core::ptr::read_volatile(addr_of!(APP_SHELL_INPUT_BYTES)) + 1,
                    );
                }
                if let Some(cmd) = app.input.push(byte[0]) {
                    #[cfg(feature = "emulator-stop")]
                    if cmd != Command::Empty {
                        unsafe {
                            core::ptr::write_volatile(
                                addr_of_mut!(APP_SHELL_COMMANDS),
                                core::ptr::read_volatile(addr_of!(APP_SHELL_COMMANDS)) + 1,
                            );
                        }
                    }
                    let sender = Sender::new(handle);
                    if !command(&sender, &mut app.output, cmd)
                        || cmd == Command::Quit
                        || !sender.send(b"> ")
                    {
                        break;
                    }
                }
            }
            Err(Error::Again) => {
                if event::wait(event::NET_RX | event::NET_CLOSED, event::FOREVER).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}
#[inline(never)]
pub fn run() {
    'listen: loop {
        let handle = match socket::listen(23) {
            Ok(h) => h,
            Err(Error::Closed) => return,
            Err(Error::Busy) => {
                let _ = event::wait(event::NET_CLOSED, 500);
                continue;
            }
            Err(_) => panic!("shell listen"),
        };
        #[cfg(feature = "emulator-stop")]
        unsafe {
            core::ptr::write_volatile(
                addr_of_mut!(APP_LISTENS),
                core::ptr::read_volatile(addr_of!(APP_LISTENS)) + 1,
            );
        }
        loop {
            match event::wait(event::NET_CONNECTED | event::NET_CLOSED, event::FOREVER) {
                Ok(bits) if bits & event::NET_CONNECTED != 0 => break,
                Ok(_) if socket::recv(handle, &mut []).is_ok() => {}
                Ok(_) => continue 'listen,
                Err(_) => return,
            }
        }
        #[cfg(feature = "emulator-stop")]
        unsafe {
            core::ptr::write_volatile(
                addr_of_mut!(APP_SHELL_ACCEPTS),
                core::ptr::read_volatile(addr_of!(APP_SHELL_ACCEPTS)) + 1,
            );
        }
        connection(handle);
        finish(handle);
    }
}
