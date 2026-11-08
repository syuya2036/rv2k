use crate::syscall::{raw, Error, Number};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Handle(usize);
pub fn listen(port: u16) -> Result<Handle, Error> {
    unsafe { Error::decode(raw(Number::SocketListen as usize, [port as usize, 0, 0])).map(Handle) }
}
pub fn recv(handle: Handle, out: &mut [u8]) -> Result<usize, Error> {
    unsafe {
        Error::decode(raw(
            Number::SocketRecv as usize,
            [handle.0, out.as_mut_ptr() as usize, out.len()],
        ))
    }
}
pub fn send(handle: Handle, bytes: &[u8]) -> Result<usize, Error> {
    unsafe {
        Error::decode(raw(
            Number::SocketSend as usize,
            [handle.0, bytes.as_ptr() as usize, bytes.len()],
        ))
    }
}
pub fn close(handle: Handle) -> Result<(), Error> {
    unsafe { Error::decode(raw(Number::SocketClose as usize, [handle.0, 0, 0])).map(|_| ()) }
}
