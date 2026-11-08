use crate::syscall::{raw, Error, Number};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Handle(usize);
#[inline(always)]
pub fn listen(port: u16) -> Result<Handle, Error> {
    unsafe { Error::decode(raw(Number::SocketListen as usize, [port as usize, 0, 0])).map(Handle) }
}
#[inline(always)]
/// An empty destination validates the handle and returns zero without reading.
/// With a nonempty destination, zero denotes EOF after all queued bytes.
pub fn recv(handle: Handle, out: &mut [u8]) -> Result<usize, Error> {
    unsafe {
        Error::decode(raw(
            Number::SocketRecv as usize,
            [handle.0, out.as_mut_ptr() as usize, out.len()],
        ))
    }
}
#[inline(always)]
pub fn send(handle: Handle, bytes: &[u8]) -> Result<usize, Error> {
    unsafe {
        Error::decode(raw(
            Number::SocketSend as usize,
            [handle.0, bytes.as_ptr() as usize, bytes.len()],
        ))
    }
}
#[inline(always)]
pub fn close(handle: Handle) -> Result<(), Error> {
    unsafe { Error::decode(raw(Number::SocketClose as usize, [handle.0, 0, 0])).map(|_| ()) }
}
