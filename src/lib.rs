//! Wrench: assembler, simulator and report engine for a family of teaching ISAs.

/// Run `$body` with `$a` bound to the `Arch` type of `$isa`.
#[macro_export]
macro_rules! with_arch {
    ($isa:expr, $a:ident, $body:expr) => {
        match $isa {
            $crate::isa::Isa::Acc32 => {
                type $a = $crate::isa::acc32::Acc32;
                $body
            }
            $crate::isa::Isa::F32a => {
                type $a = $crate::isa::f32a::F32a;
                $body
            }
            $crate::isa::Isa::RiscIv => {
                type $a = $crate::isa::risc_iv::RiscIv;
                $body
            }
            $crate::isa::Isa::M68k => {
                type $a = $crate::isa::m68k::M68k;
                $body
            }
            $crate::isa::Isa::VliwIv => {
                type $a = $crate::isa::vliw_iv::VliwIv;
                $body
            }
            $crate::isa::Isa::Wasm32 => {
                type $a = $crate::isa::wasm32::Wasm32;
                $body
            }
        }
    };
}

pub mod bitpack;
pub mod config;
pub mod highlight;
pub mod hs;
pub mod intervals;
pub mod isa;
pub mod machine;
pub mod memory;
pub mod parser;
pub mod report;
pub mod rng;
pub mod translator;
pub mod word;
pub mod wrench;

/// `<version>[-<suffix>] [(<commit>) <commit date>]`, from `VERSION_SUFFIX`,
/// `GIT_HASH` and `GIT_DATE` at build time.
pub fn version() -> &'static str {
    static V: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    V.get_or_init(build_version)
}

fn build_version() -> String {
    let suffix = option_env!("VERSION_SUFFIX").unwrap_or("");
    let hash = option_env!("GIT_HASH").unwrap_or("");
    let date = option_env!("GIT_DATE").unwrap_or("");
    let mut v = env!("CARGO_PKG_VERSION").to_string();
    if !suffix.is_empty() {
        v = format!("{}-{}", v, suffix);
    }
    let short: String = hash.chars().take(7).collect();
    match (hash.is_empty(), date.is_empty()) {
        (true, _) => v,
        (false, true) => format!("{} ({})", v, short),
        (false, false) => format!("{} ({}) {}", v, short, date),
    }
}
