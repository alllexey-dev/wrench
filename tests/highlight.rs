//! Syntax highlighting of sources and reports, and ISA detection.

use std::path::Path;

use wrench::highlight::{tokenize_line, tokenize_report_line, TokenKind, TokenKind::*};
use wrench::isa::f32a::{F32a, F32aIsa, MNEMONICS as F32A_MNEMONICS};
use wrench::isa::wasm32::MNEMONICS as WASM32_MNEMONICS;
use wrench::isa::{detect_isa, Isa, ALL_ISA};
use wrench::machine::Arch;
use wrench::parser::P;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn sources(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).collect())
        .unwrap_or_default();
    files.retain(|p| p.extension().is_some_and(|e| e == "s"));
    files.sort();
    files
}

fn significant(isa: Isa, line: &str) -> Vec<(TokenKind, String)> {
    tokenize_line(isa, line)
        .into_iter()
        .filter(|(k, _)| *k != Plain)
        .collect()
}

fn significant_report(isa: Isa, line: &str) -> Vec<(TokenKind, String)> {
    tokenize_report_line(isa, line)
        .into_iter()
        .filter(|(k, _)| *k != Plain)
        .collect()
}

fn toks(xs: &[(TokenKind, &str)]) -> Vec<(TokenKind, String)> {
    xs.iter().map(|(k, t)| (*k, t.to_string())).collect()
}

#[test]
fn tokens_concatenate_back_to_every_source() {
    for isa in ALL_ISA {
        for dir in ["example", "test/golden"] {
            for file in sources(&root().join(dir).join(isa.name())) {
                let src = std::fs::read_to_string(&file).unwrap();
                for line in src.lines() {
                    let back: String = tokenize_line(isa, line).into_iter().map(|(_, t)| t).collect();
                    assert_eq!(back, line, "{}", file.display());
                }
            }
        }
    }
}

#[test]
fn risc_iv_32() {
    assert_eq!(
        significant(Isa::RiscIv, "_start:  lw t0, -4(t0)  ; load"),
        toks(&[
            (Label, "_start:"),
            (Mnemonic, "lw"),
            (Register, "t0"),
            (Literal, "-4"),
            (Register, "t0"),
            (Comment, "; load")
        ])
    );
    assert_eq!(
        significant(Isa::RiscIv, "lui t0, %hi(input_addr)"),
        toks(&[(Mnemonic, "lui"), (Register, "t0"), (Reloc, "%hi"), (Ref, "input_addr")])
    );
    assert_eq!(
        significant(Isa::RiscIv, "buf: .byte 'a;b', 0x1_0"),
        toks(&[
            (Label, "buf:"),
            (Directive, ".byte"),
            (Literal, "'a;b'"),
            (Literal, "0x1_0")
        ])
    );
}

#[test]
fn vliw_iv_slots() {
    assert_eq!(
        significant(Isa::VliwIv, "mul t2, t2, t1 / addi t1, t1, -1 / / j loop"),
        toks(&[
            (Mnemonic, "mul"),
            (Register, "t2"),
            (Register, "t2"),
            (Register, "t1"),
            (Separator, "/"),
            (Mnemonic, "addi"),
            (Register, "t1"),
            (Register, "t1"),
            (Literal, "-1"),
            (Separator, "/"),
            (Separator, "/"),
            (Mnemonic, "j"),
            (Ref, "loop"),
        ])
    );
}

#[test]
fn m68k() {
    assert_eq!(
        significant(Isa::M68k, "move.l D0, -(A7)"),
        toks(&[(Mnemonic, "move.l"), (Register, "D0"), (Register, "A7")])
    );
}

#[test]
fn wasm32() {
    assert_eq!(
        significant(Isa::Wasm32, "i32.const factorial ; call"),
        toks(&[(Mnemonic, "i32.const"), (Ref, "factorial"), (Comment, "; call")])
    );
    assert_eq!(
        significant(Isa::Wasm32, "local.get 0 i32.const 1 i32.sub"),
        toks(&[
            (Mnemonic, "local.get"),
            (Literal, "0"),
            (Mnemonic, "i32.const"),
            (Literal, "1"),
            (Mnemonic, "i32.sub")
        ])
    );
    assert_eq!(
        significant(Isa::Wasm32, ".frame () $n, $acc"),
        toks(&[(Directive, ".frame"), (Register, "$n"), (Register, "$acc")])
    );
    assert_eq!(
        significant(Isa::Wasm32, "local.get $n"),
        toks(&[(Mnemonic, "local.get"), (Register, "$n")])
    );
}

#[test]
fn f32a() {
    assert_eq!(
        significant(Isa::F32a, "while: -1 + ; \\ loop"),
        toks(&[
            (Label, "while:"),
            (Literal, "-1"),
            (Mnemonic, "+"),
            (Mnemonic, ";"),
            (Comment, "\\ loop")
        ])
    );
    assert_eq!(
        significant(Isa::F32a, "@p input_addr a! factorial ;"),
        toks(&[
            (Mnemonic, "@p"),
            (Ref, "input_addr"),
            (Mnemonic, "a!"),
            (Call, "factorial"),
            (Mnemonic, ";")
        ])
    );
    assert_eq!(
        significant(Isa::F32a, "if end lit buf next loop"),
        toks(&[
            (Mnemonic, "if"),
            (Ref, "end"),
            (Mnemonic, "lit"),
            (Ref, "buf"),
            (Mnemonic, "next"),
            (Ref, "loop")
        ])
    );
    assert_eq!(
        significant(Isa::F32a, "buf: .byte 'a\\n'"),
        toks(&[(Label, "buf:"), (Directive, ".byte"), (Literal, "'a\\n'")])
    );
}

fn f32a_instruction(m: &str) -> bool {
    ["\n", " _start\n", " _start ;\n"].iter().any(|suffix| {
        let src: Vec<char> = format!("{}{}", m, suffix).chars().collect();
        let mut p = P::new("", &src);
        matches!(F32a::mnemonic(&mut p), Ok(i) if !matches!(i, F32aIsa::Call(_)))
    })
}

fn wasm32_parses(m: &str) -> bool {
    ["", " 0", " 0, 0"]
        .iter()
        .any(|args| Isa::Wasm32.parses(&format!(".text\n_start:\n    {}{}\n", m, args)))
}

#[test]
fn every_listed_mnemonic_parses() {
    for m in F32A_MNEMONICS {
        assert!(f32a_instruction(m), "f32a: {}", m);
    }
    for m in WASM32_MNEMONICS {
        assert!(wasm32_parses(m), "wasm32: {}", m);
    }
    assert!(!f32a_instruction("bogus"), "f32a control");
    assert!(!wasm32_parses("i32.bogus"), "wasm32 control");
}

#[test]
fn only_labels_are_label_references_in_examples() {
    for isa in [Isa::F32a, Isa::Wasm32] {
        for file in sources(&root().join("example").join(isa.name())) {
            let src = std::fs::read_to_string(&file).unwrap();
            let tokens: Vec<(TokenKind, String)> = src.lines().flat_map(|l| tokenize_line(isa, l)).collect();
            let labels: Vec<&str> = tokens
                .iter()
                .filter(|(k, _)| *k == Label)
                .map(|(_, t)| &t[..t.len() - 1])
                .collect();
            for (_, t) in tokens.iter().filter(|(k, _)| [Ref, Call].contains(k)) {
                assert!(labels.contains(&t.as_str()), "{}: {}", file.display(), t);
            }
        }
    }
}

#[test]
fn report_lines() {
    assert_eq!(
        significant_report(
            Isa::RiscIv,
            "8:\tLw {rd = T0, offsetRs1 = MemRef {mrOffset = 0, mrReg = T0}}\t@_start"
        ),
        toks(&[
            (Separator, "8:"),
            (Mnemonic, "Lw"),
            (Register, "T0"),
            (Mnemonic, "MemRef"),
            (Literal, "0"),
            (Register, "T0"),
            (Label, "@_start"),
        ])
    );
    assert_eq!(
        significant_report(Isa::RiscIv, "mem[0x000..0x003] (4 B): \t48 65 0a ff\t@\"buf\""),
        toks(&[
            (Separator, "mem[0x000..0x003] (4 B):"),
            (Literal, "48"),
            (Literal, "65"),
            (Literal, "0a"),
            (Literal, "ff"),
            (Label, "@\"buf\""),
        ])
    );
    assert_eq!(
        significant_report(Isa::Wasm32, "0x00c:\toutput_addr"),
        toks(&[(Separator, "0x00c:"), (Label, "output_addr")])
    );
    assert_eq!(
        significant_report(Isa::Wasm32, "0x008:\t_start"),
        toks(&[(Separator, "0x008:"), (Label, "_start")])
    );
    assert_eq!(
        significant_report(Isa::RiscIv, "mem:io-ranges:  0x80..0x87 (8 B)"),
        toks(&[
            (Ref, "mem:io-ranges:"),
            (Literal, "0x80"),
            (Literal, "0x87"),
            (Literal, "8")
        ])
    );
    assert_eq!(
        significant_report(Isa::Wasm32, "numio[0x84]: [] >>> [65]"),
        toks(&[(Ref, "numio[0x84]:"), (Literal, "65")])
    );
    assert_eq!(
        significant_report(Isa::Wasm32, "# Check results"),
        toks(&[(Directive, "# Check results")])
    );
}

#[test]
fn report_tokens_concatenate_back() {
    for l in [
        "mem[0x000..0x004] (5 B): \tI32Const 132 \t@_start",
        "128:132",
        "symio[0x84]: \"\" >>> \"Hello\\n\\0World\"",
        "[unknown-view <T0>] -1 @",
    ] {
        let back: String = tokenize_report_line(Isa::RiscIv, l)
            .into_iter()
            .map(|(_, t)| t)
            .collect();
        assert_eq!(back, l);
    }
}

/// Sample programs more than one architecture can parse.
const AMBIGUOUS: [&str; 4] = [
    "example/risc-iv-32/random-memory-init.s",
    "test/golden/acc32/dup.s",
    "test/golden/acc32/label_like_instr.s",
    "test/golden/risc-iv-32/memory_init.s",
];

#[test]
fn every_example_and_golden_source_is_detected() {
    let mut failures = Vec::new();
    for dir in ["example", "test/golden"] {
        for sub in std::fs::read_dir(root().join(dir)).unwrap().filter_map(Result::ok) {
            for file in sources(&sub.path()) {
                let rel = file.strip_prefix(root()).unwrap().to_string_lossy().into_owned();
                let expected = if AMBIGUOUS.contains(&rel.as_str()) {
                    None
                } else {
                    Isa::from_name(&sub.file_name().to_string_lossy())
                };
                let src = std::fs::read_to_string(&file).unwrap();
                match (expected, detect_isa(&src)) {
                    (None, Err(_)) => {}
                    (None, Ok(i)) => failures.push(format!("{}: expected no detection, got {}", rel, i.name())),
                    (Some(e), Ok(i)) if e == i => {}
                    (Some(e), Ok(i)) => failures.push(format!("{}: expected {}, got {}", rel, e.name(), i.name())),
                    (Some(_), Err(r)) => failures.push(format!("{}: {}", rel, r)),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn detect(lines: &[&str]) -> Result<Isa, String> {
    detect_isa(&lines.iter().map(|l| format!("{}\n", l)).collect::<String>())
}

#[test]
fn sources_no_single_architecture_owns() {
    assert!(
        detect(&["    .text", "_start:", "    halt"]).is_err(),
        "a bare halt parses under all but VLIW-IV"
    );
    assert!(
        detect(&[
            "    .text",
            "_start:",
            "    addi t0, t0, 1",
            "    frobnicate t0",
            "    halt"
        ])
        .is_err(),
        "a mnemonic that does not exist parses under none"
    );
}

#[test]
fn architectures_that_look_alike() {
    assert_eq!(
        detect(&["    .text", "_start:", "    addi t0, t0, 1", "    halt"]),
        Ok(Isa::RiscIv)
    );
    assert_eq!(
        detect(&[
            "    .text",
            "_start:",
            "    addi t0, t0, 1 / nop / nop / nop",
            "    addi t0, t0, 1 / nop / nop / halt"
        ]),
        Ok(Isa::VliwIv)
    );
    assert_eq!(
        detect(&[
            "    .text",
            "_start:",
            "    move.l   (A6), D0",
            "    add.l    D1, D0",
            "    halt"
        ]),
        Ok(Isa::M68k)
    );
    assert_eq!(
        detect(&[
            "    .text",
            "_start:",
            "    addi t0, t0, 1",
            "    j nowhere",
            "    halt"
        ]),
        Ok(Isa::RiscIv),
        "an unresolved label does not cost the source its architecture"
    );
}
