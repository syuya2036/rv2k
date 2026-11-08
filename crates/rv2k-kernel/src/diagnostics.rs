#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reason {
    None = 0,
    Panic,
    StackGuard,
    InvalidSp,
    TaskTrap,
    KernelTrap,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FaultInfo {
    pub reason: Reason,
    pub task: u32,
    pub pc: u32,
    pub cause: u32,
    pub tval: u32,
    pub sp: u32,
}
impl FaultInfo {
    pub const fn empty() -> Self {
        Self {
            reason: Reason::None,
            task: u32::MAX,
            pc: 0,
            cause: 0,
            tval: 0,
            sp: 0,
        }
    }
}
const _: () = assert!(core::mem::size_of::<FaultInfo>() == 24);
