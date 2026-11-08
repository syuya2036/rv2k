//! RV32EC integration probe; applications call only rv2k-api wrappers.
use core::{
    arch::{asm, global_asm},
    ptr::{addr_of, addr_of_mut, read_volatile, write_volatile},
};
use rv2k_api::{
    syscall::{raw, Error},
    task, TaskId, TaskState,
};
use rv2k_arch::{frame_address, TrapFrame, FRAME_BYTES};
use rv2k_kernel::{
    memory::{MemoryMap, Region, UserMemory},
    syscall::{dispatch, Outcome},
    Scheduler, Selection,
};

const WATERMARK: u32 = 0xa5a5a5a5;
#[repr(C, align(16))]
struct Stack([u32; 64]);
#[no_mangle]
static mut TASK_STACKS: [Stack; 2] = [Stack([0; 64]), Stack([0; 64])];
static mut SCHEDULER: Scheduler = Scheduler::new();
#[no_mangle]
static mut SYSCALL_STATUS: u32 = 0;
#[no_mangle]
static mut COUNTERS: [u32; 2] = [0; 2];
#[no_mangle]
static mut TRAPS: u32 = 0;
#[no_mangle]
static mut CALL_MASK: u32 = 0;
#[no_mangle]
static mut STACK_USED: [u32; 3] = [0; 3];
#[no_mangle]
static mut LAST_CAUSE: u32 = 0;
#[no_mangle]
static mut LAST_PC: u32 = 0;
extern "C" {
    fn rv2k_trap_entry();
    fn rv2k_first_task(sp: usize) -> !;
    fn syscall_probe_a() -> u32;
    fn syscall_probe_b() -> u32;
    fn getpid_probe_a() -> u32;
    fn getpid_probe_b() -> u32;
    static _stack_bottom: u32;
    static _stack_top: u32;
}

fn stack_bounds(id: TaskId) -> (usize, usize) {
    let base = addr_of!(TASK_STACKS) as usize + id.index() * 256;
    (base, base + 256)
}

pub fn start() -> ! {
    unsafe {
        asm!("csrci mstatus, 8", "csrw mscratch, zero");
        let mut frames = [0; 2];
        for id in 0..2 {
            let base = addr_of_mut!(TASK_STACKS).cast::<u32>().add(id * 64);
            for word in 0..64 {
                write_volatile(base.add(word), WATERMARK);
            }
            let sp = base.add(64) as usize;
            let frame = sp - FRAME_BYTES;
            (frame as *mut TrapFrame).write(TrapFrame::initial(
                task_trampoline as *const () as u32,
                sp as u32,
            ));
            frames[id] = frame;
        }
        // Baseline development board UART; J4M6 remapping belongs to Phase 5.
        write_volatile(0x40021018 as *mut u32, (1 << 0) | (1 << 5) | (1 << 14));
        let gpio = 0x40011400 as *mut u32;
        write_volatile(gpio, (read_volatile(gpio) & !(0xf << 20)) | (0xb << 20));
        write_volatile(0x40013808 as *mut u32, 208);
        write_volatile(0x4001380c as *mut u32, (1 << 13) | (1 << 3));
        let scheduler = &mut *addr_of_mut!(SCHEDULER);
        scheduler.initialize(frames);
        asm!("csrw mtvec, {0}", in(reg) rv2k_trap_entry as *const () as usize);
        if let Selection::Run(_, sp) = scheduler.select_next() {
            rv2k_first_task(sp);
        }
    }
    stop(20)
}

/// The only task-frame copy boundary; sp is validated BEFORE touching task RAM.
#[no_mangle]
unsafe extern "C" fn rv2k_prepare_frame(temporary: *const TrapFrame) -> usize {
    let frame = &*temporary;
    let Some(id) = (&*addr_of!(SCHEDULER)).current() else {
        stop(21);
    };
    let (base, top) = stack_bounds(id);
    let Some(destination) = frame_address(frame.registers[14] as usize, base, top) else {
        stop(22);
    };
    core::ptr::copy_nonoverlapping(temporary, destination as *mut TrapFrame, 1);
    destination
}

struct GuestMemory(MemoryMap);
impl UserMemory for GuestMemory {
    fn copy_out(&mut self, address: usize, bytes: &[u8], alignment: usize) -> Result<(), Error> {
        self.0.validate(address, bytes.len(), alignment, true)?;
        // The map admits only the caller's live stack above the saved frame.
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), address as *mut u8, bytes.len());
        }
        Ok(())
    }
}

#[no_mangle]
unsafe extern "C" fn rv2k_trap_dispatch(frame: *mut TrapFrame) -> usize {
    #[cfg(feature = "syscall-kernel-fault")]
    asm!("ecall");
    let scheduler = &mut *addr_of_mut!(SCHEDULER);
    let saved = &mut *frame;
    write_volatile(addr_of_mut!(TRAPS), read_volatile(addr_of!(TRAPS)) + 1);
    write_volatile(addr_of_mut!(LAST_CAUSE), saved.cause);
    write_volatile(addr_of_mut!(LAST_PC), saved.pc);
    if !saved.advance_ecall() {
        stop(23);
    }
    let Some(id) = scheduler.current() else {
        stop(24);
    };
    let (_, top) = stack_bounds(id);
    let mut memory = GuestMemory(MemoryMap {
        stack: Region {
            start: saved.registers[14] as usize,
            end: top,
        },
        application: Region { start: 0, end: 0 },
        flash: Region {
            start: 0,
            end: 16384,
        },
    });
    let args = [
        saved.registers[8] as usize,
        saved.registers[9] as usize,
        saved.registers[10] as usize,
    ];
    match dispatch(scheduler, &mut memory, saved.registers[13] as usize, args) {
        Outcome::Complete(result) => {
            saved.registers[8] = result as u32;
            frame as usize
        }
        Outcome::Switch(action) => {
            saved.registers[8] = 0;
            scheduler.suspend(frame as usize, action);
            match scheduler.select_next() {
                Selection::Run(_, sp) => sp,
                Selection::Finished => finish(),
                Selection::Idle => stop(25), // no blocking syscalls until Phase 3
            }
        }
    }
}

#[no_mangle]
extern "C" fn rv2k_kernel_fault() -> ! {
    stop(26)
}

extern "C" fn task_trampoline() -> ! {
    if task::id() == TaskId::Network {
        task_a();
    } else {
        task_b();
    }
    task::exit(0)
}

#[cfg_attr(feature = "syscall-bad-sp", allow(unreachable_code))]
fn task_a() {
    #[cfg(feature = "syscall-bad-sp")]
    unsafe {
        asm!("li sp, 0", "ecall", options(noreturn));
    }

    unsafe {
        for number in 0..=17 {
            let expected = match number {
                0 | 2 => 0,
                11 | 12 => continue,
                _ => Error::NoSys.code(),
            };
            if raw(number, [0; 3]) != expected {
                stop(30 + number as u32);
            }
            write_volatile(
                addr_of_mut!(CALL_MASK),
                read_volatile(addr_of!(CALL_MASK)) | (1 << number),
            );
        }
        let mut scratch = [0u16; 4];
        if raw(12, [0, scratch.as_mut_ptr() as usize + 1, 6]) != Error::Fault.code() {
            stop(50);
        }
        for (args, expected) in [
            ([2, 0, 6], Error::Invalid.code()),
            ([0, 0, 5], Error::Invalid.code()),
            ([0, 0, 6], Error::Fault.code()),
            ([0, usize::MAX - 1, 6], Error::Fault.code()),
            ([0, 0x20000800, 6], Error::Fault.code()),
            (
                [0, addr_of!(TASK_STACKS) as usize + 256, 6],
                Error::Fault.code(),
            ),
        ] {
            if raw(12, args) != expected {
                stop(50);
            }
        }
        if raw(11, [32768, 0, 0]) != Error::Invalid.code() {
            stop(51);
        }
    }
    for round in 1..=8 {
        unsafe {
            write_volatile(addr_of_mut!(COUNTERS).cast::<u32>(), round);
            if syscall_probe_a() != 1 || getpid_probe_a() != 1 {
                stop(52);
            }
        }
        if task::info(TaskId::Network).map(|v| v.state) != Ok(TaskState::Running) {
            stop(53);
        }
        task::yield_now();
    }
    if task::info(TaskId::Application).map(|v| v.exit_code) != Ok(-7) {
        stop(54);
    }
    unsafe {
        write_volatile(
            addr_of_mut!(CALL_MASK),
            read_volatile(addr_of!(CALL_MASK)) | (1 << 11) | (1 << 12),
        );
    }
}
fn task_b() {
    for round in 1..=3 {
        unsafe {
            write_volatile(addr_of_mut!(COUNTERS).cast::<u32>().add(1), round);
            if syscall_probe_b() != 1 || getpid_probe_b() != 1 {
                stop(55);
            }
        }
        task::yield_now();
    }
    task::exit(-7)
}

unsafe fn watermark(base: *const u32, words: usize) -> u32 {
    let mut untouched = 0;
    while untouched < words && read_volatile(base.add(untouched)) == WATERMARK {
        untouched += 1;
    }
    if untouched < 4 {
        stop(56);
    }
    ((words - untouched) * 4) as u32
}
fn finish() -> ! {
    unsafe {
        for id in 0..2 {
            let base = addr_of!(TASK_STACKS).cast::<u32>().add(id * 64);
            write_volatile(
                addr_of_mut!(STACK_USED).cast::<u32>().add(id),
                watermark(base, 64),
            );
        }
        let bottom = addr_of!(_stack_bottom);
        let words = (addr_of!(_stack_top) as usize - bottom as usize) / 4;
        write_volatile(
            addr_of_mut!(STACK_USED).cast::<u32>().add(2),
            watermark(bottom, words),
        );
        for byte in b"SYSCALL OK\n" {
            while read_volatile(0x40013800 as *const u32) & (1 << 7) == 0 {}
            write_volatile(0x40013804 as *mut u32, *byte as u32);
        }
    }
    stop(0x600d)
}
fn stop(status: u32) -> ! {
    unsafe {
        write_volatile(addr_of_mut!(SYSCALL_STATUS), status);
        asm!("ebreak");
    }
    loop {
        core::hint::spin_loop();
    }
}

global_asm!(include_str!("syscall_probe.S"));
