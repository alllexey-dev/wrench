    .text

_start:
    addi     t0, zero, 0x80
    lw       s0, 0(t0)                       ; n
    addi     s1, zero, 0                     ; count of primes below n
    addi     s2, zero, 2                     ; i

primes_outer:
    bge      s2, s0, primes_done
    addi     t1, zero, 2                     ; d

primes_inner:                                ; trial division while d * d <= i
    mul      t2, t1, t1
    blt      s2, t2, primes_found
    rem      t3, s2, t1
    beqz     t3, primes_next
    addi     t1, t1, 1
    j        primes_inner

primes_found:
    addi     s1, s1, 1

primes_next:
    addi     s2, s2, 1
    j        primes_outer

primes_done:
    addi     t0, zero, 0x84
    sw       s1, 0(t0)
    halt
