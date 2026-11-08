//! One HTTP request per connection, using only public task/socket/time APIs.
use core::ptr::{addr_of, addr_of_mut};
use rv2k_api::{
    event,
    socket::{self, Handle},
    syscall::Error,
};
use rv2k_apps::http::{Request, Status, IO_CAPACITY};
struct Application {
    request: Request,
    io: [u8; IO_CAPACITY],
}
static mut APP: Application = Application {
    request: Request::new(),
    io: [0; IO_CAPACITY],
};
#[cfg(feature = "emulator-stop")]
#[no_mangle]
static mut HTTP_ACCEPTS: u32 = 0;
#[cfg(feature = "emulator-stop")]
#[no_mangle]
static mut APP_LISTENS: u32 = 0;
#[cfg(feature = "emulator-stop")]
#[no_mangle]
static mut APP_BYTES: u32 = 0;
#[cfg(feature = "emulator-stop")]
#[no_mangle]
static mut HTTP_REQUEST_BYTES: u32 = 0;
#[cfg(feature = "emulator-stop")]
#[no_mangle]
static mut HTTP_RESPONSES: u32 = 0;
#[cfg(feature = "emulator-stop")]
#[no_mangle]
static mut HTTP_LAST_READ_TICKS: u32 = 0;
pub fn region() -> (usize, usize) {
    let start = addr_of!(APP) as usize;
    (start, start + core::mem::size_of::<Application>())
}
#[inline(never)]
fn now() -> u32 {
    rv2k_api::time::uptime().unwrap_or(0) as u32
}
// Zero denotes an expired deadline; a valid remaining interval is 1..=500.
fn remaining(deadline: u32) -> u32 {
    let left = deadline.wrapping_sub(now());
    if left > 500 {
        0
    } else {
        left
    }
}
fn wait(mask: u32, deadline: u32) -> Result<(), Error> {
    event::wait_until(mask | event::NET_CLOSED, deadline).map(|_| ())
}

#[inline(never)]
fn read_request(handle: Handle, app: &mut Application, deadline: u32) -> Option<Status> {
    loop {
        if remaining(deadline) == 0 {
            return Some(app.request.timeout());
        }
        match socket::recv(handle, &mut app.io) {
            Ok(0) => return Some(app.request.eof()),
            Ok(count) => {
                #[cfg(feature = "emulator-stop")]
                unsafe {
                    core::ptr::write_volatile(
                        addr_of_mut!(HTTP_REQUEST_BYTES),
                        core::ptr::read_volatile(addr_of!(HTTP_REQUEST_BYTES)) + count as u32,
                    );
                }
                if let Some(status) = app.request.feed(&app.io[..count]) {
                    return Some(status);
                }
            }
            Err(Error::Again) => match wait(event::NET_RX, deadline) {
                Ok(()) => {}
                Err(Error::Timeout) => return Some(app.request.timeout()),
                Err(_) => return None,
            },
            Err(_) => return None,
        }
    }
}
#[inline(never)]
fn send_all(handle: Handle, mut bytes: &[u8], deadline: u32) -> bool {
    while !bytes.is_empty() {
        if remaining(deadline) == 0 {
            return false;
        }
        match socket::send(handle, bytes) {
            Ok(0) => return false,
            Ok(n) => bytes = &bytes[n..],
            Err(Error::Again) => {
                if wait(event::NET_TX_READY, deadline).is_err() {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
    true
}
#[inline(never)]
fn respond(
    handle: Handle,
    status: Status,
    body: &[u8],
    io: &mut [u8; IO_CAPACITY],
    deadline: u32,
) -> bool {
    let len = rv2k_apps::http::response_header(status, body.len() as u8, io);
    send_all(handle, &io[..len], deadline) && send_all(handle, body, deadline)
}

#[inline(never)]
pub fn run() {
    'listen: loop {
        let handle = match socket::listen(80) {
            Ok(h) => h,
            Err(Error::Closed) => return,
            Err(Error::Busy) => {
                let _ = event::wait(event::NET_CLOSED, 500);
                continue;
            }
            Err(_) => panic!("HTTP listen"),
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
                // A delayed close event may belong to the previous connection.
                // A zero-length recv checks this handle without consuming data.
                Ok(_) if socket::recv(handle, &mut []).is_ok() => {}
                Ok(_) => continue 'listen,
                Err(_) => return,
            }
        }
        let app = unsafe { &mut *addr_of_mut!(APP) };
        app.request.reset();
        let deadline = now().wrapping_add(500);
        #[cfg(feature = "emulator-stop")]
        unsafe {
            core::ptr::write_volatile(
                addr_of_mut!(HTTP_ACCEPTS),
                core::ptr::read_volatile(addr_of!(HTTP_ACCEPTS)) + 1,
            );
        }
        if let Some(status) = read_request(handle, app, deadline) {
            #[cfg(feature = "emulator-stop")]
            unsafe {
                core::ptr::write_volatile(
                    addr_of_mut!(HTTP_LAST_READ_TICKS),
                    now().wrapping_sub(deadline.wrapping_sub(500)),
                );
            }
            let body = app.request.body();
            if respond(handle, status, body, &mut app.io, now().wrapping_add(500)) {
                #[cfg(feature = "emulator-stop")]
                unsafe {
                    core::ptr::write_volatile(
                        addr_of_mut!(APP_BYTES),
                        core::ptr::read_volatile(addr_of!(APP_BYTES)) + body.len() as u32,
                    );
                    core::ptr::write_volatile(
                        addr_of_mut!(HTTP_RESPONSES),
                        core::ptr::read_volatile(addr_of!(HTTP_RESPONSES)) + 1,
                    );
                }
            }
        }
        // A length-delimited Connection: close client can close after its full
        // response. Allow its FIN and drain unread request octets before our
        // close, avoiding a reset that could discard the response. Fall back
        // to active close after one bounded second; TCP keeps its TIME_WAIT.
        let deadline = now().wrapping_add(100);
        loop {
            match socket::recv(handle, &mut app.io) {
                Ok(0) => break,
                Ok(_) => {
                    if remaining(deadline) == 0 {
                        break;
                    }
                }
                Err(Error::Again) => {
                    if wait(event::NET_RX, deadline).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = socket::close(handle);
    }
}
