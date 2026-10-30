# Wasm32 Instruction Set Architecture (ISA) Documentation

Wasm32: a 32-bit stack machine for teaching, based on the `i32` subset of [WebAssembly](https://webassembly.github.io/spec/core/). There are no general-purpose registers: instructions pop operands from a stack and push results. Control flow is structured (`block`, `loop`, `if`, each closed by `end`), and functions are called by address.

```asm
_start:
    i32.const 2        ; 2
    i32.const 3        ; 3:2
    i32.add            ; 5
    halt
```

## Program

Sections, labels and data directives are shared by all ISAs; see the [Wrench documentation](https://github.com/ryukzak/wrench/blob/master/docs/README.md). Specific to wasm32:

- Mnemonics are WebAssembly text-format names. A line may hold several instructions. Comments start with `;`.
- Execution starts at `_start`.
- `i32.const` takes a 32-bit number or a label. Other immediates are unsigned bytes, `0` to `255`, checked at translation. A load or store offset is optional (default `0`) and must be a number.
- An instruction is one byte, plus one per byte immediate, plus four for `i32.const`.
- `.frame` names a function's locals (see [Functions and locals](#functions-and-locals)).

## Instructions

An immediate is written `<name: type>`. A stack effect `( before -- after )` has the top rightmost: `i32.sub ( a: i32, b: i32 -- difference: i32 )` pops `b`, then `a`, and pushes `a - b`. An error stops the simulation with a message.

- Constants and arithmetic: [`i32.const`](#i32const), [`i32.add`](#i32add), [`i32.sub`](#i32sub), [`i32.mul`](#i32mul), [`i32.div_s`](#i32div_s), [`i32.rem_s`](#i32rem_s)
- Bitwise and shifts: [`i32.and`](#i32and), [`i32.or`](#i32or), [`i32.xor`](#i32xor), [`i32.shl`](#i32shl), [`i32.shr_s`](#i32shr_s), [`i32.shr_u`](#i32shr_u)
- Comparison: [`i32.eqz`](#i32eqz), [`i32.eq`](#i32eq), [`i32.lt_s`](#i32lt_s), [`i32.le_s`](#i32le_s), [`i32.gt_s`](#i32gt_s), [`i32.ge_s`](#i32ge_s), [`i32.lt_u`](#i32lt_u), [`i32.le_u`](#i32le_u), [`i32.gt_u`](#i32gt_u), [`i32.ge_u`](#i32ge_u)
- Memory: [`i32.load`](#i32load), [`i32.load8_u`](#i32load8_u), [`i32.load8_s`](#i32load8_s), [`i32.store`](#i32store), [`i32.store8`](#i32store8)
- Control: [`block`](#block), [`loop`](#loop), [`if`](#if), [`else`](#else), [`end`](#end), [`br`](#br), [`br_if`](#br_if), [`unreachable`](#unreachable)
- Functions: [`call`](#call), [`return`](#return)
- Locals and stack: [`local.get`](#localget), [`local.set`](#localset), [`local.tee`](#localtee), [`locals.reserve`](#localsreserve), [`drop`](#drop), [`select`](#select)
- Machine: [`sp.init`](#spinit), [`halt`](#halt)

Each entry below starts with its syntax and stack effect.

### Constants and Arithmetic

#### `i32.const`

```text
i32.const <value: i32> ( -- value: i32 )
```

Push an immediate: a number or a label, which is how a function address is produced (see [Functions and locals](#functions-and-locals)).

#### `i32.add`

```text
i32.add ( a: i32, b: i32 -- sum: i32 )
```

#### `i32.sub`

```text
i32.sub ( a: i32, b: i32 -- difference: i32 )
```

`a - b`.

#### `i32.mul`

```text
i32.mul ( a: i32, b: i32 -- product: i32 )
```

Keeps the low 32 bits of the product.

#### `i32.div_s`

```text
i32.div_s ( a: i32, b: i32 -- quotient: i32 )
```

Signed division truncating toward zero: `-7 / 2` is `-3`. Stops with an error on a zero divisor and on `-2^31 / -1`.

#### `i32.rem_s`

```text
i32.rem_s ( a: i32, b: i32 -- remainder: i32 )
```

Signed remainder with the sign of `a`: `-7 % 2` is `-1`. Stops with an error only on a zero divisor; `-2^31 % -1` is `0`.

### Bitwise and Shifts

#### `i32.and`

```text
i32.and ( a: i32, b: i32 -- bits: i32 )
```

#### `i32.or`

```text
i32.or ( a: i32, b: i32 -- bits: i32 )
```

#### `i32.xor`

```text
i32.xor ( a: i32, b: i32 -- bits: i32 )
```

#### `i32.shl`

```text
i32.shl ( value: i32, amount: i32 -- shifted: i32 )
```

Shift left. The amount is taken modulo 32, as for every shift.

#### `i32.shr_s`

```text
i32.shr_s ( value: i32, amount: i32 -- shifted: i32 )
```

Arithmetic (sign-extending) shift right.

#### `i32.shr_u`

```text
i32.shr_u ( value: i32, amount: i32 -- shifted: i32 )
```

Logical (zero-filling) shift right.

### Comparison

#### `i32.eqz`

```text
i32.eqz ( value: i32 -- flag: i32 )
```

Push `1` if the value is `0`, else `0`.

#### `i32.eq`

```text
i32.eq ( a: i32, b: i32 -- flag: i32 )
```

Push `1` if `a = b`, else `0`, like every comparison.

#### `i32.lt_s`

```text
i32.lt_s ( a: i32, b: i32 -- flag: i32 )
```

Signed `a < b`.

#### `i32.le_s`

```text
i32.le_s ( a: i32, b: i32 -- flag: i32 )
```

Signed `a <= b`.

#### `i32.gt_s`

```text
i32.gt_s ( a: i32, b: i32 -- flag: i32 )
```

Signed `a > b`.

#### `i32.ge_s`

```text
i32.ge_s ( a: i32, b: i32 -- flag: i32 )
```

Signed `a >= b`.

#### `i32.lt_u`

```text
i32.lt_u ( a: i32, b: i32 -- flag: i32 )
```

Unsigned `a < b`, so `-1` compares greater than `1`.

#### `i32.le_u`

```text
i32.le_u ( a: i32, b: i32 -- flag: i32 )
```

Unsigned `a <= b`.

#### `i32.gt_u`

```text
i32.gt_u ( a: i32, b: i32 -- flag: i32 )
```

Unsigned `a > b`.

#### `i32.ge_u`

```text
i32.ge_u ( a: i32, b: i32 -- flag: i32 )
```

Unsigned `a >= b`.

### Memory Access

#### `i32.load`

```text
i32.load <offset: u8> ( address: i32 -- value: i32 )
```

Push the word at `address + offset`. Every load and store takes an optional offset, `0` to `255`, so a struct field or a small array element needs no `i32.add`:

```asm
i32.const point
i32.load 4            ; point.y
```

#### `i32.load8_u`

```text
i32.load8_u <offset: u8> ( address: i32 -- value: i32 )
```

Like `i32.load`, but reads one byte and zero-extends it.

#### `i32.load8_s`

```text
i32.load8_s <offset: u8> ( address: i32 -- value: i32 )
```

Like `i32.load8_u`, but sign-extends.

#### `i32.store`

```text
i32.store <offset: u8> ( address: i32, value: i32 -- )
```

Write the word at `address + offset`. The address goes under the value, so push it first.

#### `i32.store8`

```text
i32.store8 <offset: u8> ( address: i32, value: i32 -- )
```

Like `i32.store`, but writes only the low byte.

### Control

#### `block`

```text
block ( -- )
```

Open a scope; a branch to it continues after its `end`.

#### `loop`

```text
loop ( -- )
```

Open a scope; a branch to it continues at the start of its body. Without a branch the body runs once.

#### `if`

```text
if ( condition: i32 -- )
```

Run the following body if the condition is non-zero, otherwise continue after the matching `else` or `end`. Opens no scope.

#### `else`

```text
else ( -- )
```

Reached at the end of a taken `if` body: continue after the matching `end`.

#### `end`

```text
end ( -- )
```

Close the `block` or `loop` it belongs to; for an `if`, do nothing.

#### `br`

```text
br <depth: u8> ( -- )
```

Branch to the scope `depth` levels out, `0` being the innermost open `block` or `loop`, and close every scope inside it. A target `block` is closed too, since execution leaves it; a target `loop` stays open.

#### `br_if`

```text
br_if <depth: u8> ( condition: i32 -- )
```

`br` if the condition is non-zero, otherwise continue.

#### `unreachable`

```text
unreachable ( -- )
```

Stop with an error.

### Functions

#### `call`

```text
call <paramCount: u8>, <resultCount: u8> ( ...params: i32, target: i32 -- ...results: i32 )
```

Call the function at `target`: the `paramCount` values under it become its first locals, and `resultCount` values come back. Neither count is checked against the callee.

#### `return`

```text
return ( ...results: i32 -- )
```

Leave the current function, closing every scope it still has open and passing the call's `resultCount` values from the top of the stack back to the caller.

### Locals and Stack

#### `local.get`

```text
local.get <index: u8> ( -- value: i32 )
```

#### `local.set`

```text
local.set <index: u8> ( value: i32 -- )
```

#### `local.tee`

```text
local.tee <index: u8> ( value: i32 -- value: i32 )
```

Like `local.set`, but keeps the value on the stack.

#### `locals.reserve`

```text
locals.reserve <count: u8> ( -- )
```

Add `count` locals after the ones the frame has. Use it first thing in a function, before pushing anything: the new locals are the next words down the stack. They are not cleared.

#### `drop`

```text
drop ( value: i32 -- )
```

Discard the top value, e.g. a result the caller does not need.

#### `select`

```text
select ( a: i32, b: i32, condition: i32 -- chosen: i32 )
```

Push `a` if the condition is non-zero, `b` otherwise.

### Machine

#### `sp.init`

```text
sp.init ( address: i32 -- )
```

Move `stackRoot`, where both stacks start, to `address`. Both stacks restart empty there, so use it before anything is pushed: the frame's locals and any open scope are lost. Nothing checks the address.

#### `halt`

```text
halt ( -- )
```

Stop the machine.

## Architecture

Unlike the other wrench ISAs, wasm32 has no general-purpose registers, no flags and no branches to labels. Values live on stacks in memory, loops are nested scopes, and each call gets its own frame. Each section builds on the previous one.

### Memory

Code, data, I/O ports and both stacks share one byte-addressed memory. The stacks start at `stackRoot` (three quarters into memory, unless `sp.init` moves it) and grow apart: the operand stack down, the control stack up.

```text
memory end   +---------------------+
             |         ...         |
ctrlSp    -> +---------------------+
             | control stack       |  grows up
stackRoot -> +=====================+
             | operand stack       |  grows down
sp        -> +---------------------+
             |                     |
             |         ...         |
             |                     |
             | .text, .data        |
0            +---------------------+
```

Besides memory, the machine keeps only `pc` and a few stack pointers, introduced below. The stacks are plain memory: a program can read and overwrite them, and nothing stops them from running into each other or into code and data.

### Operand Stack

Every value is a 32-bit word. `i32.const` pushes a value; an operation pops its operands and pushes its result. `sp` is the address of the top value; a push moves it four bytes down, a pop four bytes up. Comments show the stack after each line as the `stack` view prints it: top first, separated by colons.

```asm
i32.const 7         ; 7
i32.const 2         ; 2:7
i32.sub             ; 5         7 - 2: the deeper value is the left operand
i32.const 3         ; 3:5
i32.mul             ; 15
```

Memory is accessed through the stack too: `i32.load` pops an address and pushes the word there; `i32.store` pops a value, then an address. I/O ports (`memory_mapped_io` in the configuration) are ordinary addresses: a load from an input port reads the next input, a store to an output port writes one. This copies one value from the input at `128` to the output at `132`:

```asm
i32.const 132       ; 132
i32.const 128       ; 128:132
i32.load            ; x:132
i32.store           ;
```

### Conditions

A comparison pushes `1` (true) or `0` (false), and `if` pops it. If it is non-zero, execution enters the body, and reaching `else` jumps past the matching `end`. If it is zero, execution jumps past the matching `else`, or past `end` if there is none.

This writes `-1` if `x` is negative, `1` otherwise:

```asm
i32.const 132                 ; 132
i32.const 128 i32.load        ; x:132
i32.const 0 i32.lt_s          ; (x<0):132
if                            ; 132
    i32.const -1              ; -1:132
else
    i32.const 1               ; 1:132
end
i32.store                     ;
```

What either part pushes stays on the stack after `end`. Nothing checks that both parts push the same number of values.

There is no table of `end` addresses. To skip, `if` or `else` reads the following instructions one by one, tracking nested `block`, `loop` and `if`, until it finds its own `else` or `end`. Each instruction read is a memory access, so each path pays for the part it skips. In the example, when `x` is negative `if` costs 2 accesses (fetch, pop) and `else` 3 (fetch, `i32.const 1`, `end`); otherwise `if` costs 4 (fetch, pop, `i32.const -1`, `else`), as the `mem:accesses` view shows. An `if`'s `end` does nothing; a missing `end` is noticed only when a skip looks for it.

### Control Stack

`if` only skips forward, and a branch cannot name a label. To repeat code, wrap it in a *scope* and branch to the scope. Scopes nest, each closed by its own `end`:

- `block ... end` -- a branch to a block *breaks* out: execution continues after its `end`.
- `loop ... end` -- a branch to a loop *continues* it: execution returns to the start of its body. Without a branch the body runs once.

Open scopes live on the *control stack*. `block` or `loop` pushes an eight-byte record with the addresses a branch needs, and its `end` pops it. `ctrlSp` points just past the top record. To fill the record, `block` and `loop` search for their `end` the way `if` skips, every time they run, so a `block` inside a loop body pays for it on each iteration.

`br n` jumps to the scope `n` records down (`0` is the innermost) and pops the records of the scopes it leaves; `br_if n` does the same if the value it pops is non-zero. `if` pushes no record, so it is not counted and cannot be a target. Branches leave the operand stack unchanged.

This writes `n`, `n - 1`, ..., `1`, with `n` a word in `.data`:

```asm
block                                 ; control stack: block
    loop                              ; control stack: loop:block
        i32.const n i32.load
        i32.eqz
        br_if 1                       ; n = 0 -> leave the block
        i32.const 132
        i32.const n i32.load
        i32.store                     ; write n
        i32.const n
        i32.const n i32.load
        i32.const 1 i32.sub
        i32.store                     ; n -= 1
        br 0                          ; back to the loop head
    end
end
```

Here `block` costs 21 accesses (fetch, 18 instructions up to its `end`, two record words) and `loop` 19. A missing `end` is reported when its scope runs.

[example/wasm32/factorial.s](https://github.com/ryukzak/wrench/blob/master/example/wasm32/factorial.s) is a complete program with a loop.

### Functions and Locals

A function is a label; its address is an ordinary value:

```asm
i32.const 10          ; 10
i32.const fib         ; fib:10
call 1, 1             ; fib(10)     one parameter, one result
```

In [f32a](https://github.com/ryukzak/wrench/blob/master/docs/f32a.md) a call only saves a return address and the callee shares the caller's data stack. Here `call p, r` pops the target and gives the callee its own *frame*:

1. The top `p` values stay in place and become the callee's *locals* `0` to `p-1` (local 0 was pushed first). These are its parameters; the caller's frame now ends below them.

2. A call record on the control stack saves the caller's frame, the return address and `r`.

3. The callee starts at the target with an empty operand stack.

A frame holds its locals, then its operands. While `_start` runs a function with two parameters and one more local:

```text
ctrlSp    -> +---------------------+
             | call record         |  caller's frame, return address, r
stackRoot -> +=====================+
             | local 0, 1, ...     |  _start's frame
             | operands            |
frameBase -> | local 0  (param)    |  callee's frame
             | local 1  (param)    |
             | local 2             |
             | operands            |
sp        -> +---------------------+
```

`frameBase` is the address of local 0, `localCount` the number of locals. `local.get i` pushes local `i`, `local.set i` pops into it, and `local.tee i` stores the top value without popping it. `locals.reserve n` adds `n` locals after the existing ones. They are just the next `n` words down the stack, so reserve them before pushing anything; they are not cleared. `_start` has a frame without parameters and may reserve locals too.

Local indices are relative to the current `frameBase`, so a function sees only its own locals, and a call, recursive or not, leaves the caller's frame intact. Indices are not checked against `localCount`.

`.frame` names locals: parameters in parentheses, then reserved locals. `.frame ($a, $b) $acc` makes `$a` and `$b` locals 0 and 1 (filled by `call 2, r`) and `$acc` local 2; `.frame () $acc` has no parameters. Names become indices at translation: `local.get $acc` is `local.get 2`. `.frame` emits no code, so the function still needs `locals.reserve`, and nothing checks the names against the counts of `call` or `locals.reserve`. Names start with `$`, last until the next label or the end of the section, and mix freely with indices. A malformed `.frame` or a repeated name is a translation error.

`return` keeps the top `r` values and discards the rest of the frame (parameters, locals, leftover operands), the call record and any scope still open. The results replace the parameters on the caller's stack, and the caller continues after `call`. `return` outside a function is an error, and `br` cannot leave a function.

A factorial with `n` as its parameter and the product in a reserved local:

```asm
factorial:
    .frame ($n) $acc                  ; locals 0, 1
    locals.reserve 1                  ; $acc
    i32.const 1 local.set $acc
    block
        loop
            local.get $n i32.const 1 i32.le_s
            br_if 1                   ; n <= 1 -> leave the block
            local.get $acc local.get $n i32.mul
            local.set $acc            ; acc *= n
            local.get $n i32.const 1 i32.sub
            local.set $n              ; n -= 1
            br 0                      ; back to the loop head
        end
    end
    local.get $acc
    return
```

Since the target is a value, calling a computed address (from a function table, say) uses the same instruction.

[example/wasm32/factorial-recursive.s](https://github.com/ryukzak/wrench/blob/master/example/wasm32/factorial-recursive.s) computes the factorial recursively, with `if` and `return`.

## ISA Specific State Views

Besides the common views (`pc`, `mem:dump`, `io`), wasm32 has these, each with `:dec` (default) or `:hex`:

- `stack` -- The current frame's operands, top first, e.g. `14:14`.
- `locals` -- The current frame's locals, local 0 first. There is no `localCount` view; this shows that many words.
- `sp`, `frame-base`, `ctrl-sp`, `stack-root` -- The pointers.
- `stack:layout` -- Every frame, oldest first: locals, control records, operands. `stack:layout:hex:2` shows only the last two frames.
- `stack:dump` -- Memory from `sp` to the end, by address. Each word appears once, with every frame it belongs to; a frame's locals are grouped, control records decoded, everything else (such as memory above `ctrl-sp`) raw. `stack:dump:hex:2` also limits it to two frames.

`{stack:layout:hex}` at the `halt` of [example/wasm32/frames.s](https://github.com/ryukzak/wrench/blob/master/example/wasm32/frames.s): `_start` called `foo` with two arguments, and `foo`, inside a `block`, called `bar` with one:

```text
#0 _start (pc=0x44)
  mem[0x7c..0x7f]: (operands)
    0x7c..0x7f: 0x0000000a (0)
#1 foo (pc=0x15)
  mem[0x70..0x7b]: locals
    0x78..0x7b: 0x0000000b (0)
    0x74..0x77: 0x0000000c (1)
    0x70..0x73: 0x00000001 (2)
  mem[0x80..0x87]: CallScope
    { csCallerFrameBase = 0x7c
    , csCallerLocalCount = 0
    , csReturnPc = 0x44
    , csResultCount = 1 }
  mem[0x88..0x8f]: BlockScope { csEnd = 0x16 }
  (operands): empty, base=0x70
#2 bar (pc=0x25, live)
  mem[0x6c..0x6f]: locals
    0x6c..0x6f: 0x00000001 (0) [frameBase]
  mem[0x90..0x97]: CallScope
    { csCallerFrameBase = 0x78
    , csCallerLocalCount = 3
    , csReturnPc = 0x15
    , csResultCount = 1 }
  mem[0x64..0x6b]: (operands)
    0x68..0x6b: 0x0000000e (0)
    0x64..0x67: 0x0000000e (1) [sp]
```

## Differences from WebAssembly

- Only `i32`. Missing: `i32.ne`, `i32.div_u`, `i32.rem_u`, rotations, `clz`/`ctz`/`popcnt`, 16-bit loads and stores, `br_table`, `nop`, globals, `memory.size`/`memory.grow`.
- No modules, function declarations or types: a function is a label, and `call` takes its target from the stack (like `call_indirect`) and its parameter and result counts from the instruction.
- Locals are added at run time by `locals.reserve` and named by `.frame ($n) $acc` instead of `(param $n i32)` and `(local $acc i32)`.
- Scopes have no result types, `if` is not a branch target, and branches neither unwind nor check the operand stack.
- The stacks live in ordinary memory, readable and writable by the program; `sp.init` places them.
- A program ends with `halt`.
