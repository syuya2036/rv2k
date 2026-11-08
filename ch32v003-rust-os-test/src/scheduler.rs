//! Phase 1 adapter: selection runs on the boot/kernel stack with IRQs disabled.
use core::{arch::{asm, global_asm}, ptr::{addr_of, addr_of_mut, read_volatile, write_volatile}};
use rv2k_api::{TaskId, TaskInfo};
use rv2k_kernel::{Action, Scheduler, Selection};
const WORDS: usize = 64;
const FRAME_WORDS: usize = 16;
const WATERMARK: u32 = 0xa5a5a5a5;
#[repr(C, align(16))]
struct TaskStack([u32; WORDS]);
#[no_mangle]
static mut TASK_STACKS: [TaskStack; 2] = [TaskStack([0; WORDS]), TaskStack([0; WORDS])];
#[no_mangle]
static mut SAVED_SP: [usize; 2] = [0; 2];
#[no_mangle]
static mut CURRENT_TASK: u32 = 0;
#[no_mangle]
static mut SWITCHES: u32 = 0;
#[no_mangle]
static mut COUNTERS: [u32; 2] = [0; 2];
#[no_mangle]
static mut STACK_USED: [u32; 2] = [0; 2];
#[no_mangle]
static mut SCHEDULER_STATUS: u32 = 0;
#[no_mangle]
static mut EXIT_CODES: [u32; 2] = [0; 2];
static mut SCHEDULER: Scheduler = Scheduler::new();
global_asm!(r#"
.option push
.option norelax
.section .text
.global yield_now
yield_now:
addi sp, sp, -64
sw ra, 0(sp)
sw gp, 4(sp)
sw tp, 8(sp)
sw t0, 12(sp)
sw t1, 16(sp)
sw t2, 20(sp)
sw s0, 24(sp)
sw s1, 28(sp)
sw a0, 32(sp)
sw a1, 36(sp)
sw a2, 40(sp)
sw a3, 44(sp)
sw a4, 48(sp)
sw a5, 52(sp)
addi t0, sp, 64
sw t0, 56(sp)
mv a0, sp
li a1, 0
j schedule_on_kernel
.global task_exit_raw
task_exit_raw:
mv a2, a0
li a0, 0
li a1, 1
schedule_on_kernel:
la sp, _stack_top
li gp, 0
li tp, 0
call schedule
mv sp, a0
j restore_context
.global first_task
first_task:
mv sp, a0
restore_context:
lw ra, 0(sp)
lw gp, 4(sp)
lw tp, 8(sp)
lw t0, 12(sp)
lw t1, 16(sp)
lw t2, 20(sp)
lw s0, 24(sp)
lw s1, 28(sp)
lw a0, 32(sp)
lw a1, 36(sp)
lw a2, 40(sp)
lw a3, 44(sp)
lw a4, 48(sp)
lw a5, 52(sp)
lw sp, 56(sp)
ret
.option pop
"#);


extern "C" {
    fn first_task(sp: usize) -> !;
    #[link_name = "yield_now"]
    fn switch_context();
    fn probe_a() -> u32;
    fn probe_b() -> u32;
    fn task_exit_raw(code: i32) -> !;
}
pub fn yield_task() { unsafe { switch_context(); } }
pub fn task_info(id: TaskId) -> TaskInfo { unsafe { (&*addr_of!(SCHEDULER)).info(id) } }
pub fn exit(code: i16) -> ! { unsafe { task_exit_raw(code as i32) } }
pub fn start() -> ! {
    unsafe {
        asm!("csrci mstatus, 8");
        let gp: usize;
        let tp: usize;
        asm!("mv {0}, gp", out(reg) gp);
        asm!("mv {0}, tp", out(reg) tp);
        let stacks = addr_of_mut!(TASK_STACKS).cast::<u32>();
        let entries = [task_trampoline as *const () as usize; 2];
        for (id, entry) in entries.into_iter().enumerate() {
            let base = stacks.add(id * WORDS);
            for i in 0..WORDS { write_volatile(base.add(i), WATERMARK); }
            let frame = base.add(WORDS - FRAME_WORDS);
            for i in 0..FRAME_WORDS { write_volatile(frame.add(i), 0); }
            write_volatile(frame, entry as u32);
            write_volatile(frame.add(1), gp as u32);
            write_volatile(frame.add(2), tp as u32);
            write_volatile(frame.add(14), base.add(WORDS) as u32);
            write_volatile(addr_of_mut!(SAVED_SP).cast::<usize>().add(id), frame as usize);
        }
        // Same USART1 setup as the original minimal firmware.
        write_volatile(0x40021018 as *mut u32, (1<<0)|(1<<5)|(1<<14));
        let gpio = 0x40011400 as *mut u32;
        write_volatile(gpio, (read_volatile(gpio) & !(0xf<<20)) | (0xb<<20));
        write_volatile(0x40013808 as *mut u32, 208);
        write_volatile(0x4001380c as *mut u32, (1<<13)|(1<<3));
        let frames = read_volatile(addr_of!(SAVED_SP));
        let scheduler = &mut *addr_of_mut!(SCHEDULER);
        scheduler.initialize(frames);
        if let Selection::Run(id, sp) = scheduler.select_next() {
            write_volatile(addr_of_mut!(CURRENT_TASK), id.index() as u32);
            first_task(sp)
        }
        stop(20)
    }
}


#[no_mangle]
extern "C" fn schedule(sp: usize, exiting: u32, code: i32) -> usize {
    unsafe {
        let scheduler = &mut *addr_of_mut!(SCHEDULER);
        scheduler.suspend(sp, if exiting == 0 { Action::Yield } else { Action::Exit(code as i16) });
        loop {
            match scheduler.select_next() {
                Selection::Run(id, next_sp) => {
                    write_volatile(addr_of_mut!(CURRENT_TASK), id.index() as u32);
                    write_volatile(addr_of_mut!(SWITCHES), read_volatile(addr_of!(SWITCHES)) + 1);
                    return next_sp;
                }
                Selection::Idle => core::hint::spin_loop(),
                Selection::Finished => finish(),
            }
        }
    }
}
extern "C" fn task_trampoline() -> ! {
    let id = unsafe { read_volatile(addr_of!(CURRENT_TASK)) };
    if id == 0 { task_a(); } else { task_b(); }
    exit(0)
}
fn task_a() {
    for round in 1..=8 {
        unsafe {
            write_volatile(addr_of_mut!(COUNTERS).cast::<u32>(), round);
            if probe_a() != 1 { stop(3); }
        }
        yield_task();
    }
    if task_info(TaskId::Application).exit_code != -7 { stop(4); }
}
fn task_b() {
    unsafe {
        for round in 1..=3 {
            write_volatile(addr_of_mut!(COUNTERS).cast::<u32>().add(1), round);
            if probe_b() != 1 { stop(3); }
            yield_task();
        }
    }
    exit(-7)
}
fn finish() -> ! {
    unsafe {
        let stacks = addr_of!(TASK_STACKS).cast::<u32>();
        for id in 0..2 {
            let base = stacks.add(id * WORDS);
            let mut untouched = 0;
            while untouched < WORDS && read_volatile(base.add(untouched)) == WATERMARK { untouched += 1; }
            if untouched < 4 { stop(7); }
            write_volatile(addr_of_mut!(STACK_USED).cast::<u32>().add(id), ((WORDS - untouched) * 4) as u32);
        }
        write_volatile(addr_of_mut!(EXIT_CODES), [task_info(TaskId::Network).exit_code as i32 as u32,
                     task_info(TaskId::Application).exit_code as i32 as u32]);
        for byte in b"SCHEDULER OK\n" {
            while read_volatile(0x40013800 as *const u32) & (1<<7) == 0 {}
            write_volatile(0x40013804 as *mut u32, *byte as u32);
        }
    }
    stop(0x600d)
}
fn stop(status: u32) -> ! {
    unsafe { write_volatile(addr_of_mut!(SCHEDULER_STATUS), status); asm!("ebreak"); }
    loop { core::hint::spin_loop(); }
}
global_asm!(include_str!("coop_probe.S"));
