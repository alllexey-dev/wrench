    .text

_start:
    addi     t0, zero, 0x80
    lw       s0, 0(t0)                       ; n
    addi     s1, zero, 0                     ; total steps for k = 1..n
    addi     s2, zero, 1                     ; k
    addi     t3, zero, 1

collatz_outer:
    blt      s0, s2, collatz_done
    mv       t0, s2

collatz_step:
    beq      t0, t3, collatz_next
    andi     t1, t0, 1
    beqz     t1, collatz_even
    slli     t2, t0, 1                       ; x = 3x + 1
    add      t0, t0, t2
    addi     t0, t0, 1
    j        collatz_count
collatz_even:
    srli     t0, t0, 1                       ; x = x / 2
collatz_count:
    addi     s1, s1, 1
    j        collatz_step

collatz_next:
    addi     s2, s2, 1
    j        collatz_outer

collatz_done:
    addi     t0, zero, 0x84
    sw       s1, 0(t0)
    halt
