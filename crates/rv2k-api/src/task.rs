use crate::{
    syscall::{raw, Error, Number},
    TaskId, TaskInfo,
};

pub fn yield_now() {
    unsafe {
        raw(Number::Yield as usize, [0; 3]);
    }
}

pub fn id() -> TaskId {
    // The kernel always returns one of the two fixed IDs for a running task.
    match unsafe { raw(Number::GetPid as usize, [0; 3]) } {
        0 => TaskId::Network,
        1 => TaskId::Application,
        _ => loop {
            core::hint::spin_loop();
        },
    }
}

pub fn info(id: TaskId) -> Result<TaskInfo, Error> {
    with_info(id, |info| *info)
}

/// Visit a task-local snapshot copy; no TCB reference crosses the syscall gate.
pub fn with_info<T>(id: TaskId, read: impl FnOnce(&TaskInfo) -> T) -> Result<T, Error> {
    let mut out = core::mem::MaybeUninit::<TaskInfo>::uninit();
    unsafe {
        Error::decode(raw(
            Number::TaskInfo as usize,
            [
                id.index(),
                out.as_mut_ptr() as usize,
                core::mem::size_of::<TaskInfo>(),
            ],
        ))?;
        Ok(read(out.assume_init_ref()))
    }
}

pub fn exit(code: i16) -> ! {
    unsafe {
        raw(Number::TaskExit as usize, [code as isize as usize, 0, 0]);
    }
    loop {
        core::hint::spin_loop();
    }
}
