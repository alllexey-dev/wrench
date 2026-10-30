//! From assembler source to an initial memory image: parse the `.text` and
//! `.data` sections, assign label addresses, resolve references and place the
//! sections in memory.

use crate::intervals::Intervals;
use crate::machine::{Arch, Instr};
use crate::memory::{Cell, Instruction, Labels, Mem};
use crate::parser::{error_pretty, PError, P, R};
use crate::word::{word_split, W};

/// A parsed instruction, before its references are resolved.
pub trait ParsedInstr {
    fn byte_size(&self) -> usize;
}

#[derive(Clone, Debug)]
pub enum DataValue {
    Bytes(Vec<u8>),
    Words(Vec<W>),
}

impl DataValue {
    pub fn byte_size(&self) -> usize {
        match self {
            DataValue::Bytes(b) => b.len(),
            DataValue::Words(w) => 4 * w.len(),
        }
    }

    fn cells<I>(&self) -> Vec<Cell<I>> {
        match self {
            DataValue::Bytes(b) => b.iter().map(|&x| Cell::Value(x)).collect(),
            DataValue::Words(ws) => ws.iter().flat_map(|&w| word_split(w)).map(Cell::Value).collect(),
        }
    }
}

pub enum CodeToken<M> {
    Label(String),
    Mnemonic(M),
}

pub struct DataToken {
    pub label: String,
    pub value: DataValue,
}

/// A parsed section; `orgs` holds every `.org` it has (at most one is valid).
pub enum Section<M> {
    Code { orgs: Vec<i64>, tokens: Vec<CodeToken<M>> },
    Data { orgs: Vec<i64>, tokens: Vec<DataToken> },
}

/// A section with its references resolved.
pub enum Resolved<I> {
    Code { org: Option<i64>, instrs: Vec<I> },
    Data { org: Option<i64>, values: Vec<DataValue> },
}

impl<I: Instruction> Resolved<I> {
    pub fn org(&self) -> Option<i64> {
        match self {
            Resolved::Code { org, .. } | Resolved::Data { org, .. } => *org,
        }
    }

    pub fn byte_size(&self) -> i64 {
        match self {
            Resolved::Code { instrs, .. } => instrs.iter().map(|i| i.byte_size() as i64).sum(),
            Resolved::Data { values, .. } => values.iter().map(|v| v.byte_size() as i64).sum(),
        }
    }

    fn is_text(&self) -> bool {
        matches!(self, Resolved::Code { .. })
    }

    fn cells(&self) -> Vec<Cell<I>> {
        match self {
            Resolved::Code { instrs, .. } => instrs
                .iter()
                .flat_map(|i| {
                    std::iter::once(Cell::Instruction(i.clone())).chain(std::iter::repeat_n(
                        Cell::InstructionPart,
                        i.byte_size().saturating_sub(1),
                    ))
                })
                .collect(),
            Resolved::Data { values, .. } => values.iter().flat_map(|v| v.cells()).collect(),
        }
    }
}

enum Item<T> {
    Item(T),
    Org(i64),
}

fn split_items<T>(items: Vec<Option<Item<T>>>) -> (Vec<i64>, Vec<T>) {
    let mut orgs = Vec::new();
    let mut out = Vec::new();
    for i in items.into_iter().flatten() {
        match i {
            Item::Item(t) => out.push(t),
            Item::Org(o) => orgs.push(o),
        }
    }
    (orgs, out)
}

fn code_section<A: Arch>(p: &mut P) -> R<Section<A::Parsed>> {
    let c = A::COMMENT;
    p.string(".text")?;
    p.eol_(c)?;
    let items = p.many(|p| {
        p.choice(&[
            &|p| p.or(|p| p.hspace1(), |p| p.eol_(c)).map(|_| None),
            &|p| p.org_directive(c).map(|o| Some(Item::Org(o))),
            &|p| p.label().map(|l| Some(Item::Item(CodeToken::Label(l)))),
            &|p| A::mnemonic(p).map(|m| Some(Item::Item(CodeToken::Mnemonic(m)))),
        ])
    })?;
    let (orgs, tokens) = split_items(items);
    Ok(Section::Code { orgs, tokens })
}

fn string_array(p: &mut P) -> R<Vec<u32>> {
    p.char('\'')?;
    let cs = p.many_till(|p| p.char_literal(), |p| p.char('\'').map(|_| ()))?;
    Ok(cs.into_iter().map(|c| c as u32).collect())
}

fn data_items<T>(p: &mut P, item: &dyn Fn(&mut P) -> R<T>, from_char: fn(u32) -> T) -> R<Vec<T>> {
    let groups = p.sep_by(
        |p| {
            p.or(
                |p| string_array(p).map(|cs| cs.into_iter().map(from_char).collect()),
                |p| item(p).map(|x| vec![x]),
            )
        },
        |p| {
            p.attempt(|p| {
                p.hspace()?;
                p.string(",")?;
                p.hspace()
            })
        },
    )?;
    Ok(groups.into_iter().flatten().collect())
}

fn data_value(p: &mut P, c: &str) -> R<DataValue> {
    let byte = |p: &mut P| -> R<DataValue> {
        p.string(".byte")?;
        p.hspace1()?;
        let v = data_items(p, &|p| p.byte_literal(), |c| c as u8)?;
        p.eol_(c)?;
        Ok(DataValue::Bytes(v))
    };
    let word = |p: &mut P| -> R<DataValue> {
        p.string(".word")?;
        p.hspace1()?;
        let v = data_items(p, &|p| p.word_literal(), |c| c as W)?;
        p.eol_(c)?;
        Ok(DataValue::Words(v))
    };
    p.choice(&[&byte, &word])
}

fn data_section<M>(p: &mut P, c: &str) -> R<Section<M>> {
    p.string(".data")?;
    p.eol_(c)?;
    let items = p.many(|p| {
        p.choice(&[
            &|p| p.or(|p| p.hspace1(), |p| p.eol_(c)).map(|_| None),
            &|p| {
                let label = p.label()?;
                p.hspace1()?;
                let value = data_value(p, c)?;
                Ok(Some(Item::Item(DataToken { label, value })))
            },
            &|p| p.org_directive(c).map(|o| Some(Item::Org(o))),
        ])
    })?;
    let (orgs, tokens) = split_items(items);
    Ok(Section::Data { orgs, tokens })
}

/// The whole program: sections separated by blank lines and comments.
pub fn asm_parser<A: Arch>(p: &mut P) -> R<Vec<Section<A::Parsed>>> {
    let c = A::COMMENT;
    let secs = p.many(|p| {
        p.choice(&[
            &|p| p.or(|p| p.space1(), |p| p.eol_(c)).map(|_| None),
            &|p| data_section(p, c).map(Some),
            &|p| code_section::<A>(p).map(Some),
        ])
    })?;
    p.eof()?;
    Ok(secs.into_iter().flatten().collect())
}

/// Parse after preprocessing; the error is rendered against the original source.
pub fn parse<A: Arch>(fname: &str, src: &str) -> Result<Vec<Section<A::Parsed>>, String> {
    let original: Vec<char> = src.chars().collect();
    let pre = A::preprocess(src).map_err(|(offset, msg)| {
        error_pretty(
            fname,
            &original,
            &PError {
                offset,
                messages: vec![msg],
                ..Default::default()
            },
        )
    })?;
    let chars: Vec<char> = pre.chars().collect();
    let mut p = P::new(fname, &chars);
    asm_parser::<A>(&mut p).map_err(|e| error_pretty(fname, &original, &e))
}

/// Does the source parse as this ISA?
pub fn parses_as<A: Arch>(src: &str) -> bool {
    match A::preprocess(src) {
        Err(_) => true,
        Ok(pre) => {
            let chars: Vec<char> = pre.chars().collect();
            let mut p = P::new("", &chars);
            asm_parser::<A>(&mut p).is_ok()
        }
    }
}

fn section_org(orgs: &[i64]) -> Result<Option<i64>, String> {
    match orgs {
        [] => Ok(None),
        [x] => Ok(Some(*x)),
        _ => Err("error: multiple .org directives not allowed".to_string()),
    }
}

fn parsed_size<M: ParsedInstr>(s: &Section<M>) -> W {
    match s {
        Section::Code { tokens, .. } => tokens
            .iter()
            .map(|t| match t {
                CodeToken::Mnemonic(m) => m.byte_size() as W,
                CodeToken::Label(_) => 0,
            })
            .sum(),
        Section::Data { tokens, .. } => tokens.iter().map(|t| t.value.byte_size() as W).sum(),
    }
}

fn evaluate_labels<M: ParsedInstr>(sections: &[Section<M>]) -> Result<Labels, String> {
    let mut offset: W = 0;
    let mut found: Vec<(String, W)> = Vec::new();
    for s in sections {
        match s {
            Section::Code { orgs, tokens } => {
                if let Some(o) = section_org(orgs)? {
                    offset = o as W;
                }
                for t in tokens {
                    match t {
                        CodeToken::Mnemonic(m) => offset = offset.wrapping_add(m.byte_size() as W),
                        CodeToken::Label(l) => found.push((l.clone(), offset)),
                    }
                }
            }
            Section::Data { orgs, tokens } => {
                if let Some(o) = section_org(orgs)? {
                    offset = o as W;
                }
                for t in tokens {
                    found.push((t.label.clone(), offset));
                    offset = offset.wrapping_add(t.value.byte_size() as W);
                }
            }
        }
    }
    let mut labels = Labels::new();
    for (l, a) in found.into_iter().rev() {
        if labels.contains_key(&l) {
            return Err(format!("Duplicate label: {}", l));
        }
        labels.insert(l, a);
    }
    Ok(labels)
}

/// Translation-time layout statistics (`layout:*`).
#[derive(Clone, Debug, Default)]
pub struct DumpStats {
    pub sections_total_bytes: i64,
    pub text_sections_bytes: i64,
    pub data_sections_bytes: i64,
    pub text_intervals: Intervals,
    pub data_intervals: Intervals,
}

fn compute_dump_stats<I: Instruction>(sections: &[Resolved<I>]) -> DumpStats {
    let mut st = DumpStats::default();
    let mut offset = 0;
    for s in sections {
        let start = s.org().unwrap_or(offset);
        let n = s.byte_size();
        if s.is_text() {
            st.text_sections_bytes += n;
            if n > 0 {
                st.text_intervals.record(start, n);
            }
        } else {
            st.data_sections_bytes += n;
            if n > 0 {
                st.data_intervals.record(start, n);
            }
        }
        offset = start + n;
    }
    st.sections_total_bytes = st.text_sections_bytes + st.data_sections_bytes;
    st
}

fn prepare_dump<I: Instruction>(memory_size: i64, fill: &[u8], sections: &[Resolved<I>]) -> Result<Mem<I>, String> {
    let mut offset = 0;
    let mut used = Intervals::new();
    let mut placed: Vec<(i64, Cell<I>)> = Vec::new();
    for s in sections {
        let start = s.org().unwrap_or(offset);
        let cells = s.cells();
        let len = cells.len() as i64;
        let new_range = if len > 0 {
            Intervals::new().with(start, len)
        } else {
            Intervals::new()
        };
        let overlap = new_range.intersect(&used);
        if overlap.size() > 0 {
            return Err(format!(
                "sections overlap: {} section at mem[{}] overlaps bytes already placed at mem[{}]",
                if s.is_text() { "text" } else { "data" },
                new_range.render_hex(),
                overlap.render_hex()
            ));
        }
        if start < 0 && len > 0 {
            return Err(format!("section at negative address {} (.org)", start));
        }
        placed.extend(cells.into_iter().enumerate().map(|(k, c)| (start + k as i64, c)));
        offset = start + len;
        used = used.union(&new_range);
    }
    let max_address = placed.iter().map(|(a, _)| *a).max().unwrap_or(0).max(0);
    if max_address > memory_size {
        return Err(format!(
            "program does not fit in memory: it needs {} bytes, but memory_size is {} bytes. Increase memory_size in the configuration.",
            max_address, memory_size
        ));
    }
    let len = memory_size.max(max_address + 1).max(0) as usize;
    let mut data: Vec<Cell<I>> = (0..len)
        .map(|a| {
            Cell::Value(if (a as i64) < memory_size {
                fill.get(a).copied().unwrap_or(0)
            } else {
                0
            })
        })
        .collect();
    for (a, c) in placed {
        data[a as usize] = c;
    }
    Ok(Mem {
        size: memory_size,
        data: data.into_iter().collect(),
    })
}

pub struct TranslatorResult<I: Clone> {
    pub dump: Mem<I>,
    pub labels: Labels,
    pub dump_stats: DumpStats,
}

/// Translate a program: parse, lay out, resolve and place in memory filled with `fill`.
pub fn translate<A: Arch>(
    memory_size: i64,
    fill: &[u8],
    fname: &str,
    src: &str,
) -> Result<TranslatorResult<Instr<A>>, String> {
    let sections = parse::<A>(fname, src)?;
    let labels = evaluate_labels(&sections)?;
    let lookup = |l: &str| labels.get(l).copied();
    let mut resolved = Vec::new();
    let mut offset: W = 0;
    for s in &sections {
        let (orgs, size) = match s {
            Section::Code { orgs, .. } | Section::Data { orgs, .. } => (orgs, parsed_size(s)),
        };
        let org = section_org(orgs)?;
        let start = org.map_or(offset, |o| o as W);
        match s {
            Section::Code { tokens, .. } => {
                let mut at = start;
                let mut instrs = Vec::new();
                for t in tokens {
                    if let CodeToken::Mnemonic(m) = t {
                        instrs.push(A::deref(m, at, &lookup)?);
                        at = at.wrapping_add(m.byte_size() as W);
                    }
                }
                resolved.push(Resolved::Code { org, instrs });
            }
            Section::Data { tokens, .. } => resolved.push(Resolved::Data {
                org,
                values: tokens.iter().map(|t| t.value.clone()).collect(),
            }),
        }
        offset = start.wrapping_add(size);
    }
    let dump = prepare_dump(memory_size, fill, &resolved)?;
    Ok(TranslatorResult {
        dump,
        labels,
        dump_stats: compute_dump_stats(&resolved),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::isa::risc_iv::RiscIv;

    #[derive(Clone, Debug)]
    struct Isa(usize);

    impl Instruction for Isa {
        fn byte_size(&self) -> usize {
            self.0
        }

        fn show(&self) -> String {
            format!("Isa {}", self.0)
        }
    }

    fn code(org: i64) -> Resolved<Isa> {
        Resolved::Code {
            org: Some(org),
            instrs: vec![Isa(4)],
        }
    }

    fn data(org: i64) -> Resolved<Isa> {
        Resolved::Data {
            org: Some(org),
            values: vec![DataValue::Words(vec![0])],
        }
    }

    #[test]
    fn overlapping_sections_are_rejected() {
        let err = |s: &[Resolved<Isa>]| prepare_dump(16, &[0; 16], s).err();
        assert_eq!(
            err(&[code(0), data(2)]).as_deref(),
            Some("sections overlap: data section at mem[0x02..0x05 (4 B)] overlaps bytes already placed at mem[0x02..0x03 (2 B)]")
        );
        assert_eq!(
            err(&[data(0), data(3)]).as_deref(),
            Some("sections overlap: data section at mem[0x03..0x06 (4 B)] overlaps bytes already placed at mem[0x03..0x03 (1 B)]")
        );
        assert!(err(&[code(0), data(4)]).is_none());
        assert!(err(&[code(8), data(0)]).is_none());
    }

    fn translate_src(file: &str, lines: &[&str]) -> Result<TranslatorResult<Instr<RiscIv>>, String> {
        let src: String = lines.iter().map(|l| format!("{}\n", l)).collect();
        translate::<RiscIv>(1000, &[0; 1000], file, &src)
    }

    #[test]
    fn unresolved_label_reports_where_it_is_used() {
        let err = translate_src("typo.s", &["    .text", "_start:", "    j        nowhere"])
            .err()
            .unwrap();
        assert!(err.contains("typo.s:3:"), "no source position in: {}", err);
        assert!(err.contains("nowhere"), "no label name in: {}", err);
    }

    #[test]
    fn too_wide_word_reports_its_position() {
        let err = translate_src("data.s", &["    .data", "big: .word 0xFFFFFFFF, 4294967297"])
            .err()
            .unwrap();
        assert!(err.contains("data.s:2:24"), "no source position in: {}", err);
        assert!(err.contains("machine word"), "no machine word in: {}", err);
    }

    #[test]
    fn too_wide_byte_reports_its_position() {
        let err = translate_src("data.s", &["    .data", "small: .byte 300"])
            .err()
            .unwrap();
        assert!(err.contains("data.s:2:14"), "no source position in: {}", err);
        assert!(err.contains("expected -128..255"), "no byte range in: {}", err);
    }
}
