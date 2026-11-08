//! CPU-only loops never yield; only timer preemption can run their peer.
use core::ptr::{addr_of, addr_of_mut, read_volatile, write_volatile};
use rv2k_api::{
    syscall::{raw, Error},
    task, time, TaskId,
};
#[no_mangle]
static mut PREEMPT_PROGRESS: [u32; 2] = [0; 2];
#[no_mangle]
static mut PREEMPT_STARTED: [u32; 2] = [0; 2];
#[no_mangle]
static mut PREEMPT_PROBE_STATUS: u32 = 0;
extern "C" {
    static TIMER_IRQS: u32;
    fn preempt_probe_a() -> u32;
    fn preempt_probe_b() -> u32;
}
fn pause(clocks: u32) {
    let start = unsafe { read_volatile(0xe000f008 as *const u32) };
    while unsafe { read_volatile(0xe000f008 as *const u32) }.wrapping_sub(start) < clocks {}
}
#[inline(never)]
fn spi_probe() {
    use rv2k_drivers::spi::SpiBus;
    // This standalone probe owns the board pins; network/driver features are
    // excluded. Arm a real peripheral interrupt inside the mode-0 bit loop.
    let (mut spi, _) = unsafe { rv2k_drivers::board::j4m6() };
    spi.select(true).unwrap();
    let before = unsafe { read_volatile(addr_of!(TIMER_IRQS)) };
    unsafe {
        let now = read_volatile(0xe000f008 as *const u32);
        write_volatile(0xe000f010 as *mut u32, now.wrapping_add(64));
    }
    assert_eq!(spi.transfer(0xa5).unwrap(), 0);
    assert_eq!(spi.transfer(0x3c).unwrap(), 0);
    assert!(unsafe { read_volatile(addr_of!(TIMER_IRQS)) } > before);
    spi.select(false).unwrap();
}
pub fn task() {
    let id = task::id();
    unsafe {
        write_volatile(
            addr_of_mut!(PREEMPT_STARTED).cast::<u32>().add(id.index()),
            1,
        );
    }
    if id == TaskId::Network {
        let before = unsafe { read_volatile(addr_of!(TIMER_IRQS)) };
        {
            let outer = rv2k_arch::preempt::disable();
            {
                let _inner = rv2k_arch::preempt::disable();
                for n in [0, 1, 4, 11] {
                    assert_eq!(unsafe { raw(n, [0; 3]) }, Error::Busy.code());
                }
                spi_probe();
                pause(240_000 * 2);
                assert_eq!(
                    unsafe { read_volatile(addr_of!(PREEMPT_STARTED).cast::<u32>().add(1)) },
                    0
                );
            }
            pause(240_000 * 2);
            assert_eq!(
                unsafe { read_volatile(addr_of!(PREEMPT_STARTED).cast::<u32>().add(1)) },
                0
            );
            rv2k_arch::critical(|| {
                drop(outer);
                assert_eq!(task::id(), TaskId::Network);
                rv2k_arch::preempt::checkpoint();
                assert_eq!(
                    unsafe { read_volatile(addr_of!(PREEMPT_STARTED).cast::<u32>().add(1)) },
                    0
                );
            });
        }
        assert!(unsafe { read_volatile(addr_of!(TIMER_IRQS)) }.wrapping_sub(before) >= 4);
        // The deferred request now reaches an interrupt-enabled nonblocking
        // syscall boundary, rather than surviving until another timer tick.
        assert_eq!(task::id(), TaskId::Network);
        rv2k_arch::preempt::checkpoint();
    }
    for round in 1..=3 {
        let ok = unsafe {
            if id == TaskId::Network {
                preempt_probe_a()
            } else {
                preempt_probe_b()
            }
        };
        assert_eq!(ok, 1);
        unsafe {
            write_volatile(
                addr_of_mut!(PREEMPT_PROGRESS).cast::<u32>().add(id.index()),
                round,
            );
        }
    }
    while unsafe { read_volatile(addr_of!(PREEMPT_PROGRESS).cast::<u32>().add(1 - id.index())) }
        != 3
    {
        time::sleep(1).unwrap();
    }
    if id == TaskId::Network {
        let before = time::uptime().unwrap();
        rv2k_arch::critical(|| {
            let irqs = unsafe { read_volatile(addr_of!(TIMER_IRQS)) };
            pause(480000);
            for n in [0, 1, 4, 11] {
                assert_eq!(unsafe { raw(n, [0; 3]) }, Error::Busy.code());
            }
            assert_eq!(unsafe { read_volatile(addr_of!(TIMER_IRQS)) }, irqs);
        });
        assert!(time::uptime().unwrap() >= before + 2);
    }
    unsafe {
        write_volatile(
            addr_of_mut!(PREEMPT_PROBE_STATUS),
            read_volatile(addr_of!(PREEMPT_PROBE_STATUS)) | (1 << id.index()),
        );
    }
}

core::arch::global_asm!(
    r#"
.section .text
.option push
.option norelax
.macro check reg, value
li a5, \value
bne \reg, a5, 3f
.endm
.macro probe name, salt
.global \name
\name:
addi sp, sp, -32
sw ra, 0(sp)
sw gp, 4(sp)
sw tp, 8(sp)
sw s0, 12(sp)
sw s1, 16(sp)
li ra, \salt+1
li gp, \salt+2
li tp, \salt+3
li t0, \salt+4
li t1, \salt+5
li t2, \salt+6
li s0, \salt+7
li s1, \salt+8
li a0, \salt+9
li a1, \salt+10
li a2, \salt+11
li a3, \salt+12
li a4, \salt+13
li a5, 600000
.option push
.option norvc
1: addi a5, a5, -1
addi a4, a4, 1
bnez a5, 1b
.option pop
bnez a5, 3f
check ra, \salt+1
check gp, \salt+2
check tp, \salt+3
check t0, \salt+4
check t1, \salt+5
check t2, \salt+6
check s0, \salt+7
check s1, \salt+8
check a0, \salt+9
check a1, \salt+10
check a2, \salt+11
check a3, \salt+12
check a4, \salt+13+600000
li a0, 1
j 2f
3: li a0, 0
2: lw ra, 0(sp)
lw gp, 4(sp)
lw tp, 8(sp)
lw s0, 12(sp)
lw s1, 16(sp)
addi sp, sp, 32
ret
.global \name\()_end
\name\()_end:
.endm
probe preempt_probe_a, 0x11110000
probe preempt_probe_b, 0x22220000
.option pop
"#
);
