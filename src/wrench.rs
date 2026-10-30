//! A whole run: configuration, translation, simulation and reports.

use std::collections::BTreeMap;

use crate::config::Config;
use crate::machine::{power_on, Arch, Instr, Machine};
use crate::memory::{Cell, IoMem, IoStream, Labels};
use crate::report::view::CheckEnv;
use crate::report::{compile_reports, prepare_report};
use crate::rng::{RandomStream, StdGen};
use crate::translator::translate;
use crate::word::W;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum AddrFormat {
    Hex,
    Dec,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum DumpSize {
    Show,
    Hide,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub input: String,
    pub verbose: bool,
    pub max_state_log_limit: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            input: String::new(),
            verbose: false,
            max_state_log_limit: 10000,
        }
    }
}

pub struct WrenchResult<I: Clone> {
    pub trace: String,
    pub warnings: Vec<String>,
    pub labels: Labels,
    pub success: bool,
    pub dump: Vec<(i64, Cell<I>)>,
    /// Memory size, for address formatting.
    pub memory_size: i64,
}

/// Translate, simulate and render every report.
pub fn wrench<A: Arch>(opts: &Options, conf: &Config, src: &str) -> Result<WrenchResult<Instr<A>>, String> {
    let (memory_gen, isa_gen) = StdGen::new(conf.seed.unwrap_or(0)).split();
    let fill: Vec<u8> = if conf.zero_memory_init.unwrap_or(false) {
        vec![0; conf.memory_size.max(0) as usize]
    } else {
        let mut g = memory_gen;
        (0..conf.memory_size.max(0))
            .map(|_| g.uniform_r(0, 255) as u8)
            .collect()
    };
    let io = conf.memory_mapped_io.clone().unwrap_or_default();
    let check_env = CheckEnv {
        memory_size: conf.memory_size,
        io_ports: io.keys().copied().collect(),
    };
    let (warnings, reports) = compile_reports::<A::State>(&check_env, conf.reports.as_deref().unwrap_or(&[]))?;
    let tr = translate::<A>(conf.memory_size, &fill, &opts.input, src)?;
    let pc = *tr.labels.get("_start").ok_or("_start label should be defined.")?;

    let mut streams = BTreeMap::new();
    for (&addr, inputs) in &io {
        let input = inputs.iter().map(|&x| int_to_word(x)).collect::<Result<_, _>>()?;
        streams.insert(
            addr,
            IoStream {
                input,
                output: imbl::Vector::new(),
            },
        );
    }
    let st = A::State::init(
        pc as i64,
        IoMem::new(streams, tr.dump.clone()),
        RandomStream::new(isa_gen),
    );
    let (log, final_state) = power_on(conf.limit.max(0) as usize, opts.max_state_log_limit, st);

    let results: Vec<(bool, String)> = reports
        .iter()
        .map(|r| prepare_report(&tr, opts.verbose, &final_state, &log, r))
        .collect();
    let trace = results
        .iter()
        .map(|(_, t)| format!("{}\n", format!("---\n{}", t).trim()))
        .collect();
    Ok(WrenchResult {
        trace,
        warnings,
        success: results.iter().all(|(ok, _)| *ok),
        labels: tr.labels,
        dump: tr.dump.cells(),
        memory_size: conf.memory_size,
    })
}

fn int_to_word(x: i64) -> Result<W, String> {
    if (W::MIN as i64..=u32::MAX as i64).contains(&x) {
        Ok(x as u32 as W)
    } else {
        Err(format!("integer value out of machine word range: {}", x))
    }
}

/// Run `f`, turning a panic into an error message, as the original reported
/// `error` calls from pure code.
pub fn catch<T>(f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    std::panic::set_hook(prev);
    match r {
        Ok(r) => r,
        Err(p) => Err(p
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "panic".to_string())),
    }
}
