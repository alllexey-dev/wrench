//! M68k: a subset of the Motorola 68000
//! (<http://wpage.unina.it/rcanonic/didattica/ce1/docs/68000.pdf>).

use crate::hs::{show_arg, show_char};
use crate::machine::{Arch, Machine, HALTED};
use crate::memory::{Instruction, IoMem};
use crate::parser::{Alt, Ref, P, R};
use crate::report::value_view;
use crate::report::view::{Format, Spec, View};
use crate::rng::RandomStream;
use crate::translator::ParsedInstr;
use crate::word::{add_ext, b8, div_floor, lshr, mul_ext, sar, shl, sub_ext, u5, Ext, W};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Long,
    Byte,
}

use Mode::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DataReg {
    D0,
    D1,
    D2,
    D3,
    D4,
    D5,
    D6,
    D7,
}

pub const DATA_REGISTERS: [DataReg; 8] = [
    DataReg::D0,
    DataReg::D1,
    DataReg::D2,
    DataReg::D3,
    DataReg::D4,
    DataReg::D5,
    DataReg::D6,
    DataReg::D7,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AddrReg {
    A0,
    A1,
    A2,
    A3,
    A4,
    A5,
    A6,
    A7,
}

pub const ADDR_REGISTERS: [AddrReg; 8] = [
    AddrReg::A0,
    AddrReg::A1,
    AddrReg::A2,
    AddrReg::A3,
    AddrReg::A4,
    AddrReg::A5,
    AddrReg::A6,
    AddrReg::A7,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexRegister {
    AddrIndex(AddrReg),
    DataIndex(DataReg),
}

impl IndexRegister {
    fn show(self) -> String {
        match self {
            IndexRegister::AddrIndex(r) => format!("AddrIndex {:?}", r),
            IndexRegister::DataIndex(r) => format!("DataIndex {:?}", r),
        }
    }
}

/// For `(A0)+` and `-(A0)` the address register moves by the operand size:
/// a byte access by 1, a long by 4.
#[derive(Clone, Debug)]
pub enum Argument<L> {
    DirectDataReg(DataReg),
    DirectAddrReg(AddrReg),
    /// Address with a signed displacement and maybe an index, e.g. `16(A0)`, `8(A0,D1)`.
    IndirectAddrReg(i64, AddrReg, Option<IndexRegister>),
    /// `-(A0)`
    IndirectAddrRegPreDecrement(AddrReg),
    /// `(A0)+`
    IndirectAddrRegPostIncrement(AddrReg),
    Immediate(L),
}

use Argument::*;

impl<L> Argument<L> {
    fn byte_size(&self) -> usize {
        match self {
            IndirectAddrReg(..) => 2,
            Immediate(_) => 4,
            _ => 0,
        }
    }

    fn resolve(&self, imm: impl Fn(&L) -> Result<W, String>) -> Result<Argument<W>, String> {
        Ok(match self {
            DirectDataReg(r) => DirectDataReg(*r),
            DirectAddrReg(r) => DirectAddrReg(*r),
            IndirectAddrReg(o, r, i) => IndirectAddrReg(*o, *r, *i),
            IndirectAddrRegPreDecrement(r) => IndirectAddrRegPreDecrement(*r),
            IndirectAddrRegPostIncrement(r) => IndirectAddrRegPostIncrement(*r),
            Immediate(l) => Immediate(imm(l)?),
        })
    }
}

impl Argument<W> {
    fn show(&self) -> String {
        match self {
            DirectDataReg(r) => format!("DirectDataReg {:?}", r),
            DirectAddrReg(r) => format!("DirectAddrReg {:?}", r),
            IndirectAddrReg(o, r, i) => {
                let i = i.map_or("Nothing".to_string(), |i| format!("(Just ({}))", i.show()));
                format!("IndirectAddrReg {} {:?} {}", show_arg(*o), r, i)
            }
            IndirectAddrRegPreDecrement(r) => format!("IndirectAddrRegPreDecrement {:?}", r),
            IndirectAddrRegPostIncrement(r) => format!("IndirectAddrRegPostIncrement {:?}", r),
            Immediate(v) => format!("Immediate {}", show_arg(*v)),
        }
    }
}

/// Instructions with a mode, a source and a destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op2 {
    Move,
    MoveA,
    And,
    Or,
    Xor,
    Add,
    Sub,
    Mul,
    Div,
    Cmp,
    Asl,
    Asr,
    Lsl,
    Lsr,
}

/// Instructions with a mode and a destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op1 {
    Not,
    Neg,
    Clr,
}

/// Jumps and branches to an absolute address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jump {
    Jmp,
    Bcc,
    Bcs,
    Beq,
    Bne,
    Blt,
    Bgt,
    Ble,
    Bge,
    Bmi,
    Bpl,
    Bvc,
    Bvs,
    Jsr,
}

/// The constructor name lowercased is the assembler mnemonic.
fn mnemonic_of(op: impl std::fmt::Debug) -> String {
    format!("{:?}", op).to_lowercase()
}

/// Constructors are grouped by operand shape; each still shows as its own
/// constructor (`Move {mode = ..}`), as in the original.
#[derive(Clone, Debug)]
pub enum M68kIsa<L> {
    Op2 {
        op: Op2,
        mode: Mode,
        src: Argument<L>,
        dst: Argument<L>,
    },
    Op1 {
        op: Op1,
        mode: Mode,
        dst: Argument<L>,
    },
    Jump {
        op: Jump,
        target: L,
    },
    Rts,
    Link {
        addr_reg: AddrReg,
        offset: i64,
    },
    Unlk {
        addr_reg: AddrReg,
    },
    Halt,
}

use M68kIsa as I;

impl<L> M68kIsa<L> {
    fn size(&self) -> usize {
        match self {
            I::Op2 { src, dst, .. } => 2 + src.byte_size() + dst.byte_size(),
            I::Op1 { dst, .. } => 2 + dst.byte_size(),
            I::Jump { .. } => 6,
            I::Link { .. } => 4,
            I::Rts | I::Unlk { .. } | I::Halt => 2,
        }
    }
}

impl ParsedInstr for M68kIsa<Ref> {
    fn byte_size(&self) -> usize {
        self.size()
    }
}

impl Instruction for M68kIsa<W> {
    fn byte_size(&self) -> usize {
        self.size()
    }

    fn show(&self) -> String {
        match self {
            I::Op2 { op, mode, src, dst } => {
                format!(
                    "{:?} {{mode = {:?}, src = {}, dst = {}}}",
                    op,
                    mode,
                    src.show(),
                    dst.show()
                )
            }
            I::Op1 { op, mode, dst } => format!("{:?} {{mode = {:?}, dst = {}}}", op, mode, dst.show()),
            I::Jump { op, target } => format!("{:?} {{ref = {}}}", op, target),
            I::Rts => "Rts".to_string(),
            I::Link { addr_reg, offset } => format!("Link {{addrReg = {:?}, offset = {}}}", addr_reg, offset),
            I::Unlk { addr_reg } => format!("Unlk {{addrReg = {:?}}}", addr_reg),
            I::Halt => "Halt".to_string(),
        }
    }
}

const COMMENT: &str = ";";

/// `oneOf`: one of these characters.
fn one_of(p: &mut P, cs: &[char]) -> R<char> {
    match p.peek() {
        Some(c) if cs.contains(&c) => {
            p.pos += 1;
            Ok(c)
        }
        _ => {
            let mut e = p.expected::<()>("").unwrap_err();
            e.expected = cs.iter().map(|&c| show_char(c)).collect();
            Err(e)
        }
    }
}

/// `notFollowedBy`: succeeds, consuming nothing, where `f` fails.
fn not_followed_by<T>(p: &mut P, f: impl FnOnce(&mut P) -> R<T>) -> R<()> {
    let start = p.pos;
    let r = p.attempt(f);
    p.pos = start;
    match r {
        Err(_) => Ok(()),
        Ok(_) => {
            let mut e = p.expected::<()>("").unwrap_err();
            e.expected.clear();
            Err(e)
        }
    }
}

const REG_DIGITS: [char; 8] = ['0', '1', '2', '3', '4', '5', '6', '7'];

fn reg_number(p: &mut P) -> R<usize> {
    one_of(p, &REG_DIGITS).map(|c| c as usize - '0' as usize)
}

fn data_register_(p: &mut P) -> R<DataReg> {
    p.attempt(|p| {
        p.string("D")?;
        Ok(DATA_REGISTERS[reg_number(p)?])
    })
}

fn addr_register_(p: &mut P) -> R<AddrReg> {
    p.attempt(|p| {
        p.string("A")?;
        Ok(ADDR_REGISTERS[reg_number(p)?])
    })
}

type Arg = Argument<Ref>;

fn data_register(p: &mut P) -> R<Arg> {
    data_register_(p).map(DirectDataReg)
}

fn addr_register(p: &mut P) -> R<Arg> {
    addr_register_(p).map(DirectAddrReg)
}

/// `d(An)` and `link` have a whole extension word for the displacement.
const DISPLACEMENT_BITS: u32 = 16;

/// `d(An, Xn)` shares a brief extension word with its index register, which
/// leaves the displacement a byte.
const INDEXED_DISPLACEMENT_BITS: u32 = 8;

fn require_displacement(p: &mut P, field_bits: u32, operand_pos: usize, value: i64) -> R<i64> {
    let hi = (1i64 << (field_bits - 1)) - 1;
    let lo = -(1i64 << (field_bits - 1));
    if lo <= value && value <= hi {
        Ok(value)
    } else {
        p.fail_at(
            operand_pos,
            format!(
                "displacement {} doesn't fit the {}-bit signed field of an extension word, expected {}..{}",
                value, field_bits, lo, hi
            ),
        )
    }
}

/// `An` in parentheses, with the spaces around it.
fn inner_addr_reg(p: &mut P) -> R<AddrReg> {
    p.hspace()?;
    p.string("A")?;
    let n = reg_number(p)?;
    p.hspace()?;
    Ok(ADDR_REGISTERS[n])
}

fn indirect_addr_register(p: &mut P) -> R<Arg> {
    let (operand_pos, offset, r, index) = p.attempt(|p| {
        let operand_pos = p.offset();
        let offset = p.optional(|p| p.int_literal())?;
        p.string("(")?;
        let r = inner_addr_reg(p)?;
        let index = p.optional(|p| {
            p.string(",")?;
            p.hspace()?;
            p.or(
                |p| data_register_(p).map(IndexRegister::DataIndex),
                |p| addr_register_(p).map(IndexRegister::AddrIndex),
            )
        })?;
        p.string(")")?;
        Ok((operand_pos, offset, r, index))
    })?;
    let displacement = match (offset, index) {
        (None, _) => 0,
        (Some(v), None) => require_displacement(p, DISPLACEMENT_BITS, operand_pos, v)?,
        (Some(v), Some(_)) => require_displacement(p, INDEXED_DISPLACEMENT_BITS, operand_pos, v)?,
    };
    Ok(IndirectAddrReg(displacement, r, index))
}

fn indirect_addr_reg_pre_decrement(p: &mut P) -> R<Arg> {
    p.attempt(|p| {
        p.string("-(")?;
        let r = inner_addr_reg(p)?;
        p.string(")")?;
        Ok(IndirectAddrRegPreDecrement(r))
    })
}

fn indirect_addr_reg_post_increment(p: &mut P) -> R<Arg> {
    p.attempt(|p| {
        p.string("(")?;
        let r = inner_addr_reg(p)?;
        p.string(")+")?;
        Ok(IndirectAddrRegPostIncrement(r))
    })
}

fn all_indirect_addr(p: &mut P) -> R<Arg> {
    p.choice(&[
        &indirect_addr_reg_pre_decrement,
        &indirect_addr_reg_post_increment,
        &indirect_addr_register,
    ])
}

/// A bare register name (`A2`, `D3`): `immediate` refuses it as a label.
fn register_like_name(p: &mut P) -> R<()> {
    p.attempt(|p| {
        one_of(p, &['A', 'D'])?;
        one_of(p, &REG_DIGITS)?;
        not_followed_by(p, |p| {
            p.satisfy(|c| c.is_alphanumeric() || c == '_', "alphanumeric character")
        })
    })
}

fn immediate(p: &mut P) -> R<Arg> {
    not_followed_by(p, register_like_name)?;
    p.reference().map(Immediate)
}

/// The source where Address Register Direct is not a valid mode, so
/// `move.l A2, D0` is a parse error rather than a label `A2` (issue #143).
fn src(p: &mut P) -> R<Arg> {
    p.choice(&[&data_register, &all_indirect_addr, &immediate])
}

/// `movea` is the one instruction that takes An as source.
fn src_movea(p: &mut P) -> R<Arg> {
    p.choice(&[&data_register, &addr_register, &all_indirect_addr, &immediate])
}

fn dst(p: &mut P) -> R<Arg> {
    p.or(data_register, all_indirect_addr)
}

fn shift_count(p: &mut P) -> R<Arg> {
    p.or(data_register, |p| p.constant_ref().map(Immediate))
}

fn long_mode(p: &mut P) -> R<Mode> {
    p.string(".l").map(|_| Long)
}

fn any_mode(p: &mut P) -> R<Mode> {
    p.or(long_mode, |p| p.string(".b").map(|_| Byte))
}

type Parsed = M68kIsa<Ref>;
type ModeP = fn(&mut P) -> R<Mode>;
type ArgP = fn(&mut P) -> R<Arg>;

fn cmd2args(p: &mut P, op: Op2, mode: ModeP, src: ArgP, dst: ArgP) -> R<Parsed> {
    let mode = p.attempt(|p| {
        p.string(&mnemonic_of(op))?;
        let m = mode(p)?;
        p.hspace1()?;
        Ok(m)
    })?;
    let src = src(p)?;
    p.comma()?;
    let dst = dst(p)?;
    p.eol_(COMMENT)?;
    Ok(I::Op2 { op, mode, src, dst })
}

fn cmd1args(p: &mut P, op: Op1) -> R<Parsed> {
    p.attempt(|p| {
        p.string(&mnemonic_of(op))?;
        let mode = any_mode(p)?;
        p.hspace1()?;
        let dst = dst(p)?;
        p.eol_(COMMENT)?;
        Ok(I::Op1 { op, mode, dst })
    })
}

fn branch_cmd(p: &mut P, op: Jump) -> R<Parsed> {
    p.attempt(|p| {
        p.string(&mnemonic_of(op))?;
        p.hspace1()
    })?;
    let target = p.reference()?;
    p.eol_(COMMENT)?;
    Ok(I::Jump { op, target })
}

fn cmd0args(p: &mut P, m: &str, i: Parsed) -> R<Parsed> {
    p.attempt(|p| {
        p.string(m)?;
        p.eol_(COMMENT)?;
        Ok(i)
    })
}

fn link(p: &mut P) -> R<Parsed> {
    let addr_reg = p.attempt(|p| {
        p.string("link")?;
        p.hspace1()?;
        addr_register_(p)
    })?;
    p.hspace()?;
    p.comma()?;
    p.hspace()?;
    let operand_pos = p.offset();
    let offset = match p.optional(|p| p.int_literal())? {
        Some(v) => require_displacement(p, DISPLACEMENT_BITS, operand_pos, v)?,
        None => 0,
    };
    p.eol_(COMMENT)?;
    Ok(I::Link { addr_reg, offset })
}

fn unlk(p: &mut P) -> R<Parsed> {
    p.attempt(|p| {
        p.string("unlk")?;
        p.hspace1()?;
        let addr_reg = addr_register_(p)?;
        p.eol_(COMMENT)?;
        Ok(I::Unlk { addr_reg })
    })
}

type BoxedAlt = Box<dyn Fn(&mut P) -> R<Parsed>>;

fn op2(op: Op2, mode: ModeP, s: ArgP, d: ArgP) -> BoxedAlt {
    Box::new(move |p: &mut P| cmd2args(p, op, mode, s, d))
}

fn op1(op: Op1) -> BoxedAlt {
    Box::new(move |p: &mut P| cmd1args(p, op))
}

fn jump(op: Jump) -> BoxedAlt {
    Box::new(move |p: &mut P| branch_cmd(p, op))
}

fn command(p: &mut P) -> R<Parsed> {
    use Jump::*;
    use Op1::*;
    use Op2::*;
    let alts: Vec<BoxedAlt> = vec![
        op2(Move, any_mode, src, dst),
        op2(MoveA, long_mode, src_movea, addr_register),
        op1(Not),
        op1(Neg),
        op1(Clr),
        op2(And, any_mode, src, dst),
        op2(Or, any_mode, src, dst),
        op2(Xor, any_mode, src, dst),
        op2(Add, any_mode, src, dst),
        op2(Sub, any_mode, src, dst),
        op2(Mul, any_mode, src, dst),
        op2(Div, any_mode, src, dst),
        op2(Cmp, any_mode, src, dst),
        op2(Asl, any_mode, shift_count, dst),
        op2(Asr, any_mode, shift_count, dst),
        op2(Lsl, any_mode, shift_count, dst),
        op2(Lsr, any_mode, shift_count, dst),
        jump(Jmp),
        jump(Bcc),
        jump(Bcs),
        jump(Beq),
        jump(Bne),
        jump(Blt),
        jump(Bgt),
        jump(Ble),
        jump(Bge),
        jump(Bmi),
        jump(Bpl),
        jump(Bvc),
        jump(Bvs),
        jump(Jsr),
        Box::new(|p: &mut P| cmd0args(p, "rts", I::Rts)),
        Box::new(link),
        Box::new(unlk),
        Box::new(|p: &mut P| cmd0args(p, "halt", I::Halt)),
    ];
    let refs: Vec<Alt<Parsed>> = alts.iter().map(|b| b.as_ref()).collect();
    p.choice(&refs)
}

pub struct M68k;

impl Arch for M68k {
    type Parsed = Parsed;
    type State = M68kSt;
    const NAME: &'static str = "m68k";
    const COMMENT: &'static str = COMMENT;

    fn mnemonic(p: &mut P) -> R<Parsed> {
        command(p)
    }

    fn deref(i: &Parsed, _offset: W, f: &dyn Fn(&str) -> Option<W>) -> Result<M68kIsa<W>, String> {
        let arg = |a: &Arg| a.resolve(|l| l.resolve(Ok, f));
        Ok(match i {
            I::Op2 { op, mode, src, dst } => {
                let m = mnemonic_of(op);
                let src = match (op, mode) {
                    (Op2::Asl | Op2::Asr | Op2::Lsl | Op2::Lsr, _) => {
                        let m = format!("{}{}", m, if *mode == Long { ".l" } else { ".b" });
                        src.resolve(|l| {
                            l.resolve(|x| u5(|v| format!("{}: shift count {} is not in 0..31", m, v), x), f)
                        })?
                    }
                    // A byte-mode operation reads one byte of its immediate, so a
                    // wider value has nowhere to go.
                    (
                        Op2::Move
                        | Op2::And
                        | Op2::Or
                        | Op2::Xor
                        | Op2::Add
                        | Op2::Sub
                        | Op2::Mul
                        | Op2::Div
                        | Op2::Cmp,
                        Byte,
                    ) => src.resolve(|l| {
                        l.resolve(
                            |x| b8(|v| format!("{}.b: immediate {} is not in -128..255", m, v), x),
                            f,
                        )
                    })?,
                    _ => arg(src)?,
                };
                I::Op2 {
                    op: *op,
                    mode: *mode,
                    src,
                    dst: arg(dst)?,
                }
            }
            I::Op1 { op, mode, dst } => I::Op1 {
                op: *op,
                mode: *mode,
                dst: arg(dst)?,
            },
            I::Jump { op, target } => I::Jump {
                op: *op,
                target: target.resolve(Ok, f)?,
            },
            I::Rts => I::Rts,
            I::Link { addr_reg, offset } => I::Link {
                addr_reg: *addr_reg,
                offset: *offset,
            },
            I::Unlk { addr_reg } => I::Unlk { addr_reg: *addr_reg },
            I::Halt => I::Halt,
        })
    }
}

#[derive(Clone)]
pub struct M68kSt {
    pub pc: i64,
    pub data_regs: [W; 8],
    pub addr_regs: [W; 8],
    pub mem: IoMem<M68kIsa<W>>,
    pub stopped: bool,
    pub internal_error: Option<String>,
    pub n_flag: bool,
    pub z_flag: bool,
    pub v_flag: bool,
    pub c_flag: bool,
}

/// `addExt` on a byte; the value comes back sign-extended.
fn add_ext8(x: i8, y: i8) -> Ext {
    let value = x.wrapping_add(y);
    let overflow = (x > 0 && y > 0 && value < 0) || (x < 0 && y < 0 && value > 0);
    let carry = (x as u8 as u16 + y as u8 as u16) >> 8 & 1 == 1;
    Ext {
        value: value as W,
        overflow,
        carry,
    }
}

fn sub_ext8(x: i8, y: i8) -> Ext {
    let value = x.wrapping_sub(y);
    let overflow = (x > 0 && y < 0 && value < 0) || (x < 0 && y > 0 && value > 0);
    let carry = (x as u8) < (y as u8);
    Ext {
        value: value as W,
        overflow,
        carry,
    }
}

fn mul_ext8(x: i8, y: i8) -> Ext {
    let value = x.wrapping_mul(y);
    let overflow = x != 0 && y != 0 && div_floor(value as W, x as W) != y as W;
    Ext {
        value: value as W,
        overflow,
        carry: false,
    }
}

/// Shifts of a byte by at most 8, as Haskell's `Int8`/`Word8` shifts.
fn byte_shl(x: i8, n: i64) -> i8 {
    if n >= 8 {
        0
    } else {
        ((x as u8) << n) as i8
    }
}

fn byte_sar(x: i8, n: i64) -> i8 {
    x >> n.min(7)
}

fn byte_lshr(x: i8, n: i64) -> i8 {
    if n >= 8 {
        0
    } else {
        ((x as u8) >> n) as i8
    }
}

/// The last bit shifted out, or none when the count is 0 or past the width.
fn shift_carry(left: bool, bits: u32, width: i64, count: i64) -> bool {
    if count <= 0 || count > width {
        return false;
    }
    let bit = if left { width - count } else { count - 1 };
    bits >> bit & 1 == 1
}

type Arg32 = Argument<W>;

const SP: Arg32 = DirectAddrReg(AddrReg::A7);
const PUSH: Arg32 = IndirectAddrRegPreDecrement(AddrReg::A7);
const POP: Arg32 = IndirectAddrRegPostIncrement(AddrReg::A7);

impl M68kSt {
    fn raise_internal_error(&mut self, msg: impl Into<String>) {
        self.internal_error = Some(msg.into());
    }

    /// Past the current instruction, read again from memory as the original does.
    fn next_pc(&mut self) {
        match self.fetch() {
            Ok((pc, i)) => self.pc = pc + i.byte_size() as i64,
            Err(e) => self.raise_internal_error(format!("nextPc: {}", e)),
        }
    }

    fn set_flags(&mut self, e: Ext) {
        self.n_flag = e.value < 0;
        self.z_flag = e.value == 0;
        self.v_flag = e.overflow;
        self.c_flag = e.carry;
    }

    fn clear_vc(&mut self) {
        self.v_flag = false;
        self.c_flag = false;
    }

    fn indirect_addr(&self, f: impl Fn(i64) -> i64, r: AddrReg, index: Option<IndexRegister>) -> i64 {
        let offset = match index {
            Some(IndexRegister::DataIndex(d)) => self.data_regs[d as usize] as i64,
            Some(IndexRegister::AddrIndex(a)) => self.addr_regs[a as usize] as i64,
            None => 0,
        };
        offset + f(self.addr_regs[r as usize] as i64)
    }

    fn mem_error(&mut self, err: String) {
        self.raise_internal_error(format!("memory access error: {}", err));
    }

    fn read_memory_word(&mut self, addr: i64) -> W {
        self.mem.read_word(addr).unwrap_or_else(|e| {
            self.mem_error(e);
            0
        })
    }

    fn write_memory_word(&mut self, addr: i64, w: W) {
        if let Err(e) = self.mem.write_word(addr, w) {
            self.mem_error(e);
        }
    }

    fn read_memory_byte(&mut self, addr: i64) -> i8 {
        match self.mem.read_byte(addr) {
            Ok(b) => b as i8,
            Err(e) => {
                self.mem_error(e);
                0
            }
        }
    }

    fn write_memory_byte(&mut self, addr: i64, b: i8) {
        if let Err(e) = self.mem.write_byte(addr, b as u8) {
            self.mem_error(e);
        }
    }

    fn fetch_word(&mut self, arg: &Arg32) -> W {
        match *arg {
            DirectDataReg(r) => self.data_regs[r as usize],
            DirectAddrReg(r) => self.addr_regs[r as usize],
            IndirectAddrReg(offset, r, index) => {
                let addr = self.indirect_addr(|a| a + offset, r, index);
                self.read_memory_word(addr)
            }
            IndirectAddrRegPreDecrement(r) => {
                let addr = self.indirect_addr(|a| a - 4, r, None);
                self.store_word(&DirectAddrReg(r), addr as W);
                self.read_memory_word(addr)
            }
            IndirectAddrRegPostIncrement(r) => {
                let addr = self.indirect_addr(|a| a, r, None);
                let w = self.read_memory_word(addr);
                self.store_word(&DirectAddrReg(r), (addr + 4) as W);
                w
            }
            Immediate(v) => v,
        }
    }

    fn store_word(&mut self, arg: &Arg32, v: W) {
        match *arg {
            DirectDataReg(r) => {
                self.data_regs[r as usize] = v;
                self.z_flag = v == 0;
                self.n_flag = v < 0;
            }
            DirectAddrReg(r) => self.addr_regs[r as usize] = v,
            IndirectAddrReg(offset, r, index) => {
                let addr = self.indirect_addr(|a| a + offset, r, index);
                self.write_memory_word(addr, v);
            }
            IndirectAddrRegPreDecrement(r) => {
                let addr = self.indirect_addr(|a| a - 4, r, None);
                self.store_word(&DirectAddrReg(r), addr as W);
                self.write_memory_word(addr, v);
            }
            IndirectAddrRegPostIncrement(r) => {
                let addr = self.indirect_addr(|a| a, r, None);
                self.store_word(&DirectAddrReg(r), (addr + 4) as W);
                self.write_memory_word(addr, v);
            }
            Immediate(_) => panic!("can not store word: {}", arg.show()),
        }
    }

    fn fetch_byte(&mut self, arg: &Arg32) -> i8 {
        match *arg {
            DirectDataReg(r) => self.data_regs[r as usize] as i8,
            IndirectAddrReg(offset, r, index) => {
                let addr = self.indirect_addr(|a| a + offset, r, index);
                self.read_memory_byte(addr)
            }
            IndirectAddrRegPreDecrement(r) => {
                let addr = self.indirect_addr(|a| a - 1, r, None);
                self.store_word(&DirectAddrReg(r), addr as W);
                self.read_memory_byte(addr)
            }
            IndirectAddrRegPostIncrement(r) => {
                let addr = self.indirect_addr(|a| a, r, None);
                let b = self.read_memory_byte(addr);
                self.store_word(&DirectAddrReg(r), (addr + 1) as W);
                b
            }
            Immediate(v) => v as i8,
            DirectAddrReg(_) => panic!("can not fetch byte: {}", arg.show()),
        }
    }

    fn store_byte(&mut self, arg: &Arg32, v: i8) {
        match *arg {
            DirectDataReg(r) => {
                // The original merges the byte sign-extended, so a negative one fills the register.
                let w = self.data_regs[r as usize];
                self.data_regs[r as usize] = (w & !0xFF) | v as W;
                self.z_flag = v == 0;
                self.n_flag = v < 0;
            }
            IndirectAddrReg(offset, r, index) => {
                let addr = self.indirect_addr(|a| a + offset, r, index);
                self.write_memory_byte(addr, v);
            }
            IndirectAddrRegPreDecrement(r) => {
                let addr = self.indirect_addr(|a| a - 1, r, None);
                self.store_word(&DirectAddrReg(r), addr as W);
                self.write_memory_byte(addr, v);
            }
            IndirectAddrRegPostIncrement(r) => {
                let addr = self.indirect_addr(|a| a, r, None);
                self.store_word(&DirectAddrReg(r), (addr + 1) as W);
                self.write_memory_byte(addr, v);
            }
            Immediate(_) => panic!("impossible to store into immediate destination"),
            DirectAddrReg(_) => panic!("can not store byte: {}", arg.show()),
        }
    }

    fn word_cmd1(&mut self, src: &Arg32, dst: &Arg32, f: impl Fn(W) -> W) {
        let a = self.fetch_word(src);
        self.store_word(dst, f(a));
        self.clear_vc();
        self.next_pc();
    }

    fn word_cmd2(&mut self, src: &Arg32, dst: &Arg32, f: impl Fn(W, W) -> W) {
        let a = self.fetch_word(dst);
        let b = self.fetch_word(src);
        self.store_word(dst, f(a, b));
        self.clear_vc();
        self.next_pc();
    }

    fn word_cmd2_ext(&mut self, src: &Arg32, dst: &Arg32, f: impl Fn(W, W) -> Ext) {
        let a = self.fetch_word(dst);
        let b = self.fetch_word(src);
        let e = f(a, b);
        self.store_word(dst, e.value);
        self.set_flags(e);
        self.next_pc();
    }

    fn byte_cmd1(&mut self, src: &Arg32, dst: &Arg32, f: impl Fn(i8) -> i8) {
        let a = self.fetch_byte(src);
        self.store_byte(dst, f(a));
        self.clear_vc();
        self.next_pc();
    }

    fn byte_cmd2(&mut self, src: &Arg32, dst: &Arg32, f: impl Fn(i8, i8) -> i8) {
        let a = self.fetch_byte(dst);
        let b = self.fetch_byte(src);
        self.store_byte(dst, f(a, b));
        self.clear_vc();
        self.next_pc();
    }

    fn byte_cmd2_ext(&mut self, src: &Arg32, dst: &Arg32, f: impl Fn(i8, i8) -> Ext) {
        let a = self.fetch_byte(dst);
        let b = self.fetch_byte(src);
        let e = f(a, b);
        self.store_byte(dst, e.value as i8);
        self.set_flags(e);
        self.next_pc();
    }

    /// The count is taken modulo 64 and held at the width.
    fn word_shift(&mut self, src: &Arg32, dst: &Arg32, f: fn(W, i64) -> W, left: bool) {
        let a = self.fetch_word(dst);
        let b = self.fetch_word(src);
        let requested = (b & 0x3F) as i64;
        self.store_word(dst, f(a, requested.min(32)));
        self.v_flag = false;
        self.c_flag = shift_carry(left, a as u32, 32, requested);
        self.next_pc();
    }

    fn byte_shift(&mut self, src: &Arg32, dst: &Arg32, f: fn(i8, i64) -> i8, left: bool) {
        let a = self.fetch_byte(dst);
        let b = self.fetch_byte(src);
        let requested = (b & 0x3F) as i64;
        self.store_byte(dst, f(a, requested.min(8)));
        self.v_flag = false;
        self.c_flag = shift_carry(left, a as u8 as u32, 8, requested);
        self.next_pc();
    }

    fn op2(&mut self, op: Op2, mode: Mode, src: &Arg32, dst: &Arg32) {
        use Op2::*;
        match (op, mode) {
            (Move | MoveA, Long) => self.word_cmd1(src, dst, |a| a),
            (Move, Byte) => self.byte_cmd1(src, dst, |a| a),
            (MoveA, Byte) => panic!("not implemented"),
            (And, Long) => self.word_cmd2(src, dst, |a, b| a & b),
            (And, Byte) => self.byte_cmd2(src, dst, |a, b| a & b),
            (Or, Long) => self.word_cmd2(src, dst, |a, b| a | b),
            (Or, Byte) => self.byte_cmd2(src, dst, |a, b| a | b),
            (Xor, Long) => self.word_cmd2(src, dst, |a, b| a ^ b),
            (Xor, Byte) => self.byte_cmd2(src, dst, |a, b| a ^ b),
            (Add, Long) => self.word_cmd2_ext(src, dst, add_ext),
            (Add, Byte) => self.byte_cmd2_ext(src, dst, add_ext8),
            (Sub, Long) => self.word_cmd2_ext(src, dst, sub_ext),
            (Sub, Byte) => self.byte_cmd2_ext(src, dst, sub_ext8),
            (Mul, Long) => self.word_cmd2_ext(src, dst, mul_ext),
            (Mul, Byte) => self.byte_cmd2_ext(src, dst, mul_ext8),
            // The divisor is read once more to check it, as in the original.
            (Div, Long) => {
                if self.fetch_word(src) == 0 {
                    self.raise_internal_error("division by zero");
                } else {
                    self.word_cmd2(src, dst, div_floor);
                }
            }
            (Div, Byte) => {
                if self.fetch_byte(src) == 0 {
                    self.raise_internal_error("division by zero");
                } else {
                    self.byte_cmd2(src, dst, |a, b| div_floor(a as W, b as W) as i8);
                }
            }
            (Cmp, Long) => {
                let a = self.fetch_word(dst);
                let b = self.fetch_word(src);
                self.set_flags(sub_ext(a, b));
                self.next_pc();
            }
            (Cmp, Byte) => {
                let a = self.fetch_byte(dst);
                let b = self.fetch_byte(src);
                self.set_flags(sub_ext8(a, b));
                self.next_pc();
            }
            (Asl | Lsl, Long) => self.word_shift(src, dst, shl, true),
            (Asl | Lsl, Byte) => self.byte_shift(src, dst, byte_shl, true),
            (Asr, Long) => self.word_shift(src, dst, sar, false),
            (Asr, Byte) => self.byte_shift(src, dst, byte_sar, false),
            (Lsr, Long) => self.word_shift(src, dst, lshr, false),
            (Lsr, Byte) => self.byte_shift(src, dst, byte_lshr, false),
        }
    }

    fn op1(&mut self, op: Op1, mode: Mode, dst: &Arg32) {
        match (op, mode) {
            (Op1::Not, Long) => self.word_cmd1(dst, dst, |a| !a),
            (Op1::Not, Byte) => self.byte_cmd1(dst, dst, |a| !a),
            (Op1::Clr, _) => {
                match mode {
                    Long => self.store_word(dst, 0),
                    Byte => self.store_byte(dst, 0),
                }
                self.set_flags(Ext {
                    value: 0,
                    overflow: false,
                    carry: false,
                });
                self.next_pc();
            }
            (Op1::Neg, Long) => {
                let a = self.fetch_word(dst);
                let e = sub_ext(0, a);
                self.store_word(dst, e.value);
                self.set_flags(e);
                self.next_pc();
            }
            (Op1::Neg, Byte) => {
                let a = self.fetch_byte(dst);
                let e = sub_ext8(0, a);
                self.store_byte(dst, e.value as i8);
                self.set_flags(e);
                self.next_pc();
            }
        }
    }

    fn taken(&self, op: Jump) -> bool {
        use Jump::*;
        let (n, z, v, c) = (self.n_flag, self.z_flag, self.v_flag, self.c_flag);
        match op {
            Jmp | Jsr => true,
            Bcc => !c,
            Bcs => c,
            Beq => z,
            Bne => !z,
            Blt => n != v,
            Bgt => !z && n == v,
            Ble => z || n != v,
            Bge => n == v,
            Bmi => n,
            Bpl => !n,
            Bvc => !v,
            Bvs => v,
        }
    }

    fn status_bits(&self) -> String {
        [self.n_flag, self.z_flag, self.v_flag, self.c_flag]
            .iter()
            .map(|&f| if f { '1' } else { '0' })
            .collect()
    }
}

impl Machine for M68kSt {
    type Instr = M68kIsa<W>;

    fn init(pc: i64, mem: IoMem<Self::Instr>, _randoms: RandomStream) -> Self {
        M68kSt {
            pc,
            data_regs: [0; 8],
            addr_regs: [0; 8],
            mem,
            stopped: false,
            internal_error: None,
            n_flag: false,
            z_flag: true,
            v_flag: false,
            c_flag: false,
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
            I::Op2 { op, mode, src, dst } => self.op2(op, mode, &src, &dst),
            I::Op1 { op, mode, dst } => self.op1(op, mode, &dst),
            I::Jump { op: Jump::Jsr, target } => {
                self.next_pc();
                let pc = self.pc as W;
                self.store_word(&PUSH, pc);
                self.pc = target as i64;
            }
            I::Jump { op, target } => {
                if self.taken(op) {
                    self.pc = target as i64;
                } else {
                    self.next_pc();
                }
            }
            I::Rts => self.pc = self.fetch_word(&POP) as i64,
            I::Link { addr_reg, offset } => {
                let fp = self.fetch_word(&DirectAddrReg(addr_reg));
                self.store_word(&PUSH, fp);
                let sp = self.fetch_word(&SP);
                self.store_word(&DirectAddrReg(addr_reg), sp);
                self.store_word(&SP, sp.wrapping_add(offset as W));
                self.next_pc();
            }
            I::Unlk { addr_reg } => {
                let sp = self.fetch_word(&DirectAddrReg(addr_reg));
                self.store_word(&SP, sp);
                let fp = self.fetch_word(&POP);
                self.store_word(&DirectAddrReg(addr_reg), fp);
                self.next_pc();
            }
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
        let sr = View::<M68kSt>::new(
            Spec::lit("SR").format(&[Format::Bin], Format::Bin),
            "Status register flags `NZVC` as bits.",
            |e, _| e.state.status_bits(),
        );
        let data = DATA_REGISTERS.into_iter().map(|r| {
            let n = format!("{:?}", r);
            value_view::<M68kSt>(&n, &format!("Data register `{}`.", n), move |e| {
                e.state.data_regs[r as usize] as i64
            })
        });
        let addr = ADDR_REGISTERS.into_iter().map(|r| {
            let n = format!("{:?}", r);
            value_view::<M68kSt>(&n, &format!("Address register `{}`.", n), move |e| {
                e.state.addr_regs[r as usize] as i64
            })
        });
        std::iter::once(sr).chain(data).chain(addr).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::memory::{Cell, Mem};
    use crate::parser::error_pretty;
    use crate::rng::StdGen;
    use DataReg::*;

    fn translate(code: &str) -> Result<Parsed, String> {
        let src: Vec<char> = format!("{}\n", code).chars().collect();
        let mut p = P::new("-", &src);
        M68k::mnemonic(&mut p).map_err(|e| error_pretty("-", &src, &e))
    }

    /// A displacement is checked while parsing and a shift count at deref,
    /// so both land in `Err` here.
    fn field_outcome(code: &str) -> Result<M68kIsa<W>, String> {
        M68k::deref(&translate(code)?, 0, &|_| Some(0))
    }

    fn shown(code: &str) -> String {
        field_outcome(code).unwrap().show()
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

    /// Memory of 256 bytes, each holding its own address; `Dn = n`.
    fn st0() -> M68kSt {
        let mem = Mem {
            size: 256,
            data: (0..256).map(|a| Cell::Value(a as u8)).collect(),
        };
        let mut st = M68kSt::init(256, IoMem::new(BTreeMap::new(), mem), RandomStream::new(StdGen::new(0)));
        st.data_regs = [0, 1, 2, 3, 4, 5, 6, 7];
        st
    }

    fn with_data(regs: &[(DataReg, W)]) -> M68kSt {
        let mut st = st0();
        for &(r, v) in regs {
            st.data_regs[r as usize] = v;
        }
        st
    }

    fn with_addr(regs: &[(AddrReg, W)]) -> M68kSt {
        let mut st = st0();
        for &(r, v) in regs {
            st.addr_regs[r as usize] = v;
        }
        st
    }

    fn simulate(code: &str, mut st: M68kSt) -> M68kSt {
        let i = M68k::deref(&translate(code).unwrap(), 0, &|_| panic!("labels not defined")).unwrap();
        st.execute(0, i);
        st
    }

    fn d(st: &M68kSt, r: DataReg) -> W {
        st.data_regs[r as usize]
    }

    fn a(st: &M68kSt, r: AddrReg) -> W {
        st.addr_regs[r as usize]
    }

    fn bytes(st: &M68kSt, addrs: &[i64]) -> Vec<u8> {
        addrs.iter().filter_map(|&x| st.mem.cells.read_byte(x).ok()).collect()
    }

    fn word(st: &M68kSt, addr: i64) -> W {
        st.mem.cells.read_word(addr).unwrap()
    }

    #[test]
    fn byte_arithmetic_operations() {
        assert_eq!(d(&simulate("add.b D1, D0", with_data(&[(D1, 3), (D0, 4)])), D0), 7);
        let st = simulate("add.b D1, D0", with_data(&[(D1, 0xFF), (D0, 1)]));
        assert_eq!(d(&st, D0), 0);
        assert!(st.z_flag);
        assert_eq!(d(&simulate("sub.b D1, D0", with_data(&[(D1, 2), (D0, 7)])), D0), 5);
        let st = simulate("sub.b D1, D0", with_data(&[(D1, 10), (D0, 5)]));
        assert_eq!(d(&st, D0), -5);
        assert!(st.n_flag);
        assert_eq!(d(&simulate("mul.b D1, D0", with_data(&[(D1, 3), (D0, 4)])), D0), 12);
        assert_eq!(d(&simulate("mul.b D1, D0", with_data(&[(D1, 10), (D0, 10)])), D0), 100);
    }

    #[test]
    fn compare_operations() {
        for (d1, z, n, c) in [
            (5, true, false, false),
            (10, false, true, true),
            (3, false, false, false),
        ] {
            let st = simulate("cmp.l D1, D0", with_data(&[(D1, d1), (D0, 5)]));
            assert_eq!((d(&st, D0), d(&st, D1)), (5, d1));
            assert_eq!((st.z_flag, st.n_flag, st.c_flag), (z, n, c), "D1 = {}", d1);
        }
        let st = simulate("cmp.b D1, D0", with_data(&[(D1, 0x100), (D0, 0)]));
        assert_eq!((d(&st, D0), d(&st, D1)), (0, 0x100));
        assert!(st.z_flag);
    }

    #[test]
    fn logic_ops_clear_v_and_c_flags() {
        for (code, regs) in [
            ("and.l D1, D0", vec![(D1, 0xFF), (D0, 0xFF)]),
            ("move.l D1, D0", vec![(D1, 1)]),
            ("not.l D0", vec![(D0, 0)]),
        ] {
            let mut st = with_data(&regs);
            st.v_flag = true;
            st.c_flag = true;
            let st = simulate(code, st);
            assert!(!st.v_flag && !st.c_flag, "{}", code);
        }
    }

    #[test]
    fn translator() {
        let cases = [
            (
                "movea.l D0, A0",
                "MoveA {mode = Long, src = DirectDataReg D0, dst = DirectAddrReg A0}",
            ),
            (
                "move.l 12, D0",
                "Move {mode = Long, src = Immediate 12, dst = DirectDataReg D0}",
            ),
            (
                "move.l 8(A2), D0",
                "Move {mode = Long, src = IndirectAddrReg 8 A2 Nothing, dst = DirectDataReg D0}",
            ),
            (
                "move.l -8(A2), D0",
                "Move {mode = Long, src = IndirectAddrReg (-8) A2 Nothing, dst = DirectDataReg D0}",
            ),
            (
                "move.l -8(A2,D1), D0",
                "Move {mode = Long, src = IndirectAddrReg (-8) A2 (Just (DataIndex D1)), dst = DirectDataReg D0}",
            ),
            (
                "move.l -8(A2,A1), D0",
                "Move {mode = Long, src = IndirectAddrReg (-8) A2 (Just (AddrIndex A1)), dst = DirectDataReg D0}",
            ),
            (
                "cmp.l D1, D0",
                "Cmp {mode = Long, src = DirectDataReg D1, dst = DirectDataReg D0}",
            ),
            (
                "cmp.b 10, D0",
                "Cmp {mode = Byte, src = Immediate 10, dst = DirectDataReg D0}",
            ),
            (
                "cmp.l (A1), D0",
                "Cmp {mode = Long, src = IndirectAddrReg 0 A1 Nothing, dst = DirectDataReg D0}",
            ),
            // movea is the one instruction that legitimately accepts An as source.
            (
                "movea.l A2, A0",
                "MoveA {mode = Long, src = DirectAddrReg A2, dst = DirectAddrReg A0}",
            ),
            ("jsr 0x20", "Jsr {ref = 32}"),
            ("rts", "Rts"),
        ];
        for (code, expected) in cases {
            assert_eq!(shown(code), expected);
        }
    }

    #[test]
    fn an_as_source_rejected_outside_movea() {
        for code in [
            "move.l A2, D0",
            "add.l A0, D1",
            "sub.l A0, D1",
            "cmp.l A0, D0",
            "and.l A0, D1",
        ] {
            assert!(translate(code).is_err(), "{}", code);
        }
        assert!(translate("movea.l A2, A0").is_ok());
    }

    #[test]
    fn displacement_field() {
        for code in [
            "move.l -4(A6), D0",
            "move.l 32767(A0), D0",
            "move.l -32768(A0), D0",
            "link A6, -4",
        ] {
            assert!(translate(code).is_ok(), "{}", code);
        }
        rejected("move.l 32768(A0), D0", "-32768..32767");
        rejected("move.l -32769(A0), D0", "-32768..32767");
        // Shares a brief extension word with the index register.
        accepted("move.l 127(A0,D1), D0");
        rejected("move.l 128(A0,D1), D0", "-128..127");
        rejected("move.l -129(A0,D1), D0", "-128..127");
        rejected("link A6, 100000", "-32768..32767");
        let err = field_outcome("move.l 100000(A0), D0").unwrap_err();
        for part in ["100000", "16-bit", "extension word", "-32768..32767"] {
            assert!(err.contains(part), "{} in: {}", part, err);
        }
    }

    #[test]
    fn shift_count() {
        for code in [
            "asl.l 0, D0",
            "asl.l 31, D0",
            "lsr.b 7, D0",
            "asl.b 8, D0",
            "asl.l D1, D0",
        ] {
            accepted(code);
        }
        // An immediate count of 8 or more clears a byte.
        assert_eq!(d(&simulate("asl.b 8, D0", with_data(&[(D0, 0x10)])), D0), 0);
        let st = simulate("asl.b 9, D0", with_data(&[(D0, 0x11)]));
        assert_eq!(d(&st, D0), 0);
        assert!(!st.c_flag);
        rejected("asl.l 32, D0", "0..31");
        rejected("asr.b -1, D0", "0..31");
        for op in ["asl", "asr", "lsl", "lsr"] {
            let err = field_outcome(&format!("{}.l 100, D0", op)).unwrap_err();
            assert!(err.contains(&format!("{}.l: shift count", op)), "{} in: {}", op, err);
            assert!(err.contains("0..31"), "range in: {}", err);
        }
        // A label is not a shift count, every other immediate still takes one.
        assert!(translate("asl.l shamt, D0").is_err());
        for code in ["movea.l array, A0", "move.l array, D0", "add.l array, D0"] {
            assert!(translate(code).is_ok(), "{}", code);
        }
    }

    #[test]
    fn register_shift_count() {
        // At the width, above it, or negative (modulo 64): the datum is shifted out.
        for count in [32, 33, -1] {
            assert_eq!(
                d(&simulate("asl.l D1, D0", with_data(&[(D1, count), (D0, 1)])), D0),
                0,
                "{}",
                count
            );
        }
        // At the width the carry is the last bit of the datum, past it the carry is clear.
        assert!(simulate("asl.l D1, D0", with_data(&[(D1, 32), (D0, 1)])).c_flag);
        let st = simulate("asl.l D1, D0", with_data(&[(D1, 40), (D0, 0x11)]));
        assert_eq!(d(&st, D0), 0);
        assert!(!st.c_flag);
        // The same holds in byte mode, held at its own width.
        assert!(simulate("asl.b D1, D0", with_data(&[(D1, 8), (D0, 0x11)])).c_flag);
        assert!(!simulate("asl.b D1, D0", with_data(&[(D1, 9), (D0, 0x11)])).c_flag);
        assert_eq!(d(&simulate("asl.b D1, D0", with_data(&[(D1, 8), (D0, 0x10)])), D0), 0);
        assert_eq!(d(&simulate("asr.b D1, D0", with_data(&[(D1, -1), (D0, 0x10)])), D0), 0);
        // A count that fits is unaffected.
        assert_eq!(d(&simulate("lsl.l D1, D0", with_data(&[(D1, 2), (D0, 3)])), D0), 12);
    }

    #[test]
    fn byte_mode_immediate() {
        for code in ["move.b 255, D0", "move.b -128, D0", "add.b 0xFF, D0"] {
            accepted(code);
        }
        rejected("move.b 300, D0", "-128..255");
        rejected("move.b -129, D0", "-128..255");
        for op in ["move", "and", "or", "xor", "add", "sub", "mul", "div", "cmp"] {
            let err = field_outcome(&format!("{}.b 300, D0", op)).unwrap_err();
            assert!(err.contains(&format!("{}.b: immediate", op)), "{} in: {}", op, err);
            assert!(err.contains("-128..255"), "range in: {}", err);
        }
        // Long mode still takes a whole word; only the immediate is checked.
        for code in [
            "move.l 100000, D0",
            "move.l 0xFFFFFFFF, D0",
            "move.b D1, D0",
            "move.b (A1), D0",
        ] {
            accepted(code);
        }
    }

    #[test]
    fn read_byte_by_address_register() {
        let a2 = || with_addr(&[(AddrReg::A2, 6)]);
        let st = simulate("move.b (A2), D0", a2());
        assert_eq!((d(&st, D0), a(&st, AddrReg::A2)), (6, 6));
        let mut st = a2();
        st.data_regs[D0 as usize] = 0x10203040;
        let st = simulate("move.b (A2), D0", st);
        assert_eq!((d(&st, D0), a(&st, AddrReg::A2)), (0x10203006, 6));
        let st = simulate("move.b -(A2), D0", a2());
        assert_eq!((d(&st, D0), a(&st, AddrReg::A2)), (5, 5));
        let st = simulate("move.b (A2)+, D0", a2());
        assert_eq!((d(&st, D0), a(&st, AddrReg::A2)), (6, 7));
        let st = simulate("move.b 2(A2), D0", a2());
        assert_eq!((d(&st, D0), a(&st, AddrReg::A2)), (8, 6));
        assert_eq!(bytes(&st, &[6, 7, 8, 9]), [6, 7, 8, 9]);
        let st = simulate("move.b -2(A2), D0", a2());
        assert_eq!((d(&st, D0), a(&st, AddrReg::A2)), (4, 6));
        assert_eq!(bytes(&st, &[3, 4, 5, 6]), [3, 4, 5, 6]);
    }

    #[test]
    fn write_byte_by_address_register() {
        let a2 = || with_addr(&[(AddrReg::A2, 6)]);
        let st = simulate("move.b D2, (A2)", a2());
        assert_eq!((d(&st, D2), a(&st, AddrReg::A2)), (2, 6));
        assert_eq!(bytes(&st, &[5, 6, 7]), [5, 2, 7]);
        let mut st = a2();
        st.data_regs[D2 as usize] = 0x10203040;
        let st = simulate("move.b D2, (A2)", st);
        assert_eq!((d(&st, D2), a(&st, AddrReg::A2)), (0x10203040, 6));
        assert_eq!(bytes(&st, &[5, 6, 7]), [5, 0x40, 7]);
        let st = simulate("move.b D2, -(A2)", a2());
        assert_eq!((d(&st, D2), a(&st, AddrReg::A2)), (2, 5));
        assert_eq!(bytes(&st, &[5, 6, 7]), [2, 6, 7]);
        let st = simulate("move.b D2, (A2)+", a2());
        assert_eq!((d(&st, D2), a(&st, AddrReg::A2)), (2, 7));
        assert_eq!(bytes(&st, &[5, 6, 7]), [5, 2, 7]);
        let st = simulate("move.b D2, 2(A2)", a2());
        assert_eq!((d(&st, D2), a(&st, AddrReg::A2)), (2, 6));
        assert_eq!(bytes(&st, &[6, 7, 8, 9]), [6, 7, 2, 9]);
        let st = simulate("move.b D2, -2(A2)", a2());
        assert_eq!((d(&st, D2), a(&st, AddrReg::A2)), (2, 6));
        assert_eq!(bytes(&st, &[2, 3, 4, 5, 6]), [2, 3, 2, 5, 6]);
    }

    #[test]
    fn read_word_by_address_register() {
        let a2 = || with_addr(&[(AddrReg::A2, 6)]);
        let st = simulate("move.l (A2), D0", a2());
        assert_eq!((d(&st, D0), a(&st, AddrReg::A2)), (0x09080706, 6));
        assert_eq!(bytes(&st, &[5, 6, 7, 8, 9, 10]), [5, 6, 7, 8, 9, 10]);
        let st = simulate("move.l -(A2), D0", a2());
        assert_eq!((d(&st, D0), a(&st, AddrReg::A2)), (0x05040302, 2));
        let st = simulate("move.l (A2)+, D0", a2());
        assert_eq!((d(&st, D0), a(&st, AddrReg::A2)), (0x09080706, 10));
        let st = simulate("move.l 2(A2), D0", a2());
        assert_eq!((d(&st, D0), a(&st, AddrReg::A2)), (0x0B0A0908, 6));
        let st = simulate("move.l -4(A2), D0", a2());
        assert_eq!((d(&st, D0), a(&st, AddrReg::A2)), (0x05040302, 6));
        assert_eq!(bytes(&st, &[1, 2, 3, 4, 5, 6]), [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn write_word_by_address_register() {
        let a2 = || with_addr(&[(AddrReg::A2, 6)]);
        let st = simulate("move.l D2, (A2)", a2());
        assert_eq!((d(&st, D2), a(&st, AddrReg::A2)), (2, 6));
        assert_eq!(bytes(&st, &[5, 6, 7, 8, 9, 10]), [5, 2, 0, 0, 0, 10]);
        let mut st = a2();
        st.data_regs[D2 as usize] = 0x10203040;
        let st = simulate("move.l D2, (A2)", st);
        assert_eq!(bytes(&st, &[5, 6, 7, 8, 9, 10]), [5, 0x40, 0x30, 0x20, 0x10, 10]);
        let st = simulate("move.l D2, -(A2)", a2());
        assert_eq!((d(&st, D2), a(&st, AddrReg::A2)), (2, 2));
        assert_eq!(bytes(&st, &[1, 2, 3, 4, 5, 6]), [1, 2, 0, 0, 0, 6]);
        let st = simulate("move.l D2, (A2)+", a2());
        assert_eq!((d(&st, D2), a(&st, AddrReg::A2)), (2, 10));
        assert_eq!(bytes(&st, &[5, 6, 7, 8, 9, 10]), [5, 2, 0, 0, 0, 10]);
        let st = simulate("move.l D2, 2(A2)", a2());
        assert_eq!(bytes(&st, &[7, 8, 9, 10, 11, 12]), [7, 2, 0, 0, 0, 12]);
        let st = simulate("move.l D3, -2(A2)", a2());
        assert_eq!((d(&st, D3), a(&st, AddrReg::A2)), (3, 6));
        assert_eq!(bytes(&st, &[3, 4, 5, 6, 7, 8]), [3, 3, 0, 0, 0, 8]);
    }

    #[test]
    fn index_register_addressing() {
        let mut st = with_addr(&[(AddrReg::A2, 6)]);
        st.data_regs[D1 as usize] = 3;
        assert_eq!(d(&simulate("move.b 2(A2,D1), D0", st), D0), 11);
        let st = simulate("move.b 2(A2,A1), D0", with_addr(&[(AddrReg::A2, 6), (AddrReg::A1, 3)]));
        assert_eq!(d(&st, D0), 11);
        let mut st = with_addr(&[(AddrReg::A2, 10)]);
        st.data_regs[D1 as usize] = 2;
        assert_eq!(d(&simulate("move.l 4(A2,D1), D0", st), D0), 0x13121110);

        let mut st = with_addr(&[(AddrReg::A2, 6)]);
        st.data_regs[D1 as usize] = 3;
        st.data_regs[D0 as usize] = 0x99;
        let st = simulate("move.b D0, 2(A2,D1)", st);
        assert_eq!(bytes(&st, &[10, 11, 12]), [10, 0x99, 12]);
        let mut st = with_addr(&[(AddrReg::A2, 10), (AddrReg::A1, 2)]);
        st.data_regs[D0 as usize] = 0x7BCDEF12;
        let st = simulate("move.l D0, 4(A2,A1)", st);
        assert_eq!(bytes(&st, &[15, 16, 17, 18, 19, 20]), [15, 0x12, 0xEF, 0xCD, 0x7B, 20]);
    }

    #[test]
    fn jsr_and_rts() {
        let st = simulate("jsr 0x20", with_addr(&[(AddrReg::A7, 0x100)]));
        assert_eq!((st.pc, a(&st, AddrReg::A7)), (0x20, 0xFC));
        let st = simulate("rts", with_addr(&[(AddrReg::A7, 0x0F)]));
        assert_eq!((st.pc, a(&st, AddrReg::A7)), (0x1211100F, 0x13));
    }

    #[test]
    fn shift_carry_flag() {
        // LSR: the last bit shifted out is bit 0; ASL: bit 31.
        assert!(simulate("lsr.l D1, D0", with_data(&[(D1, 1), (D0, 3)])).c_flag);
        assert!(!simulate("lsr.l D1, D0", with_data(&[(D1, 1), (D0, 2)])).c_flag);
        assert!(simulate("asl.l D1, D0", with_data(&[(D1, 1), (D0, W::MIN)])).c_flag);
    }

    #[test]
    fn negate_and_clear() {
        let st = simulate("neg.l D0", with_data(&[(D0, 5)]));
        assert_eq!(d(&st, D0), -5);
        assert!(st.n_flag);
        let st = simulate("neg.l D0", with_data(&[(D0, 0)]));
        assert_eq!(d(&st, D0), 0);
        assert!(st.z_flag);
        let st = simulate("neg.b D0", with_data(&[(D0, 3)]));
        assert_eq!(d(&st, D0), -3);
        assert!(st.n_flag);
        let st = simulate("clr.l D0", with_data(&[(D0, 42)]));
        assert_eq!(d(&st, D0), 0);
        assert_eq!(
            (st.z_flag, st.n_flag, st.v_flag, st.c_flag),
            (true, false, false, false)
        );
        let st = simulate("clr.b D0", with_data(&[(D0, 0xFF)]));
        assert_eq!(d(&st, D0), 0);
        assert!(st.z_flag);
    }

    #[test]
    fn division_by_zero() {
        for code in ["div.l D1, D0", "div.b D1, D0"] {
            let st = simulate(code, with_data(&[(D1, 0), (D0, 10)]));
            assert_eq!(st.internal_error.as_deref(), Some("division by zero"));
        }
    }

    #[test]
    fn link_and_unlk() {
        let st = simulate("link A6, -8", with_addr(&[(AddrReg::A6, 0x20), (AddrReg::A7, 0x10)]));
        assert_eq!((a(&st, AddrReg::A6), a(&st, AddrReg::A7)), (0x0C, 0x04));
        assert_eq!(word(&st, 0x1C), 0x1F1E1D1C);
        assert_eq!(word(&st, 0x0C), 0x20);

        let mut st = with_addr(&[(AddrReg::A6, 0x0C), (AddrReg::A7, 0x14)]);
        st.mem.cells.write_word(0x1C, 0x1F1E1D1C).unwrap();
        st.mem.cells.write_word(0x0C, 0x20).unwrap();
        let st = simulate("unlk A6", st);
        assert_eq!((a(&st, AddrReg::A6), a(&st, AddrReg::A7)), (0x20, 0x10));
        assert_eq!(word(&st, 0x1C), 0x1F1E1D1C);
        assert_eq!(word(&st, 0x10), 0x13121110);
        assert_eq!(word(&st, 0x20), 0x23222120);
    }
}
