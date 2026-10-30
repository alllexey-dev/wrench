//! F32a: a stack machine inspired by the GreenArrays F18A
//! (<https://www.greenarraychips.com/home/documents/greg/DB001-221113-F18a.pdf>).

use crate::hs::show_arg;
use crate::machine::{Arch, Machine, HALTED};
use crate::memory::{Instruction, IoMem};
use crate::parser::{Ref, P, R};
use crate::report::view::{Format, Spec, Stat, View};
use crate::report::{flag_view, show_values, value_view};
use crate::rng::RandomStream;
use crate::translator::ParsedInstr;
use crate::word::{add_ext, W};

/// Every word the parser accepts as an instruction.
pub const MNEMONICS: &[&str] = &[
    "lit", "next", "if", "-if", "a!", "b!", "+*", "+/", "2*", "2/", "inv", "+", "eam", "and", "xor", "drop", "dup",
    "over", "a", "@p", "@+", "@b", "@", "!p", "!b", "!+", "!", "r>", ">r", "halt", ";",
];

#[derive(Clone, Debug)]
pub enum F32aIsa<L> {
    /// `;` ( -- ): pops the return stack into P.
    Return,
    /// `<address>` ( -- ): pushes P onto the return stack and jumps.
    Call(L),
    /// `lit <value>` or a bare number ( -- value ).
    Lit(L),
    /// `next <address>` ( -- ): pops R; falls through when it was 0,
    /// otherwise pushes `R-1` back and jumps.
    Next(L),
    /// `<address> ;` ( -- ).
    Jump(L),
    /// `if <address>` ( flag -- ): jumps if T was zero.
    If(L),
    /// `-if <address>` ( value -- ): jumps if T was non-negative.
    MinusIf(L),
    /// `a!` ( value -- ).
    AStore,
    /// `b!` ( value -- ).
    BStore,
    /// `@p <address>` ( -- value ): reads `[address]`.
    FetchP(L),
    /// `@+` ( -- value ): reads `[A]`, increments A.
    FetchPlus,
    /// `@b` ( -- value ): reads `[B]`.
    FetchB,
    /// `@` ( -- value ): reads `[A]`.
    Fetch,
    /// `!p <address>` ( value -- ): writes `[address]`.
    StoreP(L),
    /// `!b` ( value -- ): writes `[B]`.
    StoreB,
    /// `!+` ( value -- ): writes `[A]`, increments A.
    StorePlus,
    /// `!` ( value -- ): writes `[A]`.
    Store,
    /// `+*` ( s t -- s t ): one step of a shift-and-add multiply.
    MulStep,
    /// `+/` ( remainder quotient -- remainder quotient ): one step of a
    /// division, A holds the dividend and `[B]` the divisor.
    DivStep,
    /// `2*` ( value -- shifted ).
    LShift,
    /// `2/` ( value -- shifted ): signed.
    RShift,
    /// `inv` ( value -- inverted ).
    Inv,
    /// `+` ( s t -- sum ): with carry in extended arithmetic mode.
    Add,
    /// `eam` ( flag -- ): extended arithmetic mode iff T was non-zero.
    Eam,
    /// `and` ( s t -- bits ).
    And,
    /// `xor` ( s t -- bits ).
    Xor,
    /// `drop` ( value -- ).
    Drop,
    /// `dup` ( value -- value value ).
    Dup,
    /// `r>` ( -- value ): moves R into T.
    RIntoT,
    /// `>r` ( value -- ): moves T into R.
    TIntoR,
    /// `over` ( s t -- s t s ).
    Over,
    /// `a` ( -- value ): reads register A.
    AFetch,
    /// `halt` ( -- ).
    Halt,
}

use F32aIsa as I;

impl<L> F32aIsa<L> {
    // NOTE: in f18a several instructions fit one machine word; here an
    // opcode is one byte and an argument one machine word.
    fn size(&self) -> usize {
        match self {
            I::Lit(_)
            | I::Call(_)
            | I::Jump(_)
            | I::Next(_)
            | I::If(_)
            | I::MinusIf(_)
            | I::FetchP(_)
            | I::StoreP(_) => 5,
            _ => 1,
        }
    }
}

impl ParsedInstr for F32aIsa<Ref> {
    fn byte_size(&self) -> usize {
        self.size()
    }
}

impl Instruction for F32aIsa<W> {
    fn byte_size(&self) -> usize {
        self.size()
    }

    fn show(&self) -> String {
        let arg = |n: &str, l: &W| format!("{} {}", n, show_arg(*l));
        match self {
            I::Call(l) => arg("Call", l),
            I::Lit(l) => arg("Lit", l),
            I::Next(l) => arg("Next", l),
            I::Jump(l) => arg("Jump", l),
            I::If(l) => arg("If", l),
            I::MinusIf(l) => arg("MinusIf", l),
            I::FetchP(l) => arg("FetchP", l),
            I::StoreP(l) => arg("StoreP", l),
            I::Return => "Return".to_string(),
            I::AStore => "AStore".to_string(),
            I::BStore => "BStore".to_string(),
            I::FetchPlus => "FetchPlus".to_string(),
            I::FetchB => "FetchB".to_string(),
            I::Fetch => "Fetch".to_string(),
            I::StoreB => "StoreB".to_string(),
            I::StorePlus => "StorePlus".to_string(),
            I::Store => "Store".to_string(),
            I::MulStep => "MulStep".to_string(),
            I::DivStep => "DivStep".to_string(),
            I::LShift => "LShift".to_string(),
            I::RShift => "RShift".to_string(),
            I::Inv => "Inv".to_string(),
            I::Add => "Add".to_string(),
            I::Eam => "Eam".to_string(),
            I::And => "And".to_string(),
            I::Xor => "Xor".to_string(),
            I::Drop => "Drop".to_string(),
            I::Dup => "Dup".to_string(),
            I::RIntoT => "RIntoT".to_string(),
            I::TIntoR => "TIntoR".to_string(),
            I::Over => "Over".to_string(),
            I::AFetch => "AFetch".to_string(),
            I::Halt => "Halt".to_string(),
        }
    }
}

const COMMENT: &str = "\\";

/// After a word: a space, or the end of line.
fn word_end(p: &mut P) -> R<()> {
    p.or(|p| p.hspace1(), |p| p.eol_(COMMENT))
}

fn cmd0(p: &mut P, m: &str, i: Parsed) -> R<Parsed> {
    p.attempt(|p| {
        p.hspace()?;
        p.string(m)?;
        word_end(p)?;
        Ok(i)
    })
}

fn cmd1(p: &mut P, m: &str, f: fn(Ref) -> Parsed) -> R<Parsed> {
    p.attempt(|p| {
        p.hspace()?;
        p.string(m)?;
        p.hspace1()?;
        let r = p.reference()?;
        word_end(p)?;
        Ok(f(r))
    })
}

/// A number or a quoted character on its own.
fn bare_literal(p: &mut P) -> R<Ref> {
    p.attempt(|p| {
        p.hspace()?;
        let r = p.constant_ref()?;
        word_end(p)?;
        Ok(r)
    })
}

type Parsed = F32aIsa<Ref>;

pub struct F32a;

impl Arch for F32a {
    type Parsed = Parsed;
    type State = F32aSt;
    const NAME: &'static str = "f32a";
    const COMMENT: &'static str = COMMENT;

    fn mnemonic(p: &mut P) -> R<Parsed> {
        p.choice(&[
            &|p| cmd1(p, "lit", I::Lit),
            &|p| cmd1(p, "next", I::Next),
            &|p| cmd1(p, "if", I::If),
            &|p| cmd1(p, "-if", I::MinusIf),
            &|p| cmd0(p, "a!", I::AStore),
            &|p| cmd0(p, "b!", I::BStore),
            &|p| cmd0(p, "+*", I::MulStep),
            &|p| cmd0(p, "+/", I::DivStep),
            &|p| cmd0(p, "2*", I::LShift),
            &|p| cmd0(p, "2/", I::RShift),
            &|p| cmd0(p, "inv", I::Inv),
            &|p| cmd0(p, "+", I::Add),
            &|p| cmd0(p, "eam", I::Eam),
            &|p| cmd0(p, "and", I::And),
            &|p| cmd0(p, "xor", I::Xor),
            &|p| cmd0(p, "drop", I::Drop),
            &|p| cmd0(p, "dup", I::Dup),
            &|p| cmd0(p, "over", I::Over),
            &|p| cmd0(p, "a", I::AFetch),
            &|p| cmd1(p, "@p", I::FetchP),
            &|p| cmd0(p, "@+", I::FetchPlus),
            &|p| cmd0(p, "@b", I::FetchB),
            &|p| cmd0(p, "@", I::Fetch),
            &|p| cmd1(p, "!p", I::StoreP),
            &|p| cmd0(p, "!b", I::StoreB),
            &|p| cmd0(p, "!+", I::StorePlus),
            &|p| cmd0(p, "!", I::Store),
            &|p| cmd0(p, "r>", I::RIntoT),
            &|p| cmd0(p, ">r", I::TIntoR),
            &|p| cmd0(p, "halt", I::Halt),
            &|p| cmd0(p, ";", I::Return),
            &|p| {
                p.attempt(|p| {
                    let label = p.reference()?;
                    p.hspace1()?;
                    p.string(";")?;
                    Ok(I::Jump(label))
                })
            },
            &|p| bare_literal(p).map(I::Lit),
            &|p| {
                p.attempt(|p| {
                    let label = p.reference()?;
                    word_end(p)?;
                    Ok(I::Call(label))
                })
            },
        ])
    }

    fn deref(i: &Parsed, _offset: W, f: &dyn Fn(&str) -> Option<W>) -> Result<F32aIsa<W>, String> {
        let r = |l: &Ref| l.resolve(Ok::<W, String>, f);
        Ok(match i {
            I::Lit(l) => I::Lit(r(l)?),
            I::Call(l) => I::Call(r(l)?),
            I::Jump(l) => I::Jump(r(l)?),
            I::Next(l) => I::Next(r(l)?),
            I::If(l) => I::If(r(l)?),
            I::MinusIf(l) => I::MinusIf(r(l)?),
            I::FetchP(l) => I::FetchP(r(l)?),
            I::StoreP(l) => I::StoreP(r(l)?),
            I::Return => I::Return,
            I::AStore => I::AStore,
            I::BStore => I::BStore,
            I::AFetch => I::AFetch,
            I::Fetch => I::Fetch,
            I::FetchPlus => I::FetchPlus,
            I::FetchB => I::FetchB,
            I::StoreB => I::StoreB,
            I::StorePlus => I::StorePlus,
            I::Store => I::Store,
            I::RIntoT => I::RIntoT,
            I::TIntoR => I::TIntoR,
            I::MulStep => I::MulStep,
            I::DivStep => I::DivStep,
            I::LShift => I::LShift,
            I::RShift => I::RShift,
            I::Inv => I::Inv,
            I::Add => I::Add,
            I::Eam => I::Eam,
            I::And => I::And,
            I::Xor => I::Xor,
            I::Drop => I::Drop,
            I::Dup => I::Dup,
            I::Over => I::Over,
            I::Halt => I::Halt,
        })
    }
}

#[derive(Clone)]
pub struct F32aSt {
    pub p: i64,
    pub a: W,
    pub b: W,
    pub ram: IoMem<F32aIsa<W>>,
    /// Top first, as the views show it.
    pub data_stack: Vec<W>,
    pub return_stack: Vec<W>,
    /// High-water mark of the data stack depth over the run.
    pub data_stack_max: usize,
    /// High-water mark of the return stack depth over the run.
    pub return_stack_max: usize,
    pub stopped: bool,
    pub extended_arithmetic_mode: bool,
    pub carry_flag: bool,
    pub internal_error: Option<String>,
}

impl F32aSt {
    /// Past the current instruction, read again from memory.
    fn next_p(&mut self) {
        match self.fetch() {
            Ok((p, i)) => self.p = p + i.byte_size() as i64,
            Err(e) => self.internal_error = Some(format!("nextPc: {}", e)),
        }
    }

    fn mem_error(&mut self, err: String) {
        self.internal_error = Some(format!("memory access error: {}", err));
    }

    fn get_word(&mut self, addr: W) -> W {
        self.ram.read_word(addr as i64).unwrap_or_else(|e| {
            self.mem_error(e);
            0
        })
    }

    fn set_word(&mut self, addr: W, w: W) {
        if let Err(e) = self.ram.write_word(addr as i64, w) {
            self.mem_error(e);
        }
    }

    fn data_push(&mut self, w: W) {
        self.carry_flag = false;
        self.data_stack.insert(0, w);
        self.data_stack_max = self.data_stack_max.max(self.data_stack.len());
    }

    fn data_pop(&mut self) -> W {
        if self.data_stack.is_empty() {
            self.internal_error = Some("empty data stack".to_string());
            0
        } else {
            self.data_stack.remove(0)
        }
    }

    fn return_push(&mut self, w: W) {
        self.return_stack.insert(0, w);
        self.return_stack_max = self.return_stack_max.max(self.return_stack.len());
    }

    fn return_pop(&mut self) -> W {
        if self.return_stack.is_empty() {
            self.internal_error = Some("empty return stack".to_string());
            0
        } else {
            self.return_stack.remove(0)
        }
    }

    /// Pops T, then S; pushes `f(s, t)`.
    fn binary(&mut self, f: impl Fn(W, W) -> W) {
        let t = self.data_pop();
        let s = self.data_pop();
        self.data_push(f(s, t));
        self.next_p();
    }

    fn unary(&mut self, f: impl Fn(W) -> W) {
        let w = self.data_pop();
        self.data_push(f(w));
        self.next_p();
    }

    fn push_then_next(&mut self, w: W) {
        self.data_push(w);
        self.next_p();
    }
}

impl Machine for F32aSt {
    type Instr = F32aIsa<W>;

    fn init(pc: i64, mem: IoMem<Self::Instr>, _randoms: RandomStream) -> Self {
        F32aSt {
            p: pc,
            a: 0,
            b: 0,
            ram: mem,
            data_stack: Vec::new(),
            return_stack: Vec::new(),
            data_stack_max: 0,
            return_stack_max: 0,
            stopped: false,
            extended_arithmetic_mode: false,
            carry_flag: false,
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
        let i = self.ram.read_instruction(self.p)?;
        Ok((self.p, i))
    }

    fn execute(&mut self, _pc: i64, instr: Self::Instr) {
        match instr {
            I::Lit(l) => self.push_then_next(l),
            I::Return => {
                self.p = self.return_pop() as i64;
                self.next_p();
            }
            I::Call(l) => {
                self.return_push(self.p as W);
                self.p = l as i64;
            }
            I::Jump(l) => self.p = l as i64,
            I::Next(l) => {
                let r = self.return_pop();
                if r == 0 {
                    self.next_p();
                } else {
                    self.return_push(r.wrapping_sub(1));
                    self.p = l as i64;
                }
            }
            I::If(l) => {
                if self.data_pop() == 0 {
                    self.p = l as i64;
                } else {
                    self.next_p();
                }
            }
            I::MinusIf(l) => {
                if self.data_pop() >= 0 {
                    self.p = l as i64;
                } else {
                    self.next_p();
                }
            }
            I::AStore => {
                self.a = self.data_pop();
                self.next_p();
            }
            I::BStore => {
                self.b = self.data_pop();
                self.next_p();
            }
            I::AFetch => self.push_then_next(self.a),
            // [    T   ][    A   ] >> 1
            //      ^            |
            //      |            |
            //      +?<----------+ lower bit
            //      ^
            //      |
            // [    S   ]
            I::MulStep => {
                let a = self.a;
                let t = self.data_pop();
                let s = self.data_pop();
                let t1 = t.wrapping_add(if a & 1 != 0 { s } else { 0 });
                let a1 = a >> 1;
                let a2 = if t1 & 1 != 0 { a1 | W::MIN } else { a1 & W::MAX };
                self.data_push(s);
                self.data_push(t1 >> 1);
                self.a = a2;
                self.next_p();
            }
            // A <- dividend
            // [B] <- divisor
            // T -> quotient
            // S -> remainder
            I::DivStep => {
                let quotient1 = self.data_pop();
                let remainder1 = self.data_pop();
                let dividend1 = self.a;
                let divisor = self.get_word(self.b);
                let remainder3 = (remainder1 << 1) | W::from(dividend1 < 0);
                let quotient2 = quotient1 << 1;
                let (remainder4, quotient3) = if remainder3 >= divisor {
                    (remainder3.wrapping_sub(divisor), quotient2 | 1)
                } else {
                    (remainder3, quotient2)
                };
                self.a = dividend1 << 1;
                self.data_push(remainder4);
                self.data_push(quotient3);
                self.next_p();
            }
            I::LShift => self.unary(|w| w << 1),
            I::RShift => self.unary(|w| w >> 1),
            I::Inv => self.unary(|w| !w),
            I::FetchPlus => {
                let a = self.a;
                let w = self.get_word(a);
                self.data_push(w);
                self.a = a.wrapping_add(1);
                self.next_p();
            }
            I::FetchB => {
                let w = self.get_word(self.b);
                self.push_then_next(w);
            }
            I::FetchP(l) => {
                let w = self.get_word(l);
                self.push_then_next(w);
            }
            I::Fetch => {
                let w = self.get_word(self.a);
                self.push_then_next(w);
            }
            I::StoreP(l) => {
                let w = self.data_pop();
                self.set_word(l, w);
                self.next_p();
            }
            I::StorePlus => {
                let w = self.data_pop();
                let a = self.a;
                self.set_word(a, w);
                self.a = a.wrapping_add(1);
                self.next_p();
            }
            I::StoreB => {
                let w = self.data_pop();
                self.set_word(self.b, w);
                self.next_p();
            }
            I::Store => {
                let w = self.data_pop();
                self.set_word(self.a, w);
                self.next_p();
            }
            I::RIntoT => {
                let w = self.return_pop();
                self.push_then_next(w);
            }
            I::TIntoR => {
                let w = self.data_pop();
                self.return_push(w);
                self.next_p();
            }
            I::Add => {
                let t = self.data_pop();
                let s = self.data_pop();
                let e1 = add_ext(s, t);
                let (result, carry) = if self.extended_arithmetic_mode && self.carry_flag {
                    let e2 = add_ext(e1.value, 1);
                    (e2.value, e1.carry || e2.carry)
                } else {
                    (e1.value, e1.carry)
                };
                self.data_push(result);
                self.carry_flag = carry;
                self.next_p();
            }
            I::Eam => {
                self.extended_arithmetic_mode = self.data_pop() != 0;
                self.next_p();
            }
            I::And => self.binary(|s, t| s & t),
            I::Xor => self.binary(|s, t| s ^ t),
            I::Drop => {
                self.data_pop();
                self.carry_flag = false;
                self.next_p();
            }
            I::Dup => {
                let carry = self.carry_flag;
                let w = self.data_pop();
                self.data_push(w);
                self.data_push(w);
                self.carry_flag = carry;
                self.next_p();
            }
            I::Over => {
                let t = self.data_pop();
                let s = self.data_pop();
                self.data_push(s);
                self.data_push(t);
                self.push_then_next(s);
            }
            I::Halt => self.stopped = true,
        }
    }

    fn pc(&self) -> i64 {
        self.p
    }

    fn mem(&self) -> &IoMem<Self::Instr> {
        &self.ram
    }

    fn is_halted(&self) -> bool {
        self.stopped
    }

    fn views() -> Vec<View<Self>> {
        let nth = |xs: &[W], i: usize| xs.get(i).copied().unwrap_or(0) as i64;
        let stack = |name: &str, doc: &str, f: fn(&F32aSt) -> &[W]| {
            View::<F32aSt>::new(
                Spec::lit(name).format(&[Format::Dec, Format::Hex], Format::Dec),
                doc,
                move |e, a| {
                    let xs: Vec<i64> = f(e.state).iter().map(|&x| x as i64).collect();
                    show_values(a[0].fmt(), &xs)
                },
            )
        };
        let bit = |f: bool| if f { "1" } else { "0" }.to_string();
        vec![
            value_view::<F32aSt>("A", "Register `A`.", |e| e.state.a as i64),
            value_view::<F32aSt>("B", "Register `B`.", |e| e.state.b as i64),
            value_view::<F32aSt>("T", "Top of the data stack, `0` if empty.", move |e| {
                nth(&e.state.data_stack, 0)
            }),
            value_view::<F32aSt>("S", "Second on the data stack, `0` if absent.", move |e| {
                nth(&e.state.data_stack, 1)
            }),
            value_view::<F32aSt>("R", "Top of the return stack, `0` if empty.", move |e| {
                nth(&e.state.return_stack, 0)
            }),
            stack("stack", "Data stack, top first: `a:b:c`.", |s| &s.data_stack),
            stack("rstack", "Return stack, top first: `a:b:c`.", |s| &s.return_stack),
            flag_view::<F32aSt>("EAM", "Extended arithmetic mode, `1` or `0`.", move |e| {
                bit(e.state.extended_arithmetic_mode)
            }),
            flag_view::<F32aSt>("C", "Carry flag, `1` or `0`.", move |e| bit(e.state.carry_flag)),
        ]
    }

    fn stats() -> Vec<Stat<Self>> {
        vec![
            Stat::<F32aSt>::new("f32a:data-stack-max", "Deepest data stack of the run.", |e| {
                e.final_state.data_stack_max.to_string()
            }),
            Stat::<F32aSt>::new("f32a:return-stack-max", "Deepest return stack of the run.", |e| {
                e.final_state.return_stack_max.to_string()
            }),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Cell, Mem};
    use crate::rng::StdGen;
    use std::collections::BTreeMap;

    fn st0(data_stack: Vec<W>) -> F32aSt {
        let mem = Mem {
            size: 256,
            data: (0..256).map(|a| Cell::Value(a as u8)).collect(),
        };
        let mut st = F32aSt::init(256, IoMem::new(BTreeMap::new(), mem), RandomStream::new(StdGen::new(0)));
        st.data_stack = data_stack;
        st
    }

    fn simulate(code: &str, mut st: F32aSt) -> F32aSt {
        let src: Vec<char> = format!("{}\n", code).chars().collect();
        let mut p = P::new("-", &src);
        let parsed = F32a::mnemonic(&mut p).unwrap_or_else(|e| panic!("{:?}", e));
        let labels = |_: &str| -> Option<W> { panic!("labels not defined") };
        let instr = F32a::deref(&parsed, 0, &labels).unwrap();
        st.execute(0, instr);
        st
    }

    #[test]
    fn over_copies_second_element_to_top() {
        assert_eq!(simulate("over", st0(vec![10, 20, 30])).data_stack, vec![20, 10, 20, 30]);
    }

    #[test]
    fn over_with_two_elements() {
        assert_eq!(simulate("over", st0(vec![1, 2])).data_stack, vec![2, 1, 2]);
    }

    #[test]
    fn dup_duplicates_top() {
        assert_eq!(simulate("dup", st0(vec![42, 7])).data_stack, vec![42, 42, 7]);
    }

    #[test]
    fn drop_removes_top() {
        assert_eq!(simulate("drop", st0(vec![1, 2, 3])).data_stack, vec![2, 3]);
    }

    #[test]
    fn bare_decimal_literal_pushes_to_stack() {
        assert_eq!(simulate("42", st0(vec![])).data_stack, vec![42]);
    }

    #[test]
    fn bare_hex_literal_pushes_to_stack() {
        assert_eq!(simulate("0xFF", st0(vec![])).data_stack, vec![255]);
    }

    #[test]
    fn bare_negative_literal_pushes_to_stack() {
        assert_eq!(simulate("-1", st0(vec![])).data_stack, vec![-1]);
    }

    #[test]
    fn bare_char_literal_pushes_to_stack() {
        assert_eq!(simulate("'A'", st0(vec![])).data_stack, vec![65]);
    }

    #[test]
    fn lit_keyword_still_works() {
        assert_eq!(simulate("lit 42", st0(vec![])).data_stack, vec![42]);
    }
}
