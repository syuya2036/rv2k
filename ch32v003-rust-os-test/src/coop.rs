//! Two fixed cooperative tasks. Interrupts must remain disabled.
use core::{arch::{asm, global_asm}, ptr::{addr_of, addr_of_mut, read_volatile, write_volatile}};

const WORDS: usize = 64;
const FRAME_WORDS: usize = 16;
const WATERMARK: u32 = 0xa5a5a5a5;
const ROUNDS: u32 = 8;

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
static mut LOCAL_ADDR: [usize; 2] = [0; 2];
#[no_mangle]
static mut STACK_USED: [u32; 2] = [0; 2];
#[no_mangle]
static mut TRACE: [u8; 16] = [0; 16];
#[no_mangle]
static mut TRACE_LEN: u32 = 0;
#[no_mangle]
static mut COOP_STATUS: u32 = 0;

// 64-byte frame: ra,gp,tp,t0,t1,t2,s0,s1,a0..a5,original sp,padding.
// The scheduler uses scratch registers only AFTER saving them. No x16..x31.
// Calling this from Rust obeys ILP32E: callers may still spill caller-saved regs.
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
la t0, CURRENT_TASK
lw t1, 0(t0)
slli t2, t1, 2
la a0, SAVED_SP
add t2, a0, t2
sw sp, 0(t2)
xori t1, t1, 1
sw t1, 0(t0)
slli t1, t1, 2
add a0, a0, t1
lw sp, 0(a0)
la t0, SWITCHES
lw t1, 0(t0)
addi t1, t1, 1
sw t1, 0(t0)
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
}

/// Yield to the other fixed task. Only task context may call this; IRQs stay disabled.
pub fn yield_task() { unsafe { switch_context(); } }

pub fn start() -> ! {
    unsafe {
        asm!("csrci mstatus, 8");
        let gp: usize;
        let tp: usize;
        asm!("mv {0}, gp", out(reg) gp);
        asm!("mv {0}, tp", out(reg) tp);
        let stacks = addr_of_mut!(TASK_STACKS).cast::<u32>();
        let entries = [task_a as *const () as usize, task_b as *const () as usize];
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
        first_task(read_volatile(addr_of!(SAVED_SP).cast::<usize>()))
    }
}

extern "C" fn task_a() -> ! { task(0) }
extern "C" fn task_b() -> ! { task(1) }

fn put(byte: u8) {
    unsafe {
        while read_volatile(0x40013800 as *const u32) & (1<<7) == 0 {}
        write_volatile(0x40013804 as *mut u32, byte as u32);
    }
}

#[inline(never)]
fn task(id: usize) -> ! {
    let mut local = 0u32;
    let mut cookie = [0xabc00000 | id as u32; 8];
    unsafe {
        let counts = addr_of_mut!(COUNTERS).cast::<u32>();
        write_volatile(addr_of_mut!(LOCAL_ADDR).cast::<usize>().add(id), addr_of!(local) as usize);
        loop {
            // Volatile locals force stack storage across calls and context switches.
            let next = read_volatile(addr_of!(local)) + 1;
            write_volatile(addr_of_mut!(local), next);
            let peer = read_volatile(counts.add(1-id));
            if next > ROUNDS || peer != next - (1-id) as u32 { stop(1); }
            write_volatile(counts.add(id), next);
            let pos = read_volatile(addr_of!(TRACE_LEN));
            if pos >= 16 { stop(2); }
            write_volatile(addr_of_mut!(TRACE).cast::<u8>().add(pos as usize), b'A' + id as u8);
            write_volatile(addr_of_mut!(TRACE_LEN), pos+1);
            put(b'A' + id as u8);
            // Task-specific sentinels in every x3..x15, plus a stack sentinel.
            let ok = if id == 0 { probe_a() } else { probe_b() };
            if ok != 1 { stop(3); }
            // Also exercise the ordinary Rust ABI call path and its live locals.
            yield_task();
            for i in 0..8 {
                let ptr = addr_of_mut!(cookie).cast::<u32>().add(i);
                if read_volatile(ptr) != (0xabc00000 | id as u32) { stop(4); }
            }
            if read_volatile(addr_of!(local)) != next { stop(5); }
            if id == 0 && next == ROUNDS {
                if read_volatile(counts.add(1)) != ROUNDS { stop(6); }
                let stacks = addr_of!(TASK_STACKS).cast::<u32>();
                for task_id in 0..2 {
                    let base = stacks.add(task_id*WORDS);
                    let mut untouched = 0;
                    while untouched < WORDS && read_volatile(base.add(untouched)) == WATERMARK { untouched += 1; }
                    if untouched < 4 { stop(7); }
                    write_volatile(addr_of_mut!(STACK_USED).cast::<u32>().add(task_id), ((WORDS-untouched)*4) as u32);
                }
                for byte in b"\nCOOP OK\n" { put(*byte); }
                stop(0x600d);
            }
        }
    }
}

fn stop(status: u32) -> ! {
    unsafe { write_volatile(addr_of_mut!(COOP_STATUS), status); asm!("ebreak"); }
    loop { core::hint::spin_loop(); }
}

global_asm!(include_str!("coop_probe.S"));
