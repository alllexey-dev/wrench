//! The interface every ISA implements, and the simulation loop that drives it.

use crate::memory::{Instruction, IoMem};
use crate::parser::{P, R};
use crate::report::view::{Stat, View};
use crate::rng::RandomStream;
use crate::translator::ParsedInstr;
use crate::word::W;

/// The error an ISA's fetch reports once the machine has stopped normally.
pub const HALTED: &str = "halted";

/// A machine state of one ISA.
pub trait Machine: Clone + 'static {
    type Instr: Instruction;

    fn init(pc: i64, mem: IoMem<Self::Instr>, randoms: RandomStream) -> Self;

    /// Fetch the next instruction; [`HALTED`] once stopped.
    fn fetch(&mut self) -> Result<(i64, Self::Instr), String>;

    fn execute(&mut self, pc: i64, instr: Self::Instr);

    fn step(&mut self) {
        match self.fetch() {
            Ok((pc, i)) => self.execute(pc, i),
            Err(e) => panic!("internal error: {}", e),
        }
    }

    fn pc(&self) -> i64;
    fn mem(&self) -> &IoMem<Self::Instr>;
    fn is_halted(&self) -> bool;

    /// The ISA's own report views: registers, flags, stacks.
    fn views() -> Vec<View<Self>>;

    /// Run-wide stats, listed by `{isa-specific}`.
    fn stats() -> Vec<Stat<Self>> {
        Vec::new()
    }
}

/// An ISA: its assembler syntax and its machine.
pub trait Arch: 'static {
    type Parsed: ParsedInstr;
    type State: Machine;

    /// `--isa` name.
    const NAME: &'static str;
    const COMMENT: &'static str;

    /// Rewrite the source before parsing, keeping every character at its
    /// offset; fails with the offset of what it rejects.
    fn preprocess(src: &str) -> Result<String, (usize, String)> {
        Ok(src.to_string())
    }

    /// One instruction line, up to and including its end of line.
    fn mnemonic(p: &mut P) -> R<Self::Parsed>;

    /// Resolve the references of an instruction placed at `offset`.
    fn deref(m: &Self::Parsed, offset: W, labels: &dyn Fn(&str) -> Option<W>) -> Result<Instr<Self>, String>;
}

pub type Instr<A> = <<A as Arch>::State as Machine>::Instr;

pub enum Trace<S: Machine> {
    /// The state after `count` instructions; `prev` produced it.
    State {
        count: usize,
        prev: Option<S::Instr>,
        state: S,
    },
    Error(String),
    Warn(String),
}

/// Run until halt, error or the instruction limit. Keeps at most
/// `log_limit` states: past it, the older states give way to a warning.
pub fn power_on<S: Machine>(limit: usize, log_limit: usize, mut st: S) -> (Vec<Trace<S>>, S) {
    let mut log: Vec<Trace<S>> = Vec::new();
    let mut records = 0usize;
    let mut count = 0usize;
    let mut prev: Option<S::Instr> = None;
    let keep = log_limit / 4;

    let mut tell_state = |log: &mut Vec<Trace<S>>, prev: Option<S::Instr>, state: &S, count: usize| {
        if records >= log_limit {
            let split = log.len().saturating_sub(keep);
            let newest = log.split_off(split);
            let older = std::mem::take(log);
            let before = older.len();
            let kept: Vec<Trace<S>> = older
                .into_iter()
                .filter(|t| !matches!(t, Trace::State { .. }))
                .collect();
            let dropped = before - kept.len();
            log.push(Trace::Warn(format!("Dropped {} states", dropped)));
            log.extend(kept);
            log.extend(newest);
            records -= dropped;
        } else {
            log.push(Trace::State {
                count,
                prev,
                state: state.clone(),
            });
            records += 1;
        }
    };

    loop {
        if count >= limit {
            log.push(Trace::Error("Simulation limit reached".to_string()));
            break;
        }
        match st.clone().fetch() {
            Ok((_, instr)) => {
                tell_state(&mut log, prev.take(), &st, count);
                st.step();
                count += 1;
                prev = Some(instr);
            }
            Err(e) if e == HALTED => {
                tell_state(&mut log, prev.take(), &st, count);
                break;
            }
            Err(e) => {
                log.push(Trace::Error(e));
                break;
            }
        }
    }
    (log, st)
}
