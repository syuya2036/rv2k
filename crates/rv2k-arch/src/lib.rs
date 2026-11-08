#![no_std]

#[cfg(target_arch = "riscv32")]
core::arch::global_asm!(include_str!("trap.S"));

pub const FRAME_BYTES: usize = 80;
pub const STACK_GUARD: usize = 16;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TrapFrame {
    /// ra, gp, tp, t0..t2, s0..s1, a0..a5, original sp.
    pub registers: [u32; 15],
    pub pc: u32,
    pub status: u32,
    pub cause: u32,
    pub tval: u32,
    pub reserved: u32,
}

const _: () = assert!(core::mem::size_of::<TrapFrame>() == FRAME_BYTES);

impl TrapFrame {
    pub const fn initial(pc: u32, sp: u32) -> Self {
        let mut registers = [0; 15];
        registers[14] = sp;
        Self {
            registers,
            pc,
            status: 0x1800,
            cause: 0,
            tval: 0,
            reserved: 0,
        }
    }
    pub fn advance_ecall(&mut self) -> bool {
        if self.cause != 11 {
            return false;
        }
        if let Some(pc) = self.pc.checked_add(4) {
            self.pc = pc;
            true
        } else {
            false
        }
    }
}

/// Check original task sp before storing anything to the task stack.
pub fn frame_address(sp: usize, stack_base: usize, stack_top: usize) -> Option<usize> {
    let lowest = stack_base.checked_add(STACK_GUARD)?;
    let frame = sp.checked_sub(FRAME_BYTES)?;
    if sp & 3 == 0 && frame >= lowest && sp <= stack_top {
        Some(frame)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_layout_pc_and_bad_stack() {
        let mut f = TrapFrame::initial(0x100, 0x1100);
        assert_eq!(core::mem::offset_of!(TrapFrame, pc), 60);
        assert_eq!(core::mem::offset_of!(TrapFrame, status), 64);
        assert_eq!(core::mem::offset_of!(TrapFrame, cause), 68);
        assert_eq!(core::mem::offset_of!(TrapFrame, tval), 72);
        assert!(!f.advance_ecall());
        f.cause = 11;
        assert!(f.advance_ecall());
        assert_eq!(f.pc, 0x104);
        f.pc = u32::MAX - 2;
        assert!(!f.advance_ecall());
        for (sp, expected) in [
            (0x1100, Some(0x10b0)),
            (0x1060, Some(0x1010)),
            (0x105c, None),
            (0x1104, None),
            (0x10ff, None),
            (0, None),
        ] {
            assert_eq!(frame_address(sp, 0x1000, 0x1100), expected);
        }
    }
}
