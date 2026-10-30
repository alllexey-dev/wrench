//! The simulated architectures, and how to guess one from a source file.

pub mod acc32;
pub mod f32a;
pub mod m68k;
pub mod risc_iv;
pub mod vliw_iv;
pub mod wasm32;

use crate::translator::parses_as;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Isa {
    Acc32,
    F32a,
    RiscIv,
    M68k,
    VliwIv,
    Wasm32,
}

pub const ALL_ISA: [Isa; 6] = [Isa::Acc32, Isa::F32a, Isa::RiscIv, Isa::M68k, Isa::VliwIv, Isa::Wasm32];

impl Isa {
    /// The name `--isa` and the example directories use.
    pub fn name(self) -> &'static str {
        match self {
            Isa::Acc32 => "acc32",
            Isa::F32a => "f32a",
            Isa::RiscIv => "risc-iv-32",
            Isa::M68k => "m68k",
            Isa::VliwIv => "vliw-iv",
            Isa::Wasm32 => "wasm32",
        }
    }

    pub fn from_name(s: &str) -> Option<Isa> {
        if s == "risc-iv" {
            return Some(Isa::RiscIv); // historical spelling of risc-iv-32
        }
        ALL_ISA.into_iter().find(|i| i.name() == s)
    }

    /// Does the source parse as this ISA?
    pub fn parses(self, src: &str) -> bool {
        let src = format!("{}\n", src);
        crate::with_arch!(self, A, parses_as::<A>(&src))
    }
}

impl std::fmt::Display for Isa {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Isa::Acc32 => "Acc32",
            Isa::F32a => "F32a",
            Isa::RiscIv => "RiscIv",
            Isa::M68k => "M68k",
            Isa::VliwIv => "VliwIv",
            Isa::Wasm32 => "Wasm32",
        };
        f.write_str(s)
    }
}

/// The only ISA the source parses as.
pub fn detect_isa(src: &str) -> Result<Isa, String> {
    let fits: Vec<Isa> = ALL_ISA.into_iter().filter(|i| i.parses(src)).collect();
    match fits.as_slice() {
        [isa] => Ok(*isa),
        [] => Err("no isa can parse the source".to_string()),
        several => Err(format!(
            "source parses as [{}]",
            several.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")
        )),
    }
}
