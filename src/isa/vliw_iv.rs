//! VLIW-IV: a four-slot VLIW bundle (two ALU, one memory, one control
//! operation) inspired by VLIW architectures and RISC-V.

use crate::isa::risc_iv::cmd_mnemonic;
use crate::machine::{Arch, Machine, HALTED};
use crate::memory::{Instruction, IoMem};
use crate::parser::{Ref, P, R};
use crate::report::value_view;
use crate::report::view::{with_renames, Stat, View};
use crate::rng::RandomStream;
use crate::translator::ParsedInstr;
use crate::word::{fit_signed, i10, i15, i17, i20, lshr, sar, shl, u20, W};

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
pub const REGISTER_NAMES: [(&str, Register); 32] = [
    ("zero", Zero),
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
    ("ra", Ra),
    ("s4", S4),
    ("s5", S5),
    ("s6", S6),
    ("s7", S7),
    ("s8", S8),
    ("s9", S9),
    ("s10", S10),
    ("s11", S11),
    ("t3", T3),
    ("t4", T4),
    ("sp", Sp),
    ("t5", T5),
    ("t6", T6),
    ("gp", Gp),
    ("tp", Tp),
    ("t0", T0),
    ("t1", T1),
    ("t2", T2),
    ("s0fp", S0Fp),
    ("s1", S1),
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

/// Width of the signed offset field of the memory slot.
const MEM_REF_OFFSET_BITS: u32 = 11;

/// Check the offset against the field it is encoded into while parsing, so the
/// caret lands under the operand.
fn require_signed_field(p: &mut P, field_bits: u32, operand_pos: usize, value: W) -> R<W> {
    if value == fit_signed(field_bits, value) {
        Ok(value)
    } else {
        let hi = (1i64 << (field_bits - 1)) - 1;
        let lo = -(1i64 << (field_bits - 1));
        p.fail_at(
            operand_pos,
            format!(
                "offset {} doesn't fit the {}-bit signed field of a 14 byte bundle, expected {}..{}",
                value, field_bits, lo, hi
            ),
        )
    }
}

fn mem_ref(p: &mut P) -> R<MemRef> {
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

// * Slot operations

#[derive(Clone, Copy, Debug)]
pub enum MemoryOp {
    Lw { rd: Register, offset_rs1: MemRef },
    Lb { rd: Register, offset_rs1: MemRef },
    Sw { rs2: Register, offset_rs1: MemRef },
    Sb { rs2: Register, offset_rs1: MemRef },
    NopM,
}

/// The three-register ALU operations; each is its own constructor in the original.
#[derive(Clone, Copy, Debug)]
pub enum RegOp {
    Add,
    Sub,
    Mul,
    Mulh,
    Div,
    Rem,
    Sll,
    Srl,
    Sra,
    And,
    Or,
    Xor,
}

#[derive(Clone, Debug)]
pub enum AluOp<L> {
    Addi {
        rd: Register,
        rs1: Register,
        k: L,
    },
    Reg {
        op: RegOp,
        rd: Register,
        rs1: Register,
        rs2: Register,
    },
    Slti {
        rd: Register,
        rs1: Register,
        k: L,
    },
    Lui {
        rd: Register,
        k: L,
    },
    Mv {
        rd: Register,
        rs: Register,
    },
    NopA,
}

/// The two-register branches; each is its own constructor in the original.
#[derive(Clone, Copy, Debug)]
pub enum Cmp {
    Blt,
    Bge,
    Bltu,
    Bgeu,
    Beq,
    Bne,
}

#[derive(Clone, Debug)]
pub enum ControlOp<L> {
    J {
        k: L,
    },
    Jal {
        rd: Register,
        k: L,
    },
    Jalr {
        rd: Register,
        rs1: Register,
        k: L,
    },
    Beqz {
        rs1: Register,
        k: L,
    },
    Bnez {
        rs1: Register,
        k: L,
    },
    Branch {
        cmp: Cmp,
        rs1: Register,
        rs2: Register,
        k: L,
    },
    Halt,
    NopC,
}

#[derive(Clone, Debug)]
pub struct VliwIvIsa<L> {
    pub mem_op: MemoryOp,
    pub alu1_op: AluOp<L>,
    pub alu2_op: AluOp<L>,
    pub ctrl_op: ControlOp<L>,
}

/// A record as derived `Show` writes it; field names are the constructor
/// name, lowercased, followed by the field suffix (`addiRd`, `lwOffsetRs1`).
fn record(ctor: &str, fields: &[(&str, String)]) -> String {
    let prefix = ctor.to_lowercase();
    let fs: Vec<String> = fields.iter().map(|(f, v)| format!("{}{} = {}", prefix, f, v)).collect();
    format!("{} {{{}}}", ctor, fs.join(", "))
}

impl MemoryOp {
    fn show(&self) -> String {
        match self {
            MemoryOp::Lw { rd, offset_rs1 } => record("Lw", &[("Rd", rd.show()), ("OffsetRs1", offset_rs1.show())]),
            MemoryOp::Lb { rd, offset_rs1 } => record("Lb", &[("Rd", rd.show()), ("OffsetRs1", offset_rs1.show())]),
            MemoryOp::Sw { rs2, offset_rs1 } => record("Sw", &[("Rs2", rs2.show()), ("OffsetRs1", offset_rs1.show())]),
            MemoryOp::Sb { rs2, offset_rs1 } => record("Sb", &[("Rs2", rs2.show()), ("OffsetRs1", offset_rs1.show())]),
            MemoryOp::NopM => "NopM".to_string(),
        }
    }

    fn is_active(&self) -> bool {
        !matches!(self, MemoryOp::NopM)
    }
}

impl AluOp<W> {
    fn show(&self) -> String {
        match self {
            AluOp::Addi { rd, rs1, k } => {
                record("Addi", &[("Rd", rd.show()), ("Rs1", rs1.show()), ("K", k.to_string())])
            }
            AluOp::Reg { op, rd, rs1, rs2 } => record(
                &format!("{:?}", op),
                &[("Rd", rd.show()), ("Rs1", rs1.show()), ("Rs2", rs2.show())],
            ),
            AluOp::Slti { rd, rs1, k } => {
                record("Slti", &[("Rd", rd.show()), ("Rs1", rs1.show()), ("K", k.to_string())])
            }
            AluOp::Lui { rd, k } => record("Lui", &[("Rd", rd.show()), ("K", k.to_string())]),
            AluOp::Mv { rd, rs } => record("Mv", &[("Rd", rd.show()), ("Rs", rs.show())]),
            AluOp::NopA => "NopA".to_string(),
        }
    }
}

impl<L> AluOp<L> {
    fn is_active(&self) -> bool {
        !matches!(self, AluOp::NopA)
    }
}

impl ControlOp<W> {
    fn show(&self) -> String {
        match self {
            ControlOp::J { k } => record("J", &[("K", k.to_string())]),
            ControlOp::Jal { rd, k } => record("Jal", &[("Rd", rd.show()), ("K", k.to_string())]),
            ControlOp::Jalr { rd, rs1, k } => {
                record("Jalr", &[("Rd", rd.show()), ("Rs1", rs1.show()), ("K", k.to_string())])
            }
            ControlOp::Beqz { rs1, k } => record("Beqz", &[("Rs1", rs1.show()), ("K", k.to_string())]),
            ControlOp::Bnez { rs1, k } => record("Bnez", &[("Rs1", rs1.show()), ("K", k.to_string())]),
            ControlOp::Branch { cmp, rs1, rs2, k } => record(
                &format!("{:?}", cmp),
                &[("Rs1", rs1.show()), ("Rs2", rs2.show()), ("K", k.to_string())],
            ),
            ControlOp::Halt => "Halt".to_string(),
            ControlOp::NopC => "NopC".to_string(),
        }
    }
}

impl<L> ControlOp<L> {
    fn is_active(&self) -> bool {
        !matches!(self, ControlOp::NopC)
    }
}

impl<L> VliwIvIsa<L> {
    fn active_count(&self) -> usize {
        [
            self.mem_op.is_active(),
            self.alu1_op.is_active(),
            self.alu2_op.is_active(),
            self.ctrl_op.is_active(),
        ]
        .iter()
        .filter(|&&a| a)
        .count()
    }
}

/// Bundle size in bytes.
const BUNDLE_SIZE: usize = 14;

impl ParsedInstr for VliwIvIsa<Ref> {
    fn byte_size(&self) -> usize {
        BUNDLE_SIZE
    }
}

impl Instruction for VliwIvIsa<W> {
    fn byte_size(&self) -> usize {
        BUNDLE_SIZE
    }

    fn show(&self) -> String {
        format!(
            "VliwIvIsa {{memOp = {}, alu1Op = {}, alu2Op = {}, ctrlOp = {}}}",
            self.mem_op.show(),
            self.alu1_op.show(),
            self.alu2_op.show(),
            self.ctrl_op.show()
        )
    }
}

// * Load-level accumulator

/// Per-run histogram of how many slots were active in each executed bundle:
/// index is the active-slot count (0..4), value the number of bundles.
pub type VliwLoadAcc = [i64; 5];

fn load_totals(m: &VliwLoadAcc) -> (i64, i64) {
    let total = m.iter().sum();
    let active = m.iter().enumerate().map(|(k, n)| k as i64 * n).sum();
    (total, active)
}

fn vliw_load_percent(m: &VliwLoadAcc) -> i64 {
    let (total, active) = load_totals(m);
    if total == 0 {
        0
    } else {
        (active * 100) / (total * 4)
    }
}

/// Average number of active slots per executed bundle, with two decimal places.
fn vliw_avg_load(m: &VliwLoadAcc) -> String {
    let (total, active) = load_totals(m);
    if total == 0 {
        return "0.00".to_string();
    }
    let scaled = (active * 100) / total;
    format!("{}.{:02}", scaled / 100, scaled % 100)
}

/// `K:N (P%)` per non-empty bucket, or `-` for an empty histogram.
fn render_bundles_by_load(m: &VliwLoadAcc) -> String {
    let total: i64 = m.iter().sum();
    let parts: Vec<String> = m
        .iter()
        .enumerate()
        .filter(|(_, &n)| n > 0)
        .map(|(k, &n)| format!("{}:{} ({}%)", k, n, n * 100 / total))
        .collect();
    if parts.is_empty() {
        "-".to_string()
    } else {
        parts.join(", ")
    }
}

// * Parser

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

fn refr(p: &mut P) -> R<Ref> {
    p.reference()
}

fn reg_op(p: &mut P, m: &str, op: RegOp) -> R<AluOp<Ref>> {
    cmd3(p, m, reg, reg, reg, |rd, rs1, rs2| AluOp::Reg { op, rd, rs1, rs2 })
}

fn branch(p: &mut P, m: &str, cmp: Cmp) -> R<ControlOp<Ref>> {
    cmd3(p, m, reg, reg, refr, |rs1, rs2, k| ControlOp::Branch {
        cmp,
        rs1,
        rs2,
        k,
    })
}

fn parse_mem_op(p: &mut P) -> R<MemoryOp> {
    use MemoryOp as M;
    p.choice(&[
        &|p| cmd2(p, "lw", reg, mem_ref, |rd, offset_rs1| M::Lw { rd, offset_rs1 }),
        &|p| cmd2(p, "lb", reg, mem_ref, |rd, offset_rs1| M::Lb { rd, offset_rs1 }),
        &|p| cmd2(p, "sw", reg, mem_ref, |rs2, offset_rs1| M::Sw { rs2, offset_rs1 }),
        &|p| cmd2(p, "sb", reg, mem_ref, |rs2, offset_rs1| M::Sb { rs2, offset_rs1 }),
        &|p| p.string("nop").map(|_| M::NopM),
        &|_| Ok(M::NopM),
    ])
}

fn parse_alu_op(p: &mut P) -> R<AluOp<Ref>> {
    use AluOp as A;
    p.choice(&[
        &|p| cmd3(p, "addi", reg, reg, imm, |rd, rs1, k| A::Addi { rd, rs1, k }),
        &|p| reg_op(p, "add", RegOp::Add),
        &|p| reg_op(p, "sub", RegOp::Sub),
        &|p| reg_op(p, "mul", RegOp::Mul),
        &|p| reg_op(p, "mulh", RegOp::Mulh),
        &|p| reg_op(p, "div", RegOp::Div),
        &|p| reg_op(p, "rem", RegOp::Rem),
        &|p| reg_op(p, "sll", RegOp::Sll),
        &|p| reg_op(p, "srl", RegOp::Srl),
        &|p| reg_op(p, "sra", RegOp::Sra),
        &|p| reg_op(p, "and", RegOp::And),
        &|p| reg_op(p, "or", RegOp::Or),
        &|p| reg_op(p, "xor", RegOp::Xor),
        &|p| cmd3(p, "slti", reg, reg, imm, |rd, rs1, k| A::Slti { rd, rs1, k }),
        &|p| cmd2(p, "lui", reg, imm, |rd, k| A::Lui { rd, k }),
        &|p| cmd2(p, "mv", reg, reg, |rd, rs| A::Mv { rd, rs }),
        &|p| p.string("nop").map(|_| A::NopA),
        &|_| Ok(A::NopA),
    ])
}

fn parse_ctrl_op(p: &mut P) -> R<ControlOp<Ref>> {
    use ControlOp as C;
    p.choice(&[
        &|p| cmd1(p, "j", refr, |k| C::J { k }),
        &|p| cmd2(p, "jal", reg, refr, |rd, k| C::Jal { rd, k }),
        &|p| cmd3(p, "jalr", reg, reg, imm, |rd, rs1, k| C::Jalr { rd, rs1, k }),
        &|p| cmd2(p, "beqz", reg, refr, |rs1, k| C::Beqz { rs1, k }),
        &|p| cmd2(p, "bnez", reg, refr, |rs1, k| C::Bnez { rs1, k }),
        &|p| branch(p, "blt", Cmp::Blt),
        &|p| branch(p, "bge", Cmp::Bge),
        &|p| branch(p, "bltu", Cmp::Bltu),
        &|p| branch(p, "bgeu", Cmp::Bgeu),
        &|p| branch(p, "beq", Cmp::Beq),
        &|p| branch(p, "bne", Cmp::Bne),
        &|p| p.string("halt").map(|_| C::Halt),
        &|p| p.string("nop").map(|_| C::NopC),
        &|_| Ok(C::NopC),
    ])
}

fn slot_separator(p: &mut P) -> R<()> {
    p.hspace()?;
    p.string("/")?;
    p.hspace()
}

type Parsed = VliwIvIsa<Ref>;

pub struct VliwIv;

impl Arch for VliwIv {
    type Parsed = Parsed;
    type State = VliwIvSt;
    const NAME: &'static str = "vliw-iv";
    const COMMENT: &'static str = ";";

    fn mnemonic(p: &mut P) -> R<Parsed> {
        p.hspace()?;
        let alu1_op = parse_alu_op(p)?;
        slot_separator(p)?;
        let alu2_op = parse_alu_op(p)?;
        slot_separator(p)?;
        let mem_op = parse_mem_op(p)?;
        slot_separator(p)?;
        let ctrl_op = parse_ctrl_op(p)?;
        p.eol_(Self::COMMENT)?;
        Ok(VliwIvIsa {
            mem_op,
            alu1_op,
            alu2_op,
            ctrl_op,
        })
    }

    fn deref(i: &Parsed, offset: W, f: &dyn Fn(&str) -> Option<W>) -> Result<VliwIvIsa<W>, String> {
        let alu_imm = |m: &'static str| move |x: W| i17(|v| format!("{}: ALU imm {} is not in -65536..65535", m, v), x);
        let lui_imm = |m: &'static str| move |x: W| u20(|v| format!("{}: U-type imm {} is not in 0..1048575", m, v), x);
        let ctrl_imm = |m: &'static str| move |x: W| i10(|v| format!("{}: CTRL imm {} is not in -512..511", m, v), x);
        let branch_disp = |m: &'static str| {
            move |x: W| {
                i10(
                    |v| format!("{}: CTRL disp {} is not in -512..511", m, v),
                    x.wrapping_sub(offset),
                )
            }
        };
        let link_disp = |m: &'static str| {
            move |x: W| {
                i15(
                    |v| format!("{}: CTRL disp {} is not in -16384..16383", m, v),
                    x.wrapping_sub(offset),
                )
            }
        };
        let jump_disp = |m: &'static str| {
            move |x: W| {
                i20(
                    |v| format!("{}: CTRL disp {} is not in -524288..524287", m, v),
                    x.wrapping_sub(offset),
                )
            }
        };
        let alu = |a: &AluOp<Ref>| -> Result<AluOp<W>, String> {
            Ok(match a {
                AluOp::Addi { rd, rs1, k } => AluOp::Addi {
                    rd: *rd,
                    rs1: *rs1,
                    k: k.resolve(alu_imm("addi"), f)?,
                },
                AluOp::Slti { rd, rs1, k } => AluOp::Slti {
                    rd: *rd,
                    rs1: *rs1,
                    k: k.resolve(alu_imm("slti"), f)?,
                },
                AluOp::Lui { rd, k } => AluOp::Lui {
                    rd: *rd,
                    k: k.resolve(lui_imm("lui"), f)?,
                },
                AluOp::Reg { op, rd, rs1, rs2 } => AluOp::Reg {
                    op: *op,
                    rd: *rd,
                    rs1: *rs1,
                    rs2: *rs2,
                },
                AluOp::Mv { rd, rs } => AluOp::Mv { rd: *rd, rs: *rs },
                AluOp::NopA => AluOp::NopA,
            })
        };
        let cmp_name = |c: Cmp| match c {
            Cmp::Blt => "blt",
            Cmp::Bge => "bge",
            Cmp::Bltu => "bltu",
            Cmp::Bgeu => "bgeu",
            Cmp::Beq => "beq",
            Cmp::Bne => "bne",
        };
        // Fields in the order `show` forces them in the original: memory, ALU 1, ALU 2, control.
        let alu1_op = alu(&i.alu1_op)?;
        let alu2_op = alu(&i.alu2_op)?;
        let ctrl_op = match &i.ctrl_op {
            ControlOp::J { k } => ControlOp::J {
                k: k.resolve(jump_disp("j"), f)?,
            },
            ControlOp::Jal { rd, k } => ControlOp::Jal {
                rd: *rd,
                k: k.resolve(link_disp("jal"), f)?,
            },
            ControlOp::Jalr { rd, rs1, k } => ControlOp::Jalr {
                rd: *rd,
                rs1: *rs1,
                k: k.resolve(ctrl_imm("jalr"), f)?,
            },
            ControlOp::Beqz { rs1, k } => ControlOp::Beqz {
                rs1: *rs1,
                k: k.resolve(link_disp("beqz"), f)?,
            },
            ControlOp::Bnez { rs1, k } => ControlOp::Bnez {
                rs1: *rs1,
                k: k.resolve(link_disp("bnez"), f)?,
            },
            ControlOp::Branch { cmp, rs1, rs2, k } => ControlOp::Branch {
                cmp: *cmp,
                rs1: *rs1,
                rs2: *rs2,
                k: k.resolve(branch_disp(cmp_name(*cmp)), f)?,
            },
            ControlOp::Halt => ControlOp::Halt,
            ControlOp::NopC => ControlOp::NopC,
        };
        Ok(VliwIvIsa {
            mem_op: i.mem_op,
            alu1_op,
            alu2_op,
            ctrl_op,
        })
    }
}

// * Machine

#[derive(Clone)]
pub struct VliwIvSt {
    pub pc: i64,
    pub mem: IoMem<VliwIvIsa<W>>,
    pub regs: [W; 32],
    pub stopped: bool,
    pub internal_error: Option<String>,
    pub randoms: RandomStream,
    pub vliw_load: VliwLoadAcc,
}

/// A register write computed in phase 1 of a bundle, applied in phase 2.
type Write = Option<(Register, W)>;

/// Pick items by index out of what is left, as the original does; an index
/// past the end keeps the rest in order.
fn shuffle<T>(mut xs: Vec<T>, idxs: &[usize]) -> Vec<T> {
    let mut out = Vec::with_capacity(xs.len());
    for &i in idxs {
        if xs.len() <= 1 || i >= xs.len() {
            break;
        }
        out.push(xs.remove(i));
    }
    out.extend(xs);
    out
}

fn addr(base: W, offset: W) -> i64 {
    base.wrapping_add(offset) as i64
}

impl VliwIvSt {
    pub fn reg(&self, r: Register) -> W {
        self.regs[r as usize]
    }

    fn set_reg(&mut self, r: Register, v: W) {
        if r != Zero {
            self.regs[r as usize] = v;
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

    /// Loads read now and write later; stores write memory right away.
    fn compute_mem(&mut self, op: MemoryOp) -> Write {
        match op {
            MemoryOp::NopM => None,
            MemoryOp::Lw { rd, offset_rs1: m } => {
                let a = addr(m.offset, self.reg(m.reg));
                Some((rd, self.get_word(a)))
            }
            MemoryOp::Lb { rd, offset_rs1: m } => {
                let a = addr(m.offset, self.reg(m.reg));
                Some((rd, self.get_byte(a) as i8 as W))
            }
            MemoryOp::Sw { rs2, offset_rs1: m } => {
                let v = self.reg(rs2);
                self.set_word(addr(self.reg(m.reg), m.offset), v);
                None
            }
            MemoryOp::Sb { rs2, offset_rs1: m } => {
                let v = self.reg(rs2);
                self.set_byte(addr(self.reg(m.reg), m.offset), v as u8);
                None
            }
        }
    }

    fn compute_alu(&self, op: &AluOp<W>) -> Write {
        let r = |x| self.reg(x);
        match *op {
            AluOp::NopA => None,
            AluOp::Addi { rd, rs1, k } => Some((rd, r(rs1).wrapping_add(k))),
            AluOp::Slti { rd, rs1, k } => Some((rd, W::from(r(rs1) < k))),
            AluOp::Lui { rd, k } => Some((rd, shl(k, 12))),
            AluOp::Mv { rd, rs } => Some((rd, r(rs))),
            AluOp::Reg { op, rd, rs1, rs2 } => {
                let (a, b) = (r(rs1), r(rs2));
                let v = match op {
                    RegOp::Add => a.wrapping_add(b),
                    RegOp::Sub => a.wrapping_sub(b),
                    RegOp::Mul => a.wrapping_mul(b),
                    RegOp::Mulh => ((a as i64 * b as i64) >> 32) as W,
                    RegOp::Div => match b {
                        0 => -1,
                        -1 if a == W::MIN => a,
                        _ => a / b,
                    },
                    RegOp::Rem => match b {
                        0 => a,
                        -1 if a == W::MIN => 0,
                        _ => a % b,
                    },
                    RegOp::Sll => shl(a, (b & 0x1F) as i64),
                    RegOp::Srl => lshr(a, (b & 0x1F) as i64),
                    RegOp::Sra => sar(a, (b & 0x1F) as i64),
                    RegOp::And => a & b,
                    RegOp::Or => a | b,
                    RegOp::Xor => a ^ b,
                };
                Some((rd, v))
            }
        }
    }

    /// Run the control slot; true if it set the pc.
    fn exec_ctrl(&mut self, op: ControlOp<W>) -> bool {
        let pc = self.pc;
        let link = (pc as W).wrapping_add(BUNDLE_SIZE as W);
        let taken = match op {
            ControlOp::NopC => return false,
            ControlOp::Halt => {
                self.stopped = true;
                return true;
            }
            ControlOp::J { k } => Some(k),
            ControlOp::Jal { rd, k } => {
                self.set_reg(rd, link);
                Some(k)
            }
            ControlOp::Jalr { rd, rs1, k } => {
                let base = self.reg(rs1);
                self.set_reg(rd, link);
                self.pc = addr(base, k);
                return true;
            }
            ControlOp::Beqz { rs1, k } => (self.reg(rs1) == 0).then_some(k),
            ControlOp::Bnez { rs1, k } => (self.reg(rs1) != 0).then_some(k),
            ControlOp::Branch { cmp, rs1, rs2, k } => {
                let (a, b) = (self.reg(rs1), self.reg(rs2));
                let cond = match cmp {
                    Cmp::Blt => a < b,
                    Cmp::Bge => a >= b,
                    Cmp::Bltu => (a as u32) < (b as u32),
                    Cmp::Bgeu => (a as u32) >= (b as u32),
                    Cmp::Beq => a == b,
                    Cmp::Bne => a != b,
                };
                cond.then_some(k)
            }
        };
        match taken {
            Some(k) => {
                self.pc = pc + k as i64;
                true
            }
            None => false,
        }
    }
}

impl Machine for VliwIvSt {
    type Instr = VliwIvIsa<W>;

    fn init(pc: i64, mem: IoMem<Self::Instr>, randoms: RandomStream) -> Self {
        VliwIvSt {
            pc,
            mem,
            regs: [0; 32],
            stopped: false,
            internal_error: None,
            randoms,
            vliw_load: [0; 5],
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

    fn execute(&mut self, _pc: i64, bundle: Self::Instr) {
        self.vliw_load[bundle.active_count()] += 1;
        // Phase 1: read all sources and compute the results.
        let mem_result = self.compute_mem(bundle.mem_op);
        let alu1_result = self.compute_alu(&bundle.alu1_op);
        let alu2_result = self.compute_alu(&bundle.alu2_op);
        // Phase 2: apply the register writes in random order.
        let results = vec![mem_result, alu1_result, alu2_result];
        let n = results.len();
        let idxs: Vec<usize> = self
            .randoms
            .take(n)
            .into_iter()
            .map(|x| (x % n as i64) as usize)
            .collect();
        for (r, v) in shuffle(results, &idxs).into_iter().flatten() {
            self.set_reg(r, v);
        }
        // Phase 3: the control slot, last, sees the writes.
        if !self.exec_ctrl(bundle.ctrl_op) {
            self.pc += BUNDLE_SIZE as i64;
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
                    .map(move |name| value_view::<VliwIvSt>(name, &doc, move |e| e.state.reg(r) as i64))
            })
            .collect();
        with_renames(&renames, views)
    }

    fn stats() -> Vec<Stat<Self>> {
        vec![
            Stat::<VliwIvSt>::new(
                "vliw:load-percent",
                "Average slot use over the run, in percent of the four slots.",
                |e| format!("{}%", vliw_load_percent(&e.final_state.vliw_load)),
            ),
            Stat::<VliwIvSt>::new(
                "vliw:avg-load",
                "Average active slots per executed bundle, `0.00`-`4.00`.",
                |e| vliw_avg_load(&e.final_state.vliw_load),
            ),
            Stat::<VliwIvSt>::new(
                "vliw:bundles-by-load",
                "Executed bundles by active slot count: `K:N (P%)`.",
                |e| render_bundles_by_load(&e.final_state.vliw_load),
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{Cell, Mem};
    use crate::parser::error_pretty;
    use crate::rng::StdGen;

    fn st0() -> VliwIvSt {
        let cells = Mem {
            size: 1000,
            data: (0..1000).map(|_| Cell::Value(0)).collect(),
        };
        VliwIvSt::init(
            0,
            IoMem::new(Default::default(), cells),
            RandomStream::new(StdGen::new(0)),
        )
    }

    fn with_regs(pairs: &[(Register, W)]) -> VliwIvSt {
        let mut st = st0();
        for &(r, v) in pairs {
            st.regs[r as usize] = v;
        }
        st
    }

    fn parse(code: &str) -> Result<Parsed, String> {
        let src: Vec<char> = format!("{}\n", code).chars().collect();
        let mut p = P::new("-", &src);
        VliwIv::mnemonic(&mut p).map_err(|e| error_pretty("-", &src, &e))
    }

    fn translate(code: &str) -> bool {
        parse(code).is_ok()
    }

    fn simulate(code: &str, mut st: VliwIvSt) -> VliwIvSt {
        let instr = VliwIv::deref(&parse(code).unwrap(), 0, &|_| None).unwrap();
        st.execute(0, instr);
        st
    }

    fn reg_after(code: &str, regs: &[(Register, W)], r: Register) -> W {
        simulate(code, with_regs(regs)).reg(r)
    }

    fn pc_after(code: &str, regs: &[(Register, W)]) -> i64 {
        simulate(code, with_regs(regs)).pc
    }

    /// Parse one bundle and resolve its operands, which is where the field guards live.
    fn field_outcome(code: &str) -> Result<VliwIvIsa<W>, String> {
        VliwIv::deref(&parse(code)?, 0, &|_| Some(0))
    }

    fn accepted(code: &str) {
        if let Err(e) = field_outcome(code) {
            panic!("{} should be accepted, got: {}", code, e);
        }
    }

    fn rejected(code: &str, range: &str) {
        match field_outcome(code) {
            Ok(m) => panic!("{} should be rejected, got: {}", code, m.show()),
            Err(e) => assert!(e.contains(range), "expected range {} in: {}", range, e),
        }
    }

    #[test]
    fn alu_operations() {
        assert_eq!(reg_after("addi a1, a0, 3 / nop / nop / nop", &[(A0, 5)], A1), 8);
        let st = simulate(
            "add a2, a0, a1 / sub a3, a0, a1 / nop / nop",
            with_regs(&[(A0, 10), (A1, 3)]),
        );
        assert_eq!((st.reg(A2), st.reg(A3)), (13, 7));
        let st = simulate(
            "and a2, a0, a1 / or a3, a0, a1 / nop / nop",
            with_regs(&[(A0, 0xFF00), (A1, 0x0FF0)]),
        );
        assert_eq!((st.reg(A2), st.reg(A3)), (0x0F00, 0xFFF0));
        assert_eq!(
            reg_after("xor a2, a0, a1 / nop / nop / nop", &[(A0, 0xFF00), (A1, 0x0FF0)], A2),
            0xF0F0
        );
        let st = simulate(
            "mul a2, a0, a1 / div a3, a0, a1 / nop / nop",
            with_regs(&[(A0, 6), (A1, 3)]),
        );
        assert_eq!((st.reg(A2), st.reg(A3)), (18, 2));
        assert_eq!(
            reg_after("rem a2, a0, a1 / nop / nop / nop", &[(A0, 17), (A1, 5)], A2),
            2
        );
        assert_eq!(
            reg_after("mulh a2, a0, a1 / nop / nop / nop", &[(A0, 0x10000000), (A1, 0x10)], A2),
            1
        );
        let st = simulate("slti a1, a0, 10 / slti a2, a0, 3 / nop / nop", with_regs(&[(A0, 5)]));
        assert_eq!((st.reg(A1), st.reg(A2)), (1, 0));
        assert_eq!(reg_after("lui a0, 0xABCDE / nop / nop / nop", &[], A0), -1412571136);
        assert_eq!(reg_after("mv a1, a0 / nop / nop / nop", &[(A0, 42)], A1), 42);
    }

    #[test]
    fn shifts_mask_the_amount_to_5_bits() {
        assert_eq!(
            reg_after("sll a2, a0, a1 / nop / nop / nop", &[(A0, 3), (A1, 2)], A2),
            12
        );
        assert_eq!(
            reg_after("sll a2, a0, a1 / nop / nop / nop", &[(A0, 1), (A1, 33)], A2),
            2
        );
        assert_eq!(
            reg_after("sll a2, a0, a1 / nop / nop / nop", &[(A0, 1), (A1, -1)], A2),
            W::MIN
        );
        assert_eq!(
            reg_after("srl a2, a0, a1 / nop / nop / nop", &[(A0, 16), (A1, 33)], A2),
            8
        );
        assert_eq!(
            reg_after("sra a2, a0, a1 / nop / nop / nop", &[(A0, -16), (A1, -1)], A2),
            -1
        );
        assert_eq!(
            reg_after("srl a2, a0, a1 / nop / nop / nop", &[(A0, -16), (A1, 2)], A2),
            1073741820
        );
        assert_eq!(
            reg_after("sra a2, a0, a1 / nop / nop / nop", &[(A0, -16), (A1, 2)], A2),
            -4
        );
    }

    #[test]
    fn div_and_rem_corner_cases() {
        let both = |a, b| {
            let st = simulate(
                "div a2, a0, a1 / rem a3, a0, a1 / nop / nop",
                with_regs(&[(A0, a), (A1, b)]),
            );
            (st.reg(A2), st.reg(A3))
        };
        assert_eq!(both(-7, 2), (-3, -1));
        assert_eq!(both(42, 0), (-1, 42));
        assert_eq!(both(W::MIN, -1), (W::MIN, 0));
        assert_eq!(
            reg_after("div a2, a0, a1 / nop / nop / nop", &[(A0, 6), (A1, -1)], A2),
            -6
        );
    }

    #[test]
    fn memory_operations() {
        let mut st = st0();
        for (a, b) in [(20, 0x12), (21, 0x34), (22, 0x56), (23, 0x78)] {
            st.mem.write_byte(a, b).unwrap();
        }
        assert_eq!(simulate("nop / nop / lw a1, 20(a0) / nop", st).reg(A1), 0x78563412);

        let mut st = simulate("nop / nop / sw a1, 20(a0) / nop", with_regs(&[(A1, -559038737)]));
        assert_eq!(st.mem.read_word(20).unwrap(), -559038737);
        let mut st = simulate("nop / nop / sb a1, 20(a0) / nop", with_regs(&[(A1, 0x12345678)]));
        assert_eq!(st.mem.read_byte(20).unwrap(), 0x78);

        let lb = |at: i64, b: u8, code: &str| {
            let mut st = st0();
            st.mem.write_byte(at, b).unwrap();
            simulate(code, st).reg(A1)
        };
        assert_eq!(lb(20, 0x41, "nop / nop / lb a1, 20(a0) / nop"), 0x41);
        assert_eq!(lb(20, 0x80, "nop / nop / lb a1, 20(a0) / nop"), -128);
        assert_eq!(lb(25, 0xFF, "nop / nop / lb a1, 25(a0) / nop"), -1);
    }

    #[test]
    fn control_flow() {
        assert_eq!(pc_after("nop / nop / nop / j 100", &[]), 100);
        let mut at50 = st0();
        at50.pc = 50;
        let st = simulate("nop / nop / nop / jal ra, 100", at50.clone());
        assert_eq!((st.pc, st.reg(Ra)), (150, 64));
        at50.regs[A0 as usize] = 200;
        let st = simulate("nop / nop / nop / jalr ra, a0, 4", at50);
        assert_eq!((st.pc, st.reg(Ra)), (204, 64));
        assert_eq!(pc_after("nop / nop / nop / beqz a0, 50", &[(A0, 0)]), 50);
        assert_eq!(pc_after("nop / nop / nop / beqz a0, 50", &[(A0, 5)]), 14);
        assert_eq!(pc_after("nop / nop / nop / bnez a0, 50", &[(A0, 5)]), 50);
        assert_eq!(pc_after("nop / nop / nop / blt a0, a1, 50", &[(A0, 5), (A1, 10)]), 50);
        assert_eq!(pc_after("nop / nop / nop / blt a0, a1, 50", &[(A0, 10), (A1, 5)]), 14);
        assert_eq!(pc_after("nop / nop / nop / bge a0, a1, 50", &[(A0, 10), (A1, 10)]), 50);
        assert_eq!(pc_after("nop / nop / nop / bltu a0, a1, 50", &[(A0, 5), (A1, -1)]), 50);
        assert_eq!(pc_after("nop / nop / nop / bgeu a0, a1, 50", &[(A0, -1), (A1, 5)]), 50);
        assert!(simulate("nop / nop / nop / halt", st0()).stopped);
    }

    #[test]
    fn translator_and_blank_slots() {
        assert!(translate("addi a1, a0, 3 / nop / nop / nop"));
        assert!(translate("nop / nop / lw a1, 20(a0) / j 100"));
        assert!(translate("add a2, a0, a1 / sub a3, a0, a1 / sw a1, 0(a0) / halt"));
        assert!(translate("addi a1, a0, 3 /  /  /  "));
        assert!(translate(" /  / lw a1, 20(a0) / j 100"));
        let blank = simulate("addi a1, a0, 3 /  /  /  ", with_regs(&[(A0, 5)]));
        let nop = simulate("addi a1, a0, 3 / nop / nop / nop", with_regs(&[(A0, 5)]));
        assert_eq!(blank.regs, nop.regs);
    }

    #[test]
    fn operand_fields() {
        accepted("addi a0, zero, 65535 / nop / nop / nop");
        rejected("addi a0, zero, 65536 / nop / nop / nop", "-65536..65535");
        rejected("addi a0, zero, 0x1FFFF / nop / nop / nop", "-65536..65535");
        rejected("slti a0, zero, -65537 / nop / nop / nop", "-65536..65535");
        accepted("lui a0, 0xfffff / nop / nop / nop");
        rejected("lui a0, 0x100000 / nop / nop / nop", "0..1048575");
        rejected("lui a0, -1 / nop / nop / nop", "0..1048575");
        accepted("nop / nop / lw a1, 1023(a0) / nop");
        rejected("nop / nop / lw a1, 0x800(a0) / nop", "-1024..1023");
        rejected("nop / nop / sw a1, -1025(a0) / nop", "-1024..1023");
        accepted("nop / nop / nop / beq a0, a1, 511");
        rejected("nop / nop / nop / beq a0, a1, 0x401", "-512..511");
        accepted("nop / nop / nop / jalr zero, a0, 511");
        rejected("nop / nop / nop / jalr zero, a0, 512", "-512..511");
        accepted("nop / nop / nop / jal a0, 16383");
        rejected("nop / nop / nop / beqz a0, 16384", "-16384..16383");
        accepted("nop / nop / nop / j 524287");
        rejected("nop / nop / nop / j 524288", "-524288..524287");
    }

    #[test]
    fn a_constant_is_an_address_the_same_as_a_label() {
        let resolved = |code: &str| {
            let labels = |l: &str| (l == "target").then_some(108);
            VliwIv::deref(&parse(code).unwrap(), 100, &labels).map(|m| m.show())
        };
        assert_eq!(
            resolved("nop / nop / nop / beq a0, a1, 108"),
            resolved("nop / nop / nop / beq a0, a1, target")
        );
        assert_eq!(
            resolved("nop / nop / nop / j 108"),
            resolved("nop / nop / nop / j target")
        );
    }

    #[test]
    fn rejection_names_the_position_the_mnemonic_the_field_and_the_range() {
        let err = field_outcome("nop / nop / nop / beq a0, a1, 1025").unwrap_err();
        for part in ["1:", "beq", "CTRL disp", "1025", "-512..511"] {
            assert!(err.contains(part), "{} in: {}", part, err);
        }
    }

    #[test]
    fn the_memory_offset_is_reported_under_the_operand_while_parsing() {
        let err = parse("nop / nop / lw a1, 0x800(a0) / nop").unwrap_err();
        for part in ["11-bit", "14 byte bundle", "-1024..1023"] {
            assert!(err.contains(part), "{} in: {}", part, err);
        }
    }
}
