use crate::{memory::UserMemory, Action, Scheduler};
use rv2k_api::{
    syscall::{Error, Number},
    TaskId, TaskInfo,
};

#[derive(Clone, Copy)]
pub enum Outcome {
    Complete(isize),
    Switch(Action),
}

/// Phase 2 implements task operations. Later-phase numbers are reserved and
/// return NoSys until their services are integrated, rather than false success.
pub fn dispatch(
    scheduler: &Scheduler,
    memory: &mut impl UserMemory,
    number: usize,
    args: [usize; 3],
) -> Outcome {
    let Some(caller) = scheduler.current() else {
        return Outcome::Complete(Error::Invalid.code());
    };
    let Some(number) = Number::decode(number) else {
        return Outcome::Complete(Error::NoSys.code());
    };
    let result = match number {
        Number::Yield => return Outcome::Switch(Action::Yield),
        Number::GetPid => Ok(caller.index()),
        Number::TaskExit => {
            let code = args[0] as isize;
            if code < i16::MIN as isize || code > i16::MAX as isize {
                Err(Error::Invalid)
            } else {
                return Outcome::Switch(Action::Exit(code as i16));
            }
        }
        Number::TaskInfo => task_info(scheduler, memory, args),
        _ => Err(Error::NoSys),
    };
    Outcome::Complete(match result {
        Ok(value) => value as isize,
        Err(error) => error.code(),
    })
}

pub(crate) fn task_info(
    scheduler: &Scheduler,
    memory: &mut impl UserMemory,
    args: [usize; 3],
) -> Result<usize, Error> {
    let id = TaskId::from_index(args[0]).ok_or(Error::Invalid)?;
    if args[2] != core::mem::size_of::<TaskInfo>() {
        return Err(Error::Invalid);
    }
    let info = scheduler.info(id);
    info.with_bytes(|bytes| memory.copy_out(args[1], bytes, core::mem::align_of::<TaskInfo>()))?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Output {
        bytes: [u8; 6],
    }
    impl UserMemory for Output {
        fn copy_out(
            &mut self,
            address: usize,
            bytes: &[u8],
            alignment: usize,
        ) -> Result<(), Error> {
            if address != 0x1000 || alignment != 2 {
                return Err(Error::Fault);
            }
            self.bytes.copy_from_slice(bytes);
            Ok(())
        }
    }
    #[test]
    fn every_number_and_invalid_snapshot() {
        let mut s = Scheduler::new();
        s.initialize([0x100, 0x200]);
        s.select_next();
        let mut mem = Output { bytes: [0xcc; 6] };
        for n in 0..=17 {
            let out = dispatch(&s, &mut mem, n, [0, 0x1000, 6]);
            match n {
                0 => assert!(matches!(out, Outcome::Switch(Action::Yield))),
                2 | 12 => assert!(matches!(out, Outcome::Complete(0))),
                11 => assert!(matches!(out, Outcome::Switch(Action::Exit(0)))),
                _ => assert!(matches!(out, Outcome::Complete(-2))),
            }
        }
        assert_eq!(mem.bytes, [1, 0, 1, 0, 0, 0]);
        for (args, error) in [
            ([2, 0x1000, 6], -1),
            ([0, 0x1000, 5], -1),
            ([0, 0x1001, 6], -8),
            ([0, usize::MAX, 6], -8),
        ] {
            assert!(matches!(dispatch(&s, &mut mem, 12, args), Outcome::Complete(e) if e == error));
        }
        assert!(matches!(
            dispatch(&s, &mut mem, 11, [32768, 0, 0]),
            Outcome::Complete(-1)
        ));
        assert!(matches!(
            dispatch(&s, &mut mem, 11, [(-7isize) as usize, 0, 0]),
            Outcome::Switch(Action::Exit(-7))
        ));
    }
}
