    .text

_start:
    addi     t0, zero, 0x80
    lw       a0, 0(t0)                       ; n
    addi     sp, zero, 0x7f0                 ; stack grows down from the top of memory
    jal      ra, fib
    addi     t0, zero, 0x84
    sw       a0, 0(t0)
    halt

fib:                                         ; a0 <- fib(a0), naive recursion
    addi     t0, zero, 2
    blt      a0, t0, fib_return
    addi     sp, sp, -12
    sw       ra, 8(sp)
    sw       a0, 4(sp)
    addi     a0, a0, -1
    jal      ra, fib                         ; fib(n - 1)
    sw       a0, 0(sp)
    lw       a0, 4(sp)
    addi     a0, a0, -2
    jal      ra, fib                         ; fib(n - 2)
    lw       t1, 0(sp)
    add      a0, a0, t1
    lw       ra, 8(sp)
    addi     sp, sp, 12
fib_return:
    jalr     zero, ra, 0
