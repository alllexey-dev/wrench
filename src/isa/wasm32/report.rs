//! The wasm32 views: the operand stack, locals, the stack pointers and two
//! renderings of the frames, `stack:layout` and `stack:dump`.

use std::collections::{BTreeMap, HashSet};

use super::state::{scope_word_addrs, Scope, Wasm32St, SCOPE_RECORD_BYTES, WORD};
use super::Wasm32Isa;
use crate::bitpack::unpack_words;
use crate::memory::{hex_addr, hex_addr_width, pretty_dump, word32_to_hex, IoMem, Labels};
use crate::report::view::{with_renames, Format, Spec, View};
use crate::report::{show_values, value_view, Env};
use crate::word::W;

type Mem = IoMem<Wasm32Isa<W>>;

pub fn wasm32_views() -> Vec<View<Wasm32St>> {
    with_renames(
        &[
            ("stackLayout", "stack:layout"),
            ("stackDump", "stack:dump"),
            ("frameBase", "frame-base"),
            ("ctrlSp", "ctrl-sp"),
            ("stackRoot", "stack-root"),
        ],
        vec![
            values("stack", "Operand stack of the current frame, top first: `a:b:c`.", stack_words),
            values("locals", "Locals of the current frame, `$0` first.", local_words),
            layout(
                "stack:layout",
                "Call frames, outermost first: locals, open scopes, operands; `<frames>` keeps only the innermost ones.",
                render_stack_layout,
            ),
            layout(
                "stack:dump",
                "Stack memory word by word, by address, with pointer tags; `<frames>` keeps only the innermost frames.",
                render_stack_dump,
            ),
            value_view("sp", "Address of the topmost operand.", |e: &Env<Wasm32St>| e.state.sp),
            value_view("frame-base", "Address of local 0 of the current frame.", |e: &Env<Wasm32St>| {
                e.state.frame_base
            }),
            value_view("ctrl-sp", "Top of the control stack.", |e: &Env<Wasm32St>| e.state.ctrl_sp),
            value_view("stack-root", "Where both stacks start, set by `sp.init`.", |e: &Env<Wasm32St>| {
                e.state.stack_root
            }),
        ],
    )
}

fn values(name: &str, doc: &str, f: fn(&Wasm32St) -> Vec<W>) -> View<Wasm32St> {
    View::new(
        Spec::lit(name).format(&[Format::Dec, Format::Hex], Format::Dec),
        doc,
        move |e, a| {
            let xs: Vec<i64> = f(e.state).into_iter().map(i64::from).collect();
            show_values(a[0].fmt(), &xs)
        },
    )
}

type Render = fn(&Labels, &Wasm32St, &LayoutFormat) -> String;

fn layout(name: &str, doc: &str, render: Render) -> View<Wasm32St> {
    View::new(
        Spec::lit(name)
            .format(&[Format::Dec, Format::Hex], Format::Dec)
            .opt_arg("frames"),
        doc,
        move |e, a| render(e.labels, e.state, &layout_format(e.state, a[0].fmt(), a[1].opt_int())),
    )
}

struct LayoutFormat {
    word: Box<dyn Fn(W) -> String>,
    addr: Box<dyn Fn(i64) -> String>,
    frame_limit: Option<i64>,
}

fn layout_format(st: &Wasm32St, fmt: Format, limit: Option<i64>) -> LayoutFormat {
    if fmt == Format::Hex {
        let width = hex_addr_width(st.mem.capacity());
        LayoutFormat {
            word: Box::new(word32_to_hex),
            addr: Box::new(move |a| hex_addr(width, a)),
            frame_limit: limit,
        }
    } else {
        LayoutFormat {
            word: Box::new(|w| w.to_string()),
            addr: Box::new(|a| a.to_string()),
            frame_limit: limit,
        }
    }
}

/// The word at `addr`, as a read would see it, without recording the read.
fn word_at(mem: &Mem, addr: i64) -> Option<W> {
    mem.clone().read_word(addr).ok()
}

/// `from, from + step, ..` while not past `to`, as Haskell's `[a, b .. c]`.
fn range(from: i64, step: i64, to: i64) -> impl Iterator<Item = i64> {
    std::iter::successors(Some(from), move |a| Some(a + step))
        .take_while(move |&a| if step > 0 { a <= to } else { a >= to })
}

fn frame_stack_base(st: &Wasm32St) -> i64 {
    st.frame_base - st.local_count * WORD
}

fn stack_words(st: &Wasm32St) -> Vec<W> {
    range(st.sp, WORD, frame_stack_base(st))
        .filter_map(|a| word_at(&st.mem, a))
        .collect()
}

fn local_words(st: &Wasm32St) -> Vec<W> {
    range(st.frame_base, -WORD, frame_stack_base(st) + WORD)
        .filter_map(|a| word_at(&st.mem, a))
        .collect()
}

/// One call frame: where it is, how far its operands reach down, and the
/// control records it owns as `(lo, hi, scope)`.
struct FrameLayout {
    pc: i64,
    frame_base: i64,
    local_count: i64,
    scan_lower: i64,
    controls: Vec<(i64, i64, Scope)>,
}

/// The frames, innermost first.
fn walk_frames(st: &Wasm32St) -> Vec<FrameLayout> {
    let addrs = st.scope_addrs();
    let mut frames = Vec::new();
    let (mut fb, mut lc, mut pc, mut scan_lower) = (st.frame_base, st.local_count, st.pc, st.sp);
    let mut rest: &[i64] = &addrs;
    loop {
        let mut controls = Vec::new();
        let mut next = None;
        while let Some((&addr, outer)) = rest.split_first() {
            rest = outer;
            let Some(scope) = read_scope_from(&st.mem, addr) else {
                rest = &[];
                break;
            };
            controls.push((addr, addr + SCOPE_RECORD_BYTES, scope));
            if let Scope::Call {
                caller_frame_base,
                caller_local_count,
                return_pc,
                ..
            } = scope
            {
                next = Some((caller_frame_base, caller_local_count, return_pc));
                break;
            }
        }
        controls.sort_by_key(|c| c.0);
        frames.push(FrameLayout {
            pc,
            frame_base: fb,
            local_count: lc,
            scan_lower,
            controls,
        });
        match next {
            None => return frames,
            Some((fb2, lc2, pc2)) => {
                scan_lower = fb + WORD;
                (fb, lc, pc) = (fb2, lc2, pc2);
            }
        }
    }
}

fn read_scope_from(mem: &Mem, addr: i64) -> Option<Scope> {
    let words: Option<Vec<W>> = scope_word_addrs(addr).into_iter().map(|a| word_at(mem, a)).collect();
    unpack_words(&words?).ok()
}

/// The last `limit` items, all if no limit.
fn keep_last<T>(limit: Option<i64>, xs: Vec<T>) -> Vec<T> {
    match limit {
        None => xs,
        Some(n) => {
            let drop = (xs.len() as i64 - n).max(0) as usize;
            xs.into_iter().skip(drop).collect()
        }
    }
}

/// The label nearest below `addr`: the function it is in.
fn func_name_at(labels: &Labels, addr: i64) -> String {
    let mut pairs: Vec<(&String, &W)> = labels.iter().collect();
    // shortcut: the original takes the first of tied labels in HashMap order; alphabetical here.
    pairs.sort();
    let mut best: Option<(i64, &String)> = None;
    for (label, &a) in pairs {
        let a = a as i64;
        if a <= addr && best.is_none_or(|(b, _)| b < a) {
            best = Some((a, label));
        }
    }
    best.map_or("?".to_string(), |(_, l)| l.clone())
}

fn describe_scope(show_addr: &dyn Fn(i64) -> String, scope: &Scope) -> (String, Vec<String>) {
    match scope {
        Scope::Loop { start, end } => (
            format!(
                "LoopScope {{ csStart = {}, csEnd = {} }}",
                show_addr(*start),
                show_addr(*end)
            ),
            Vec::new(),
        ),
        Scope::Block { end } => (format!("BlockScope {{ csEnd = {} }}", show_addr(*end)), Vec::new()),
        Scope::Call {
            caller_frame_base,
            caller_local_count,
            return_pc,
            result_count,
        } => (
            "CallScope".to_string(),
            vec![
                format!("{{ csCallerFrameBase = {}", show_addr(*caller_frame_base)),
                format!(", csCallerLocalCount = {}", caller_local_count),
                format!(", csReturnPc = {}", show_addr(*return_pc)),
                format!(", csResultCount = {} }}", result_count),
            ],
        ),
    }
}

fn pointer_tags(pointers: &[(&str, i64)], addr: i64) -> String {
    pointers
        .iter()
        .filter(|(_, p)| *p == addr)
        .map(|(name, _)| format!(" [{}]", name))
        .collect()
}

fn render_stack_layout(labels: &Labels, st: &Wasm32St, lf: &LayoutFormat) -> String {
    let show_addr = &lf.addr;
    let show_word = |a: i64| word_at(&st.mem, a).map_or("?".to_string(), |w| (lf.word)(w));
    let mut frames = walk_frames(st);
    frames.reverse();
    let live_index = frames.len() - 1;
    let locals_range = |f: &FrameLayout| {
        (f.local_count != 0).then(|| (f.frame_base - f.local_count * WORD + WORD, f.frame_base + WORD))
    };
    let tagged: Vec<(usize, Option<&FrameLayout>, &FrameLayout)> = frames
        .iter()
        .enumerate()
        .map(|(no, f)| (no, no.checked_sub(1).map(|c| &frames[c]), f))
        .collect();
    let total = tagged.len();
    let kept = keep_last(lf.frame_limit, tagged);
    let mut out: Vec<String> = Vec::new();
    if total > kept.len() {
        out.push(format!("  ({} earlier frame(s) omitted)", total - kept.len()));
    }
    for (no, caller, f) in kept {
        let is_live = no == live_index;
        let mut pointers: Vec<(&str, i64)> = Vec::new();
        if is_live {
            pointers.push(("sp", st.sp));
            if f.local_count > 0 {
                pointers.push(("frameBase", st.frame_base));
            }
        }
        let indexed_span = |out: &mut Vec<String>, lo: i64, hi: i64, tag: &str, note: &dyn Fn(i64) -> String| {
            out.push(format!("  mem[{}..{}]: {}", show_addr(lo), show_addr(hi - 1), tag));
            for (idx, addr) in range(hi - WORD, -WORD, lo).enumerate() {
                out.push(format!(
                    "    {}..{}: {} ({}){}{}",
                    show_addr(addr),
                    show_addr(addr + WORD - 1),
                    show_word(addr),
                    idx,
                    pointer_tags(&pointers, addr),
                    note(addr)
                ));
            }
        };
        let alias_note = |addr: i64| match caller.and_then(|c| locals_range(c).map(|r| (c, r))) {
            Some((c, (lo, hi))) if addr >= lo && addr < hi => {
                format!(" [also #{} local {}]", no - 1, (c.frame_base - addr).div_euclid(WORD))
            }
            _ => String::new(),
        };
        let live_note = if is_live { ", live" } else { "" };
        out.push(format!(
            "#{} {} (pc={}{})",
            no,
            func_name_at(labels, f.pc),
            show_addr(f.pc),
            live_note
        ));
        let op_top = f.frame_base - f.local_count * WORD;
        if f.local_count != 0 {
            indexed_span(&mut out, op_top + WORD, f.frame_base + WORD, "locals", &alias_note);
        }
        for (lo, hi, scope) in &f.controls {
            let (first, rest) = describe_scope(show_addr, scope);
            out.push(format!("  mem[{}..{}]: {}", show_addr(*lo), show_addr(hi - 1), first));
            out.extend(rest.into_iter().map(|l| format!("    {}", l)));
        }
        let operand_top = op_top + WORD;
        if f.scan_lower >= operand_top {
            out.push(format!("  (operands): empty, base={}", show_addr(operand_top)));
        } else {
            indexed_span(&mut out, f.scan_lower, operand_top, "(operands)", &|_| String::new());
        }
    }
    out.join("\n")
}

/// A run of lines over `lo..hi`.
type Item = (i64, i64, Vec<String>);

fn render_stack_dump(labels: &Labels, st: &Wasm32St, lf: &LayoutFormat) -> String {
    let show_addr = &lf.addr;
    let mem_end = st.mem.capacity();
    let mut walked = walk_frames(st);
    walked.reverse();
    let frames: Vec<(usize, &FrameLayout)> = walked.iter().enumerate().collect();
    let kept = keep_last(lf.frame_limit, frames.clone());
    let omitted = frames.len() - kept.len();

    let mut pointers: Vec<(&str, i64)> = vec![("sp", st.sp)];
    if st.local_count > 0 {
        pointers.push(("frameBase", st.frame_base));
    }
    pointers.push(("ctrlSp", st.ctrl_sp));
    let pointers = &pointers;

    let owner = |(no, f): (usize, &FrameLayout)| format!("#{} {}", no, func_name_at(labels, f.pc));
    let frame_locals = |fr: (usize, &FrameLayout)| -> Vec<(i64, String)> {
        let f = fr.1;
        (0..f.local_count)
            .map(|idx| (f.frame_base - idx * WORD, format!("{} local {}", owner(fr), idx)))
            .collect()
    };
    let frame_operands = |fr: (usize, &FrameLayout)| -> Vec<(i64, String)> {
        let f = fr.1;
        let op_top = f.frame_base - f.local_count * WORD;
        range(op_top, -WORD, f.scan_lower)
            .enumerate()
            .map(|(idx, addr)| (addr, format!("{} operand {}", owner(fr), idx)))
            .collect()
    };
    let mut roles: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    for &fr in frames.iter().rev() {
        for (addr, role) in frame_locals(fr).into_iter().chain(frame_operands(fr)) {
            roles.entry(addr).or_default().push(role);
        }
    }

    let word_line = |addr: i64| {
        let value = word_at(&st.mem, addr).map_or("?".to_string(), |w| (lf.word)(w));
        let owners = roles.get(&addr).map(|r| r.join(", ")).unwrap_or_default();
        format!(
            "mem[{}..{}]: {}  {}{}",
            show_addr(addr),
            show_addr(addr + WORD - 1),
            value,
            owners,
            pointer_tags(pointers, addr)
        )
    };

    let mut items: Vec<Item> = Vec::new();
    let mut shown: HashSet<i64> = HashSet::new();
    for &fr in kept.iter().rev() {
        let mut locals: Vec<(i64, String)> = frame_locals(fr)
            .into_iter()
            .filter(|(a, _)| !shown.contains(a))
            .collect();
        locals.sort_by_key(|l| l.0);
        let operands: Vec<(i64, String)> = frame_operands(fr)
            .into_iter()
            .filter(|(a, _)| !shown.contains(a))
            .collect();
        shown.extend(locals.iter().chain(&operands).map(|l| l.0));
        if let Some(&(lo, _)) = locals.first() {
            let hi = locals.iter().map(|l| l.0).max().unwrap_or(lo) + WORD;
            let mut lines = vec![format!(
                "mem[{}..{}]: {} locals",
                show_addr(lo),
                show_addr(hi - 1),
                owner(fr)
            )];
            lines.extend(locals.iter().map(|(a, _)| format!("    {}", word_line(*a))));
            items.push((lo, hi, lines));
        }
        items.extend(operands.iter().map(|&(a, _)| (a, a + WORD, vec![word_line(a)])));
    }
    for &fr in &kept {
        for (lo, hi, scope) in &fr.1.controls {
            let (first, rest) = describe_scope(show_addr, scope);
            let mut lines = vec![format!(
                "mem[{}..{}]: {}  {}{}",
                show_addr(*lo),
                show_addr(hi - 1),
                first,
                owner(fr),
                pointer_tags(pointers, *lo)
            )];
            lines.extend(rest.into_iter().map(|l| format!("    {}", l)));
            items.push((*lo, *hi, lines));
        }
    }
    if let Some((_, f)) = kept.first() {
        if let (Some(&(call_lo, _, _)), true) = (f.controls.first(), omitted > 0) {
            let lo = f.frame_base + WORD;
            items.push((
                lo,
                call_lo,
                vec![format!(
                    "mem[{}..{}]: ({} earlier frame(s) omitted)",
                    show_addr(lo),
                    show_addr(call_lo - 1),
                    omitted
                )],
            ));
        }
    }

    // Memory between the items, as the memory dump shows it, split at the pointers.
    let raw = |lo: i64, hi: i64| -> Vec<String> {
        let (lo, hi) = (lo.max(0), hi.min(mem_end));
        if lo >= hi {
            return Vec::new();
        }
        let mut bounds: Vec<i64> = vec![lo, hi];
        bounds.extend(pointers.iter().map(|p| p.1).filter(|&p| lo < p && p < hi));
        bounds.sort();
        bounds.dedup();
        bounds
            .windows(2)
            .flat_map(|ab| {
                let cells = st.mem.cells.slice(ab[0], ab[1] - 1).unwrap_or_else(|e| panic!("{}", e));
                let mut lines: Vec<String> = pretty_dump(true, show_addr, labels, &cells)
                    .lines()
                    .map(String::from)
                    .collect();
                if let Some(first) = lines.first_mut() {
                    first.push_str(&pointer_tags(pointers, ab[0]));
                }
                lines
            })
            .collect()
    };

    let start = items.iter().map(|i| i.0).fold(st.sp, i64::min).max(0);
    items.sort_by_key(|i| i.0);
    let mut out = Vec::new();
    let mut cursor = start;
    for (lo, hi, lines) in items {
        out.extend(raw(cursor, lo));
        out.extend(lines);
        cursor = cursor.max(hi);
    }
    out.extend(raw(cursor, mem_end));
    out.join("\n")
}
