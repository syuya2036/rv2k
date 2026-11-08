//! RV32E byte loops: libc-compatible returns, no stack, aligned or unaligned.
//! Direction is selected before copying, including overlapping memmove ranges.
core::arch::global_asm!(
    r#"
.section .text
.global memcpy
memcpy:
mv a3, a0
1: beqz a2, 2f
lbu a4, 0(a1)
sb a4, 0(a3)
addi a1, a1, 1
addi a3, a3, 1
addi a2, a2, -1
j 1b
2: ret
.global memmove
memmove:
bltu a0, a1, memcpy
beq a0, a1, 2b
add a3, a0, a2
add a1, a1, a2
3: beqz a2, 2b
addi a1, a1, -1
addi a3, a3, -1
lbu a4, 0(a1)
sb a4, 0(a3)
addi a2, a2, -1
j 3b
.global memset
memset:
mv a3, a0
4: beqz a2, 2b
sb a1, 0(a3)
addi a3, a3, 1
addi a2, a2, -1
j 4b
"#
);

#[cfg(feature = "compact-probe")]
pub unsafe fn check() {
    use core::ptr::{addr_of_mut, write_volatile};
    extern "C" {
        fn memcpy(dst: *mut u8, src: *const u8, len: usize) -> *mut u8;
        fn memmove(dst: *mut u8, src: *const u8, len: usize) -> *mut u8;
        fn memset(dst: *mut u8, value: i32, len: usize) -> *mut u8;
    }
    static mut BYTES: [u8; 64] = [0; 64];
    #[no_mangle]
    static mut COMPACT_STATUS: u32 = 0;
    let b = &mut *addr_of_mut!(BYTES);
    for (i, v) in b.iter_mut().enumerate() {
        *v = i as u8;
    }
    let p = b.as_mut_ptr();
    assert_eq!(memcpy(p.add(3), p.add(32), 17), p.add(3));
    for i in 0..17 {
        assert_eq!(b[3 + i], 32 + i as u8);
    }
    assert_eq!(memcpy(p.add(3), p.add(32), 0), p.add(3));
    for (i, v) in b.iter_mut().enumerate() {
        *v = i as u8;
    }
    assert_eq!(memmove(p.add(5), p, 31), p.add(5));
    for i in 0..31 {
        assert_eq!(b[5 + i], i as u8);
    }
    assert_eq!(memmove(p, p.add(5), 31), p);
    for (i, v) in b[..31].iter().enumerate() {
        assert_eq!(*v, i as u8);
    }
    assert_eq!(memmove(p, p, 64), p);
    assert_eq!(memmove(p, p.add(1), 0), p);
    for (i, v) in b.iter_mut().enumerate() {
        *v = i as u8;
    }
    assert_eq!(memset(p.add(3), 0x1234, 17), p.add(3));
    for v in &b[3..20] {
        assert_eq!(*v, 0x34);
    }
    assert_eq!(b[2], 2);
    assert_eq!(b[20], 20);
    assert_eq!(memset(p, -1, 0), p);
    assert_eq!(b[0], 0);
    write_volatile(addr_of_mut!(COMPACT_STATUS), 0xc0);
}
