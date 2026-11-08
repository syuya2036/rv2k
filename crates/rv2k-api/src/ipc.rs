use crate::{
    syscall::{raw, Error, Number},
    TaskId,
};

pub fn send(id: TaskId, bytes: &[u8]) -> Result<(), Error> {
    unsafe {
        Error::decode(raw(
            Number::MessageSend as usize,
            [id.index(), bytes.as_ptr() as usize, bytes.len()],
        ))
        .map(|_| ())
    }
}

pub fn recv(out: &mut [u8]) -> Result<usize, Error> {
    unsafe {
        Error::decode(raw(
            Number::MessageRecv as usize,
            [out.as_mut_ptr() as usize, out.len(), 0],
        ))
    }
}
