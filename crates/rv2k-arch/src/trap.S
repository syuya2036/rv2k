.option push
.option norelax
.section .text
.balign 4
.global rv2k_trap_entry
rv2k_trap_entry:
# mscratch=kernel top in task context; zero while executing the kernel.
csrrw sp, mscratch, sp
beqz sp, rv2k_nested_fault
addi sp, sp, -80
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
csrr t0, mscratch
sw t0, 56(sp)
csrr t0, mepc
sw t0, 60(sp)
csrr t0, mstatus
sw t0, 64(sp)
csrr t0, mcause
sw t0, 68(sp)
csrr t0, mtval
sw t0, 72(sp)
sw zero, 76(sp)
csrw mscratch, zero
la gp, __global_pointer$
li tp, 0
mv a0, sp
.if {timer}
lw t0, 68(sp)
li t1, 0x8000000c
beq t0, t1, rv2k_timer_trap
.endif
call rv2k_prepare_frame
# a0 is now a checked frame on the task stack. Discard temporary frame.
la sp, _stack_top
call rv2k_trap_dispatch
.if {timer}
beqz a0, rv2k_idle
.endif
j rv2k_restore
.if {timer}
rv2k_timer_trap:
call rv2k_timer_irq
.if {preempt}
beqz a0, rv2k_timer_resume
mv a0, sp
call rv2k_prepare_frame
# The checked copy now lives on the task stack. No kernel temporary frame
# survives across scheduler dispatch or the selected task's restore.
la sp, _stack_top
call rv2k_preempt_dispatch
j rv2k_restore
rv2k_timer_resume:
.endif
mv a0, sp
j rv2k_restore
rv2k_idle:
# No Rust call stack is live during the interrupt-enabled window. Polling and
# selecting run non-reentrantly with MIE off and mscratch=0.
la sp, _stack_top
csrci mstatus, 8
csrw mscratch, zero
call rv2k_idle_poll
bnez a0, rv2k_restore
la t0, _stack_top
csrw mscratch, t0
csrsi mstatus, 8
nop
nop
csrci mstatus, 8
j rv2k_idle
.endif
.global rv2k_first_task
rv2k_first_task:
rv2k_restore:
mv sp, a0
lw t0, 60(sp)
csrw mepc, t0
lw t0, 64(sp)
csrw mstatus, t0
la t0, _stack_top
csrw mscratch, t0
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
mret
rv2k_nested_fault:
# Recover the faulting kernel sp without touching a task stack.
csrr sp, mscratch
csrw mscratch, zero
mv a0, sp
la gp, __global_pointer$
li tp, 0
j rv2k_kernel_fault
.option pop
