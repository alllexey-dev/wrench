//! Wasm32: a 32-bit stack machine for teaching, based on the `i32` subset of
//! WebAssembly (<https://webassembly.github.io/spec/core/>). There are no
//! general-purpose registers: instructions pop operands from a stack and push
//! results. Control flow is structured (`block`, `loop`, `if`, each closed by
//! `end`), and functions are called by address.

mod execute;
mod local_names;
mod report;
mod state;

pub use local_names::resolve_local_names;
pub use state::{Scope, Wasm32St};

use crate::hs::show_arg;
use crate::machine::Arch;
use crate::memory::Instruction;
use crate::parser::{Ref, P, R};
use crate::translator::ParsedInstr;
use crate::word::{u8, W};

/// One instruction; the immediates are `L`: references when parsed, words
/// once resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wasm32Isa<L> {
    /// `i32.const <value: i32> ( -- value: i32 )`
    I32Const(L),
    I32Add,
    I32Sub,
    I32Mul,
    I32DivS,
    I32RemS,
    I32And,
    I32Or,
    I32Xor,
    I32Shl,
    I32ShrS,
    I32ShrU,
    I32Eqz,
    I32Eq,
    I32LtS,
    I32LeS,
    I32GtS,
    I32GeS,
    I32LtU,
    I32LeU,
    I32GtU,
    I32GeU,
    /// `i32.load <offset: u8> ( address: i32 -- value: i32 )`
    I32Load(L),
    I32Load8U(L),
    I32Load8S(L),
    /// `i32.store <offset: u8> ( address: i32, value: i32 -- )`
    I32Store(L),
    I32Store8(L),
    Block,
    Loop,
    If,
    Else,
    End,
    /// `br <depth: u8> ( -- )`
    Br(L),
    BrIf(L),
    Unreachable,
    /// `call <paramCount: u8>, <resultCount: u8> ( ...params, target -- ...results )`
    Call {
        param_count: L,
        result_count: L,
    },
    Return,
    LocalGet(L),
    LocalSet(L),
    LocalTee(L),
    /// `locals.reserve <count: u8> ( -- )`
    LocalsReserve(L),
    Drop,
    Select,
    /// `sp.init ( address: i32 -- )`
    SpInit,
    Halt,
}

use Wasm32Isa as I;

/// Every mnemonic the parser accepts.
pub const MNEMONICS: &[&str] = &[
    "i32.const",
    "i32.add",
    "i32.sub",
    "i32.mul",
    "i32.div_s",
    "i32.rem_s",
    "i32.and",
    "i32.or",
    "i32.xor",
    "i32.shl",
    "i32.shr_s",
    "i32.shr_u",
    "i32.eqz",
    "i32.eq",
    "i32.lt_s",
    "i32.le_s",
    "i32.gt_s",
    "i32.ge_s",
    "i32.lt_u",
    "i32.le_u",
    "i32.gt_u",
    "i32.ge_u",
    "i32.load8_u",
    "i32.load8_s",
    "i32.load",
    "i32.store8",
    "i32.store",
    "if",
    "else",
    "end",
    "loop",
    "block",
    "br",
    "br_if",
    "drop",
    "select",
    "unreachable",
    "local.get",
    "local.set",
    "local.tee",
    "locals.reserve",
    "call",
    "return",
    "sp.init",
    "halt",
];

impl<L> Wasm32Isa<L> {
    /// The constructor name, as instructions are shown.
    fn name(&self) -> &'static str {
        match self {
            I::I32Const(_) => "I32Const",
            I::I32Add => "I32Add",
            I::I32Sub => "I32Sub",
            I::I32Mul => "I32Mul",
            I::I32DivS => "I32DivS",
            I::I32RemS => "I32RemS",
            I::I32And => "I32And",
            I::I32Or => "I32Or",
            I::I32Xor => "I32Xor",
            I::I32Shl => "I32Shl",
            I::I32ShrS => "I32ShrS",
            I::I32ShrU => "I32ShrU",
            I::I32Eqz => "I32Eqz",
            I::I32Eq => "I32Eq",
            I::I32LtS => "I32LtS",
            I::I32LeS => "I32LeS",
            I::I32GtS => "I32GtS",
            I::I32GeS => "I32GeS",
            I::I32LtU => "I32LtU",
            I::I32LeU => "I32LeU",
            I::I32GtU => "I32GtU",
            I::I32GeU => "I32GeU",
            I::I32Load(_) => "I32Load",
            I::I32Load8U(_) => "I32Load8U",
            I::I32Load8S(_) => "I32Load8S",
            I::I32Store(_) => "I32Store",
            I::I32Store8(_) => "I32Store8",
            I::Block => "Block",
            I::Loop => "Loop",
            I::If => "If",
            I::Else => "Else",
            I::End => "End",
            I::Br(_) => "Br",
            I::BrIf(_) => "BrIf",
            I::Unreachable => "Unreachable",
            I::Call { .. } => "Call",
            I::Return => "Return",
            I::LocalGet(_) => "LocalGet",
            I::LocalSet(_) => "LocalSet",
            I::LocalTee(_) => "LocalTee",
            I::LocalsReserve(_) => "LocalsReserve",
            I::Drop => "Drop",
            I::Select => "Select",
            I::SpInit => "SpInit",
            I::Halt => "Halt",
        }
    }

    /// The immediate and, for a byte field, what it is called in errors.
    fn immediate(&self) -> Option<(&L, Option<&'static str>)> {
        match self {
            I::I32Const(k) => Some((k, None)),
            I::I32Load(k) => Some((k, Some("i32.load offset"))),
            I::I32Store(k) => Some((k, Some("i32.store offset"))),
            I::I32Load8U(k) => Some((k, Some("i32.load8_u offset"))),
            I::I32Load8S(k) => Some((k, Some("i32.load8_s offset"))),
            I::I32Store8(k) => Some((k, Some("i32.store8 offset"))),
            I::Br(k) => Some((k, Some("br depth"))),
            I::BrIf(k) => Some((k, Some("br_if depth"))),
            I::LocalGet(k) => Some((k, Some("local.get index"))),
            I::LocalSet(k) => Some((k, Some("local.set index"))),
            I::LocalTee(k) => Some((k, Some("local.tee index"))),
            I::LocalsReserve(k) => Some((k, Some("locals.reserve count"))),
            _ => None,
        }
    }

    /// The same instruction with every immediate mapped by `f`, given the
    /// field's error name (`None` for a full word).
    fn try_map<M>(
        &self,
        mut f: impl FnMut(&L, Option<&'static str>) -> Result<M, String>,
    ) -> Result<Wasm32Isa<M>, String> {
        let mut one = |k: &L| f(k, self.immediate().and_then(|(_, what)| what));
        Ok(match self {
            I::I32Const(k) => I::I32Const(one(k)?),
            I::I32Load(k) => I::I32Load(one(k)?),
            I::I32Store(k) => I::I32Store(one(k)?),
            I::I32Load8U(k) => I::I32Load8U(one(k)?),
            I::I32Load8S(k) => I::I32Load8S(one(k)?),
            I::I32Store8(k) => I::I32Store8(one(k)?),
            I::Br(k) => I::Br(one(k)?),
            I::BrIf(k) => I::BrIf(one(k)?),
            I::LocalGet(k) => I::LocalGet(one(k)?),
            I::LocalSet(k) => I::LocalSet(one(k)?),
            I::LocalTee(k) => I::LocalTee(one(k)?),
            I::LocalsReserve(k) => I::LocalsReserve(one(k)?),
            I::Call {
                param_count,
                result_count,
            } => I::Call {
                param_count: f(param_count, Some("call paramCount"))?,
                result_count: f(result_count, Some("call resultCount"))?,
            },
            I::I32Add => I::I32Add,
            I::I32Sub => I::I32Sub,
            I::I32Mul => I::I32Mul,
            I::I32DivS => I::I32DivS,
            I::I32RemS => I::I32RemS,
            I::I32And => I::I32And,
            I::I32Or => I::I32Or,
            I::I32Xor => I::I32Xor,
            I::I32Shl => I::I32Shl,
            I::I32ShrS => I::I32ShrS,
            I::I32ShrU => I::I32ShrU,
            I::I32Eqz => I::I32Eqz,
            I::I32Eq => I::I32Eq,
            I::I32LtS => I::I32LtS,
            I::I32LeS => I::I32LeS,
            I::I32GtS => I::I32GtS,
            I::I32GeS => I::I32GeS,
            I::I32LtU => I::I32LtU,
            I::I32LeU => I::I32LeU,
            I::I32GtU => I::I32GtU,
            I::I32GeU => I::I32GeU,
            I::Block => I::Block,
            I::Loop => I::Loop,
            I::If => I::If,
            I::Else => I::Else,
            I::End => I::End,
            I::Unreachable => I::Unreachable,
            I::Return => I::Return,
            I::Drop => I::Drop,
            I::Select => I::Select,
            I::SpInit => I::SpInit,
            I::Halt => I::Halt,
        })
    }

    /// One opcode byte, one per byte immediate, four for `i32.const`.
    fn size(&self) -> usize {
        1 + match self {
            I::I32Const(_) => 4,
            I::Call { .. } => 2,
            _ if self.immediate().is_some() => 1,
            _ => 0,
        }
    }
}

impl ParsedInstr for Wasm32Isa<Ref> {
    fn byte_size(&self) -> usize {
        self.size()
    }
}

impl Instruction for Wasm32Isa<W> {
    fn byte_size(&self) -> usize {
        self.size()
    }

    fn show(&self) -> String {
        match self {
            I::Call {
                param_count,
                result_count,
            } => format!(
                "Call {{callParamCount = {}, callResultCount = {}}}",
                param_count, result_count
            ),
            _ => match self.immediate() {
                Some((k, _)) => format!("{} {}", self.name(), show_arg(*k)),
                None => self.name().to_string(),
            },
        }
    }
}

type Parsed = Wasm32Isa<Ref>;

/// A mnemonic with nothing after it.
fn cmd0(p: &mut P, op: &str, constructor: Parsed) -> R<Parsed> {
    p.string(op)?;
    Ok(constructor)
}

/// A mnemonic and, after white space, its immediate.
fn cmd1(p: &mut P, op: &str, arg: fn(&mut P) -> R<Ref>, constructor: fn(Ref) -> Parsed) -> R<Parsed> {
    p.attempt(|p| {
        p.string(op)?;
        p.hspace1()
    })?;
    arg(p).map(constructor)
}

/// A load or store, with an optional numeric offset (default 0).
fn mem_op(p: &mut P, op: &str, constructor: fn(Ref) -> Parsed) -> R<Parsed> {
    let pos = p.source_pos();
    p.string(op)?;
    let offset = p.optional(|p| {
        p.attempt(|p| {
            p.hspace1()?;
            match p.peek() {
                Some(c) if c.is_ascii_digit() => Ok(()),
                _ => p.expected("digit"),
            }
        })?;
        p.constant_ref()
    })?;
    Ok(constructor(offset.unwrap_or_else(|| Ref::value(pos, 0))))
}

fn call(p: &mut P) -> R<Parsed> {
    p.attempt(|p| {
        p.string("call")?;
        p.hspace1()
    })?;
    let param_count = p.constant_ref()?;
    p.comma()?;
    let result_count = p.constant_ref()?;
    Ok(I::Call {
        param_count,
        result_count,
    })
}

fn imm(p: &mut P) -> R<Ref> {
    p.reference_with_directive()
}

fn cnst(p: &mut P) -> R<Ref> {
    p.constant_ref()
}

fn command(p: &mut P) -> R<Parsed> {
    p.choice(&[
        &|p| cmd1(p, "i32.const", imm, I::I32Const),
        &|p| cmd0(p, "i32.add", I::I32Add),
        &|p| cmd0(p, "i32.sub", I::I32Sub),
        &|p| cmd0(p, "i32.mul", I::I32Mul),
        &|p| cmd0(p, "i32.div_s", I::I32DivS),
        &|p| cmd0(p, "i32.rem_s", I::I32RemS),
        &|p| cmd0(p, "i32.and", I::I32And),
        &|p| cmd0(p, "i32.or", I::I32Or),
        &|p| cmd0(p, "i32.xor", I::I32Xor),
        &|p| cmd0(p, "i32.shl", I::I32Shl),
        &|p| cmd0(p, "i32.shr_s", I::I32ShrS),
        &|p| cmd0(p, "i32.shr_u", I::I32ShrU),
        &|p| cmd0(p, "i32.eqz", I::I32Eqz),
        &|p| cmd0(p, "i32.eq", I::I32Eq),
        &|p| cmd0(p, "i32.lt_s", I::I32LtS),
        &|p| cmd0(p, "i32.le_s", I::I32LeS),
        &|p| cmd0(p, "i32.gt_s", I::I32GtS),
        &|p| cmd0(p, "i32.ge_s", I::I32GeS),
        &|p| cmd0(p, "i32.lt_u", I::I32LtU),
        &|p| cmd0(p, "i32.le_u", I::I32LeU),
        &|p| cmd0(p, "i32.gt_u", I::I32GtU),
        &|p| cmd0(p, "i32.ge_u", I::I32GeU),
        &|p| mem_op(p, "i32.load8_u", I::I32Load8U),
        &|p| mem_op(p, "i32.load8_s", I::I32Load8S),
        &|p| mem_op(p, "i32.load", I::I32Load),
        &|p| mem_op(p, "i32.store8", I::I32Store8),
        &|p| mem_op(p, "i32.store", I::I32Store),
        &|p| cmd0(p, "if", I::If),
        &|p| cmd0(p, "else", I::Else),
        &|p| cmd0(p, "end", I::End),
        &|p| cmd0(p, "loop", I::Loop),
        &|p| cmd0(p, "block", I::Block),
        &|p| cmd1(p, "br", cnst, I::Br),
        &|p| cmd1(p, "br_if", cnst, I::BrIf),
        &|p| cmd0(p, "drop", I::Drop),
        &|p| cmd0(p, "select", I::Select),
        &|p| cmd0(p, "unreachable", I::Unreachable),
        &|p| cmd1(p, "local.get", cnst, I::LocalGet),
        &|p| cmd1(p, "local.set", cnst, I::LocalSet),
        &|p| cmd1(p, "local.tee", cnst, I::LocalTee),
        &|p| cmd1(p, "locals.reserve", cnst, I::LocalsReserve),
        &call,
        &|p| cmd0(p, "return", I::Return),
        &|p| cmd0(p, "sp.init", I::SpInit),
        &|p| cmd0(p, "halt", I::Halt),
    ])
}

pub struct Wasm32;

impl Arch for Wasm32 {
    type Parsed = Parsed;
    type State = Wasm32St;
    const NAME: &'static str = "wasm32";
    const COMMENT: &'static str = ";";

    fn preprocess(src: &str) -> Result<String, (usize, String)> {
        resolve_local_names(src)
    }

    /// One instruction; several may share a line.
    fn mnemonic(p: &mut P) -> R<Parsed> {
        p.hspace()?;
        let m = command(p)?;
        p.or(|p| p.hspace1(), |p| p.eol_(Self::COMMENT))?;
        Ok(m)
    }

    fn deref(i: &Parsed, _offset: W, f: &dyn Fn(&str) -> Option<W>) -> Result<Wasm32Isa<W>, String> {
        i.try_map(|r, what| match what {
            None => r.resolve(Ok, f),
            Some(what) => r.resolve(|x| u8(|v| format!("{} {} is not in 0..255", what, v), x), f),
        })
    }
}

#[cfg(test)]
mod tests;
