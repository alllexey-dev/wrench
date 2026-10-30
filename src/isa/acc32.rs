//! Acc32: an accumulator machine. Every instruction works on one register,
//! the accumulator, and an operand in memory.

use crate::hs::show_arg;
use crate::machine::{Arch, Machine, HALTED};
use crate::memory::{Instruction, IoMem};
use crate::parser::{Ref, P, R};
use crate::report::view::View;
use crate::report::{flag_view, value_view};
use crate::rng::RandomStream;
use crate::translator::ParsedInstr;
use crate::word::{add_ext, div_floor, i8, mul_ext, sar, shl, sub_ext, u16, Ext, W};

#[derive(Clone, Debug)]
pub enum Acc32Isa<L> {
    /// `load_imm <address>`: load an immediate value into the accumulator.
    LoadImm(L),
    /// `load_addr <address>`: load a value from a specific address.
    LoadAddr(L),
    /// `load <address>`: load a value from a nearby address, encoded as the offset `<address> - pc`.
    Load(L),
    /// `load_acc`: load a value from the address in acc.
    LoadAcc,
    /// `store_addr <address>`: store acc into a specific address.
    StoreAddr(L),
    /// `store <address>`: store acc into a nearby address, encoded as the offset `<address> - pc`.
    Store(L),
    /// `store_ind <address>`: store acc into an indirect address.
    StoreInd(L),
    Add(L),
    Sub(L),
    Mul(L),
    Div(L),
    Rem(L),
    /// `clv`: clear the overflow flag.
    Clv,
    /// `clc`: clear the carry flag.
    Clc,
    ShiftL(L),
    ShiftR(L),
    And(L),
    Or(L),
    Xor(L),
    Not,
    Jmp(L),
    Beqz(L),
    Bnez(L),
    /// `bgtz`.
    Bgz(L),
    /// `bltz`.
    Blz(L),
    Bgez(L),
    Bvs(L),
    Bvc(L),
    Bcs(L),
    Bcc(L),
    Halt,
}

use Acc32Isa as I;

impl<L> Acc32Isa<L> {
    fn size(&self) -> usize {
        match self {
            I::LoadImm(_) | I::LoadAddr(_) | I::StoreAddr(_) | I::StoreInd(_) => 5,
            I::Beqz(_) | I::Bnez(_) | I::Bgz(_) | I::Bgez(_) | I::Blz(_) => 5,
            I::Bvs(_) | I::Bvc(_) | I::Bcs(_) | I::Bcc(_) | I::Jmp(_) => 5,
            I::Load(_) | I::Store(_) => 2,
            I::LoadAcc | I::Not | I::Clv | I::Clc | I::Halt => 1,
            _ => 3,
        }
    }
}

impl ParsedInstr for Acc32Isa<Ref> {
    fn byte_size(&self) -> usize {
        self.size()
    }
}

impl Instruction for Acc32Isa<W> {
    fn byte_size(&self) -> usize {
        self.size()
    }

    fn show(&self) -> String {
        let (name, arg) = match self {
            I::LoadImm(a) => ("LoadImm", a),
            I::LoadAddr(a) => ("LoadAddr", a),
            I::Load(a) => ("Load", a),
            I::LoadAcc => return "LoadAcc".to_string(),
            I::StoreAddr(a) => ("StoreAddr", a),
            I::Store(a) => ("Store", a),
            I::StoreInd(a) => ("StoreInd", a),
            I::Add(a) => ("Add", a),
            I::Sub(a) => ("Sub", a),
            I::Mul(a) => ("Mul", a),
            I::Div(a) => ("Div", a),
            I::Rem(a) => ("Rem", a),
            I::Clv => return "Clv".to_string(),
            I::Clc => return "Clc".to_string(),
            I::ShiftL(a) => ("ShiftL", a),
            I::ShiftR(a) => ("ShiftR", a),
            I::And(a) => ("And", a),
            I::Or(a) => ("Or", a),
            I::Xor(a) => ("Xor", a),
            I::Not => return "Not".to_string(),
            I::Jmp(a) => ("Jmp", a),
            I::Beqz(a) => ("Beqz", a),
            I::Bnez(a) => ("Bnez", a),
            I::Bgz(a) => ("Bgz", a),
            I::Blz(a) => ("Blz", a),
            I::Bgez(a) => ("Bgez", a),
            I::Bvs(a) => ("Bvs", a),
            I::Bvc(a) => ("Bvc", a),
            I::Bcs(a) => ("Bcs", a),
            I::Bcc(a) => ("Bcc", a),
            I::Halt => return "Halt".to_string(),
        };
        format!("{} {}", name, show_arg(*arg))
    }
}

/// `hspace1 <|> eol' "\\"`: what ends a mnemonic; the comment start here is `\`, not `;`.
fn mnemonic_end(p: &mut P) -> R<()> {
    p.or(|p| p.hspace1(), |p| p.eol_("\\"))
}

fn cmd0(p: &mut P, m: &str, i: Parsed) -> R<Parsed> {
    p.attempt(|p| {
        p.hspace()?;
        p.string(m)?;
        mnemonic_end(p)?;
        Ok(i)
    })
}

fn cmd1(p: &mut P, m: &str, f: fn(Ref) -> Parsed) -> R<Parsed> {
    p.attempt(|p| {
        p.hspace()?;
        p.string(m)?;
        p.hspace1()?;
        let r = p.reference()?;
        mnemonic_end(p)?;
        Ok(f(r))
    })
}

type Parsed = Acc32Isa<Ref>;

pub struct Acc32;

impl Arch for Acc32 {
    type Parsed = Parsed;
    type State = Acc32St;
    const NAME: &'static str = "acc32";
    const COMMENT: &'static str = ";";

    fn mnemonic(p: &mut P) -> R<Parsed> {
        p.choice(&[
            &|p| cmd1(p, "load_imm", I::LoadImm),
            &|p| cmd1(p, "load_addr", I::LoadAddr),
            &|p| cmd0(p, "load_acc", I::LoadAcc),
            &|p| cmd1(p, "load", I::Load),
            &|p| cmd1(p, "store_addr", I::StoreAddr),
            &|p| cmd1(p, "store_ind", I::StoreInd),
            &|p| cmd1(p, "store", I::Store),
            &|p| cmd1(p, "add", I::Add),
            &|p| cmd1(p, "sub", I::Sub),
            &|p| cmd1(p, "mul", I::Mul),
            &|p| cmd1(p, "div", I::Div),
            &|p| cmd1(p, "rem", I::Rem),
            &|p| cmd0(p, "clv", I::Clv),
            &|p| cmd0(p, "clc", I::Clc),
            &|p| cmd1(p, "shiftl", I::ShiftL),
            &|p| cmd1(p, "shiftr", I::ShiftR),
            &|p| cmd1(p, "and", I::And),
            &|p| cmd1(p, "or", I::Or),
            &|p| cmd1(p, "xor", I::Xor),
            &|p| cmd0(p, "not", I::Not),
            &|p| cmd1(p, "jmp", I::Jmp),
            &|p| cmd1(p, "beqz", I::Beqz),
            &|p| cmd1(p, "bnez", I::Bnez),
            &|p| cmd1(p, "bgtz", I::Bgz),
            &|p| cmd1(p, "bgez", I::Bgez),
            &|p| cmd1(p, "bltz", I::Blz),
            &|p| cmd1(p, "bvs", I::Bvs),
            &|p| cmd1(p, "bvc", I::Bvc),
            &|p| cmd1(p, "bcs", I::Bcs),
            &|p| cmd1(p, "bcc", I::Bcc),
            &|p| cmd0(p, "halt", I::Halt),
        ])
    }

    fn deref(i: &Parsed, offset: W, f: &dyn Fn(&str) -> Option<W>) -> Result<Acc32Isa<W>, String> {
        let rel = |m: &'static str| {
            move |x: W| {
                i8(
                    |v| format!("{}: relative offset {} is not in -128..127", m, v),
                    x.wrapping_sub(offset),
                )
            }
        };
        let abs = |m: &'static str| move |x: W| u16(|v| format!("{}: address {} is not in 0..65535", m, v), x);
        let any = |l: &Ref| l.resolve(Ok, f);
        Ok(match i {
            I::LoadImm(l) => I::LoadImm(any(l)?),
            I::LoadAddr(l) => I::LoadAddr(any(l)?),
            I::Load(l) => I::Load(l.resolve(rel("load"), f)?),
            I::LoadAcc => I::LoadAcc,
            I::StoreAddr(l) => I::StoreAddr(any(l)?),
            I::Store(l) => I::Store(l.resolve(rel("store"), f)?),
            I::StoreInd(l) => I::StoreInd(any(l)?),
            I::Add(l) => I::Add(l.resolve(abs("add"), f)?),
            I::Sub(l) => I::Sub(l.resolve(abs("sub"), f)?),
            I::Mul(l) => I::Mul(l.resolve(abs("mul"), f)?),
            I::Div(l) => I::Div(l.resolve(abs("div"), f)?),
            I::Rem(l) => I::Rem(l.resolve(abs("rem"), f)?),
            I::Clv => I::Clv,
            I::Clc => I::Clc,
            I::ShiftL(l) => I::ShiftL(l.resolve(abs("shiftl"), f)?),
            I::ShiftR(l) => I::ShiftR(l.resolve(abs("shiftr"), f)?),
            I::And(l) => I::And(l.resolve(abs("and"), f)?),
            I::Or(l) => I::Or(l.resolve(abs("or"), f)?),
            I::Xor(l) => I::Xor(l.resolve(abs("xor"), f)?),
            I::Not => I::Not,
            I::Beqz(l) => I::Beqz(any(l)?),
            I::Bnez(l) => I::Bnez(any(l)?),
            I::Bgz(l) => I::Bgz(any(l)?),
            I::Bgez(l) => I::Bgez(any(l)?),
            I::Blz(l) => I::Blz(any(l)?),
            I::Bvs(l) => I::Bvs(any(l)?),
            I::Bvc(l) => I::Bvc(any(l)?),
            I::Bcs(l) => I::Bcs(any(l)?),
            I::Bcc(l) => I::Bcc(any(l)?),
            I::Jmp(l) => I::Jmp(any(l)?),
            I::Halt => I::Halt,
        })
    }
}

#[derive(Clone)]
pub struct Acc32St {
    pub pc: i64,
    pub acc: W,
    pub overflow_flag: bool,
    pub carry_flag: bool,
    pub mem: IoMem<Acc32Isa<W>>,
    pub stopped: bool,
    pub internal_error: Option<String>,
}

impl Acc32St {
    /// Steps over the current instruction by fetching it again, as the
    /// original does: the second read is counted too.
    fn next_pc(&mut self) {
        match self.fetch() {
            Ok((pc, i)) => self.pc = pc + i.byte_size() as i64,
            Err(e) => self.internal_error = Some(format!("nextPc: {}", e)),
        }
    }

    fn mem_error(&mut self, err: String) {
        self.internal_error = Some(format!("memory access error: {}", err));
    }

    fn get_word(&mut self, addr: i64) -> W {
        self.mem.read_word(addr).unwrap_or_else(|e| {
            self.mem_error(e);
            0
        })
    }

    fn set_word(&mut self, addr: i64, w: W) {
        if let Err(e) = self.mem.write_word(addr, w) {
            self.mem_error(e);
        }
    }

    fn with_ext(&mut self, f: fn(W, W) -> Ext, addr: W) {
        let value = self.get_word(addr as i64);
        let Ext { value, overflow, carry } = f(self.acc, value);
        self.acc = value;
        self.overflow_flag = overflow;
        self.carry_flag = carry;
        self.next_pc();
    }

    fn with_acc(&mut self, f: impl Fn(W, W) -> W, addr: W) {
        let value = self.get_word(addr as i64);
        self.acc = f(self.acc, value);
        self.next_pc();
    }

    fn jump_if(&mut self, taken: bool, a: W) {
        if taken {
            self.pc = a as i64;
        } else {
            self.next_pc();
        }
    }
}

impl Machine for Acc32St {
    type Instr = Acc32Isa<W>;

    fn init(pc: i64, mem: IoMem<Self::Instr>, _randoms: RandomStream) -> Self {
        Acc32St {
            pc,
            acc: 0,
            overflow_flag: false,
            carry_flag: false,
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
        let i = self.mem.read_instruction(self.pc)?;
        Ok((self.pc, i))
    }

    fn execute(&mut self, pc: i64, instr: Self::Instr) {
        match instr {
            I::LoadImm(a) => {
                self.acc = a;
                self.next_pc();
            }
            I::LoadAddr(a) => {
                self.acc = self.get_word(a as i64);
                self.next_pc();
            }
            I::Load(a) => {
                self.acc = self.get_word(pc + a as i64);
                self.next_pc();
            }
            I::LoadAcc => {
                self.acc = self.get_word(self.acc as i64);
                self.next_pc();
            }
            I::StoreAddr(a) => {
                self.set_word(a as i64, self.acc);
                self.next_pc();
            }
            I::StoreInd(a) => {
                let addr = self.get_word(a as i64);
                self.set_word(addr as i64, self.acc);
                self.next_pc();
            }
            I::Store(a) => {
                self.set_word(pc + a as i64, self.acc);
                self.next_pc();
            }
            I::Add(a) => self.with_ext(add_ext, a),
            I::Sub(a) => self.with_ext(sub_ext, a),
            I::Mul(a) => self.with_ext(mul_ext, a),
            I::Div(a) => self.with_acc(div_floor, a),
            I::Rem(a) => self.with_acc(W::wrapping_rem, a),
            I::Clv => {
                self.overflow_flag = false;
                self.next_pc();
            }
            I::Clc => {
                self.carry_flag = false;
                self.next_pc();
            }
            I::ShiftL(a) => self.with_acc(|x, y| shl(x, y as i64), a),
            I::ShiftR(a) => self.with_acc(|x, y| sar(x, y as i64), a),
            I::And(a) => self.with_acc(|x, y| x & y, a),
            I::Or(a) => self.with_acc(|x, y| x | y, a),
            I::Xor(a) => self.with_acc(|x, y| x ^ y, a),
            I::Not => {
                self.acc = !self.acc;
                self.next_pc();
            }
            I::Jmp(a) => self.pc = a as i64,
            I::Beqz(a) => self.jump_if(self.acc == 0, a),
            I::Bnez(a) => self.jump_if(self.acc != 0, a),
            I::Bgz(a) => self.jump_if(self.acc > 0, a),
            I::Bgez(a) => self.jump_if(self.acc >= 0, a),
            I::Blz(a) => self.jump_if(self.acc < 0, a),
            I::Bvs(a) => self.jump_if(self.overflow_flag, a),
            I::Bvc(a) => self.jump_if(!self.overflow_flag, a),
            I::Bcs(a) => self.jump_if(self.carry_flag, a),
            I::Bcc(a) => self.jump_if(!self.carry_flag, a),
            I::Halt => self.stopped = true,
        }
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
        let bit = |f: bool| if f { "1" } else { "0" }.to_string();
        vec![
            value_view::<Acc32St>("Acc", "Accumulator.", |e| e.state.acc as i64),
            flag_view::<Acc32St>("V", "Overflow flag, `1` or `0`.", move |e| bit(e.state.overflow_flag)),
            flag_view::<Acc32St>("C", "Carry flag, `1` or `0`.", move |e| bit(e.state.carry_flag)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Cell, Mem};
    use crate::rng::StdGen;
    use crate::translator::translate;

    /// The translated program as text, or the translation error.
    fn translate_program(src: &str) -> Result<String, String> {
        let tr = translate::<Acc32>(1000, &[0], "test.s", src)?;
        let mut labels: Vec<_> = tr.labels.into_iter().collect();
        labels.sort();
        Ok(format!("{:?} {:?}", tr.dump.data, labels))
    }

    fn accepted(src: &str) {
        if let Err(e) = translate_program(src) {
            panic!("should be accepted, got: {}", e);
        }
    }

    fn rejected(src: &str, expected: &str) {
        match translate_program(src) {
            Ok(n) => panic!("should be rejected, got {} characters", n.len()),
            Err(e) => assert_eq!(e, expected),
        }
    }

    /// A `LoadImm` at 0 (5 bytes) followed by the branch at 5 (5 bytes);
    /// after two steps the branch is evaluated.
    fn run_branch(branch: Acc32Isa<W>, acc: W) -> i64 {
        let mut data: Vec<Cell<Acc32Isa<W>>> = vec![Cell::Instruction(I::LoadImm(acc))];
        data.extend((1..=4).map(|_| Cell::InstructionPart));
        data.push(Cell::Instruction(branch));
        data.extend((6..=9).map(|_| Cell::InstructionPart));
        let mem = Mem {
            size: 10,
            data: data.into_iter().collect(),
        };
        let mut st = Acc32St::init(
            0,
            IoMem::new(Default::default(), mem),
            RandomStream::new(StdGen::new(0)),
        );
        st.step();
        st.step();
        st.pc()
    }

    #[test]
    fn bgtz() {
        assert_eq!(run_branch(I::Bgz(100), 5), 100);
        assert_eq!(run_branch(I::Bgz(100), 0), 10);
        assert_eq!(run_branch(I::Bgz(100), -1), 10);
    }

    #[test]
    fn bltz() {
        assert_eq!(run_branch(I::Blz(100), -1), 100);
        assert_eq!(run_branch(I::Blz(100), 0), 10);
        assert_eq!(run_branch(I::Blz(100), 5), 10);
    }

    #[test]
    fn bgez() {
        assert_eq!(run_branch(I::Bgez(100), 0), 100);
        assert_eq!(run_branch(I::Bgez(100), 5), 100);
        assert_eq!(run_branch(I::Bgez(100), -1), 10);
    }

    #[test]
    fn absolute_address_that_fits_is_accepted() {
        accepted(".text\n_start:\n    add cell\n    halt\n\n    .data\ncell:    .word 7\n");
    }

    #[test]
    fn absolute_address_rejection_names_position_mnemonic_field_and_range() {
        rejected(
            ".text\n_start:\n    add 0x10000\n    halt\n",
            "test.s:3:9: add: address 65536 is not in 0..65535",
        );
    }

    #[test]
    fn negative_absolute_address_is_rejected() {
        rejected(
            ".text\n_start:\n    sub -1\n    halt\n",
            "test.s:3:9: sub: address -1 is not in 0..65535",
        );
    }

    #[test]
    fn every_arithmetic_and_bitwise_operand_is_checked() {
        for m in [
            "add", "sub", "mul", "div", "rem", "shiftl", "shiftr", "and", "or", "xor",
        ] {
            match translate_program(&format!(".text\n_start:\n    {} 0x10000\n    halt\n", m)) {
                Ok(n) => panic!("{} should be rejected, got {} characters", m, n.len()),
                Err(e) => {
                    assert!(e.contains(&format!("{}: address", m)), "{} in: {}", m, e);
                    assert!(e.contains("0..65535"), "range in: {}", e);
                }
            }
        }
    }

    #[test]
    fn relative_address_in_reach_is_accepted() {
        accepted(".text\n_start:\n    load cell\n    halt\n\n    .data\ncell:    .word 7\n");
    }

    #[test]
    fn relative_constant_is_an_address_like_a_label() {
        let program = |operand: &str| {
            translate_program(&format!(
                ".text\n_start:\n    load_imm 0\n    load {}\n    halt\n\n    .data\n    .org 0x10\ncell:    .word 7\n",
                operand
            ))
        };
        assert_eq!(program("0x10"), program("cell"));
    }

    #[test]
    fn relative_address_out_of_reach_is_rejected_by_distance() {
        rejected(
            ".text\n_start:\n    load_imm 0\n    load 200\n    halt\n",
            "test.s:4:10: load: relative offset 195 is not in -128..127",
        );
    }

    #[test]
    fn relative_address_behind_the_instruction() {
        accepted(".text\n    .org 0x100\n_start:\n    store 0x80\n    halt\n");
        rejected(
            ".text\n    .org 0x100\n_start:\n    store 0x7f\n    halt\n",
            "test.s:4:11: store: relative offset -129 is not in -128..127",
        );
    }

    #[test]
    fn relative_label_out_of_reach_is_rejected_by_distance() {
        rejected(
            ".text\n_start:\n    store far\n    halt\n\n    .data\n    .org 0x200\nfar:    .word 0\n",
            "test.s:3:11: store: relative offset 512 is not in -128..127",
        );
    }

    #[test]
    fn absolute_operands_take_a_full_machine_word() {
        accepted(".text\n_start:\n    load_addr 0x12345678\n    halt\n");
    }
}
