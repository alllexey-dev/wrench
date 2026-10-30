    .text
    .org 0x100

_start:
    addi     t0, zero, 0x80
    lw       s0, 0(t0)                       ; n
    addi     s1, zero, 0x400                 ; array base
    lui      s2, %hi(1103515245)             ; LCG multiplier
    addi     s2, s2, %lo(1103515245)
    lui      s3, %hi(12345)                  ; LCG increment
    addi     s3, s3, %lo(12345)
    addi     s4, zero, 1                     ; LCG state
    addi     t1, zero, 0                     ; i

fill:                                        ; a[i] = next_lcg() >> 16
    bge      t1, s0, sort
    mul      s4, s4, s2
    add      s4, s4, s3
    srli     t2, s4, 16
    slli     t3, t1, 2
    add      t3, t3, s1
    sw       t2, 0(t3)
    addi     t1, t1, 1
    j        fill

sort:
    addi     t1, s0, -1                      ; last unsorted index

sort_outer:
    bge      zero, t1, checksum
    addi     t2, s1, 0                       ; p = &a[0]
    slli     t4, t1, 2
    add      t4, t4, s1                      ; &a[last]

sort_inner:
    bge      t2, t4, sort_outer_next
    lw       t5, 0(t2)
    lw       t6, 4(t2)
    bge      t6, t5, sort_no_swap
    sw       t6, 0(t2)
    sw       t5, 4(t2)
sort_no_swap:
    addi     t2, t2, 4
    j        sort_inner

sort_outer_next:
    addi     t1, t1, -1
    j        sort_outer

checksum:                                    ; sum of a[i] * (i + 1)
    addi     t1, zero, 0
    addi     a0, zero, 0

checksum_loop:
    bge      t1, s0, done
    slli     t3, t1, 2
    add      t3, t3, s1
    lw       t2, 0(t3)
    addi     t5, t1, 1
    mul      t2, t2, t5
    add      a0, a0, t2
    addi     t1, t1, 1
    j        checksum_loop

done:
    addi     t0, zero, 0x84
    sw       a0, 0(t0)
    halt
