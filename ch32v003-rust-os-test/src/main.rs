#![no_std]
#![no_main]
#[cfg(feature = "diagnostics")]
mod diagnostics;
#[cfg(feature = "services")]
mod services;
#[cfg(all(feature = "services", any(feature = "coop", feature = "timer", feature = "scheduler", feature = "syscall")))]
compile_error!("services must be tested separately");
#[cfg(feature = "syscall")]
mod syscall;
#[cfg(all(feature = "syscall", any(feature = "coop", feature = "timer", feature = "scheduler")))]
compile_error!("syscall must be tested separately");
#[cfg(feature = "coop")]
mod coop;
#[cfg(feature = "scheduler")]
mod scheduler;
#[cfg(all(feature = "scheduler", any(feature = "coop", feature = "timer")))]
compile_error!("scheduler, coop and timer must be tested separately");
#[cfg(all(feature = "coop", feature = "timer"))]
compile_error!("coop and timer must be tested separately");
use core::arch::global_asm;
#[cfg(not(any(feature = "coop", feature = "scheduler", feature = "syscall", feature = "services")))]
use core::{arch::asm, ptr::{read_volatile, write_volatile}};
global_asm!(r#"
.section .init,"ax"
.global _start
_start:
.option push
.option norelax
la sp, _stack_top
la a0, _sidata
la a1, _sdata
la a2, _edata
1: bgeu a1, a2, 2f
lw a3, 0(a0)
sw a3, 0(a1)
addi a0, a0, 4
addi a1, a1, 4
j 1b
2: la a1, _sbss
la a2, _ebss
3: bgeu a1, a2, 4f
sw zero, 0(a1)
addi a1, a1, 4
j 3b
4:
.if {fill_stack}
li gp, 0
li tp, 0
la t0, _stack_bottom
li t1, 0xa5a5a5a5
6: bgeu t0, sp, 7f
sw t1, 0(t0)
addi t0, t0, 4
j 6b
7:
.endif
call rust_main
5: j 5b
.option pop
"#, fill_stack = const cfg!(any(feature = "syscall", feature = "services")) as usize);
#[cfg(feature = "timer")]
global_asm!(r#"
.section .text
.balign 4
.global systick_handler
systick_handler:
addi sp, sp, -16
sw t0, 0(sp)
sw t1, 4(sp)
la t0, TICKS
li t1, 1
sw t1, 0(t0)
li t0, 0xe000f000
sw zero, 0(t0)
sw zero, 4(t0)
lw t0, 0(sp)
lw t1, 4(sp)
addi sp, sp, 16
mret
"#);
#[cfg(feature = "timer")]
#[no_mangle]
static mut TICKS: u32 = 0;
#[no_mangle]
static mut RESULT: u32 = 0;
#[no_mangle]
static mut SEED: u32 = 0x12345678;
#[cfg(feature = "coop")]
#[no_mangle]
extern "C" fn rust_main() -> ! { coop::start() }
#[cfg(feature = "scheduler")]
#[no_mangle]
extern "C" fn rust_main() -> ! { scheduler::start() }
#[cfg(feature = "services")]
#[no_mangle]
extern "C" fn rust_main() -> ! { services::start() }
#[cfg(feature = "syscall")]
#[no_mangle]
extern "C" fn rust_main() -> ! { syscall::start() }
#[cfg(not(any(feature = "coop", feature = "scheduler", feature = "syscall", feature = "services")))]
#[no_mangle]
extern "C" fn rust_main() -> ! {
    unsafe {
        let value = read_volatile(core::ptr::addr_of!(SEED));
        write_volatile(core::ptr::addr_of_mut!(RESULT), value.wrapping_add(42));
        #[cfg(feature = "timer")]
        {
            extern "C" { fn systick_handler(); }
            asm!("csrw mtvec, {0}", in(reg) systick_handler as *const () as usize);
            write_volatile(0xe000f008 as *mut u32, 0);
            write_volatile(0xe000f010 as *mut u32, 100);
            write_volatile(0xe000f000 as *mut u32, 7);
            asm!("csrsi mstatus, 8");
            while read_volatile(core::ptr::addr_of!(TICKS)) == 0 {}
            asm!("csrci mstatus, 8");
        }
        // RCC APB2: AFIO, GPIOD, USART1. PD5 TX push-pull, 24MHz HSI / 115200.
        write_volatile(0x40021018 as *mut u32, (1<<0)|(1<<5)|(1<<14));
        let gpio = 0x40011400 as *mut u32;
        write_volatile(gpio, (read_volatile(gpio) & !(0xf<<20)) | (0xb<<20));
        write_volatile(0x40013808 as *mut u32, 208);
        write_volatile(0x4001380c as *mut u32, (1<<13)|(1<<3));
        for c in b"Rust RV32EC OK\n" {
            while read_volatile(0x40013800 as *const u32) & (1<<7) == 0 {}
            write_volatile(0x40013804 as *mut u32, *c as u32);
        }
        // Renvo stops on EBREAK; hardware requires a debugger or stays in loop.
        asm!("ebreak");
    }
    loop { core::hint::spin_loop(); }
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    #[cfg(feature = "diagnostics")]
    diagnostics::panic();
    #[cfg(not(feature = "diagnostics"))]
    loop { core::hint::spin_loop(); }
}
