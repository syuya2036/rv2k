//! Phase 3 integration: cooperative tasks, timed waits and single-slot IPC.
use core::{
    arch::asm,
    ptr::{addr_of, addr_of_mut, read_volatile, write_volatile},
};
use rv2k_api::{event, ipc, syscall::Error, task, time, TaskId};
use rv2k_arch::{clock::Clock, critical, frame_address, TrapFrame, FRAME_BYTES};
use rv2k_kernel::{
    ipc::Mailbox,
    memory::{MemoryMap, Region, UserMemory},
    syscall::Outcome,
    Action, Scheduler, Selection,
};

#[cfg(not(feature = "diagnostics"))]
use rv2k_kernel::services::dispatch;

const WATERMARK: u32 = 0xa5a5a5a5;
#[cfg(feature = "diagnostics")]
const FIRST_MESSAGE: &[u8] = &[b'X'; 64];
#[cfg(not(feature = "diagnostics"))]
const FIRST_MESSAGE: &[u8] = b"hello";
#[repr(C, align(16))]
struct Stack([u32; 64]);
#[no_mangle]
static mut TASK_STACKS: [Stack; 2] = [Stack([0; 64]), Stack([0; 64])];
static mut SCHEDULER: Scheduler = Scheduler::new();
static mut MAILBOX: Mailbox = Mailbox::new();
const CLOCKS_PER_TICK: u32 = 240_000; // 24 MHz HCLK, 10 ms target
static mut CLOCK: Clock = Clock::new(CLOCKS_PER_TICK, 0);
#[no_mangle]
static mut TIMER_IRQS: u32 = 0;
#[no_mangle]
static mut IDLE_POLLS: u32 = 0;
#[no_mangle]
static mut UPTIME: u64 = 0;
#[no_mangle]
static mut SERVICES_STATUS: u32 = 0;
#[no_mangle]
static mut COUNTERS: [u32; 2] = [0; 2];
#[no_mangle]
static mut TRAPS: u32 = 0;
#[no_mangle]
static mut WAIT_RESULTS: u32 = 0;
#[no_mangle]
static mut STACK_USED: [u32; 3] = [0; 3];
#[no_mangle]
static mut LAST_CAUSE: u32 = 0;
#[no_mangle]
static mut LAST_PC: u32 = 0;
extern "C" {
    fn rv2k_trap_entry();
    fn rv2k_first_task(sp: usize) -> !;
    static _stack_bottom: u32;
    static _stack_top: u32;
}

#[cfg(feature = "diagnostics")]
pub(super) fn stack_base() -> *mut u32 {
    addr_of_mut!(TASK_STACKS).cast::<u32>()
}
#[cfg(feature = "diagnostics")]
pub(super) fn current_task() -> Option<TaskId> {
    unsafe { (&*addr_of!(SCHEDULER)).current() }
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
            let mut initial = TrapFrame::initial(task_trampoline as *const () as u32, sp as u32);
            initial.status |= 0x80;
            (frame as *mut TrapFrame).write(initial);
            frames[id] = frame;
        }
        #[cfg(feature = "drivers")]
        crate::drivers::init();
        #[cfg(not(feature = "drivers"))]
        {
            // Baseline development board UART; J4M6 remapping belongs to Phase 5.
            write_volatile(0x40021018 as *mut u32, (1 << 0) | (1 << 5) | (1 << 14));
            let gpio = 0x40011400 as *mut u32;
            write_volatile(gpio, (read_volatile(gpio) & !(0xf << 20)) | (0xb << 20));
            write_volatile(0x40013808 as *mut u32, 208);
            write_volatile(0x4001380c as *mut u32, (1 << 13) | (1 << 3));
        }
        let scheduler = &mut *addr_of_mut!(SCHEDULER);
        scheduler.initialize(frames);
        asm!("csrw mtvec, {0}", in(reg) rv2k_trap_entry as *const () as usize);
        write_volatile(0xe000f008 as *mut u32, 0);
        write_volatile(0xe000f010 as *mut u32, CLOCKS_PER_TICK);
        write_volatile(0xe000f000 as *mut u32, 7);
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
    #[cfg(feature = "diagnostics")]
    crate::diagnostics::check(Some(frame));
    let (base, top) = stack_bounds(id);
    let Some(destination) = frame_address(frame.registers[14] as usize, base, top) else {
        #[cfg(feature = "diagnostics")]
        crate::diagnostics::fail(rv2k_kernel::diagnostics::Reason::InvalidSp, Some(frame), 0);
        #[cfg(not(feature = "diagnostics"))]
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
    fn copy_in(&mut self, address: usize, bytes: &mut [u8]) -> Result<(), Error> {
        self.0.validate(address, bytes.len(), 1, false)?;
        unsafe {
            core::ptr::copy_nonoverlapping(address as *const u8, bytes.as_mut_ptr(), bytes.len());
        }
        Ok(())
    }
}

#[no_mangle]
unsafe extern "C" fn rv2k_trap_dispatch(frame: *mut TrapFrame) -> usize {
    #[cfg(feature = "fault-kernel-illegal")]
    asm!(".word 0xffffffff");
    update_clock();
    let scheduler = &mut *addr_of_mut!(SCHEDULER);
    let saved = &mut *frame;
    write_volatile(addr_of_mut!(TRAPS), read_volatile(addr_of!(TRAPS)) + 1);
    write_volatile(addr_of_mut!(LAST_CAUSE), saved.cause);
    write_volatile(addr_of_mut!(LAST_PC), saved.pc);
    if !saved.advance_ecall() {
        #[cfg(feature = "diagnostics")]
        crate::diagnostics::fail(rv2k_kernel::diagnostics::Reason::TaskTrap, Some(saved), 0);
        #[cfg(not(feature = "diagnostics"))]
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
    #[cfg(feature = "diagnostics")]
    let snapshot = crate::diagnostics::info();
    #[cfg(all(feature = "diagnostics", not(feature = "drivers")))]
    let outcome = rv2k_kernel::services::dispatch_with_info(
        scheduler,
        &mut *addr_of_mut!(MAILBOX),
        &mut memory,
        saved.registers[13] as usize,
        args,
        Some(&snapshot),
    );
    #[cfg(not(feature = "diagnostics"))]
    let outcome = dispatch(
        scheduler,
        &mut *addr_of_mut!(MAILBOX),
        &mut memory,
        saved.registers[13] as usize,
        args,
    );
    #[cfg(feature = "drivers")]
    let outcome = rv2k_kernel::services::dispatch_with_devices(
        scheduler,
        &mut *addr_of_mut!(MAILBOX),
        &mut memory,
        saved.registers[13] as usize,
        args,
        rv2k_kernel::services::DeviceServices {
            console: crate::drivers::console(),
            info: Some(&snapshot),
        },
    );
    match outcome {
        Outcome::Complete(result) => {
            saved.registers[8] = result as u32;
            frame as usize
        }
        Outcome::Switch(action) => {
            saved.registers[8] = 0;
            scheduler.suspend(frame as usize, action);
            if matches!(action, Action::Exit(_)) {
                (&mut *addr_of_mut!(MAILBOX)).task_exited(scheduler, id);
            }
            select_next()
        }
    }
}

#[no_mangle]
extern "C" fn rv2k_kernel_fault(_sp: usize) -> ! {
    #[cfg(feature = "diagnostics")]
    crate::diagnostics::kernel_fault(_sp as u32);
    #[cfg(not(feature = "diagnostics"))]
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

fn checked<T>(result: Result<T, Error>, code: u32) -> T {
    match result {
        Ok(value) => value,
        Err(_) => stop(code),
    }
}
fn mark(bit: u32) {
    unsafe {
        write_volatile(
            addr_of_mut!(WAIT_RESULTS),
            read_volatile(addr_of!(WAIT_RESULTS)) | bit,
        );
    }
}
#[cfg_attr(
    any(
        feature = "fault-panic",
        feature = "fault-sp",
        feature = "fault-illegal",
        feature = "fault-irq-canary"
    ),
    allow(unreachable_code)
)]
fn task_a() {
    #[cfg(feature = "drivers")]
    if checked(rv2k_api::console::write(b"UART via API\n"), 94) != 13 {
        stop(95);
    }

    #[cfg(feature = "fault-irq-canary")]
    unsafe {
        write_volatile(stack_base().add(2), 0);
        extern "C" {
            fn fault_irq_wait() -> !;
        }
        fault_irq_wait();
    }
    #[cfg(feature = "fault-canary")]
    unsafe {
        write_volatile(stack_base().add(2), 0);
        task::yield_now();
    }
    #[cfg(feature = "fault-kernel-canary")]
    unsafe {
        write_volatile(addr_of!(_stack_bottom).cast_mut().add(2), 0);
        task::yield_now();
    }
    #[cfg(feature = "fault-sp")]
    unsafe {
        asm!("li sp, 0", "ecall", options(noreturn));
    }
    #[cfg(feature = "fault-illegal")]
    unsafe {
        asm!(".word 0xffffffff", options(noreturn));
    }
    #[cfg(feature = "fault-panic")]
    panic!("injected panic");
    #[cfg(feature = "diagnostics")]
    {
        let memory = checked(rv2k_api::mem::info(), 92);
        if memory.version != 1
            || memory.guard_failures != 0
            || memory.static_bytes + memory.kernel_reserved > 2048
            || memory.flash_bytes > 16384
            || memory.used.iter().any(|used| *used > 240)
        {
            stop(93);
        }
    }

    // The inner critical section must leave the outer one masked, and the
    // outer section must restore task context's enabled MIE.
    let mie: usize;
    critical(|| {
        critical(|| {});
        let inner: usize;
        unsafe {
            asm!("csrr {0}, mstatus", out(reg) inner);
        }
        if inner & 8 != 0 {
            stop(60);
        }
    });
    unsafe {
        asm!("csrr {0}, mstatus", out(reg) mie);
    }
    if mie & 8 == 0 {
        stop(61);
    }
    mark(1);
    let before = checked(time::uptime(), 88);
    critical(|| unsafe {
        let interrupts = read_volatile(addr_of!(TIMER_IRQS));
        let start = read_volatile(0xe000f008 as *const u32);
        while read_volatile(0xe000f008 as *const u32).wrapping_sub(start) < CLOCKS_PER_TICK * 2 {}
        if read_volatile(addr_of!(TIMER_IRQS)) != interrupts {
            stop(89);
        }
    });
    if checked(time::uptime(), 90) < before + 2 {
        stop(91);
    }
    mark(64);
    checked(event::signal(TaskId::Application, event::NET_RX), 62);
    checked(time::sleep(0), 63);
    let mut bytes = [0u8; if cfg!(feature = "diagnostics") { 64 } else { 8 }];
    if checked(ipc::recv(&mut bytes), 64) != FIRST_MESSAGE.len()
        || &bytes[..FIRST_MESSAGE.len()] != FIRST_MESSAGE
    {
        stop(65);
    }
    mark(2);
    if event::wait(event::NET_CONNECTED, 1) != Err(Error::Timeout) {
        stop(66);
    }
    mark(4);
    checked(event::signal(TaskId::Application, event::NET_TX_READY), 67);
    checked(time::sleep(4), 68);
    if checked(ipc::recv(&mut bytes), 69) != 0 {
        stop(70);
    }
    checked(event::signal(TaskId::Application, event::NET_CLOSED), 71);
    loop {
        match ipc::recv(&mut bytes) {
            Ok(4) if &bytes[..4] == b"done" => break,
            Err(Error::Again) => {
                checked(event::wait(event::MSG_AVAILABLE, event::FOREVER), 72);
            }
            _ => stop(74),
        }
    }
    if checked(time::uptime(), 75) < 4 {
        stop(76);
    }
    mark(8);
    unsafe {
        write_volatile(addr_of_mut!(COUNTERS).cast::<u32>(), 1);
    }
}
fn task_b() {
    if checked(event::wait(event::NET_RX, event::FOREVER), 77) != event::NET_RX {
        stop(78);
    }
    checked(ipc::send(TaskId::Network, FIRST_MESSAGE), 79);
    if ipc::send(TaskId::Network, b"full") != Err(Error::Again) {
        stop(80);
    }
    checked(event::wait(event::MSG_SPACE, event::FOREVER), 81);
    mark(16);
    checked(time::sleep(3), 82);
    if checked(event::wait(event::NET_TX_READY, 1), 83) != event::NET_TX_READY {
        stop(84);
    }
    checked(ipc::send(TaskId::Network, b""), 85);
    checked(event::wait(event::NET_CLOSED, event::FOREVER), 86);
    checked(ipc::send(TaskId::Network, b"done"), 87);
    mark(32);
    unsafe {
        write_volatile(addr_of_mut!(COUNTERS).cast::<u32>().add(1), 1);
    }
}

#[no_mangle]
unsafe extern "C" fn rv2k_timer_irq(_frame: *const TrapFrame) {
    #[cfg(feature = "diagnostics")]
    crate::diagnostics::check(Some(&*_frame));
    let count = read_volatile(0xe000f008 as *const u32);
    write_volatile(0xe000f010 as *mut u32, count.wrapping_add(CLOCKS_PER_TICK));
    write_volatile(0xe000f004 as *mut u32, 0);
    write_volatile(
        addr_of_mut!(TIMER_IRQS),
        read_volatile(addr_of!(TIMER_IRQS)) + 1,
    );
}
unsafe fn update_clock() {
    #[cfg(feature = "drivers")]
    crate::drivers::poll();
    let elapsed = (&mut *addr_of_mut!(CLOCK)).sample(read_volatile(0xe000f008 as *const u32));
    (&mut *addr_of_mut!(SCHEDULER)).advance_ticks(elapsed);
}
unsafe fn select_next() -> usize {
    #[cfg(feature = "diagnostics")]
    crate::diagnostics::check(None);
    let scheduler = &mut *addr_of_mut!(SCHEDULER);
    match scheduler.select_next() {
        Selection::Run(id, sp) => {
            if let Some(result) = scheduler.take_result(id) {
                (*(sp as *mut TrapFrame)).registers[8] = result as u32;
            }
            sp
        }
        Selection::Idle => 0,
        Selection::Finished => finish(),
    }
}
#[no_mangle]
unsafe extern "C" fn rv2k_idle_poll() -> usize {
    write_volatile(
        addr_of_mut!(IDLE_POLLS),
        read_volatile(addr_of!(IDLE_POLLS)) + 1,
    );
    update_clock();
    select_next()
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
        write_volatile(0xe000f000 as *mut u32, 0);
        write_volatile(addr_of_mut!(UPTIME), (&*addr_of!(SCHEDULER)).uptime());
        #[cfg(feature = "drivers")]
        crate::drivers::complete();
        #[cfg(not(feature = "drivers"))]
        for byte in b"SERVICES OK\n" {
            while read_volatile(0x40013800 as *const u32) & (1 << 7) == 0 {}
            write_volatile(0x40013804 as *mut u32, *byte as u32);
        }
    }
    stop(0x600d)
}
fn stop(status: u32) -> ! {
    unsafe {
        write_volatile(addr_of_mut!(SERVICES_STATUS), status);
        #[cfg(feature = "diagnostics")]
        asm!("csrci mstatus, 8");
        #[cfg(any(not(feature = "diagnostics"), feature = "emulator-stop"))]
        asm!("ebreak");
    }
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(feature = "fault-irq-canary")]
core::arch::global_asm!(
    ".section .text",
    ".global fault_irq_wait",
    "fault_irq_wait:",
    "nop",
    "j fault_irq_wait"
);
