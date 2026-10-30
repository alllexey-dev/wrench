//! Byte-addressed memory holding data bytes and instructions, the
//! memory-mapped IO layer over it, and the memory dump rendering.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use crate::intervals::Intervals;
use crate::word::{word_combine, word_split, W};

/// What sits in one byte of memory.
#[derive(Clone, Debug)]
pub enum Cell<I> {
    Instruction(I),
    InstructionPart,
    Value(u8),
}

/// An instruction as the simulator sees it: its size and how it is shown.
pub trait Instruction: Clone + 'static {
    fn byte_size(&self) -> usize;
    /// The instruction as in reports and dumps.
    fn show(&self) -> String;
}

/// Plain memory: `size` addressable bytes.
#[derive(Clone)]
pub struct Mem<I: Clone> {
    pub size: i64,
    pub data: imbl::Vector<Cell<I>>,
}

pub fn word8_to_hex(b: u8) -> String {
    format!("{:02x}", b)
}

pub fn word32_to_hex(w: W) -> String {
    format!("0x{:08x}", w as u32)
}

/// Hex digits an address into memory of this capacity could need.
pub fn hex_addr_width(capacity: i64) -> usize {
    format!("{:x}", (capacity - 1).max(0)).len().max(1)
}

/// An address padded to `width` hex digits, `-0x..` when negative.
pub fn hex_addr(width: usize, idx: i64) -> String {
    let hex = format!("{:x}", idx.unsigned_abs());
    let pad = "0".repeat(width.saturating_sub(hex.len()));
    if idx < 0 {
        format!("-0x{}{}", pad, hex)
    } else {
        format!("0x{}{}", pad, hex)
    }
}

fn err_addr(capacity: i64, idx: i64) -> String {
    hex_addr(hex_addr_width(capacity), idx)
}

impl<I: Instruction> Mem<I> {
    pub fn cell(&self, idx: i64) -> Option<&Cell<I>> {
        if idx < 0 {
            None
        } else {
            self.data.get(idx as usize)
        }
    }

    pub fn read_instruction(&self, idx: i64) -> Result<I, String> {
        let at = || format!("memory[{}]", err_addr(self.size, idx));
        match self.cell(idx) {
            Some(Cell::Instruction(i)) => {
                let n = i.byte_size() as i64;
                if (idx + 1..idx + n).all(|a| matches!(self.cell(a), Some(Cell::InstructionPart))) {
                    Ok(i.clone())
                } else {
                    Err(format!("{}: instruction in memory corrupted", at()))
                }
            }
            Some(Cell::InstructionPart) => Err(format!("{}: instruction in memory corrupted", at())),
            Some(Cell::Value(_)) => Err(format!("{}: can't read instruction from data cell", at())),
            None => Err(format!("{}: out of memory", at())),
        }
    }

    pub fn read_byte(&self, idx: i64) -> Result<u8, String> {
        match self.cell(idx) {
            Some(Cell::Value(v)) => Ok(*v),
            Some(_) => Err(format!(
                "memory[{}]: can't read byte from instruction cell",
                err_addr(self.size, idx)
            )),
            None => Err(format!("memory[{}]: out of memory", err_addr(self.size, idx))),
        }
    }

    pub fn read_word(&self, idx: i64) -> Result<W, String> {
        let mut bytes = [0u8; 4];
        let mut errs = String::new();
        for (k, b) in bytes.iter_mut().enumerate() {
            match self.read_byte(idx + k as i64) {
                Ok(v) => *b = v,
                Err(e) => {
                    errs.push_str(&e);
                    errs.push('\n');
                }
            }
        }
        if errs.is_empty() {
            Ok(word_combine(bytes))
        } else {
            Err(errs)
        }
    }

    pub fn write_word(&mut self, idx: i64, w: W) -> Result<(), String> {
        if idx < 0 || self.size < idx + 4 {
            return Err(format!(
                "memory[{}]: out of memory for word access",
                err_addr(self.size, idx)
            ));
        }
        for (k, b) in word_split(w).into_iter().enumerate() {
            self.write_byte(idx + k as i64, b)?;
        }
        Ok(())
    }

    pub fn write_byte(&mut self, idx: i64, b: u8) -> Result<(), String> {
        if idx < 0 || self.size <= idx {
            return Err(format!("memory[{}]: out of memory", err_addr(self.size, idx)));
        }
        let i = idx as usize;
        if i < self.data.len() {
            self.data.set(i, Cell::Value(b));
        }
        Ok(())
    }

    /// Every cell with its address, ascending.
    pub fn cells(&self) -> Vec<(i64, Cell<I>)> {
        self.data
            .iter()
            .cloned()
            .enumerate()
            .map(|(a, c)| (a as i64, c))
            .collect()
    }

    /// The cells at these addresses; an address outside memory is an error.
    pub fn slice(&self, from: i64, to: i64) -> Result<Vec<(i64, Cell<I>)>, String> {
        (from..=to)
            .map(|a| match self.cell(a) {
                Some(c) => Ok((a, c.clone())),
                None => Err(format!("memory[{}]: outside the configured memory", a)),
            })
            .collect()
    }
}

/// Reads and writes served, one per operation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccessCount {
    pub reads: i64,
    pub writes: i64,
}

impl AccessCount {
    pub fn total(&self) -> i64 {
        self.reads + self.writes
    }

    /// What happened between `before` and `self`.
    pub fn since(&self, before: &AccessCount) -> AccessCount {
        AccessCount {
            reads: self.reads - before.reads,
            writes: self.writes - before.writes,
        }
    }
}

/// Address ranges touched at runtime.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccessLog {
    pub instr: Intervals,
    pub data: Intervals,
    pub io: Intervals,
}

/// One memory-mapped port: what is left to read, and what was written, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IoStream {
    pub input: imbl::Vector<W>,
    pub output: imbl::Vector<W>,
}

/// Memory with memory-mapped IO ports, one machine word each.
#[derive(Clone)]
pub struct IoMem<I: Clone> {
    pub streams: BTreeMap<i64, IoStream>,
    pub cells: Mem<I>,
    pub keys: Rc<Vec<i64>>,
    byte_to_word: Rc<HashMap<i64, i64>>,
    pub access_log: AccessLog,
    pub access_count: AccessCount,
}

const WORD: i64 = 4;

impl<I: Instruction> IoMem<I> {
    pub fn new(streams: BTreeMap<i64, IoStream>, cells: Mem<I>) -> Self {
        let keys: Vec<i64> = streams.keys().copied().collect();
        let byte_to_word = keys.iter().flat_map(|&k| (k..k + WORD).map(move |b| (b, k))).collect();
        IoMem {
            streams,
            cells,
            keys: Rc::new(keys),
            byte_to_word: Rc::new(byte_to_word),
            access_log: AccessLog::default(),
            access_count: AccessCount::default(),
        }
    }

    pub fn capacity(&self) -> i64 {
        self.cells.size
    }

    fn io_err(&self, idx: i64, what: &str) -> String {
        format!("iomemory[{}]: {}", err_addr(self.capacity(), idx), what)
    }

    fn instruction_collision(&self, addr: i64, n: i64) -> bool {
        self.keys
            .iter()
            .any(|&idx| (idx - n < addr && addr < idx) || (idx < addr && addr < idx + WORD))
    }

    fn word_collision(&self, addr: i64) -> bool {
        self.instruction_collision(addr, WORD)
    }

    fn byte_collision(&self, addr: i64) -> bool {
        self.keys.iter().any(|&idx| idx < addr && addr < idx + WORD)
    }

    fn count(&mut self, write: bool) {
        if write {
            self.access_count.writes += 1;
        } else {
            self.access_count.reads += 1;
        }
    }

    fn record_data(&mut self, write: bool, addr: i64, len: i64) {
        self.access_log.data.record(addr, len);
        self.count(write);
    }

    fn record_io(&mut self, write: bool, addr: i64, len: i64) {
        self.access_log.io.record(addr, len);
        self.count(write);
    }

    pub fn read_instruction(&mut self, idx: i64) -> Result<I, String> {
        if self.streams.contains_key(&idx) {
            return Err(self.io_err(idx, "instruction in memory corrupted"));
        }
        let instr = self.cells.read_instruction(idx)?;
        let n = instr.byte_size() as i64;
        if self.instruction_collision(idx, n) {
            return Err(self.io_err(idx, "instruction in memory corrupted"));
        }
        self.access_log.instr.record(idx, n);
        self.count(false);
        Ok(instr)
    }

    /// The instruction at `idx` without recording an access.
    pub fn peek_instruction(&self, idx: i64) -> Result<I, String> {
        self.clone().read_instruction(idx)
    }

    pub fn read_byte(&mut self, idx: i64) -> Result<u8, String> {
        if let Some(&word_idx) = self.byte_to_word.get(&idx) {
            let w = self.read_word(word_idx)?;
            return Ok(word_split(w)[(idx - word_idx) as usize]);
        }
        let v = self.cells.read_byte(idx)?;
        self.record_data(false, idx, 1);
        Ok(v)
    }

    pub fn read_word(&mut self, idx: i64) -> Result<W, String> {
        if self.word_collision(idx) {
            return Err(self.io_err(idx, "can't read word from input port"));
        }
        match self.streams.get_mut(&idx) {
            Some(stream) => match stream.input.pop_front() {
                None => Err(self.io_err(idx, "input is depleted")),
                Some(w) => {
                    self.record_io(false, idx, WORD);
                    Ok(w)
                }
            },
            None => {
                let w = self.cells.read_word(idx)?;
                self.record_data(false, idx, WORD);
                Ok(w)
            }
        }
    }

    pub fn write_word(&mut self, idx: i64, w: W) -> Result<(), String> {
        if self.word_collision(idx) {
            return Err(self.io_err(idx, "can't write word to input port"));
        }
        match self.streams.get_mut(&idx) {
            Some(stream) => {
                stream.output.push_back(w);
                self.record_io(true, idx, WORD);
            }
            None => {
                self.cells.write_word(idx, w)?;
                self.record_data(true, idx, WORD);
            }
        }
        Ok(())
    }

    pub fn write_byte(&mut self, idx: i64, b: u8) -> Result<(), String> {
        if self.byte_collision(idx) {
            return Err(self.io_err(idx, "can't write byte to input port"));
        }
        match self.streams.get_mut(&idx) {
            Some(stream) => {
                stream.output.push_back(b as W);
                self.record_io(true, idx, 1);
            }
            None => {
                self.cells.write_byte(idx, b)?;
                self.record_data(true, idx, 1);
            }
        }
        Ok(())
    }
}

pub type Labels = HashMap<String, W>;

/// Labels by address; of several at one address the alphabetically first.
pub fn offset_to_label(labels: &Labels) -> HashMap<i64, String> {
    let mut m: HashMap<i64, String> = HashMap::new();
    for (l, &a) in labels {
        let e = m.entry(a as i64).or_insert_with(|| l.clone());
        if l < e {
            *e = l.clone();
        }
    }
    m
}

/// `addr:\tlabel` lines, by address.
pub fn pretty_labels(show_addr: &dyn Fn(i64) -> String, labels: &Labels) -> String {
    let mut pairs: Vec<(&String, &W)> = labels.iter().collect();
    pairs.sort_by(|a, b| a.1.cmp(b.1).then(a.0.cmp(b.0)));
    pairs
        .iter()
        .map(|(l, &w)| format!("{}:\t{}", show_addr(w as i64), l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The memory dump: instructions one per line, data runs merged by label.
pub fn pretty_dump<I: Instruction>(
    show_size: bool,
    show_addr: &dyn Fn(i64) -> String,
    labels: &Labels,
    cells: &[(i64, Cell<I>)],
) -> String {
    let size_suffix = |n: usize| {
        if show_size {
            format!(" ({} B)", n)
        } else {
            String::new()
        }
    };
    let offset2label = offset_to_label(labels);
    let mut out: Vec<String> = Vec::new();
    let mut k = 0;
    while k < cells.len() {
        let (offset, cell) = &cells[k];
        match cell {
            Cell::Instruction(i) => {
                let n = i.byte_size();
                let place = format!(
                    "mem[{}..{}]{}",
                    show_addr(*offset),
                    show_addr(offset + n as i64 - 1),
                    size_suffix(n)
                );
                let label = offset2label
                    .get(offset)
                    .map(|l| format!(" \t@{}", l))
                    .unwrap_or_default();
                out.push(format!("{}: \t{}{}", place, i.show(), label));
                k += n.max(1);
            }
            Cell::InstructionPart => {
                out.push(format!("{}: \tInstructionPart", show_addr(*offset)));
                k += 1;
            }
            Cell::Value(_) => {
                let mut values: Vec<(i64, u8)> = Vec::new();
                while k < cells.len() {
                    if let (a, Cell::Value(v)) = &cells[k] {
                        values.push((*a, *v));
                        k += 1;
                    } else {
                        break;
                    }
                }
                // Each byte carries the label of the last labelled address before it.
                let mut marked: Vec<(i64, Option<&String>, u8)> = Vec::new();
                let mut label: Option<&String> = None;
                for (a, v) in values {
                    label = offset2label.get(&a).or(label);
                    marked.push((a, label, v));
                }
                let mut lines = Vec::new();
                let mut s = 0;
                while s < marked.len() {
                    let lab = marked[s].1;
                    let mut e = s;
                    while e < marked.len() && marked[e].1 == lab {
                        e += 1;
                    }
                    let group = &marked[s..e];
                    let hex = if group.len() >= 16 && group.iter().all(|g| g.2 == 0) {
                        "( 00 )".to_string()
                    } else {
                        group.iter().map(|g| word8_to_hex(g.2)).collect::<Vec<_>>().join(" ")
                    };
                    lines.push(format!(
                        "mem[{}..{}]{}: \t{}{}",
                        show_addr(group[0].0),
                        show_addr(group[group.len() - 1].0),
                        size_suffix(group.len()),
                        hex,
                        lab.map(|l| format!("\t@\"{}\"", l)).unwrap_or_default()
                    ));
                    s = e;
                }
                out.push(lines.join("\n"));
            }
        }
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct Isa(usize);

    impl Instruction for Isa {
        fn byte_size(&self) -> usize {
            self.0
        }

        fn show(&self) -> String {
            format!("Isa {}", self.0)
        }
    }

    fn mem(cells: Vec<Cell<Isa>>) -> Mem<Isa> {
        Mem {
            size: cells.len() as i64,
            data: cells.into_iter().collect(),
        }
    }

    fn values(vs: impl IntoIterator<Item = u8>) -> Vec<Cell<Isa>> {
        vs.into_iter().map(Cell::Value).collect()
    }

    fn bytes(m: &Mem<Isa>) -> Vec<u8> {
        m.data
            .iter()
            .map(|c| match c {
                Cell::Value(v) => *v,
                _ => panic!("not a value"),
            })
            .collect()
    }

    fn stream(input: &[W]) -> IoStream {
        IoStream {
            input: input.iter().copied().collect(),
            output: imbl::Vector::new(),
        }
    }

    /// A 12-byte memory with a single 4-byte IO port at offset 4.
    fn iomem() -> IoMem<Isa> {
        IoMem::new(BTreeMap::from([(4, stream(&[0x04030201, 2, 3]))]), mem(values(0..12)))
    }

    /// A 19-byte memory mixing instructions, instruction parts and two IO ports.
    fn piomem() -> IoMem<Isa> {
        use Cell::{Instruction as I, InstructionPart as P};
        let cells = vec![
            I(Isa(2)),
            P,
            I(Isa(2)),
            P,
            I(Isa(2)),
            P,
            I(Isa(3)),
            P,
            P,
            I(Isa(1)),
            I(Isa(5)),
            P,
            P,
            P,
            P,
            I(Isa(1)),
            I(Isa(1)),
            I(Isa(1)),
            I(Isa(1)),
        ];
        IoMem::new(
            BTreeMap::from([(4, stream(&[1, 2, 3])), (14, stream(&[1, 2, 3]))]),
            mem(cells),
        )
    }

    /// Declared content in 0..7, zero padding in 8..15.
    fn sparse() -> IoMem<Isa> {
        IoMem::new(BTreeMap::new(), mem(values((0..8).chain([0; 8]))))
    }

    #[test]
    fn read_words() {
        let m = mem(values(10..20));
        assert_eq!(m.read_word(0), Ok(0x0D0C0B0A));
        assert_eq!(m.read_word(-1), Err("memory[-0x1]: out of memory\n".to_string()));
        assert_eq!(m.read_word(7), Err("memory[0xa]: out of memory\n".to_string()));
    }

    #[test]
    fn write_words_and_bytes() {
        let m0 = mem(values([0; 10]));
        let mut m = m0.clone();
        m.write_word(0, 0x0D0C0B0A).unwrap();
        assert_eq!(bytes(&m), vec![10, 11, 12, 13, 0, 0, 0, 0, 0, 0]);
        let err = |idx, w| m0.clone().write_word(idx, w).unwrap_err();
        assert_eq!(err(7, 1), "memory[0x7]: out of memory for word access");
        assert_eq!(err(-1, 1), "memory[-0x1]: out of memory for word access");
        let mut m = m0.clone();
        m.write_byte(3, 0x0A).unwrap();
        assert_eq!(bytes(&m), vec![0, 0, 0, 0x0A, 0, 0, 0, 0, 0, 0]);
        assert_eq!(m0.clone().write_byte(10, 1).unwrap_err(), "memory[0xa]: out of memory");
        assert_eq!(m0.clone().write_byte(-1, 1).unwrap_err(), "memory[-0x1]: out of memory");
    }

    #[test]
    fn read_word_around_io_port() {
        let r = |idx| iomem().read_word(idx);
        assert_eq!(r(0), Ok(0x03020100));
        for idx in [1, 2, 3, 5, 6, 7] {
            assert_eq!(
                r(idx),
                Err(format!("iomemory[0x{:x}]: can't read word from input port", idx))
            );
        }
        assert_eq!(r(4), Ok(0x04030201));
        assert_eq!(r(8), Ok(0x0B0A0908));
    }

    #[test]
    fn read_byte_around_io_port() {
        for (idx, v) in [(3, 3), (4, 1), (5, 2), (6, 3), (7, 4), (8, 8)] {
            assert_eq!(iomem().read_byte(idx), Ok(v), "byte {}", idx);
        }
    }

    #[test]
    fn write_byte_around_io_port() {
        for idx in [3, 4, 8] {
            assert!(iomem().write_byte(idx, 0).is_ok());
        }
        for idx in [5, 6, 7] {
            assert_eq!(
                iomem().write_byte(idx, 0),
                Err(format!("iomemory[0x{:x}]: can't write byte to input port", idx))
            );
        }
    }

    #[test]
    fn write_word_around_io_port() {
        for idx in [0, 4, 8] {
            assert!(iomem().write_word(idx, 0).is_ok());
        }
        for idx in [1, 2, 3, 5, 6, 7] {
            assert_eq!(
                iomem().write_word(idx, 0),
                Err(format!("iomemory[0x{:x}]: can't write word to input port", idx))
            );
        }
    }

    #[test]
    fn read_instruction_around_io_port() {
        let ok = [0, 2, 9, 18];
        let io = [4, 6, 10, 14, 15, 16, 17];
        for idx in 0..19 {
            let r = piomem().read_instruction(idx);
            if ok.contains(&idx) {
                assert!(r.is_ok(), "{}", idx);
            } else if io.contains(&idx) {
                assert_eq!(
                    r,
                    Err(format!("iomemory[0x{:02x}]: instruction in memory corrupted", idx))
                );
            } else {
                assert_eq!(
                    r,
                    Err(format!("memory[0x{:02x}]: instruction in memory corrupted", idx))
                );
            }
        }
    }

    #[test]
    fn fresh_memory_has_no_accesses() {
        let log = iomem().access_log;
        assert_eq!(log.instr.render(), "-");
        assert_eq!(log.data.render(), "-");
        assert_eq!(log.io.render(), "-");
    }

    #[test]
    fn data_accesses_are_recorded() {
        let mut m = iomem();
        m.read_word(0).unwrap();
        assert_eq!(m.access_log.data.render(), "0..3 (4 B)");
        let mut m = iomem();
        m.read_byte(9).unwrap();
        assert_eq!(m.access_log.data.render(), "9..9 (1 B)");
        let mut m = iomem();
        m.write_word(0, 1).unwrap();
        assert_eq!(m.access_log.data.render(), "0..3 (4 B)");
        let mut m = iomem();
        m.write_byte(9, 0xFF).unwrap();
        assert_eq!(m.access_log.data.render(), "9..9 (1 B)");
        let mut m = sparse();
        for (a, v) in [(8, 1), (9, 2), (10, 3)] {
            m.write_byte(a, v).unwrap();
        }
        assert_eq!(m.access_log.data.render(), "8..10 (3 B)");
    }

    #[test]
    fn io_accesses_are_recorded_apart() {
        let mut m = iomem();
        m.read_word(4).unwrap();
        assert_eq!(m.access_log.io.render(), "4..7 (4 B)");
        assert_eq!(m.access_log.data.render(), "-");
        let mut m = iomem();
        m.write_word(4, 0x12345678).unwrap();
        assert_eq!(m.access_log.io.render(), "4..7 (4 B)");
        let mut m = iomem();
        m.read_word(0).unwrap();
        m.read_word(4).unwrap();
        assert_eq!(m.access_log.data.render(), "0..3 (4 B)");
        assert_eq!(m.access_log.io.render(), "4..7 (4 B)");
        assert_eq!(m.access_log.instr.render(), "-");
    }

    #[test]
    fn instruction_fetches_are_recorded() {
        let mut m = piomem();
        m.read_instruction(0).unwrap();
        assert_eq!(m.access_log.instr.render(), "0..1 (2 B)");
        m.read_instruction(2).unwrap();
        assert_eq!(m.access_log.instr.render(), "0..3 (4 B)");
    }

    #[test]
    fn accesses_outside_declared_content_are_recorded() {
        let mut m = sparse();
        m.write_byte(8, 0x42).unwrap();
        assert_eq!(m.access_log.data.render(), "8..8 (1 B)");
        let mut m = sparse();
        m.write_byte(15, 0xFF).unwrap();
        assert_eq!(m.access_log.data.render(), "15..15 (1 B)");
        let mut m = sparse();
        m.write_byte(7, 1).unwrap();
        m.write_byte(8, 2).unwrap();
        assert_eq!(m.access_log.data.render(), "7..8 (2 B)");
    }
}
