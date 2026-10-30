//! Reports: which records of a run to show, rendered through a template of
//! views, and optionally asserted against an expected text.

pub mod view;

use std::cell::OnceCell;
use std::collections::BTreeMap;

use serde::Deserialize;

use crate::intervals::Intervals;
use crate::machine::{Machine, Trace};
use crate::memory::{hex_addr, hex_addr_width, pretty_dump, word32_to_hex, AccessCount, Instruction, Labels};
use crate::translator::{DumpStats, TranslatorResult};
use crate::word::W;
use view::{compile_template, render_template, with_renames, CheckEnv, Format, Spec, Template, Val, View};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportConf {
    #[serde(default)]
    pub name: Option<String>,
    pub slice: ReportSlice,
    #[serde(default)]
    pub assert: Option<String>,
    #[serde(default)]
    pub view: Option<String>,
}

/// Which records a report shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReportSlice {
    Head(usize),
    All,
    Tail(usize),
    Last,
}

impl<'de> Deserialize<'de> for ReportSlice {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_yaml::Value::deserialize(d)?;
        let bad = || {
            serde::de::Error::custom("Invalid slice format, expect: [\"head\", n], \"all\", [\"tail\", n], \"last\"")
        };
        match &v {
            serde_yaml::Value::String(s) if s == "all" => Ok(ReportSlice::All),
            serde_yaml::Value::String(s) if s == "last" => Ok(ReportSlice::Last),
            serde_yaml::Value::Sequence(xs) if xs.len() == 2 => {
                let n = xs[1].as_f64().ok_or_else(bad)?.round().max(0.0) as usize;
                match xs[0].as_str() {
                    Some("head") => Ok(ReportSlice::Head(n)),
                    Some("tail") => Ok(ReportSlice::Tail(n)),
                    _ => Err(bad()),
                }
            }
            _ => Err(bad()),
        }
    }
}

/// What a view sees: the record being rendered and the run around it.
pub struct Env<'a, S: Machine> {
    pub labels: &'a Labels,
    /// The state of the record being rendered.
    pub state: &'a S,
    /// The state the run ended in, for run-wide stats.
    pub final_state: &'a S,
    pub dump_stats: &'a DumpStats,
    pub instr_count: usize,
    pub prev_instr: Option<&'a S::Instr>,
    /// Made by the instruction that produced this record.
    pub accesses: AccessCount,
    /// The whole run's per-instruction access table, built only if asked.
    pub instruction_costs: &'a dyn Fn() -> String,
}

/// A register-like value: `dec` by default, or `hex`.
pub fn value_view<S: Machine>(name: &str, doc: &str, f: impl Fn(&Env<S>) -> i64 + 'static) -> View<S> {
    View::new(
        Spec::lit(name).format(&[Format::Dec, Format::Hex], Format::Dec),
        doc,
        move |e, a| show_value(a[0].fmt(), f(e)),
    )
}

/// A view without arguments or format.
pub fn flag_view<S: Machine>(name: &str, doc: &str, f: impl Fn(&Env<S>) -> String + 'static) -> View<S> {
    View::new(Spec::lit(name), doc, move |e, _| f(e))
}

pub fn show_value(fmt: Format, x: i64) -> String {
    match fmt {
        Format::Hex => word32_to_hex(x as W),
        _ => x.to_string(),
    }
}

/// Values joined top first, `a:b:c`, as stacks are shown.
pub fn show_values(fmt: Format, xs: &[i64]) -> String {
    xs.iter().map(|&x| show_value(fmt, x)).collect::<Vec<_>>().join(":")
}

/// The ISA's own views, its stats among them.
pub fn isa_views<S: Machine>() -> Vec<View<S>> {
    let mut vs = S::views();
    vs.extend(S::stats().iter().map(|s| s.view()));
    vs
}

/// Every view a report on this ISA can use.
pub fn report_views<S: Machine>() -> Vec<View<S>> {
    let mut vs = isa_views::<S>();
    vs.extend(shared_views::<S>());
    vs
}

fn hex(a: i64) -> String {
    format!("0x{:x}", a)
}

fn ranges<S: Machine>(name: &str, doc: &str, f: impl Fn(&Env<S>) -> Intervals + 'static) -> View<S> {
    View::new(
        Spec::lit(name).format(&[Format::Dec, Format::Hex], Format::Hex),
        doc,
        move |e, a| {
            if a[0].fmt() == Format::Hex {
                f(e).render_hex()
            } else {
                f(e).render()
            }
        },
    )
}

/// The views every ISA has.
pub fn shared_views<S: Machine>() -> Vec<View<S>> {
    with_renames(
        &[
            ("memAccesses", "mem:accesses"),
            ("memReads", "mem:reads"),
            ("memWrites", "mem:writes"),
            ("memAccessesTotal", "mem:accesses-total"),
            ("memReadsTotal", "mem:reads-total"),
            ("memWritesTotal", "mem:writes-total"),
            ("memory:table", "mem:table"),
            ("memory", "mem:dump"),
        ],
        vec![
            value_view::<S>("pc", "Program counter.", |e| e.state.pc()),
            flag_view::<S>(
                "pc:label",
                "`@label` if the program counter is at a label, otherwise empty.",
                |e| {
                    let pc = e.state.pc();
                    let mut ls: Vec<&String> = e
                        .labels
                        .iter()
                        .filter(|(_, &a)| a as i64 == pc)
                        .map(|(l, _)| l)
                        .collect();
                    ls.sort();
                    ls.first().map(|l| format!("@{}", l)).unwrap_or_default()
                },
            ),
            flag_view::<S>(
                "instruction:prev",
                "The instruction that produced this state, `-` for the initial state.",
                |e| e.prev_instr.map_or("-".to_string(), |i| i.show()),
            ),
            flag_view::<S>(
                "instruction:next",
                "The instruction to execute next, `-` once halted.",
                |e| next_instruction(e.state),
            ),
            value_view::<S>("sim:instruction-count", "Instructions executed so far.", |e| {
                e.instr_count as i64
            }),
            value_view::<S>(
                "mem:accesses",
                "Memory accesses of the instruction that produced this state, its fetch included.",
                |e| e.accesses.total(),
            ),
            value_view::<S>("mem:reads", "Reads among `mem:accesses`; a fetch is a read.", |e| {
                e.accesses.reads
            }),
            value_view::<S>("mem:writes", "Writes among `mem:accesses`.", |e| e.accesses.writes),
            value_view::<S>("mem:accesses-total", "Memory accesses of the run so far.", |e| {
                e.state.mem().access_count.total()
            }),
            value_view::<S>("mem:reads-total", "Reads of the run so far.", |e| {
                e.state.mem().access_count.reads
            }),
            value_view::<S>("mem:writes-total", "Writes of the run so far.", |e| {
                e.state.mem().access_count.writes
            }),
            value_view::<S>("layout:sections-size", "Bytes in all sections.", |e| {
                e.dump_stats.sections_total_bytes
            }),
            value_view::<S>("layout:text-sections-size", "Bytes in `.text` sections.", |e| {
                e.dump_stats.text_sections_bytes
            }),
            value_view::<S>("layout:data-sections-size", "Bytes in `.data` sections.", |e| {
                e.dump_stats.data_sections_bytes
            }),
            ranges::<S>("layout:text-ranges", "Address ranges of `.text` sections.", |e| {
                e.dump_stats.text_intervals.clone()
            }),
            ranges::<S>("layout:data-ranges", "Address ranges of `.data` sections.", |e| {
                e.dump_stats.data_intervals.clone()
            }),
            ranges::<S>(
                "mem:instr-ranges",
                "Address ranges the run fetched instructions from.",
                |e| e.final_state.mem().access_log.instr.clone(),
            ),
            ranges::<S>(
                "mem:data-ranges",
                "Address ranges the run read or wrote data at.",
                |e| e.final_state.mem().access_log.data.clone(),
            ),
            ranges::<S>("mem:io-ranges", "Memory-mapped IO addresses the run accessed.", |e| {
                e.final_state.mem().access_log.io.clone()
            }),
            flag_view::<S>(
                "mem:table",
                "Address-space table: sections, IO, free spans and their access coverage.",
                |e| render_memory_table(e.dump_stats, e.final_state),
            ),
            flag_view::<S>(
                "mem:per-instruction",
                "Reads and writes of each executed instruction, per run.",
                |e| (e.instruction_costs)(),
            ),
            View::<S>::checked(
                Spec::lit("mem:dump").arg("from").arg("to"),
                "Memory dump from `<from>` to `<to>`, inclusive.",
                |c: &CheckEnv, a: &[Val]| {
                    let (from, to) = (a[0].int(), a[1].int());
                    if from < 0 || to >= c.memory_size {
                        Err(format!(
                            "range {}..{} is outside memory 0..{}",
                            from,
                            to,
                            c.memory_size - 1
                        ))
                    } else if from > to {
                        Err(format!("range {}..{} is empty: <from> is after <to>", from, to))
                    } else {
                        Ok(())
                    }
                },
                |e, a| view_memory(e.state, a[0].int(), a[1].int()),
            ),
            View::<S>::checked(
                Spec::lit("io")
                    .arg("addr")
                    .format(&[Format::Dec, Format::Hex, Format::Sym], Format::Dec),
                "IO port at `<addr>`: `input >>> output` so far; `sym` shows characters.",
                |c: &CheckEnv, a: &[Val]| {
                    let addr = a[0].int();
                    if c.io_ports.contains(&addr) {
                        Ok(())
                    } else if c.io_ports.is_empty() {
                        Err(format!("no IO port at {}, memory_mapped_io is empty", hex(addr)))
                    } else {
                        let mut ports = c.io_ports.clone();
                        ports.sort();
                        Err(format!(
                            "no IO port at {}, configured: {}",
                            hex(addr),
                            ports.iter().map(|&p| hex(p)).collect::<Vec<_>>().join(", ")
                        ))
                    }
                },
                |e, a| view_io(a[1].fmt(), a[0].int(), e.state),
            ),
            flag_view::<S>(
                "isa-specific",
                "The ISA's run-wide stats, one `name: value` per line; empty if it has none.",
                isa_specific::<S>,
            ),
        ],
    )
}

fn next_instruction<S: Machine>(st: &S) -> String {
    if st.is_halted() {
        return "-".to_string();
    }
    st.mem().peek_instruction(st.pc()).map_or("-".to_string(), |i| i.show())
}

fn isa_specific<S: Machine>(env: &Env<S>) -> String {
    let stats = S::stats();
    let width = stats.iter().map(|s| s.name.chars().count()).max().unwrap_or(0) + 1;
    stats
        .iter()
        .map(|s| {
            let n = format!("{}:", s.name);
            format!(
                "{}{} {}",
                n,
                " ".repeat(width.saturating_sub(n.chars().count())),
                (s.render)(env)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A report with its view template compiled.
pub struct Report<S: Machine> {
    pub conf: ReportConf,
    pub template: Option<Template<S>>,
}

/// Compile every report before the simulation runs, so a mistake fails
/// the same way in every ISA, naming the report and the key.
pub fn compile_reports<S: Machine>(
    check_env: &CheckEnv,
    confs: &[ReportConf],
) -> Result<(Vec<String>, Vec<Report<S>>), String> {
    let views = report_views::<S>();
    let mut warnings: Vec<String> = Vec::new();
    let mut errs: Vec<String> = Vec::new();
    let mut reports = Vec::new();
    for (i, rc) in confs.iter().enumerate() {
        let named = |m: &String| {
            format!(
                "report {}: {}",
                rc.name
                    .as_ref()
                    .map_or(format!("#{}", i + 1), |n| crate::hs::show_str(n)),
                m
            )
        };
        match &rc.view {
            None => reports.push(Report {
                conf: rc.clone(),
                template: None,
            }),
            Some(text) => match compile_template(check_env, &views, text) {
                Ok((ws, t)) => {
                    warnings.extend(ws.iter().map(named));
                    reports.push(Report {
                        conf: rc.clone(),
                        template: Some(t),
                    });
                }
                Err(es) => errs.extend(es.iter().map(named)),
            },
        }
    }
    let nub = |xs: Vec<String>| {
        let mut out: Vec<String> = Vec::new();
        for x in xs {
            if !out.contains(&x) {
                out.push(x);
            }
        }
        out
    };
    if errs.is_empty() {
        Ok((nub(warnings), reports))
    } else {
        Err(nub(errs).join("\n"))
    }
}

/// Render one of the ISA's own views of a state.
pub fn render_isa_view<S: Machine>(labels: &Labels, st: &S, key: &str) -> Result<String, String> {
    let unchecked = CheckEnv {
        memory_size: i64::MAX,
        io_ports: Vec::new(),
    };
    let (_, template) =
        compile_template(&unchecked, &isa_views::<S>(), &format!("{{{}}}", key)).map_err(|e| e.join("\n"))?;
    let stats = DumpStats::default();
    let none = || String::new();
    Ok(render_template(
        &template,
        &Env {
            labels,
            state: st,
            final_state: st,
            dump_stats: &stats,
            instr_count: 0,
            prev_instr: None,
            accesses: AccessCount::default(),
            instruction_costs: &none,
        },
    ))
}

fn strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace())
}

fn nospaces(s: &str) -> String {
    strip(s).lines().map(|l| format!("{}\n", strip(l))).collect()
}

/// Render a report over the run; `true` if its assertion holds.
pub fn prepare_report<S: Machine>(
    tr: &TranslatorResult<S::Instr>,
    verbose: bool,
    final_state: &S,
    records: &[Trace<S>],
    report: &Report<S>,
) -> (bool, String) {
    let rc = &report.conf;
    let header = rc.name.as_ref().map(|n| format!("# {}", n)).unwrap_or_default();
    let details = if verbose { format!("{:?}", rc) } else { String::new() };
    let deltas = with_access_deltas(records);
    let sliced: Vec<&(AccessCount, &Trace<S>)> = match rc.slice {
        ReportSlice::All => deltas.iter().collect(),
        ReportSlice::Head(n) => deltas.iter().take(n).collect(),
        ReportSlice::Tail(n) => deltas.iter().skip(deltas.len().saturating_sub(n)).collect(),
        ReportSlice::Last => deltas.last().into_iter().collect(),
    };
    let costs_cell: OnceCell<String> = OnceCell::new();
    let costs = || {
        costs_cell
            .get_or_init(|| render_instruction_costs(&tr.labels, final_state, &deltas))
            .clone()
    };
    let state_views: String = match &report.template {
        None => String::new(),
        Some(template) => sliced
            .iter()
            .map(|(accesses, record)| match record {
                Trace::State { count, prev, state } => render_template(
                    template,
                    &Env {
                        labels: &tr.labels,
                        state,
                        final_state,
                        dump_stats: &tr.dump_stats,
                        instr_count: *count,
                        prev_instr: prev.as_ref(),
                        accesses: *accesses,
                        instruction_costs: &costs,
                    },
                ),
                Trace::Error(err) => format!("ERROR: {}\n", err),
                Trace::Warn(w) => format!("WARN: {}\n", w),
            })
            .collect(),
    };
    let assert_report = match &rc.assert {
        Some(expect) if nospaces(&state_views) != nospaces(expect) => {
            format!("ASSERTION FAIL, expect:\n{}", nospaces(expect))
        }
        _ => String::new(),
    };
    let text: String = [header, details, state_views, assert_report.clone()]
        .iter()
        .filter(|t| !t.is_empty())
        .map(|t| format!("{}\n", strip(t)))
        .collect();
    (assert_report.is_empty(), text)
}

/// Pair each state with the accesses of the instruction that produced it.
fn with_access_deltas<S: Machine>(records: &[Trace<S>]) -> Vec<(AccessCount, &Trace<S>)> {
    let mut before = AccessCount::default();
    records
        .iter()
        .map(|r| match r {
            Trace::State { state, .. } => {
                let after = state.mem().access_count;
                let d = after.since(&before);
                before = after;
                (d, r)
            }
            _ => (AccessCount::default(), r),
        })
        .collect()
}

/// Executions by (reads, writes).
type Histogram = BTreeMap<(i64, i64), i64>;

fn render_instruction_costs<S: Machine>(
    labels: &Labels,
    final_state: &S,
    deltas: &[(AccessCount, &Trace<S>)],
) -> String {
    // pc -> (instruction, (reads, writes) -> executions)
    let mut costs: BTreeMap<i64, (String, Histogram)> = BTreeMap::new();
    let mut prev_pc: Option<i64> = None;
    for (acc, r) in deltas {
        if let Trace::State { prev, state, .. } = r {
            if let (Some(pc), Some(instr)) = (prev_pc, prev) {
                let e = costs.entry(pc).or_insert_with(|| (instr.show(), BTreeMap::new()));
                *e.1.entry((acc.reads, acc.writes)).or_insert(0) += 1;
            }
            prev_pc = Some(state.pc());
        }
    }
    let width = hex_addr_width(final_state.mem().capacity());
    let labels_at = |pc: i64| {
        let mut ls: Vec<&String> = labels.iter().filter(|(_, &a)| a as i64 == pc).map(|(l, _)| l).collect();
        ls.sort();
        ls
    };
    let mut rows: Vec<[String; 5]> = vec![["Address", "Exec", "Read", "Write", "Instruction"].map(String::from)];
    for (pc, (instr, hist)) in &costs {
        for (k, ((r, w), n)) in hist.iter().enumerate() {
            let (addr, ins) = if k == 0 {
                let mut parts = vec![instr.clone()];
                parts.extend(labels_at(*pc).iter().map(|l| format!("@{}", l)));
                (hex_addr(width, *pc), parts.join(" "))
            } else {
                ("\"".to_string(), "\"".to_string())
            };
            rows.push([addr, n.to_string(), r.to_string(), w.to_string(), ins]);
        }
    }
    let w = |k: usize| rows.iter().map(|r| r[k].chars().count()).max().unwrap_or(0);
    let (wa, we, wr, ww) = (w(0), w(1), w(2), w(3));
    rows.iter()
        .map(|r| {
            [
                pad_r(&r[0], wa),
                pad_l(&r[1], we),
                pad_l(&r[2], wr),
                pad_l(&r[3], ww),
                r[4].clone(),
            ]
            .join("  ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn pad_r(s: &str, n: usize) -> String {
    format!("{}{}", s, " ".repeat(n.saturating_sub(s.chars().count())))
}

fn pad_l(s: &str, n: usize) -> String {
    format!("{}{}", " ".repeat(n.saturating_sub(s.chars().count())), s)
}

/// The address-space table: one row per section, IO cluster, or `x` span.
fn render_memory_table<S: Machine>(stats: &DumpStats, st: &S) -> String {
    let mem = st.mem();
    let mem_size = mem.capacity();
    let log = &mem.access_log;
    let mut io = Intervals::new();
    for &p in mem.keys.iter() {
        io.record(p, 4);
    }
    let accessed = log.instr.union(&log.data).union(&log.io);
    let mut spans: Vec<(&str, i64, i64)> = Vec::new();
    spans.extend(stats.text_intervals.to_list().into_iter().map(|(a, b)| ("text", a, b)));
    spans.extend(stats.data_intervals.to_list().into_iter().map(|(a, b)| ("data", a, b)));
    spans.extend(io.to_list().into_iter().map(|(a, b)| ("io", a, b)));
    let sections = stats.text_intervals.union(&stats.data_intervals).union(&io);
    let free = Intervals::range(0, mem_size - 1).difference(&sections);
    for (lo, hi) in free.to_list() {
        spans.extend(split_by_access(&accessed, lo, hi));
    }
    spans.sort_by_key(|s| s.1);
    let hex_w = format!("{:x}", (mem_size - 1).max(0)).len().max(3);
    let addr = |a: i64| format!("0x{:0w$x}", a, w = hex_w);
    let range = |lo: i64, hi: i64| format!("{}..{}", addr(lo), addr(hi));
    let mut rows: Vec<[String; 5]> = vec![["Kind", "Range", "Bytes", "Accessed", "Coverage"].map(String::from)];
    for (kind, lo, hi) in spans {
        let bytes = hi - lo + 1;
        let here = accessed.intersect(&Intervals::range(lo, hi));
        let cov = if bytes > 0 { here.size() * 100 / bytes } else { 0 };
        let acc = if here.is_empty() {
            "-".to_string()
        } else {
            here.to_list()
                .iter()
                .map(|&(a, b)| range(a, b))
                .collect::<Vec<_>>()
                .join(", ")
        };
        rows.push([
            kind.to_string(),
            range(lo, hi),
            bytes.to_string(),
            acc,
            format!("{}%", cov),
        ]);
    }
    let w = |k: usize| rows.iter().map(|r| r[k].chars().count()).max().unwrap_or(0);
    let ws = [w(0), w(1), w(2), w(3), w(4)];
    rows.iter()
        .map(|r| {
            [
                pad_r(&r[0], ws[0]),
                pad_r(&r[1], ws[1]),
                pad_l(&r[2], ws[2]),
                pad_r(&r[3], ws[3]),
                pad_l(&r[4], ws[4]),
            ]
            .join("  ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Split a free range at access boundaries: each accessed cluster and each
/// gap becomes its own `x` row.
fn split_by_access(accessed: &Intervals, lo: i64, hi: i64) -> Vec<(&'static str, i64, i64)> {
    let mut out = Vec::new();
    let mut cursor = lo;
    for (alo, ahi) in accessed.intersect(&Intervals::range(lo, hi)).to_list() {
        if cursor < alo {
            out.push(("x", cursor, alo - 1));
        }
        out.push(("x", alo, ahi));
        cursor = ahi + 1;
    }
    if cursor <= hi {
        out.push(("x", cursor, hi));
    }
    out
}

fn view_memory<S: Machine>(st: &S, from: i64, to: i64) -> String {
    let mem = st.mem();
    let width = hex_addr_width(mem.capacity());
    match mem.cells.slice(from, to) {
        Ok(cells) => pretty_dump(true, &|a| hex_addr(width, a), &Labels::new(), &cells),
        Err(e) => panic!("{}", e),
    }
}

fn sym(w: W) -> char {
    match w {
        0 => '\0',
        10 => '\n',
        32..=126 => char::from_u32(w as u32).unwrap_or('?'),
        _ => '?',
    }
}

/// The port was checked against the configuration when the template was compiled.
fn view_io<S: Machine>(fmt: Format, addr: i64, st: &S) -> String {
    let Some(s) = st.mem().streams.get(&addr) else {
        return "-".to_string();
    };
    match fmt {
        Format::Hex => {
            let h = |v: &imbl::Vector<W>| {
                format!(
                    "[{}]",
                    v.iter().map(|&w| word32_to_hex(w)).collect::<Vec<_>>().join(",")
                )
            };
            format!("{} >>> {}", h(&s.input), h(&s.output))
        }
        Format::Sym => {
            let t = |v: &imbl::Vector<W>| {
                crate::hs::show_str(&v.iter().map(|&w| sym(w)).collect::<String>()).replace("\\NUL", "\\0")
            };
            format!("{} >>> {}", t(&s.input), t(&s.output))
        }
        _ => format!(
            "{} >>> {}",
            crate::hs::show_list(s.input.iter()),
            crate::hs::show_list(s.output.iter())
        ),
    }
}
