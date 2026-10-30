//! Report views: template compilation, key matching, errors, and the
//! generated `docs/report-views.md`.

use std::collections::BTreeMap;

use wrench::isa::{acc32::Acc32St, f32a::F32aSt, m68k::M68kSt, risc_iv::RiscIvSt, vliw_iv::VliwIvSt, wasm32::Wasm32St};
use wrench::machine::Machine;
use wrench::memory::{AccessCount, IoMem, Labels, Mem};
use wrench::report::view::{
    compile_template, render_template, views_table, with_renames, CheckEnv, Format, Spec, Val, View,
};
use wrench::report::{isa_views, shared_views, Env};
use wrench::rng::{RandomStream, StdGen};
use wrench::translator::DumpStats;

type S = RiscIvSt;

fn fmt_name(f: Format) -> &'static str {
    match f {
        Format::Dec => "Dec",
        Format::Hex => "Hex",
        Format::Sym => "Sym",
        Format::Bin => "Bin",
    }
}

/// Views that render the key they parsed.
fn test_views() -> Vec<View<S>> {
    let dec_hex = [Format::Dec, Format::Hex];
    with_renames(
        &[("memory:table", "mem:table"), ("memory", "mem")],
        vec![
            View::new(Spec::lit("pc").format(&dec_hex, Format::Dec), "", |_, a: &[Val]| {
                format!("pc:{}", fmt_name(a[0].fmt()))
            }),
            View::new(Spec::lit("pc:label"), "", |_, _| "pc:label".to_string()),
            View::new(Spec::lit("mem:table"), "", |_, _| "mem:table".to_string()),
            View::checked(
                Spec::lit("mem").arg("from").arg("to"),
                "",
                |c: &CheckEnv, a: &[Val]| {
                    let (from, to) = (a[0].int(), a[1].int());
                    if to >= c.memory_size {
                        Err(format!(
                            "range {}..{} is outside memory 0..{}",
                            from,
                            to,
                            c.memory_size - 1
                        ))
                    } else if from > to {
                        Err(format!("range {}..{} is empty", from, to))
                    } else {
                        Ok(())
                    }
                },
                |_, a: &[Val]| format!("mem:{}:{}", a[0].int(), a[1].int()),
            ),
            View::new(Spec::lit("count").format_required(&dec_hex), "", |_, a: &[Val]| {
                fmt_name(a[0].fmt()).to_string()
            }),
            View::new(
                Spec::lit("frames").format(&dec_hex, Format::Dec).opt_arg("frames"),
                "",
                |_, a: &[Val]| {
                    let n = a[1].opt_int().map_or("Nothing".to_string(), |n| format!("Just {}", n));
                    format!("frames:{}:{}", fmt_name(a[0].fmt()), n)
                },
            ),
            View::new(Spec::lit("T1").format(&dec_hex, Format::Dec), "", |_, _| {
                "T1".to_string()
            }),
            View::new(Spec::lit("echo"), "", |_, _| "{pc}".to_string()),
        ],
    )
}

fn check_env() -> CheckEnv {
    CheckEnv {
        memory_size: 16,
        io_ports: vec![0],
    }
}

fn with_env<M: Machine, T>(f: impl FnOnce(&Env<M>) -> T) -> T {
    let mem = Mem {
        size: 16,
        data: imbl::Vector::new(),
    };
    let st = M::init(0, IoMem::new(BTreeMap::new(), mem), RandomStream::new(StdGen::new(0)));
    let labels = Labels::new();
    let stats = DumpStats::default();
    let none = String::new;
    f(&Env {
        labels: &labels,
        state: &st,
        final_state: &st,
        dump_stats: &stats,
        instr_count: 0,
        prev_instr: None,
        accesses: AccessCount::default(),
        instruction_costs: &none,
    })
}

fn rendered(input: &str) -> Result<String, Vec<String>> {
    let (_, t) = compile_template(&check_env(), &test_views(), input)?;
    Ok(with_env::<S, _>(|e| render_template(&t, e)))
}

fn warnings(input: &str) -> Vec<String> {
    compile_template(&check_env(), &test_views(), input).map_or_else(|e| e, |(w, _)| w)
}

fn assert_error_for(expected: &str, key: &str) {
    match rendered(&format!("{{{}}}", key)) {
        Err(errs) if errs.iter().any(|e| e.contains(expected)) => {}
        other => panic!("expected an error with {:?}, got {:?}", expected, other),
    }
}

#[test]
fn a_template_fills_each_hole_once() {
    assert_eq!(rendered(""), Ok(String::new()));
    assert_eq!(
        rendered("pc={pc} label={pc:label}"),
        Ok("pc=pc:Dec label=pc:label".to_string())
    );
    assert_eq!(rendered("a { b"), Ok("a { b".to_string()));
    assert_eq!(rendered("a } {pc:hex} {"), Ok("a } pc:Hex {".to_string()));
    assert_eq!(rendered("{echo} {pc}"), Ok("{pc} pc:Dec".to_string()));
}

#[test]
fn default_format_is_filled_in() {
    assert_eq!(rendered("{frames:hex:2}"), Ok("frames:Hex:Just 2".to_string()));
    assert_eq!(rendered("{frames}"), Ok("frames:Dec:Nothing".to_string()));
}

#[test]
fn longest_literal_prefix_wins() {
    assert_eq!(rendered("{mem:table}"), Ok("mem:table".to_string()));
    assert_eq!(rendered("{mem:0:0x0f}"), Ok("mem:0:15".to_string()));
    assert_error_for("usage: {mem:table}", "mem:table:hex");
}

#[test]
fn an_old_name_still_works_with_a_warning() {
    assert_eq!(rendered("{memory:0:0x0f}"), Ok("mem:0:15".to_string()));
    assert_eq!(
        warnings("{memory:0:0x0f} {memory:table} {mem:table}"),
        vec![
            "{memory:0:0x0f} is renamed, use {mem:0:0x0f}",
            "{memory:table} is renamed, use {mem:table}"
        ]
    );
    assert_error_for("usage: {memory:table}", "memory:table:hex");
}

#[test]
fn errors_name_the_problem_and_the_usage() {
    assert_error_for(
        "unknown format 'bin', one of: dec, hex, usage: {pc[:dec|hex]}",
        "pc:bin",
    );
    assert_error_for("missing format, one of: dec, hex", "count");
    assert_error_for("<frames> should be a number, got 'x'", "frames:dec:x");
    assert_error_for("<from> should be a number", "mem:a:b");
}

#[test]
fn arguments_are_checked_against_the_configuration() {
    assert_error_for("{mem:0:16}: range 0..16 is outside memory 0..15", "mem:0:16");
    assert_error_for("range 8..4 is empty", "mem:8:4");
}

#[test]
fn unknown_keys_suggest_close_keywords() {
    assert_error_for("unknown view {t2}, did you mean {T1[:dec|hex]}?", "t2");
    assert_error_for("unknown view {memry:table}, did you mean {mem:table}?", "memry:table");
    assert_eq!(rendered("{foo}"), Err(vec!["unknown view {foo}".to_string()]));
}

/// Each example key of each view parses under that view and no other.
fn unambiguous<M: Machine>(name: &str) {
    let mut views = isa_views::<M>();
    views.extend(shared_views::<M>());
    let accepts = |v: &View<M>, key: &str| {
        compile_template(&check_env(), std::slice::from_ref(v), &format!("{{{}}}", key)).is_ok()
    };
    for v in &views {
        for key in v.examples() {
            let takers: Vec<String> = views.iter().filter(|w| accepts(w, &key)).map(|w| w.usage()).collect();
            assert_eq!(
                takers,
                vec![v.usage()],
                "{}: {{{}}}, an example of {}",
                name,
                key,
                v.usage()
            );
        }
    }
}

#[test]
fn every_example_key_selects_its_own_view() {
    unambiguous::<Acc32St>("acc32");
    unambiguous::<F32aSt>("f32a");
    unambiguous::<M68kSt>("m68k");
    unambiguous::<RiscIvSt>("risc-iv-32");
    unambiguous::<VliwIvSt>("vliw-iv");
    unambiguous::<Wasm32St>("wasm32");
}

/// `docs/report-views.md`, generated from the views so it cannot fall
/// behind them. `WRENCH_ACCEPT=1` rewrites it.
#[test]
fn views_reference_is_up_to_date() {
    let section = |name: &str, table: String| format!("## {}\n\n{}", name, table);
    let parts = [
        "# Report views\n".to_string(),
        "<!-- Generated from the source by `make test-accept`. Do not edit. -->\n".to_string(),
        "Every expression a report `view` can use inside `{...}`. `[...]` is optional, `a|b` is a choice, `<name>` is a number (decimal or `0x` hex). See [Reports configuration](README.md#reports-configuration).\n".to_string(),
        section("All ISAs", views_table(&shared_views::<RiscIvSt>())),
        section("acc32", views_table(&isa_views::<Acc32St>())),
        section("f32a", views_table(&isa_views::<F32aSt>())),
        section("m68k", views_table(&isa_views::<M68kSt>())),
        section("risc-iv-32", views_table(&isa_views::<RiscIvSt>())),
        section("vliw-iv", views_table(&isa_views::<VliwIvSt>())),
        section("wasm32", views_table(&isa_views::<Wasm32St>())),
    ];
    let actual = parts.join("\n");
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/report-views.md");
    if std::env::var("WRENCH_ACCEPT").is_ok_and(|v| v == "1") {
        std::fs::write(path, &actual).unwrap();
    }
    let expected = std::fs::read_to_string(path).unwrap();
    assert!(
        expected == actual,
        "docs/report-views.md is out of date; rerun with WRENCH_ACCEPT=1"
    );
}
