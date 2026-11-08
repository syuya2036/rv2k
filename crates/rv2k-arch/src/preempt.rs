//! Single-CPU preemption exclusion. IRQ delivery remains enabled in the body.
#[derive(Default)]
#[cfg(any(target_arch = "riscv32", test))]
#[repr(C)]
struct State {
    depth: u32,
    pending: bool,
}
#[cfg(any(target_arch = "riscv32", test))]
impl State {
    const fn new() -> Self {
        Self {
            depth: 0,
            pending: false,
        }
    }
    fn enter(&mut self) {
        self.depth = self.depth.checked_add(1).expect("preemption nesting");
    }
    fn leave(&mut self) {
        assert!(self.depth != 0, "preemption underflow");
        self.depth -= 1;
    }
    fn take(&mut self) -> bool {
        if self.depth == 0 && self.pending {
            self.pending = false;
            true
        } else {
            false
        }
    }
}

#[cfg(target_arch = "riscv32")]
mod target {
    use super::State;
    use core::{
        marker::PhantomData,
        ptr::{addr_of, addr_of_mut, read_volatile, write_volatile},
    };
    #[no_mangle]
    static mut PREEMPT_STATE: State = State::new();
    static mut START: u32 = 0;
    #[no_mangle]
    static mut PREEMPT_MAX_CLOCKS: u32 = 0;
    #[no_mangle]
    static mut PREEMPT_DEFERRED: u32 = 0;
    fn count() -> u32 {
        unsafe { read_volatile(0xe000f008 as *const u32) }
    }
    /// A task must release its shared borrows before dropping this guard and
    /// reaching checkpoint. Blocking/yield/exit calls while held are forbidden.
    #[must_use]
    pub struct Guard(PhantomData<*mut ()>);
    pub fn disable() -> Guard {
        crate::critical(|| unsafe {
            let state = &mut *addr_of_mut!(PREEMPT_STATE);
            if state.depth == 0 {
                write_volatile(addr_of_mut!(START), count());
            }
            state.enter();
        });
        Guard(PhantomData)
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            crate::critical(|| unsafe {
                let state = &mut *addr_of_mut!(PREEMPT_STATE);
                state.leave();
                if state.depth == 0 {
                    let elapsed = count().wrapping_sub(read_volatile(addr_of!(START)));
                    if elapsed > read_volatile(addr_of!(PREEMPT_MAX_CLOCKS)) {
                        write_volatile(addr_of_mut!(PREEMPT_MAX_CLOCKS), elapsed);
                    }
                }
            });
        }
    }
    /// # Safety
    /// Call only with MIE clear, without another live borrow of STATE.
    pub unsafe fn disabled() -> bool {
        (*addr_of!(PREEMPT_STATE)).depth != 0
    }
    /// # Safety
    /// Call only from the non-nesting timer trap, with MIE clear.
    pub unsafe fn irq_request() -> bool {
        let state = &mut *addr_of_mut!(PREEMPT_STATE);
        state.pending = true;
        if state.depth != 0 {
            write_volatile(
                addr_of_mut!(PREEMPT_DEFERRED),
                read_volatile(addr_of!(PREEMPT_DEFERRED)).wrapping_add(1),
            );
        }
        state.take()
    }
    /// # Safety
    /// Call only in kernel context with MIE clear.
    pub unsafe fn take_pending() -> bool {
        (&mut *addr_of_mut!(PREEMPT_STATE)).take()
    }
    /// Task-only safe point, after all shared protocol/device borrows end.
    pub fn checkpoint() {
        let prior: usize;
        unsafe {
            core::arch::asm!("csrrci {0}, mstatus, 8", out(reg) prior);
        }
        if prior & 8 == 0 {
            return;
        }
        // An enclosing critical section must retain both exclusion and the
        // pending request. No ecall is permitted until its MIE is restored.
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        let pending = unsafe { take_pending() };
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        unsafe {
            core::arch::asm!("csrsi mstatus, 8");
        }
        if pending {
            let mut result = 0usize;
            unsafe {
                core::arch::asm!("ecall", inlateout("a0") result, in("a1") 0usize, in("a2") 0usize, in("a5") 0usize);
            }
            let _ = result;
        }
    }
}
#[cfg(target_arch = "riscv32")]
pub use target::*;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_borrows_coalesce_ticks_until_the_outer_borrow_ends() {
        let mut s = State::new();
        assert!(!s.take());
        s.enter();
        s.pending = true;
        s.enter();
        for _ in 0..100 {
            s.pending = true;
            assert!(!s.take());
        }
        s.leave();
        assert!(!s.take());
        s.leave();
        assert!(s.take());
        assert!(!s.take());
        s.pending = true;
        assert!(s.take());
    }
    #[test]
    #[should_panic(expected = "preemption underflow")]
    fn an_unbalanced_release_cannot_silently_enable_switching() {
        State::new().leave();
    }
}
