//! Full service dispatcher. The Phase 2 task-only dispatcher is retained for its
//! independent ABI regression probe.
use crate::{
    ipc::Mailbox,
    memory::UserMemory,
    syscall::{dispatch as task_dispatch, Outcome},
    Action, Scheduler,
};
use rv2k_api::{
    syscall::{Error, Number},
    TaskId,
};

pub fn dispatch(
    scheduler: &mut Scheduler,
    mailbox: &mut Mailbox,
    memory: &mut impl UserMemory,
    number: usize,
    args: [usize; 3],
) -> Outcome {
    let Some(caller) = scheduler.current() else {
        return Outcome::Complete(Error::Invalid.code());
    };
    let result = match Number::decode(number) {
        Some(Number::Sleep) => match u32::try_from(args[0])
            .ok()
            .and_then(|ticks| scheduler.sleep(ticks).ok())
        {
            Some(action) => return Outcome::Switch(action),
            None => Err(Error::Invalid),
        },
        Some(Number::Uptime) => {
            if args[1] != 8 {
                Err(Error::Invalid)
            } else {
                memory
                    .copy_out(
                        args[0],
                        &scheduler.uptime().to_le_bytes(),
                        core::mem::align_of::<u64>(),
                    )
                    .map(|_| 0)
            }
        }
        Some(Number::EventWait) => {
            if args[0] > u32::MAX as usize || args[1] > u32::MAX as usize {
                Err(Error::Invalid)
            } else {
                match scheduler.wait_event(
                    args[0] as u32,
                    args[1] as u32,
                    mailbox.readiness(caller),
                ) {
                    Ok(Some(bits)) => Ok(bits as usize),
                    Ok(None) => return Outcome::Switch(Action::Block),
                    Err(error) => Err(error),
                }
            }
        }
        Some(Number::EventSignal) => match TaskId::from_index(args[0]) {
            Some(id) if args[1] <= u32::MAX as usize => {
                scheduler.signal(id, args[1] as u32).map(|_| 0)
            }
            _ => Err(Error::Invalid),
        },
        Some(Number::MessageSend) => match TaskId::from_index(args[0]) {
            Some(id) => mailbox.send(scheduler, memory, id, args[1], args[2]),
            None => Err(Error::Invalid),
        },
        Some(Number::MessageRecv) => mailbox.recv(scheduler, memory, args[0], args[1]),
        _ => return task_dispatch(scheduler, memory, number, args),
    };
    Outcome::Complete(match result {
        Ok(value) => value as isize,
        Err(error) => error.code(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rv2k_api::event;
    struct Memory([u8; 64]);
    impl UserMemory for Memory {
        fn copy_out(
            &mut self,
            address: usize,
            bytes: &[u8],
            alignment: usize,
        ) -> Result<(), Error> {
            if address != 0x1000 || !address.is_multiple_of(alignment) {
                return Err(Error::Fault);
            }
            self.0[..bytes.len()].copy_from_slice(bytes);
            Ok(())
        }
        fn copy_in(&mut self, address: usize, bytes: &mut [u8]) -> Result<(), Error> {
            if address != 0x1000 {
                return Err(Error::Fault);
            }
            bytes.copy_from_slice(&self.0[..bytes.len()]);
            Ok(())
        }
    }
    #[test]
    fn syscall_timer_event_and_message_routes() {
        let mut s = Scheduler::new();
        s.initialize([0x100, 0x200]);
        s.select_next();
        let mut m = Mailbox::new();
        let mut mem = Memory([0; 64]);
        s.advance_ticks(0xffff_ffff);
        s.advance_ticks(2);
        assert!(matches!(
            dispatch(&mut s, &mut m, &mut mem, 3, [0x1000, 8, 0]),
            Outcome::Complete(0)
        ));
        assert_eq!(
            u64::from_le_bytes(mem.0[..8].try_into().unwrap()),
            0x1_0000_0001
        );
        for (number, args, result) in [
            (3, [0x1000, 7, 0], -1),
            (3, [0x1001, 8, 0], -8),
            (1, [0x8000_0000, 0, 0], -1),
            (4, [0, 1, 0], -1),
            (5, [2, 1, 0], -1),
            (5, [0, 1 << 31, 0], -1),
            (6, [0; 3], -2),
        ] {
            assert!(
                matches!(dispatch(&mut s, &mut m, &mut mem, number, args), Outcome::Complete(v) if v == result)
            );
        }
        assert!(matches!(
            dispatch(&mut s, &mut m, &mut mem, 1, [0; 3]),
            Outcome::Switch(Action::Yield)
        ));
        assert!(
            matches!(dispatch(&mut s, &mut m, &mut mem, 4, [event::DRIVER_POLL as usize,0,0]), Outcome::Complete(v) if v == event::DRIVER_POLL as isize)
        );
        assert!(matches!(
            dispatch(&mut s, &mut m, &mut mem, 15, [0, 0x1000, 8]),
            Outcome::Complete(0)
        ));
        assert!(
            matches!(dispatch(&mut s, &mut m, &mut mem, 4, [event::MSG_AVAILABLE as usize,0,0]), Outcome::Complete(v) if v == event::MSG_AVAILABLE as isize)
        );
        mem.0.fill(0);
        assert!(matches!(
            dispatch(&mut s, &mut m, &mut mem, 16, [0x1000, 8, 0]),
            Outcome::Complete(8)
        ));
        assert_eq!(
            u64::from_le_bytes(mem.0[..8].try_into().unwrap()),
            0x1_0000_0001
        );
    }
}
