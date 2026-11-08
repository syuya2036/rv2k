.option push
.option norelax
.section .text
.global probe_a
probe_a:
addi sp, sp, -32
sw ra, 0(sp)
sw gp, 4(sp)
sw tp, 8(sp)
sw s0, 12(sp)
sw s1, 16(sp)
li t0, 2659
sw t0, 20(sp)
li gp, 2560
li tp, 2561
li t0, 2562
li t1, 2563
li t2, 2564
li s0, 2565
li s1, 2566
li a0, 2567
li a1, 2568
li a2, 2569
li a3, 2570
li a4, 2571
li a5, 2572
call yield_now
li ra, 2560
bne gp, ra, probe_a_fail
li ra, 2561
bne tp, ra, probe_a_fail
li ra, 2562
bne t0, ra, probe_a_fail
li ra, 2563
bne t1, ra, probe_a_fail
li ra, 2564
bne t2, ra, probe_a_fail
li ra, 2565
bne s0, ra, probe_a_fail
li ra, 2566
bne s1, ra, probe_a_fail
li ra, 2567
bne a0, ra, probe_a_fail
li ra, 2568
bne a1, ra, probe_a_fail
li ra, 2569
bne a2, ra, probe_a_fail
li ra, 2570
bne a3, ra, probe_a_fail
li ra, 2571
bne a4, ra, probe_a_fail
li ra, 2572
bne a5, ra, probe_a_fail
lw t0, 20(sp)
li ra, 2659
bne t0, ra, probe_a_fail
li a0, 1
j probe_a_done
probe_a_fail:
li a0, 0
probe_a_done:
lw ra, 0(sp)
lw gp, 4(sp)
lw tp, 8(sp)
lw s0, 12(sp)
lw s1, 16(sp)
addi sp, sp, 32
ret
.global probe_b
probe_b:
addi sp, sp, -32
sw ra, 0(sp)
sw gp, 4(sp)
sw tp, 8(sp)
sw s0, 12(sp)
sw s1, 16(sp)
li t0, 2915
sw t0, 20(sp)
li gp, 2816
li tp, 2817
li t0, 2818
li t1, 2819
li t2, 2820
li s0, 2821
li s1, 2822
li a0, 2823
li a1, 2824
li a2, 2825
li a3, 2826
li a4, 2827
li a5, 2828
call yield_now
li ra, 2816
bne gp, ra, probe_b_fail
li ra, 2817
bne tp, ra, probe_b_fail
li ra, 2818
bne t0, ra, probe_b_fail
li ra, 2819
bne t1, ra, probe_b_fail
li ra, 2820
bne t2, ra, probe_b_fail
li ra, 2821
bne s0, ra, probe_b_fail
li ra, 2822
bne s1, ra, probe_b_fail
li ra, 2823
bne a0, ra, probe_b_fail
li ra, 2824
bne a1, ra, probe_b_fail
li ra, 2825
bne a2, ra, probe_b_fail
li ra, 2826
bne a3, ra, probe_b_fail
li ra, 2827
bne a4, ra, probe_b_fail
li ra, 2828
bne a5, ra, probe_b_fail
lw t0, 20(sp)
li ra, 2915
bne t0, ra, probe_b_fail
li a0, 1
j probe_b_done
probe_b_fail:
li a0, 0
probe_b_done:
lw ra, 0(sp)
lw gp, 4(sp)
lw tp, 8(sp)
lw s0, 12(sp)
lw s1, 16(sp)
addi sp, sp, 32
ret
.option pop
