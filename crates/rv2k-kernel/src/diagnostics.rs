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
            // Fields carry meaning only when reason != None. An all-zero
            // empty record can live in BSS rather than consume Flash.
            task: 0,
            pc: 0,
            cause: 0,
            tval: 0,
            sp: 0,
        }
    }
}
const _: () = assert!(core::mem::size_of::<FaultInfo>() == 24);

/// At BRR=208 a 10-bit UART character takes 2080 peripheral clocks. Each
/// volatile ready poll executes at least one CPU instruction; 4096 polls cover
/// a full character with the configured equal 24MHz CPU/peripheral clocks.
pub fn uart_ready(mut ready: impl FnMut() -> bool) -> bool {
    for _ in 0..4096 {
        if ready() {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uart_wait_covers_a_character_and_still_terminates() {
        let mut polls = 0;
        assert!(uart_ready(|| {
            polls += 1;
            polls > 2080
        }));
        assert_eq!(polls, 2081);
        polls = 0;
        assert!(!uart_ready(|| {
            polls += 1;
            false
        }));
        assert_eq!(polls, 4096);
    }
}
