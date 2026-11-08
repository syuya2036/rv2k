use core::{
    arch::{asm, global_asm},
    ptr::{addr_of, addr_of_mut, read_volatile, write_volatile},
};
use rv2k_api::mem::MemInfo;
use rv2k_arch::{stack::StackRegion, TrapFrame};
use rv2k_kernel::diagnostics::{FaultInfo, Reason};

#[no_mangle]
static mut FAULT_INFO: FaultInfo = FaultInfo::empty();
#[cfg(feature = "fault-uart-stuck")]
static mut STUCK_UART_STATUS: u32 = 0;
extern "C" {
    static _stack_bottom: u32;
    static _stack_top: u32;
    static _ebss: u32;
    static _flash_end: u32;
    fn rv2k_fault_entry(reason: u32, task: u32, pc: u32, cause: u32, tval: u32, sp: u32) -> !;
    fn rv2k_panic_entry(task: u32) -> !;
}

unsafe fn region(id: usize) -> StackRegion {
    if id < 2 {
        StackRegion::new(crate::services::stack_base().add(id * 64), 64)
    } else {
        StackRegion::new(
            addr_of!(_stack_bottom),
            (addr_of!(_stack_top) as usize - addr_of!(_stack_bottom) as usize) / 4,
        )
    }
}

pub fn check(frame: Option<&TrapFrame>) {
    let mut failures = 0;
    for id in 0..3 {
        if !unsafe { region(id) }.guard_ok() {
            failures |= 1 << id;
        }
    }
    if failures != 0 {
        fail(Reason::StackGuard, frame, failures);
    }
}

pub fn info() -> MemInfo {
    let mut used = [0; 3];
    let mut failures = 0;
    for id in 0..3 {
        let usage = unsafe { region(id) }.inspect();
        used[id] = usage.used;
        if !usage.guard_ok {
            failures |= 1 << id;
        }
    }
    MemInfo {
        version: 1,
        static_bytes: (addr_of!(_ebss) as usize - 0x20000000) as u16,
        kernel_reserved: (addr_of!(_stack_top) as usize - addr_of!(_stack_bottom) as usize) as u16,
        task_reserved: 256,
        used,
        guard_failures: failures,
        flash_bytes: addr_of!(_flash_end) as usize as u16,
        ram_limit: 2048,
        flash_limit: 16384,
        reserved: 0,
    }
}

pub fn fail(reason: Reason, frame: Option<&TrapFrame>, value: u32) -> ! {
    let task = crate::services::current_task().map_or(u32::MAX, |id| id.index() as u32);
    let (pc, cause, tval, sp) = if let Some(frame) = frame {
        (
            frame.pc,
            frame.cause,
            if value == 0 { frame.tval } else { value },
            frame.registers[14],
        )
    } else {
        let sp: u32;
        unsafe {
            asm!("mv {0}, sp", out(reg) sp);
        }
        (0, 0, value, sp)
    };
    unsafe {
        rv2k_fault_entry(reason as u32, task, pc, cause, tval, sp);
    }
}

pub fn kernel_fault(sp: u32) -> ! {
    let (pc, cause, tval): (u32, u32, u32);
    unsafe {
        asm!("csrr {0}, mepc", out(reg) pc);
        asm!("csrr {0}, mcause", out(reg) cause);
        asm!("csrr {0}, mtval", out(reg) tval);
        rv2k_fault_entry(Reason::KernelTrap as u32, u32::MAX, pc, cause, tval, sp);
    }
}

pub fn panic() -> ! {
    let task = crate::services::current_task().map_or(u32::MAX, |id| id.index() as u32);
    unsafe {
        rv2k_panic_entry(task);
    }
}

global_asm!(
    r#"
.section .text
.global rv2k_panic_entry
rv2k_panic_entry:
mv a1, a0
li a0, 1
li a2, 0
li a3, 0
li a4, 0
mv a5, sp
j rv2k_fault_entry
.global rv2k_fault_entry
rv2k_fault_entry:
csrci mstatus, 8
csrw mscratch, zero
la sp, _stack_top
.option push
.option norelax
la gp, __global_pointer$
.option pop
li tp, 0
j rv2k_record_fault
"#
);

#[no_mangle]
unsafe extern "C" fn rv2k_record_fault(
    reason: Reason,
    task: u32,
    pc: u32,
    cause: u32,
    tval: u32,
    sp: u32,
) -> ! {
    write_volatile(
        addr_of_mut!(FAULT_INFO),
        FaultInfo {
            reason,
            task,
            pc,
            cause,
            tval,
            sp,
        },
    );
    write_volatile(0xe000f000 as *mut u32, 0);
    for byte in b"FAULT " {
        put(*byte);
    }
    // FaultInfo is repr(C), six u32-sized fields, with a checked 24-byte size.
    // Print the saved record instead of retaining another six-word stack copy.
    for index in 0..6 {
        hex(read_volatile(addr_of!(FAULT_INFO).cast::<u32>().add(index)));
        put(b' ');
    }
    put(b'\n');
    #[cfg(feature = "emulator-stop")]
    asm!("ebreak");
    loop {
        core::hint::spin_loop();
    }
}

fn put(byte: u8) {
    let ready = rv2k_kernel::diagnostics::uart_ready(|| {
        #[cfg(not(feature = "fault-uart-stuck"))]
        let ready = unsafe { core::ptr::read_volatile(0x40013800 as *const u32) & (1 << 7) != 0 };
        #[cfg(feature = "fault-uart-stuck")]
        let ready = unsafe { core::ptr::read_volatile(addr_of!(STUCK_UART_STATUS)) != 0 };
        ready
    });
    if ready {
        unsafe {
            write_volatile(0x40013804 as *mut u32, byte as u32);
        }
    }
}
fn hex(value: u32) {
    for shift in (0..8).rev() {
        put(b"0123456789abcdef"[((value >> (shift * 4)) & 15) as usize]);
    }
}
