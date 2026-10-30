//! The machine's state and every primitive that touches it: where code, data
//! and the two stacks sit in memory, the counted word/byte accessors, and the
//! operand and control stacks -- the latter including the binary format of a
//! control record, since the format and the LIFO discipline that gives it
//! meaning are one thing.

use super::Wasm32Isa;
use crate::bitpack::{pack_words, run_bit_get_with, BitEnum, BitGet, BitPack, BitPut, Res};
use crate::memory::{hex_addr, Instruction, IoMem};
use crate::word::W;

/// Bytes in a machine word.
pub const WORD: i64 = 4;

/// The machine state.
#[derive(Clone)]
pub struct Wasm32St {
    /// Address of the next instruction.
    pub pc: i64,
    /// Address of the topmost operand.
    pub sp: i64,
    /// Address of local 0 of the current frame.
    pub frame_base: i64,
    /// How many locals the current frame has: its parameters plus whatever
    /// `locals.reserve` added.
    pub local_count: i64,
    /// Top of the control stack.
    pub ctrl_sp: i64,
    /// Where both stacks start. Set by `sp.init`; by default three quarters
    /// of the way into memory.
    pub stack_root: i64,
    /// Memory, including the memory-mapped I/O ports.
    pub mem: IoMem<Wasm32Isa<W>>,
    /// Set by `halt`.
    pub stopped: bool,
    /// The error that stopped the machine, if any.
    pub internal_error: Option<String>,
}

pub fn default_stack_root(mem: &IoMem<Wasm32Isa<W>>) -> i64 {
    mem.capacity().div_euclid(4) * 3
}

impl Wasm32St {
    pub fn set_pc(&mut self, addr: i64) {
        self.pc = addr;
    }

    pub fn next_pc(&mut self, instruction: &Wasm32Isa<W>) {
        self.pc += instruction.byte_size() as i64;
    }

    pub fn fetch_instruction_at(&mut self, addr: i64) -> Result<Wasm32Isa<W>, String> {
        self.mem.read_instruction(addr)
    }

    pub fn raise_internal_error(&mut self, msg: impl Into<String>) {
        self.internal_error = Some(msg.into());
    }

    fn mem_error(&mut self, err: String) {
        self.raise_internal_error(format!("memory access error: {}", err));
    }

    pub fn get_word(&mut self, addr: i64) -> W {
        self.mem.read_word(addr).unwrap_or_else(|e| {
            self.mem_error(e);
            0
        })
    }

    pub fn set_word(&mut self, addr: i64, w: W) {
        if let Err(e) = self.mem.write_word(addr, w) {
            self.mem_error(e);
        }
    }

    pub fn get_byte(&mut self, addr: i64) -> u8 {
        self.mem.read_byte(addr).unwrap_or_else(|e| {
            self.mem_error(e);
            0
        })
    }

    pub fn set_byte(&mut self, addr: i64, b: u8) {
        if let Err(e) = self.mem.write_byte(addr, b) {
            self.mem_error(e);
        }
    }

    // Operand stack

    pub fn push_value(&mut self, value: W) {
        self.sp -= WORD;
        self.set_word(self.sp, value);
    }

    pub fn pop_value(&mut self) -> W {
        let value = self.get_word(self.sp);
        self.sp += WORD;
        value
    }

    // Locals

    pub fn reserve_locals(&mut self, n: i64) {
        self.sp -= n * WORD;
        self.local_count += n;
    }

    pub fn local_addr(&self, idx: i64) -> i64 {
        self.frame_base - idx * WORD
    }

    // Control stack

    /// Addresses of the control records, innermost first.
    pub fn scope_addrs(&self) -> Vec<i64> {
        let root = self.stack_root;
        (1..)
            .map(|k| self.ctrl_sp - k * SCOPE_RECORD_BYTES)
            .take_while(|&a| a >= root)
            .collect()
    }

    pub fn push_scope(&mut self, scope: Scope) {
        match pack_words(&scope) {
            Err(err) => self.raise_internal_error(format!("control record doesn't fit: {}", err)),
            Ok(words) => {
                for (idx, w) in words.into_iter().enumerate() {
                    self.set_word(self.ctrl_sp + idx as i64 * WORD, w);
                }
                self.ctrl_sp += SCOPE_RECORD_BYTES;
            }
        }
    }

    pub fn retire_scopes(&mut self, n: i64) {
        self.ctrl_sp -= n * SCOPE_RECORD_BYTES;
    }

    pub fn scope_at_depth(&mut self, depth: usize) -> Option<Result<Scope, String>> {
        let addr = *self.scope_addrs().get(depth)?;
        Some(
            self.read_scope(addr, Scope::unpack)
                .map_err(|e| ctrl_corrupted_err(addr, &e)),
        )
    }

    /// The innermost call record and its address.
    pub fn innermost_call(&mut self) -> Result<(i64, Scope), String> {
        for addr in self.scope_addrs() {
            let call = self.read_scope(addr, |g| {
                let head = get_record_head(g)?;
                if head.0 == ControlTag::Call {
                    get_record_tail(g, head).map(Some)
                } else {
                    Ok(None)
                }
            });
            match call {
                Err(err) => return Err(ctrl_corrupted_err(addr, &err)),
                Ok(Some(scope)) => return Ok((addr, scope)),
                Ok(None) => {}
            }
        }
        Err("return without active function frame".to_string())
    }

    fn read_scope<T>(&mut self, addr: i64, decoder: impl FnOnce(&mut BitGet) -> Res<T>) -> Res<T> {
        run_bit_get_with(|k| self.get_word(addr + k as i64 * WORD), decoder)
    }
}

fn ctrl_corrupted_err(addr: i64, err: &str) -> String {
    format!("control stack corrupted at {}: {}", hex_addr(2, addr), err)
}

/// An open scope, as a control record keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Loop {
        start: i64,
        end: i64,
    },
    Block {
        end: i64,
    },
    Call {
        caller_frame_base: i64,
        caller_local_count: i64,
        return_pc: i64,
        result_count: i64,
    },
}

impl Scope {
    /// Haskell's derived `show`.
    pub fn show(&self) -> String {
        match self {
            Scope::Loop { start, end } => format!("LoopScope {{csStart = {}, csEnd = {}}}", start, end),
            Scope::Block { end } => format!("BlockScope {{csEnd = {}}}", end),
            Scope::Call {
                caller_frame_base,
                caller_local_count,
                return_pc,
                result_count,
            } => format!(
                "CallScope {{csCallerFrameBase = {}, csCallerLocalCount = {}, csReturnPc = {}, csResultCount = {}}}",
                caller_frame_base, caller_local_count, return_pc, result_count
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlTag {
    Loop,
    Block,
    Call,
}

impl BitEnum for ControlTag {
    const ALL: &'static [ControlTag] = &[ControlTag::Loop, ControlTag::Block, ControlTag::Call];
}

impl BitPack for ControlTag {
    fn unpack(g: &mut BitGet) -> Res<Self> {
        g.label("record kind", |g| g.get_enum(TAG_BITS))
    }

    fn pack(&self, p: &mut BitPut) -> Res<()> {
        p.label("record kind", |p| p.put_enum(TAG_BITS, *self))
    }
}

const SCOPE_RECORD_WORDS: i64 = 2;
pub const SCOPE_RECORD_BYTES: i64 = SCOPE_RECORD_WORDS * WORD;

/// Addresses of the words of the record at `addr`.
pub fn scope_word_addrs(addr: i64) -> Vec<i64> {
    (0..SCOPE_RECORD_WORDS).map(|idx| addr + idx * WORD).collect()
}

const TAG_BITS: u32 = 2;
const ADDR_BITS: u32 = 22;
const COUNT_BITS: u32 = 8;

fn put_named(p: &mut BitPut, name: &str, n: u32, value: i64) -> Res<()> {
    p.label(name, |p| p.put_field(n, value))
}

impl BitPack for Scope {
    fn unpack(g: &mut BitGet) -> Res<Self> {
        let head = get_record_head(g)?;
        get_record_tail(g, head)
    }

    fn pack(&self, p: &mut BitPut) -> Res<()> {
        match *self {
            Scope::Block { end } => {
                p.word(|p| {
                    ControlTag::Block.pack(p)?;
                    p.put_pad(COUNT_BITS)?;
                    put_named(p, "csEnd", ADDR_BITS, end)
                })?;
                p.word(|p| p.put_pad(32))
            }
            Scope::Loop { start, end } => {
                p.word(|p| {
                    ControlTag::Loop.pack(p)?;
                    p.put_pad(COUNT_BITS)?;
                    put_named(p, "csStart", ADDR_BITS, start)
                })?;
                p.word(|p| {
                    p.put_pad(10)?;
                    put_named(p, "csEnd", ADDR_BITS, end)
                })
            }
            Scope::Call {
                caller_frame_base,
                caller_local_count,
                return_pc,
                result_count,
            } => {
                p.word(|p| {
                    ControlTag::Call.pack(p)?;
                    put_named(p, "csCallerLocalCount", COUNT_BITS, caller_local_count)?;
                    put_named(p, "csCallerFrameBase", ADDR_BITS, caller_frame_base)
                })?;
                p.word(|p| {
                    p.put_pad(2)?;
                    put_named(p, "csResultCount", COUNT_BITS, result_count)?;
                    put_named(p, "csReturnPc", ADDR_BITS, return_pc)
                })
            }
        }
    }
}

/// The first word of a record: its kind, a count and an address.
fn get_record_head(g: &mut BitGet) -> Res<(ControlTag, i64, i64)> {
    g.word(|g| {
        let tag = ControlTag::unpack(g)?;
        let count = g.get_field(COUNT_BITS)?;
        let addr = g.get_field(ADDR_BITS)?;
        Ok((tag, count, addr))
    })
}

fn get_record_tail(g: &mut BitGet, (tag, count, addr): (ControlTag, i64, i64)) -> Res<Scope> {
    match tag {
        ControlTag::Block => {
            g.word(|g| g.get_pad(32))?;
            Ok(Scope::Block { end: addr })
        }
        ControlTag::Loop => {
            let end = g.word(|g| {
                g.get_pad(10)?;
                g.get_field(ADDR_BITS)
            })?;
            Ok(Scope::Loop { start: addr, end })
        }
        ControlTag::Call => {
            let (result_count, return_pc) = g.word(|g| {
                g.get_pad(2)?;
                Ok((g.get_field(COUNT_BITS)?, g.get_field(ADDR_BITS)?))
            })?;
            Ok(Scope::Call {
                caller_frame_base: addr,
                caller_local_count: count,
                return_pc,
                result_count,
            })
        }
    }
}
