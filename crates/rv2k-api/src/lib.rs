#![no_std]

#[cfg(target_arch = "riscv32")]
pub mod console;
pub mod event;
#[cfg(target_arch = "riscv32")]
pub mod ipc;
pub mod mem;
pub mod syscall;
#[cfg(target_arch = "riscv32")]
pub mod task;
#[cfg(target_arch = "riscv32")]
pub mod time;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskId {
    Network = 0,
    Application = 1,
}

impl TaskId {
    pub const fn index(self) -> usize {
        self as usize
    }
    pub const fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::Network),
            1 => Some(Self::Application),
            _ => None,
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    Ready,
    Running,
    Blocked,
    Exited,
}

/// A copy of task state; applications cannot mutate kernel task records.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskInfo {
    pub version: u8,
    pub id: TaskId,
    pub state: TaskState,
    pub reserved: u8,
    pub exit_code: i16,
}

const _: () = assert!(core::mem::size_of::<TaskInfo>() == 6);
