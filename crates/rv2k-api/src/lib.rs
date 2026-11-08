#![no_std]

#[cfg(target_arch = "riscv32")]
pub mod console;
pub mod event;
#[doc(hidden)]
pub mod integer;
#[cfg(target_arch = "riscv32")]
pub mod ipc;
pub mod mem;
pub mod net;
#[cfg(target_arch = "riscv32")]
pub mod socket;
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
impl TaskInfo {
    pub fn with_bytes<T>(&self, visit: impl FnOnce(&[u8]) -> T) -> T {
        #[cfg(target_endian = "little")]
        {
            // repr(C), four u8-sized fields followed by i16, size=6: no padding.
            visit(unsafe { core::slice::from_raw_parts((self as *const Self).cast(), 6) })
        }
        #[cfg(target_endian = "big")]
        {
            let code = self.exit_code.to_le_bytes();
            visit(&[
                self.version,
                self.id as u8,
                self.state as u8,
                self.reserved,
                code[0],
                code[1],
            ])
        }
    }
}

#[cfg(test)]
mod task_info_tests {
    use super::*;
    #[test]
    fn task_info_wire_image_has_no_padding_and_signed_exit_is_little_endian() {
        for id in [TaskId::Network, TaskId::Application] {
            for state in [
                TaskState::Ready,
                TaskState::Running,
                TaskState::Blocked,
                TaskState::Exited,
            ] {
                for exit_code in [i16::MIN, -7, 0, i16::MAX] {
                    let info = TaskInfo {
                        version: 1,
                        id,
                        state,
                        reserved: 0,
                        exit_code,
                    };
                    let code = exit_code.to_le_bytes();
                    info.with_bytes(|bytes| {
                        assert_eq!(bytes, [1, id as u8, state as u8, 0, code[0], code[1]])
                    });
                }
            }
        }
    }
}
