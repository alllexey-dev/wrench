//! What the golden corpus in `test/golden/wasm32` cannot check: the
//! arithmetic and memory edge cases no program there reaches, and every way
//! the machine is meant to refuse a program.

use std::collections::BTreeMap;

use super::*;
use crate::bitpack::{pack_words, unpack_words};
use crate::machine::{Machine, HALTED};
use crate::memory::{IoMem, IoStream, Labels};
use crate::report::render_isa_view;
use crate::rng::{RandomStream, StdGen};
use crate::translator::translate;

fn gen_field(g: &mut StdGen, width: u32) -> i64 {
    g.uniform_r(0, (1 << width) - 1)
}

fn gen_scope(g: &mut StdGen) -> Scope {
    match g.uniform_r(0, 2) {
        0 => Scope::Block { end: gen_field(g, 22) },
        1 => Scope::Loop {
            start: gen_field(g, 22),
            end: gen_field(g, 22),
        },
        _ => Scope::Call {
            caller_frame_base: gen_field(g, 22),
            caller_local_count: gen_field(g, 8),
            return_pc: gen_field(g, 22),
            result_count: gen_field(g, 8),
        },
    }
}

/// The record with the named field set to `value`.
fn with_field(scope: Scope, name: &str, value: i64) -> Scope {
    match scope {
        Scope::Block { .. } => Scope::Block { end: value },
        Scope::Loop { end, .. } if name == "csStart" => Scope::Loop { start: value, end },
        Scope::Loop { start, .. } => Scope::Loop { start, end: value },
        Scope::Call {
            mut caller_frame_base,
            mut caller_local_count,
            mut return_pc,
            mut result_count,
        } => {
            match name {
                "csCallerFrameBase" => caller_frame_base = value,
                "csCallerLocalCount" => caller_local_count = value,
                "csReturnPc" => return_pc = value,
                _ => result_count = value,
            }
            Scope::Call {
                caller_frame_base,
                caller_local_count,
                return_pc,
                result_count,
            }
        }
    }
}

#[test]
fn a_control_record_round_trips() {
    let mut g = StdGen::new(1);
    for _ in 0..1000 {
        let scope = gen_scope(&mut g);
        assert_eq!(pack_words(&scope).and_then(|w| unpack_words(&w)), Ok(scope));
    }
}

#[test]
fn any_two_words_decode_to_a_record_that_round_trips_or_fail_on_the_tag() {
    let mut g = StdGen::new(2);
    for _ in 0..1000 {
        let words = [g.next_u64() as W, g.next_u64() as W];
        match unpack_words::<Scope>(&words) {
            Err(err) => assert_eq!(err, "record kind: unknown tag 3"),
            Ok(scope) => assert_eq!(pack_words(&scope).and_then(|w| unpack_words(&w)), Ok(scope)),
        }
    }
}

#[test]
fn a_field_too_wide_for_the_record_is_named_in_the_error() {
    let mut g = StdGen::new(3);
    for _ in 0..1000 {
        let scope = gen_scope(&mut g);
        let fields: &[(&str, u32)] = match scope {
            Scope::Block { .. } => &[("csEnd", 22)],
            Scope::Loop { .. } => &[("csStart", 22), ("csEnd", 22)],
            Scope::Call { .. } => &[
                ("csCallerFrameBase", 22),
                ("csCallerLocalCount", 8),
                ("csReturnPc", 22),
                ("csResultCount", 8),
            ],
        };
        let (name, width) = fields[g.uniform_r(0, fields.len() as i64 - 1) as usize];
        let value = g.uniform_r(1 << width, 1 << (width + 4));
        assert_eq!(
            pack_words(&with_field(scope, name, value)),
            Err(format!("{}: {} doesn't fit in {} bits", name, value, width))
        );
    }
}

#[test]
fn an_unknown_tag_is_rejected() {
    assert_eq!(
        unpack_words::<Scope>(&[-1, 0]),
        Err("record kind: unknown tag 3".to_string())
    );
}

#[test]
fn div_s_truncates_toward_zero_and_rem_s_follows_the_dividend() {
    assert_eq!(run_to_stack(&["i32.const -7", "i32.const 2", "i32.div_s"]), "-3");
    assert_eq!(run_to_stack(&["i32.const 7", "i32.const -2", "i32.div_s"]), "-3");
    assert_eq!(run_to_stack(&["i32.const -7", "i32.const 2", "i32.rem_s"]), "-1");
    assert_eq!(run_to_stack(&["i32.const 7", "i32.const -2", "i32.rem_s"]), "1");
}

#[test]
fn a_zero_divisor_traps_and_so_does_the_one_unrepresentable_quotient() {
    assert_trap("integer divide by zero", &["i32.const 1", "i32.const 0", "i32.div_s"]);
    assert_trap("integer divide by zero", &["i32.const 1", "i32.const 0", "i32.rem_s"]);
    assert_trap(
        "integer overflow",
        &["i32.const -2147483648", "i32.const -1", "i32.div_s"],
    );
    assert_eq!(
        run_to_stack(&["i32.const -2147483648", "i32.const -1", "i32.rem_s"]),
        "0"
    );
}

#[test]
fn byte_loads_sign_or_zero_extend_a_high_bit_byte() {
    let store = |load: &str| {
        with_scratch(&[
            "i32.const scratch",
            "i32.const 0x80",
            "i32.store8",
            "i32.const scratch",
            load,
        ])
    };
    assert_eq!(run_source_to_stack(&store("i32.load8_u")), "128");
    assert_eq!(run_source_to_stack(&store("i32.load8_s")), "-128");
}

#[test]
fn a_load_store_offset_indexes_without_an_add() {
    let src = lines(&[
        ".data",
        "a0: .word 10",
        "a1: .word 20",
        "a2: .word 30",
        ".text",
        "_start:",
        "    i32.const a0",
        "    i32.const 99",
        "    i32.store 0x4",
        "    i32.const a0",
        "    i32.load  8",
        "    i32.const a0",
        "    i32.load  4",
        "    i32.const a0",
        "    i32.load",
        "    halt",
    ]);
    assert_eq!(run_source_to_stack(&src), "10:99:30");
}

#[test]
fn an_immediate_too_wide_for_its_field_is_rejected_where_it_is_written() {
    assert_translation_error("i32.load offset 256 is not in 0..255", &["i32.const 0", "i32.load 256"]);
    assert_translation_error("local.get index 300 is not in 0..255", &["local.get 300"]);
    assert_translation_error("br depth -1 is not in 0..255", &["block", "br -1", "end"]);
    assert_translation_error("call resultCount 300 is not in 0..255", &["i32.const 0", "call 0, 300"]);
    assert_translation_error("locals.reserve count 300 is not in 0..255", &["locals.reserve 300"]);
    assert_translation_error("4:14:", &["i32.const 0", "i32.load 256"]);
    assert_translation_error("expected", &["block", "br start", "end"]);
}

#[test]
fn a_locals_name_is_the_same_instruction_as_its_index() {
    let program = |uses: &[&str]| {
        let mut ls = vec![".frame () $a, $b", "locals.reserve 2"];
        ls.extend(uses);
        to_source(&ls)
    };
    let by_name = run_source_to_stack(&program(&[
        "i32.const 3 local.set $a",
        "i32.const 4 local.set $b",
        "local.get $a local.get 1 i32.sub",
    ]));
    let by_index = run_source_to_stack(&program(&[
        "i32.const 3 local.set 0",
        "i32.const 4 local.set 1",
        "local.get 0 local.get 1 i32.sub",
    ]));
    assert_eq!(by_name, "-1");
    assert_eq!(by_index, by_name);
}

#[test]
fn the_reserved_locals_are_numbered_after_the_parameters() {
    let src = lines(&[
        ".text",
        "f:",
        "    .frame ($a, $b) $c",
        "    locals.reserve 1",
        "    i32.const 30 local.set $c",
        "    local.get $a local.get $b local.get 2",
        "    halt",
        "_start:",
        "    i32.const 10 i32.const 20",
        "    i32.const f call 2, 0",
    ]);
    assert_eq!(run_source_to_stack(&src), "30:20:10");
}

#[test]
fn the_parentheses_may_follow_frame_without_a_space() {
    let src = to_source(&[
        ".frame() $a",
        "locals.reserve 1",
        "i32.const 7 local.set $a",
        "local.get $a",
    ]);
    assert_eq!(run_source_to_stack(&src), "7");
}

#[test]
fn names_hold_until_the_next_label() {
    assert_source_translation_error(
        "-:6:15:",
        &lines(&[
            ".text",
            "f:",
            "    .frame ($a)",
            "    local.get $a",
            "_start:",
            "    local.get $a",
            "    halt",
        ]),
    );
}

#[test]
fn a_frame_out_of_form_is_rejected_where_it_is_written() {
    let malformed = "malformed .frame, expected distinct names as in .frame ($a, $b) $c";
    assert_translation_error(malformed, &[".frame $a"]);
    assert_translation_error("-:3:5:", &[".frame $a"]);
    assert_translation_error(malformed, &[".frame"]);
    assert_translation_error(malformed, &[".frame ($a, $b"]);
    assert_translation_error(malformed, &[".frame ($a) ) $b"]);
    assert_translation_error(malformed, &[".frame ($a, b) $c"]);
    assert_translation_error(malformed, &[".frame ($a $b)"]);
    assert_translation_error(malformed, &[".frame ($a, $a)"]);
    assert_translation_error(malformed, &[".frame ($a) $a"]);
}

#[test]
fn a_name_the_frame_does_not_list_is_left_for_the_parser() {
    assert_translation_error("-:4:15:", &[".frame ($a)", "local.get $x"]);
}

#[test]
fn a_parse_error_on_a_line_with_a_name_shows_the_line_as_written() {
    assert_translation_error("local.get $a i32.konst", &[".frame ($a)", "local.get $a i32.konst 1"]);
}

#[test]
fn a_scope_with_no_matching_end_fails_at_the_scope() {
    assert_trap("scope at 0x00 has no matching end", &["block"]);
    assert_trap("has no matching end", &["loop", "block", "end"]);
}

#[test]
fn a_br_cannot_reach_past_the_enclosing_function() {
    assert_source_trap(
        "depth reaches past the enclosing function",
        &lines(&[
            ".text",
            "_start:",
            "    block",
            "        i32.const f",
            "        call 0, 0",
            "f:",
            "        br 0",
            "    end",
            "    halt",
        ]),
    );
}

#[test]
fn a_scope_cannot_span_the_gap_between_two_code_sections() {
    assert_source_trap(
        "has no matching end",
        &lines(&[
            ".text",
            "_start:",
            "    block",
            ".data",
            "pad: .word 0",
            ".text",
            "    end",
            "    halt",
        ]),
    );
    let src = lines(&[
        ".text",
        "helper:",
        "    block",
        "        i32.const 1",
        "        br 0",
        "    end",
        "    return",
        ".data",
        "pad: .word 0",
        ".text",
        "_start:",
        "    block",
        "        i32.const 7",
        "        br 0",
        "    end",
        "    halt",
    ]);
    assert_eq!(run_source_to_stack(&src), "7");
}

#[test]
fn sp_init_resets_the_frame_to_having_no_locals() {
    let src = to_source(&["locals.reserve 2", "i32.const 0x300", "sp.init"]);
    assert_eq!(run_source_to_view("locals:dec", &src), "");
}

#[test]
fn recursive_call_return_computes_factorial_5() {
    let src = std::fs::read_to_string("test/golden/wasm32/factorial.s").unwrap();
    let st = run_source_io(&[(0x80, vec![5]), (0x84, vec![])], &src).unwrap();
    let out: Vec<W> = st.mem.streams[&0x84].output.iter().copied().collect();
    assert_eq!(out, vec![120]);
}

#[test]
fn indirect_call_through_a_parameter_dispatches_to_the_right_target() {
    let src = lines(&[
        ".text",
        "apply_twice:",
        "    local.get 1",
        "    local.get 0",
        "    call 1, 1",
        "    local.get 0",
        "    call 1, 1",
        "    return",
        "double:",
        "    local.get 0",
        "    i32.const 2",
        "    i32.mul",
        "    return",
        "_start:",
        "    i32.const double",
        "    i32.const 3",
        "    i32.const apply_twice",
        "    call 2, 1",
        "    halt",
    ]);
    assert_eq!(run_source_to_stack(&src), "12");
}

fn lines(ls: &[&str]) -> String {
    ls.iter().map(|l| format!("{}\n", l)).collect()
}

/// `_start:` with these instructions, then `halt`.
fn to_source(instrs: &[&str]) -> String {
    format!(
        ".text\n_start:\n{}    halt\n",
        instrs.iter().map(|i| format!("    {}\n", i)).collect::<String>()
    )
}

/// [`to_source`] after a `.data` word at `scratch`.
fn with_scratch(instrs: &[&str]) -> String {
    format!(".data\nscratch: .word 0\n{}", to_source(instrs))
}

fn run_to_stack(instrs: &[&str]) -> String {
    run_source_to_stack(&to_source(instrs))
}

fn run_source_to_stack(src: &str) -> String {
    run_source_to_view("stack:dec", src)
}

fn run_source_to_view(view: &str, src: &str) -> String {
    let st = run_source_io(&[], src).unwrap();
    render_isa_view(&Labels::new(), &st, view).unwrap()
}

fn assert_trap(expected: &str, instrs: &[&str]) {
    assert_source_trap(expected, &to_source(instrs));
}

fn assert_source_trap(expected: &str, src: &str) {
    match run_source_io(&[], src) {
        Err(err) => assert!(err.contains(expected), "expected {:?}, got: {}", expected, err),
        Ok(_) => panic!("expected a trap matching {:?}", expected),
    }
}

fn assert_translation_error(expected: &str, instrs: &[&str]) {
    assert_source_translation_error(expected, &to_source(instrs));
}

fn assert_source_translation_error(expected: &str, src: &str) {
    match translate::<Wasm32>(1000, &[], "-", src) {
        Err(err) => assert!(err.contains(expected), "expected {:?}, got: {}", expected, err),
        Ok(_) => panic!("expected a translation error matching {:?}", expected),
    }
}

fn run_source_io(streams: &[(i64, Vec<W>)], src: &str) -> Result<Wasm32St, String> {
    let tr = translate::<Wasm32>(1000, &[], "-", src)?;
    let pc = *tr.labels.get("_start").ok_or("_start label should be defined.")?;
    let streams: BTreeMap<i64, IoStream> = streams
        .iter()
        .map(|(a, input)| {
            let stream = IoStream {
                input: input.iter().copied().collect(),
                output: imbl::Vector::new(),
            };
            (*a, stream)
        })
        .collect();
    let mut st = Wasm32St::init(
        pc as i64,
        IoMem::new(streams, tr.dump),
        RandomStream::new(StdGen::new(0)),
    );
    for _ in 0..2000 {
        match st.clone().fetch() {
            Ok(_) => st.step(),
            Err(e) if e == HALTED => return Ok(st),
            Err(e) => return Err(e),
        }
    }
    Err("test program did not halt".to_string())
}
