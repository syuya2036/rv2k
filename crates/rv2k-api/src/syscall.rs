//! RV32E ABI: a5=number, a0..a2=arguments, a0=result or negative Error.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Number {
    Yield = 0,
    Sleep,
    GetPid,
    Uptime,
    EventWait,
    EventSignal,
    Write,
    SocketListen,
    SocketRecv,
    SocketSend,
    SocketClose,
    TaskExit,
    TaskInfo,
    MemInfo,
    NetInfo,
    MessageSend,
    MessageRecv,
}

impl Number {
    pub fn decode(number: usize) -> Option<Self> {
        Some(match number {
            0 => Self::Yield,
            1 => Self::Sleep,
            2 => Self::GetPid,
            3 => Self::Uptime,
            4 => Self::EventWait,
            5 => Self::EventSignal,
            6 => Self::Write,
            7 => Self::SocketListen,
            8 => Self::SocketRecv,
            9 => Self::SocketSend,
            10 => Self::SocketClose,
            11 => Self::TaskExit,
            12 => Self::TaskInfo,
            13 => Self::MemInfo,
            14 => Self::NetInfo,
            15 => Self::MessageSend,
            16 => Self::MessageRecv,
            _ => return None,
        })
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Invalid = 1,
    NoSys,
    BadFd,
    Again,
    Timeout,
    Busy,
    Closed,
    Fault,
    MessageSize,
    Io,
}

impl Error {
    pub const fn code(self) -> isize {
        -(self as isize)
    }
    pub fn decode(result: isize) -> Result<usize, Self> {
        if result >= 0 {
            return Ok(result as usize);
        }
        Err(match result {
            -1 => Self::Invalid,
            -2 => Self::NoSys,
            -3 => Self::BadFd,
            -4 => Self::Again,
            -5 => Self::Timeout,
            -6 => Self::Busy,
            -7 => Self::Closed,
            -8 => Self::Fault,
            -9 => Self::MessageSize,
            _ => Self::Io,
        })
    }
}

/// Invoke the RV32E syscall gate. The trap preserves all registers except a0.
///
/// # Safety
/// Pointer arguments must have the lifetime, alignment and access rights required
/// by the selected call, including unique access to output objects.
#[cfg(target_arch = "riscv32")]
#[inline]
pub unsafe fn raw(number: usize, args: [usize; 3]) -> isize {
    let mut result = args[0];
    core::arch::asm!("ecall", inlateout("a0") result,
        in("a1") args[1], in("a2") args[2], in("a5") number);
    result as isize
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abi_numbers_and_errors() {
        for n in 0..=16 {
            assert_eq!(Number::decode(n).unwrap() as usize, n);
        }
        assert_eq!(Number::decode(17), None);
        assert_eq!(Number::decode(usize::MAX), None);
        for (n, e) in [
            Error::Invalid,
            Error::NoSys,
            Error::BadFd,
            Error::Again,
            Error::Timeout,
            Error::Busy,
            Error::Closed,
            Error::Fault,
            Error::MessageSize,
            Error::Io,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(e.code(), -(n as isize + 1));
            assert_eq!(Error::decode(e.code()), Err(e));
        }
    }
}
