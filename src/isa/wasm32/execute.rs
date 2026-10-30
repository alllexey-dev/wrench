//! Executing one instruction at a time.

use super::report::wasm32_views;
use super::state::{default_stack_root, Scope, Wasm32St, WORD};
use super::Wasm32Isa as I;
use super::Wasm32Isa;
use crate::machine::{Machine, HALTED};
use crate::memory::{hex_addr, Instruction, IoMem};
use crate::report::view::View;
use crate::rng::RandomStream;
use crate::word::{lshr, sar, shl, W};

impl Wasm32St {
    fn after_matching_end(&mut self, pc: i64, opener: &Wasm32Isa<W>, f: impl FnOnce(&mut Self, i64)) {
        match self.match_else_or_end(false, pc, opener) {
            Err(err) => self.raise_internal_error(err),
            Ok(after_end) => f(self, after_end),
        }
    }

    fn after_matching_else_or_end(&mut self, pc: i64, opener: &Wasm32Isa<W>) {
        match self.match_else_or_end(true, pc, opener) {
            Err(err) => self.raise_internal_error(err),
            Ok(after) => self.set_pc(after),
        }
    }

    /// The address after the `else` (if `stop_at_else`) or `end` that
    /// matches the opener at `addr`, reading every instruction on the way.
    fn match_else_or_end(&mut self, stop_at_else: bool, addr: i64, opener: &Wasm32Isa<W>) -> Result<i64, String> {
        let mut depth = 0;
        let mut at = addr + opener.byte_size() as i64;
        loop {
            let instruction = self
                .fetch_instruction_at(at)
                .map_err(|err| format!("scope at {} has no matching end: {}", hex_addr(2, addr), err))?;
            let next = at + instruction.byte_size() as i64;
            match instruction {
                I::Block | I::Loop | I::If => depth += 1,
                I::Else if stop_at_else && depth == 0 => return Ok(next),
                I::End if depth == 0 => return Ok(next),
                I::End => depth -= 1,
                _ => {}
            }
            at = next;
        }
    }

    fn return_from_call(&mut self) {
        match self.innermost_call() {
            Err(err) => self.raise_internal_error(err),
            Ok((
                addr,
                Scope::Call {
                    caller_frame_base,
                    caller_local_count,
                    return_pc,
                    result_count,
                },
            )) => {
                let mut results: Vec<W> = (0..result_count).map(|_| self.pop_value()).collect();
                results.reverse();
                self.sp = self.frame_base + WORD;
                self.ctrl_sp = addr;
                self.frame_base = caller_frame_base;
                self.local_count = caller_local_count;
                for r in results {
                    self.push_value(r);
                }
                self.set_pc(return_pc);
            }
            Ok((addr, other)) => self.raise_internal_error(format!(
                "control stack corrupted at {}: expected a call record, found {}",
                hex_addr(2, addr),
                other.show()
            )),
        }
    }

    fn branch_to(&mut self, depth: i64) {
        match self.scope_at_depth(depth as usize) {
            None => self.raise_internal_error("br/br_if: the scope it names went missing"),
            Some(Err(err)) => self.raise_internal_error(err),
            Some(Ok(Scope::Loop { start, .. })) => {
                self.retire_scopes(depth);
                self.set_pc(start);
            }
            Some(Ok(Scope::Block { end })) => {
                self.retire_scopes(depth + 1);
                self.set_pc(end + I::<W>::End.byte_size() as i64);
            }
            Some(Ok(Scope::Call { .. })) => {
                self.raise_internal_error("br/br_if: depth reaches past the enclosing function; use return to leave it")
            }
        }
    }

    fn binary(&mut self, op: impl Fn(W, W) -> W) {
        let y = self.pop_value();
        let x = self.pop_value();
        self.push_value(op(x, y));
    }

    fn compare(&mut self, op: impl Fn(W, W) -> bool) {
        self.binary(|x, y| W::from(op(x, y)));
    }

    fn compare_u(&mut self, op: impl Fn(u32, u32) -> bool) {
        self.binary(|x, y| W::from(op(x as u32, y as u32)));
    }

    fn load(&mut self, offset: W, read_at: impl FnOnce(&mut Self, i64) -> W) {
        let addr = self.pop_value();
        let v = read_at(self, addr as i64 + offset as i64);
        self.push_value(v);
    }

    fn store(&mut self, offset: W, write_at: impl FnOnce(&mut Self, i64, W)) {
        let value = self.pop_value();
        let addr = self.pop_value();
        write_at(self, addr as i64 + offset as i64, value);
    }
}

/// The amount of a shift: modulo 32.
fn amount(y: W) -> i64 {
    (y & 0x1F) as i64
}

impl Machine for Wasm32St {
    type Instr = Wasm32Isa<W>;

    fn init(pc: i64, mem: IoMem<Self::Instr>, _randoms: RandomStream) -> Self {
        let root = default_stack_root(&mem);
        Wasm32St {
            pc,
            sp: root,
            stack_root: root,
            ctrl_sp: root,
            frame_base: root - WORD,
            local_count: 0,
            mem,
            stopped: false,
            internal_error: None,
        }
    }

    fn fetch(&mut self) -> Result<(i64, Self::Instr), String> {
        if self.stopped {
            return Err(HALTED.to_string());
        }
        if let Some(e) = &self.internal_error {
            return Err(e.clone());
        }
        let i = self.fetch_instruction_at(self.pc)?;
        Ok((self.pc, i))
    }

    // Wasm32 overrides `instructionStep` in the original, with the same behaviour.
    fn step(&mut self) {
        match self.fetch() {
            Ok((pc, i)) => self.execute(pc, i),
            Err(e) => panic!("internal error: {}", e),
        }
    }

    fn execute(&mut self, pc: i64, instr: Self::Instr) {
        let next = |st: &mut Self| st.next_pc(&instr);
        match instr {
            I::I32Const(value) => self.push_value(value),
            I::I32Add => self.binary(W::wrapping_add),
            I::I32Sub => self.binary(W::wrapping_sub),
            I::I32Mul => self.binary(W::wrapping_mul),
            I::I32DivS => {
                let y = self.pop_value();
                let x = self.pop_value();
                if y == 0 {
                    self.raise_internal_error("integer divide by zero");
                } else if x == W::MIN && y == -1 {
                    self.raise_internal_error("integer overflow");
                } else {
                    self.push_value(x / y);
                }
            }
            I::I32RemS => {
                let y = self.pop_value();
                let x = self.pop_value();
                if y == 0 {
                    self.raise_internal_error("integer divide by zero");
                } else {
                    self.push_value(x.wrapping_rem(y));
                }
            }
            I::I32And => self.binary(|x, y| x & y),
            I::I32Or => self.binary(|x, y| x | y),
            I::I32Xor => self.binary(|x, y| x ^ y),
            I::I32Shl => self.binary(|x, y| shl(x, amount(y))),
            I::I32ShrS => self.binary(|x, y| sar(x, amount(y))),
            I::I32ShrU => self.binary(|x, y| lshr(x, amount(y))),
            I::I32Eqz => {
                let value = self.pop_value();
                self.push_value(W::from(value == 0));
            }
            I::I32Eq => self.compare(|x, y| x == y),
            I::I32LtS => self.compare(|x, y| x < y),
            I::I32LeS => self.compare(|x, y| x <= y),
            I::I32GtS => self.compare(|x, y| x > y),
            I::I32GeS => self.compare(|x, y| x >= y),
            I::I32LtU => self.compare_u(|x, y| x < y),
            I::I32LeU => self.compare_u(|x, y| x <= y),
            I::I32GtU => self.compare_u(|x, y| x > y),
            I::I32GeU => self.compare_u(|x, y| x >= y),
            I::I32Load(offset) => self.load(offset, Self::get_word),
            I::I32Load8U(offset) => self.load(offset, |st, a| st.get_byte(a) as W),
            I::I32Load8S(offset) => self.load(offset, |st, a| st.get_byte(a) as i8 as W),
            I::I32Store(offset) => self.store(offset, Self::set_word),
            I::I32Store8(offset) => self.store(offset, |st, a, v| st.set_byte(a, v as u8)),
            I::If => {
                let condition = self.pop_value();
                if condition == 0 {
                    return self.after_matching_else_or_end(pc, &instr);
                }
            }
            I::Else => {
                return self.after_matching_end(pc, &instr, Self::set_pc);
            }
            I::End => match self.scope_at_depth(0) {
                Some(Err(err)) => return self.raise_internal_error(err),
                Some(Ok(Scope::Block { end } | Scope::Loop { end, .. })) if end == pc => self.retire_scopes(1),
                _ => {}
            },
            I::Loop => {
                let size = instr.byte_size() as i64;
                let end_size = I::<W>::End.byte_size() as i64;
                return self.after_matching_end(pc, &instr, |st, after_end| {
                    st.push_scope(Scope::Loop {
                        start: pc + size,
                        end: after_end - end_size,
                    });
                    st.next_pc(&instr);
                });
            }
            I::Block => {
                let end_size = I::<W>::End.byte_size() as i64;
                return self.after_matching_end(pc, &instr, |st, after_end| {
                    st.push_scope(Scope::Block {
                        end: after_end - end_size,
                    });
                    st.next_pc(&instr);
                });
            }
            I::Br(depth) => return self.branch_to(depth as i64),
            I::BrIf(depth) => {
                let condition = self.pop_value();
                if condition != 0 {
                    return self.branch_to(depth as i64);
                }
            }
            I::Drop => {
                self.pop_value();
            }
            I::Select => {
                let condition = self.pop_value();
                let on_false = self.pop_value();
                let on_true = self.pop_value();
                self.push_value(if condition != 0 { on_true } else { on_false });
            }
            I::Unreachable => return self.raise_internal_error("unreachable"),
            I::LocalGet(idx) => {
                let addr = self.local_addr(idx as i64);
                let value = self.get_word(addr);
                self.push_value(value);
            }
            I::LocalSet(idx) => {
                let addr = self.local_addr(idx as i64);
                let value = self.pop_value();
                self.set_word(addr, value);
            }
            I::LocalTee(idx) => {
                let addr = self.local_addr(idx as i64);
                let value = self.pop_value();
                self.set_word(addr, value);
                self.push_value(value);
            }
            I::LocalsReserve(n) => self.reserve_locals(n as i64),
            I::Call {
                param_count,
                result_count,
            } => {
                let target = self.pop_value();
                let callee_frame_base = self.sp + (param_count as i64 - 1) * WORD;
                self.push_scope(Scope::Call {
                    caller_frame_base: self.frame_base,
                    caller_local_count: self.local_count,
                    return_pc: pc + instr.byte_size() as i64,
                    result_count: result_count as i64,
                });
                self.frame_base = callee_frame_base;
                self.local_count = param_count as i64;
                return self.set_pc(target as i64);
            }
            I::Return => return self.return_from_call(),
            I::SpInit => {
                let addr = self.pop_value() as i64;
                self.sp = addr;
                self.stack_root = addr;
                self.ctrl_sp = addr;
                self.frame_base = addr - WORD;
                self.local_count = 0;
            }
            I::Halt => return self.stopped = true,
        }
        next(self);
    }

    fn pc(&self) -> i64 {
        self.pc
    }

    fn mem(&self) -> &IoMem<Self::Instr> {
        &self.mem
    }

    fn is_halted(&self) -> bool {
        self.stopped
    }

    fn views() -> Vec<View<Self>> {
        wasm32_views()
    }
}
