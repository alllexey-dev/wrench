//! RISC-IV: a load/store RISC inspired by RISC-V
//! (<https://riscv.org/wp-content/uploads/2017/05/riscv-spec-v2.2.pdf>).

use crate::machine::{Arch, Machine, HALTED};
use crate::memory::{Instruction, IoMem};
use crate::parser::{Ref, P, R};
use crate::report::value_view;
use crate::report::view::{with_renames, View};
use crate::rng::RandomStream;
use crate::translator::ParsedInstr;
use crate::word::{fit_signed, i12, i13, i21, lshr, sar, shl, u20, u5, W};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Register {
    Zero,
    Ra,
    Sp,
    Gp,
    Tp,
    T0,
    T1,
    T2,
    S0Fp,
    S1,
    A0,
    A1,
    A2,
    A3,
    A4,
    A5,
    A6,
    A7,
    S2,
    S3,
    S4,
    S5,
    S6,
    S7,
    S8,
    S9,
    S10,
    S11,
    T3,
    T4,
    T5,
    T6,
}

use Register::*;

pub const ALL_REGISTERS: [Register; 32] = [
    Zero, Ra, Sp, Gp, Tp, T0, T1, T2, S0Fp, S1, A0, A1, A2, A3, A4, A5, A6, A7, S2, S3, S4, S5, S6, S7, S8, S9, S10,
    S11, T3, T4, T5, T6,
];

/// Assembly names of the registers, in parser order (`s10` before `s1`).
pub const REGISTER_NAMES: [(&str, Register); 33] = [
    ("zero", Zero),
    ("ra", Ra),
    ("sp", Sp),
    ("gp", Gp),
    ("tp", Tp),
    ("t0", T0),
    ("t1", T1),
    ("t2", T2),
    ("s0", S0Fp),
    ("fp", S0Fp),
    ("s10", S10),
    ("s11", S11),
    ("s1", S1),
    ("a0", A0),
    ("a1", A1),
    ("a2", A2),
    ("a3", A3),
    ("a4", A4),
    ("a5", A5),
    ("a6", A6),
    ("a7", A7),
    ("s2", S2),
    ("s3", S3),
    ("s4", S4),
    ("s5", S5),
    ("s6", S6),
    ("s7", S7),
    ("s8", S8),
    ("s9", S9),
    ("t3", T3),
    ("t4", T4),
    ("t5", T5),
    ("t6", T6),
];

impl Register {
    /// The constructor name, as instructions are shown.
    pub fn show(self) -> String {
        format!("{:?}", self)
    }

    fn asm_names(self) -> Vec<&'static str> {
        REGISTER_NAMES
            .iter()
            .filter(|(_, r)| *r == self)
            .map(|(n, _)| *n)
            .collect()
    }
}

pub fn register(p: &mut P) -> R<Register> {
    p.keyword(&REGISTER_NAMES)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemRef {
    pub offset: W,
    pub reg: Register,
}

impl MemRef {
    pub fn show(&self) -> String {
        format!("MemRef {{mrOffset = {}, mrReg = {}}}", self.offset, self.reg.show())
    }
}

/// Width of the signed offset field of `lw`, `lb`, `sw`, `sb`.
const MEM_REF_OFFSET_BITS: u32 = 12;

pub fn require_signed_field(p: &mut P, field_bits: u32, operand_pos: usize, value: W) -> R<W> {
    if value == fit_signed(field_bits, value) {
        Ok(value)
    } else {
        let hi = (1i64 << (field_bits - 1)) - 1;
        let lo = -(1i64 << (field_bits - 1));
        p.fail_at(
            operand_pos,
            format!(
                "offset {} doesn't fit the {}-bit signed field of a 4 byte instruction, expected {}..{}",
                value, field_bits, lo, hi
            ),
        )
    }
}

pub fn mem_ref(p: &mut P) -> R<MemRef> {
    p.choice(&[
        &|p| {
            let operand_pos = p.offset();
            let offset = p.word_literal()?;
            p.char('(')?;
            let reg = register(p)?;
            p.char(')')?;
            let offset = require_signed_field(p, MEM_REF_OFFSET_BITS, operand_pos, offset)?;
            Ok(MemRef { offset, reg })
        },
        &|p| register(p).map(|reg| MemRef { offset: 0, reg }),
    ])
}

#[derive(Clone, Debug)]
pub enum RiscIvIsa<L> {
    Addi { rd: Register, rs1: Register, k: L },
    Add { rd: Register, rs1: Register, rs2: Register },
    Sub { rd: Register, rs1: Register, rs2: Register },
    Mul { rd: Register, rs1: Register, rs2: Register },
    Mulh { rd: Register, rs1: Register, rs2: Register },
    Div { rd: Register, rs1: Register, rs2: Register },
    Rem { rd: Register, rs1: Register, rs2: Register },
    Sll { rd: Register, rs1: Register, rs2: Register },
    Srl { rd: Register, rs1: Register, rs2: Register },
    Sra { rd: Register, rs1: Register, rs2: Register },
    And { rd: Register, rs1: Register, rs2: Register },
    Or { rd: Register, rs1: Register, rs2: Register },
    Xor { rd: Register, rs1: Register, rs2: Register },
    Slti { rd: Register, rs1: Register, k: L },
    Slli { rd: Register, rs1: Register, k: L },
    Srli { rd: Register, rs1: Register, k: L },
    Srai { rd: Register, rs1: Register, k: L },
    Andi { rd: Register, rs1: Register, k: L },
    Ori { rd: Register, rs1: Register, k: L },
    Xori { rd: Register, rs1: Register, k: L },
    Mv { rd: Register, rs: Register },
    Sw { rs2: Register, offset_rs1: MemRef },
    Sb { rs2: Register, offset_rs1: MemRef },
    Lui { rd: Register, k: L },
    Lw { rd: Register, offset_rs1: MemRef },
    Lb { rd: Register, offset_rs1: MemRef },
    J { k: L },
    Jal { rd: Register, k: L },
    Jalr { rd: Register, rs1: Register, k: L },
    Beqz { rs1: Register, k: L },
    Bnez { rs1: Register, k: L },
    Blt { rs1: Register, rs2: Register, k: L },
    Bge { rs1: Register, rs2: Register, k: L },
    Bltu { rs1: Register, rs2: Register, k: L },
    Bgeu { rs1: Register, rs2: Register, k: L },
    Beq { rs1: Register, rs2: Register, k: L },
    Bne { rs1: Register, rs2: Register, k: L },
    Halt,
}

use RiscIvIsa as I;

impl ParsedInstr for RiscIvIsa<Ref> {
    fn byte_size(&self) -> usize {
        4
    }
}

impl Instruction for RiscIvIsa<W> {
    fn byte_size(&self) -> usize {
        4
    }

    fn show(&self) -> String {
        let r3 = |n: &str, rd: &Register, rs1: &Register, rs2: &Register| {
            format!(
                "{} {{rd = {}, rs1 = {}, rs2 = {}}}",
                n,
                rd.show(),
                rs1.show(),
                rs2.show()
            )
        };
        let ri = |n: &str, rd: &Register, rs1: &Register, k: &W| {
            format!("{} {{rd = {}, rs1 = {}, k = {}}}", n, rd.show(), rs1.show(), k)
        };
        let b2 = |n: &str, rs1: &Register, rs2: &Register, k: &W| {
            format!("{} {{rs1 = {}, rs2 = {}, k = {}}}", n, rs1.show(), rs2.show(), k)
        };
        match self {
            I::Addi { rd, rs1, k } => ri("Addi", rd, rs1, k),
            I::Add { rd, rs1, rs2 } => r3("Add", rd, rs1, rs2),
            I::Sub { rd, rs1, rs2 } => r3("Sub", rd, rs1, rs2),
            I::Mul { rd, rs1, rs2 } => r3("Mul", rd, rs1, rs2),
            I::Mulh { rd, rs1, rs2 } => r3("Mulh", rd, rs1, rs2),
            I::Div { rd, rs1, rs2 } => r3("Div", rd, rs1, rs2),
            I::Rem { rd, rs1, rs2 } => r3("Rem", rd, rs1, rs2),
            I::Sll { rd, rs1, rs2 } => r3("Sll", rd, rs1, rs2),
            I::Srl { rd, rs1, rs2 } => r3("Srl", rd, rs1, rs2),
            I::Sra { rd, rs1, rs2 } => r3("Sra", rd, rs1, rs2),
            I::And { rd, rs1, rs2 } => r3("And", rd, rs1, rs2),
            I::Or { rd, rs1, rs2 } => r3("Or", rd, rs1, rs2),
            I::Xor { rd, rs1, rs2 } => r3("Xor", rd, rs1, rs2),
            I::Slti { rd, rs1, k } => ri("Slti", rd, rs1, k),
            I::Slli { rd, rs1, k } => ri("Slli", rd, rs1, k),
            I::Srli { rd, rs1, k } => ri("Srli", rd, rs1, k),
            I::Srai { rd, rs1, k } => ri("Srai", rd, rs1, k),
            I::Andi { rd, rs1, k } => ri("Andi", rd, rs1, k),
            I::Ori { rd, rs1, k } => ri("Ori", rd, rs1, k),
            I::Xori { rd, rs1, k } => ri("Xori", rd, rs1, k),
            I::Mv { rd, rs } => format!("Mv {{rd = {}, rs = {}}}", rd.show(), rs.show()),
            I::Sw { rs2, offset_rs1 } => format!("Sw {{rs2 = {}, offsetRs1 = {}}}", rs2.show(), offset_rs1.show()),
            I::Sb { rs2, offset_rs1 } => format!("Sb {{rs2 = {}, offsetRs1 = {}}}", rs2.show(), offset_rs1.show()),
            I::Lui { rd, k } => format!("Lui {{rd = {}, k = {}}}", rd.show(), k),
            I::Lw { rd, offset_rs1 } => format!("Lw {{rd = {}, offsetRs1 = {}}}", rd.show(), offset_rs1.show()),
            I::Lb { rd, offset_rs1 } => format!("Lb {{rd = {}, offsetRs1 = {}}}", rd.show(), offset_rs1.show()),
            I::J { k } => format!("J {{k = {}}}", k),
            I::Jal { rd, k } => format!("Jal {{rd = {}, k = {}}}", rd.show(), k),
            I::Jalr { rd, rs1, k } => ri("Jalr", rd, rs1, k),
            I::Beqz { rs1, k } => format!("Beqz {{rs1 = {}, k = {}}}", rs1.show(), k),
            I::Bnez { rs1, k } => format!("Bnez {{rs1 = {}, k = {}}}", rs1.show(), k),
            I::Blt { rs1, rs2, k } => b2("Blt", rs1, rs2, k),
            I::Bge { rs1, rs2, k } => b2("Bge", rs1, rs2, k),
            I::Bltu { rs1, rs2, k } => b2("Bltu", rs1, rs2, k),
            I::Bgeu { rs1, rs2, k } => b2("Bgeu", rs1, rs2, k),
            I::Beq { rs1, rs2, k } => b2("Beq", rs1, rs2, k),
            I::Bne { rs1, rs2, k } => b2("Bne", rs1, rs2, k),
            I::Halt => "Halt".to_string(),
        }
    }
}

/// `mnemonic` followed by a space or a tab.
pub fn cmd_mnemonic(p: &mut P, m: &str) -> R<()> {
    p.or(|p| p.string(&format!("{} ", m)), |p| p.string(&format!("{}\t", m)))
}

type Arg<T> = fn(&mut P) -> R<T>;

fn cmd1<A, T>(p: &mut P, m: &str, a: Arg<A>, f: impl Fn(A) -> T) -> R<T> {
    cmd_mnemonic(p, m)?;
    p.hspace()?;
    Ok(f(a(p)?))
}

fn cmd2<A, B, T>(p: &mut P, m: &str, a: Arg<A>, b: Arg<B>, f: impl Fn(A, B) -> T) -> R<T> {
    cmd_mnemonic(p, m)?;
    p.hspace()?;
    let x = a(p)?;
    p.comma()?;
    let y = b(p)?;
    Ok(f(x, y))
}

fn cmd3<A, B, C, T>(p: &mut P, m: &str, a: Arg<A>, b: Arg<B>, c: Arg<C>, f: impl Fn(A, B, C) -> T) -> R<T> {
    cmd_mnemonic(p, m)?;
    p.hspace()?;
    let x = a(p)?;
    p.comma()?;
    let y = b(p)?;
    p.comma()?;
    let z = c(p)?;
    Ok(f(x, y, z))
}

fn reg(p: &mut P) -> R<Register> {
    register(p)
}

fn imm(p: &mut P) -> R<Ref> {
    p.reference_with_directive()
}

fn cnst(p: &mut P) -> R<Ref> {
    p.constant_ref()
}

fn refr(p: &mut P) -> R<Ref> {
    p.reference()
}

type Parsed = RiscIvIsa<Ref>;

fn command(p: &mut P) -> R<Parsed> {
    p.choice(&[
        &|p| cmd3(p, "addi", reg, reg, imm, |rd, rs1, k| I::Addi { rd, rs1, k }),
        &|p| cmd3(p, "slti", reg, reg, imm, |rd, rs1, k| I::Slti { rd, rs1, k }),
        &|p| cmd3(p, "add", reg, reg, reg, |rd, rs1, rs2| I::Add { rd, rs1, rs2 }),
        &|p| cmd3(p, "sub", reg, reg, reg, |rd, rs1, rs2| I::Sub { rd, rs1, rs2 }),
        &|p| cmd3(p, "mul", reg, reg, reg, |rd, rs1, rs2| I::Mul { rd, rs1, rs2 }),
        &|p| cmd3(p, "mulh", reg, reg, reg, |rd, rs1, rs2| I::Mulh { rd, rs1, rs2 }),
        &|p| cmd3(p, "div", reg, reg, reg, |rd, rs1, rs2| I::Div { rd, rs1, rs2 }),
        &|p| cmd3(p, "rem", reg, reg, reg, |rd, rs1, rs2| I::Rem { rd, rs1, rs2 }),
        &|p| cmd3(p, "slli", reg, reg, cnst, |rd, rs1, k| I::Slli { rd, rs1, k }),
        &|p| cmd3(p, "sll", reg, reg, reg, |rd, rs1, rs2| I::Sll { rd, rs1, rs2 }),
        &|p| cmd3(p, "srli", reg, reg, cnst, |rd, rs1, k| I::Srli { rd, rs1, k }),
        &|p| cmd3(p, "srl", reg, reg, reg, |rd, rs1, rs2| I::Srl { rd, rs1, rs2 }),
        &|p| cmd3(p, "srai", reg, reg, cnst, |rd, rs1, k| I::Srai { rd, rs1, k }),
        &|p| cmd3(p, "sra", reg, reg, reg, |rd, rs1, rs2| I::Sra { rd, rs1, rs2 }),
        &|p| cmd3(p, "andi", reg, reg, imm, |rd, rs1, k| I::Andi { rd, rs1, k }),
        &|p| cmd3(p, "and", reg, reg, reg, |rd, rs1, rs2| I::And { rd, rs1, rs2 }),
        &|p| cmd3(p, "ori", reg, reg, imm, |rd, rs1, k| I::Ori { rd, rs1, k }),
        &|p| cmd3(p, "or", reg, reg, reg, |rd, rs1, rs2| I::Or { rd, rs1, rs2 }),
        &|p| cmd3(p, "xori", reg, reg, imm, |rd, rs1, k| I::Xori { rd, rs1, k }),
        &|p| cmd3(p, "xor", reg, reg, reg, |rd, rs1, rs2| I::Xor { rd, rs1, rs2 }),
        &|p| cmd2(p, "mv", reg, reg, |rd, rs| I::Mv { rd, rs }),
        &|p| cmd2(p, "sw", reg, mem_ref, |rs2, offset_rs1| I::Sw { rs2, offset_rs1 }),
        &|p| cmd2(p, "sb", reg, mem_ref, |rs2, offset_rs1| I::Sb { rs2, offset_rs1 }),
        &|p| cmd2(p, "lui", reg, imm, |rd, k| I::Lui { rd, k }),
        &|p| cmd2(p, "lw", reg, mem_ref, |rd, offset_rs1| I::Lw { rd, offset_rs1 }),
        &|p| cmd2(p, "lb", reg, mem_ref, |rd, offset_rs1| I::Lb { rd, offset_rs1 }),
        &|p| cmd1(p, "j", refr, |k| I::J { k }),
        &|p| cmd2(p, "jal", reg, refr, |rd, k| I::Jal { rd, k }),
        &|p| cmd3(p, "jalr", reg, reg, imm, |rd, rs1, k| I::Jalr { rd, rs1, k }),
        &|p| cmd2(p, "beqz", reg, refr, |rs1, k| I::Beqz { rs1, k }),
        &|p| cmd2(p, "bnez", reg, refr, |rs1, k| I::Bnez { rs1, k }),
        &|p| cmd3(p, "blt", reg, reg, refr, |rs1, rs2, k| I::Blt { rs1, rs2, k }),
        &|p| cmd3(p, "bge", reg, reg, refr, |rs1, rs2, k| I::Bge { rs1, rs2, k }),
        &|p| cmd3(p, "bltu", reg, reg, refr, |rs1, rs2, k| I::Bltu { rs1, rs2, k }),
        &|p| cmd3(p, "bgeu", reg, reg, refr, |rs1, rs2, k| I::Bgeu { rs1, rs2, k }),
        &|p| cmd3(p, "beq", reg, reg, refr, |rs1, rs2, k| I::Beq { rs1, rs2, k }),
        &|p| cmd3(p, "bne", reg, reg, refr, |rs1, rs2, k| I::Bne { rs1, rs2, k }),
        &|p| p.string("halt").map(|_| I::Halt),
    ])
}

pub struct RiscIv;

impl Arch for RiscIv {
    type Parsed = Parsed;
    type State = RiscIvSt;
    const NAME: &'static str = "risc-iv-32";
    const COMMENT: &'static str = ";";

    fn mnemonic(p: &mut P) -> R<Parsed> {
        p.hspace()?;
        let m = command(p)?;
        p.eol_(Self::COMMENT)?;
        Ok(m)
    }

    fn deref(i: &Parsed, offset: W, f: &dyn Fn(&str) -> Option<W>) -> Result<RiscIvIsa<W>, String> {
        let i_type = |m: &'static str| move |x: W| i12(|v| format!("{}: I-type imm {} is not in -2048..2047", m, v), x);
        let shamt = |m: &'static str| move |x: W| u5(|v| format!("{}: shift amount {} is not in 0..31", m, v), x);
        let u_type = |m: &'static str| move |x: W| u20(|v| format!("{}: U-type imm {} is not in 0..1048575", m, v), x);
        let b_type = |m: &'static str| {
            move |x: W| {
                i13(
                    |v| format!("{}: B-type disp {} is not in -4096..4095", m, v),
                    x.wrapping_sub(offset),
                )
            }
        };
        let j_type = |m: &'static str| {
            move |x: W| {
                i21(
                    |v| format!("{}: J-type disp {} is not in -1048576..1048575", m, v),
                    x.wrapping_sub(offset),
                )
            }
        };
        Ok(match i {
            I::J { k } => I::J {
                k: k.resolve(j_type("j"), f)?,
            },
            I::Jal { rd, k } => I::Jal {
                rd: *rd,
                k: k.resolve(j_type("jal"), f)?,
            },
            I::Jalr { rd, rs1, k } => I::Jalr {
                rd: *rd,
                rs1: *rs1,
                k: k.resolve(i_type("jalr"), f)?,
            },
            I::Addi { rd, rs1, k } => I::Addi {
                rd: *rd,
                rs1: *rs1,
                k: k.resolve(i_type("addi"), f)?,
            },
            I::Slti { rd, rs1, k } => I::Slti {
                rd: *rd,
                rs1: *rs1,
                k: k.resolve(i_type("slti"), f)?,
            },
            I::Slli { rd, rs1, k } => I::Slli {
                rd: *rd,
                rs1: *rs1,
                k: k.resolve(shamt("slli"), f)?,
            },
            I::Srli { rd, rs1, k } => I::Srli {
                rd: *rd,
                rs1: *rs1,
                k: k.resolve(shamt("srli"), f)?,
            },
            I::Srai { rd, rs1, k } => I::Srai {
                rd: *rd,
                rs1: *rs1,
                k: k.resolve(shamt("srai"), f)?,
            },
            I::Andi { rd, rs1, k } => I::Andi {
                rd: *rd,
                rs1: *rs1,
                k: k.resolve(i_type("andi"), f)?,
            },
            I::Ori { rd, rs1, k } => I::Ori {
                rd: *rd,
                rs1: *rs1,
                k: k.resolve(i_type("ori"), f)?,
            },
            I::Xori { rd, rs1, k } => I::Xori {
                rd: *rd,
                rs1: *rs1,
                k: k.resolve(i_type("xori"), f)?,
            },
            I::Add { rd, rs1, rs2 } => I::Add {
                rd: *rd,
                rs1: *rs1,
                rs2: *rs2,
            },
            I::Sub { rd, rs1, rs2 } => I::Sub {
                rd: *rd,
                rs1: *rs1,
                rs2: *rs2,
            },
            I::Mul { rd, rs1, rs2 } => I::Mul {
                rd: *rd,
                rs1: *rs1,
                rs2: *rs2,
            },
            I::Mulh { rd, rs1, rs2 } => I::Mulh {
                rd: *rd,
                rs1: *rs1,
                rs2: *rs2,
            },
            I::Div { rd, rs1, rs2 } => I::Div {
                rd: *rd,
                rs1: *rs1,
                rs2: *rs2,
            },
            I::Rem { rd, rs1, rs2 } => I::Rem {
                rd: *rd,
                rs1: *rs1,
                rs2: *rs2,
            },
            I::Sll { rd, rs1, rs2 } => I::Sll {
                rd: *rd,
                rs1: *rs1,
                rs2: *rs2,
            },
            I::Srl { rd, rs1, rs2 } => I::Srl {
                rd: *rd,
                rs1: *rs1,
                rs2: *rs2,
            },
            I::Sra { rd, rs1, rs2 } => I::Sra {
                rd: *rd,
                rs1: *rs1,
                rs2: *rs2,
            },
            I::And { rd, rs1, rs2 } => I::And {
                rd: *rd,
                rs1: *rs1,
                rs2: *rs2,
            },
            I::Or { rd, rs1, rs2 } => I::Or {
                rd: *rd,
                rs1: *rs1,
                rs2: *rs2,
            },
            I::Xor { rd, rs1, rs2 } => I::Xor {
                rd: *rd,
                rs1: *rs1,
                rs2: *rs2,
            },
            I::Mv { rd, rs } => I::Mv { rd: *rd, rs: *rs },
            I::Sw { rs2, offset_rs1 } => I::Sw {
                rs2: *rs2,
                offset_rs1: *offset_rs1,
            },
            I::Sb { rs2, offset_rs1 } => I::Sb {
                rs2: *rs2,
                offset_rs1: *offset_rs1,
            },
            I::Lui { rd, k } => I::Lui {
                rd: *rd,
                k: k.resolve(u_type("lui"), f)?,
            },
            I::Lw { rd, offset_rs1 } => I::Lw {
                rd: *rd,
                offset_rs1: *offset_rs1,
            },
            I::Lb { rd, offset_rs1 } => I::Lb {
                rd: *rd,
                offset_rs1: *offset_rs1,
            },
            I::Beqz { rs1, k } => I::Beqz {
                rs1: *rs1,
                k: k.resolve(b_type("beqz"), f)?,
            },
            I::Bnez { rs1, k } => I::Bnez {
                rs1: *rs1,
                k: k.resolve(b_type("bnez"), f)?,
            },
            I::Blt { rs1, rs2, k } => I::Blt {
                rs1: *rs1,
                rs2: *rs2,
                k: k.resolve(b_type("blt"), f)?,
            },
            I::Bge { rs1, rs2, k } => I::Bge {
                rs1: *rs1,
                rs2: *rs2,
                k: k.resolve(b_type("bge"), f)?,
            },
            I::Bltu { rs1, rs2, k } => I::Bltu {
                rs1: *rs1,
                rs2: *rs2,
                k: k.resolve(b_type("bltu"), f)?,
            },
            I::Bgeu { rs1, rs2, k } => I::Bgeu {
                rs1: *rs1,
                rs2: *rs2,
                k: k.resolve(b_type("bgeu"), f)?,
            },
            I::Beq { rs1, rs2, k } => I::Beq {
                rs1: *rs1,
                rs2: *rs2,
                k: k.resolve(b_type("beq"), f)?,
            },
            I::Bne { rs1, rs2, k } => I::Bne {
                rs1: *rs1,
                rs2: *rs2,
                k: k.resolve(b_type("bne"), f)?,
            },
            I::Halt => I::Halt,
        })
    }
}

#[derive(Clone)]
pub struct RiscIvSt {
    pub pc: i64,
    pub mem: IoMem<RiscIvIsa<W>>,
    pub regs: [W; 32],
    pub stopped: bool,
    pub internal_error: Option<String>,
}

impl RiscIvSt {
    pub fn reg(&self, r: Register) -> W {
        self.regs[r as usize]
    }

    fn set_reg(&mut self, r: Register, v: W) {
        if r != Zero {
            self.regs[r as usize] = v;
        }
    }

    fn next_pc(&mut self) {
        self.pc += 4;
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

    fn get_byte(&mut self, addr: i64) -> u8 {
        self.mem.read_byte(addr).unwrap_or_else(|e| {
            self.mem_error(e);
            0
        })
    }

    fn set_word(&mut self, addr: i64, w: W) {
        if let Err(e) = self.mem.write_word(addr, w) {
            self.mem_error(e);
        }
    }

    fn set_byte(&mut self, addr: i64, b: u8) {
        if let Err(e) = self.mem.write_byte(addr, b) {
            self.mem_error(e);
        }
    }

    fn r_op(&mut self, rd: Register, rs1: Register, rs2: Register, f: impl Fn(W, W) -> W) {
        let v = f(self.reg(rs1), self.reg(rs2));
        self.set_reg(rd, v);
        self.next_pc();
    }

    fn i_op(&mut self, rd: Register, rs1: Register, f: impl Fn(W) -> W) {
        let v = f(self.reg(rs1));
        self.set_reg(rd, v);
        self.next_pc();
    }

    fn branch(&mut self, taken: bool, k: W) {
        if taken {
            self.pc += k as i64;
        } else {
            self.next_pc();
        }
    }
}

fn addr(base: W, offset: W) -> i64 {
    base.wrapping_add(offset) as i64
}

impl Machine for RiscIvSt {
    type Instr = RiscIvIsa<W>;

    fn init(pc: i64, mem: IoMem<Self::Instr>, _randoms: RandomStream) -> Self {
        RiscIvSt {
            pc,
            mem,
            regs: [0; 32],
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

    fn execute(&mut self, _pc: i64, instr: Self::Instr) {
        match instr {
            I::Addi { rd, rs1, k } => self.i_op(rd, rs1, |x| x.wrapping_add(k)),
            I::Slti { rd, rs1, k } => self.i_op(rd, rs1, |x| W::from(x < k)),
            I::Slli { rd, rs1, k } => self.i_op(rd, rs1, |x| shl(x, k as i64)),
            I::Srli { rd, rs1, k } => self.i_op(rd, rs1, |x| lshr(x, k as i64)),
            I::Srai { rd, rs1, k } => self.i_op(rd, rs1, |x| sar(x, k as i64)),
            I::Andi { rd, rs1, k } => self.i_op(rd, rs1, |x| x & k),
            I::Ori { rd, rs1, k } => self.i_op(rd, rs1, |x| x | k),
            I::Xori { rd, rs1, k } => self.i_op(rd, rs1, |x| x ^ k),
            I::Add { rd, rs1, rs2 } => self.r_op(rd, rs1, rs2, W::wrapping_add),
            I::Sub { rd, rs1, rs2 } => self.r_op(rd, rs1, rs2, W::wrapping_sub),
            I::Mul { rd, rs1, rs2 } => self.r_op(rd, rs1, rs2, W::wrapping_mul),
            I::Mulh { rd, rs1, rs2 } => self.r_op(rd, rs1, rs2, |a, b| ((a as i64 * b as i64) >> 32) as W),
            I::Div { rd, rs1, rs2 } => self.r_op(rd, rs1, rs2, |a, b| match b {
                0 => -1,
                -1 if a == W::MIN => a,
                _ => a / b,
            }),
            I::Rem { rd, rs1, rs2 } => self.r_op(rd, rs1, rs2, |a, b| match b {
                0 => a,
                -1 if a == W::MIN => 0,
                _ => a % b,
            }),
            I::Sll { rd, rs1, rs2 } => self.r_op(rd, rs1, rs2, |a, b| shl(a, (b & 0x1F) as i64)),
            I::Srl { rd, rs1, rs2 } => self.r_op(rd, rs1, rs2, |a, b| lshr(a, (b & 0x1F) as i64)),
            I::Sra { rd, rs1, rs2 } => self.r_op(rd, rs1, rs2, |a, b| sar(a, (b & 0x1F) as i64)),
            I::And { rd, rs1, rs2 } => self.r_op(rd, rs1, rs2, |a, b| a & b),
            I::Or { rd, rs1, rs2 } => self.r_op(rd, rs1, rs2, |a, b| a | b),
            I::Xor { rd, rs1, rs2 } => self.r_op(rd, rs1, rs2, |a, b| a ^ b),
            I::Mv { rd, rs } => self.i_op(rd, rs, |x| x),
            I::Sw { rs2, offset_rs1: m } => {
                let v = self.reg(rs2);
                let a = addr(self.reg(m.reg), m.offset);
                self.set_word(a, v);
                self.next_pc();
            }
            I::Sb { rs2, offset_rs1: m } => {
                let v = self.reg(rs2);
                let a = addr(self.reg(m.reg), m.offset);
                self.set_byte(a, v as u8);
                self.next_pc();
            }
            I::Lui { rd, k } => {
                self.set_reg(rd, shl(k, 12));
                self.next_pc();
            }
            I::Lw { rd, offset_rs1: m } => {
                let a = addr(m.offset, self.reg(m.reg));
                let w = self.get_word(a);
                self.set_reg(rd, w);
                self.next_pc();
            }
            I::Lb { rd, offset_rs1: m } => {
                let a = addr(m.offset, self.reg(m.reg));
                let b = self.get_byte(a);
                self.set_reg(rd, b as i8 as W);
                self.next_pc();
            }
            I::J { k } => self.pc += k as i64,
            I::Jal { rd, k } => {
                self.set_reg(rd, (self.pc as W).wrapping_add(4));
                self.pc += k as i64;
            }
            I::Jalr { rd, rs1, k } => {
                let base = self.reg(rs1);
                self.set_reg(rd, (self.pc as W).wrapping_add(4));
                self.pc = addr(base, k);
            }
            I::Beqz { rs1, k } => self.branch(self.reg(rs1) == 0, k),
            I::Bnez { rs1, k } => self.branch(self.reg(rs1) != 0, k),
            I::Blt { rs1, rs2, k } => self.branch(self.reg(rs1) < self.reg(rs2), k),
            I::Bge { rs1, rs2, k } => self.branch(self.reg(rs1) >= self.reg(rs2), k),
            I::Bltu { rs1, rs2, k } => self.branch((self.reg(rs1) as u32) < (self.reg(rs2) as u32), k),
            I::Bgeu { rs1, rs2, k } => self.branch((self.reg(rs1) as u32) >= (self.reg(rs2) as u32), k),
            I::Beq { rs1, rs2, k } => self.branch(self.reg(rs1) == self.reg(rs2), k),
            I::Bne { rs1, rs2, k } => self.branch(self.reg(rs1) != self.reg(rs2), k),
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
        let renames: Vec<(String, &str)> = ALL_REGISTERS
            .iter()
            .filter_map(|r| r.asm_names().first().map(|n| (r.show(), *n)))
            .collect();
        let renames: Vec<(&str, &str)> = renames.iter().map(|(a, b)| (a.as_str(), *b)).collect();
        let views = ALL_REGISTERS
            .iter()
            .flat_map(|&r| {
                let doc = format!("Register `{}`.", r.asm_names().join("`/`"));
                r.asm_names()
                    .into_iter()
                    .map(move |name| value_view::<RiscIvSt>(name, &doc, move |e| e.state.reg(r) as i64))
            })
            .collect();
        with_renames(&renames, views)
    }
}
