use std::io::Write;
use std::process::ExitCode;

use clap::Parser;
use wrench::config::Config;
use wrench::isa::{detect_isa, Isa};
use wrench::memory::{hex_addr, hex_addr_width, pretty_dump, pretty_labels};
use wrench::wrench::{catch, wrench, AddrFormat, DumpSize, Options};

/// App for laboratory course of computer architecture.
#[derive(Parser, Debug)]
#[command(version = wrench::version(), about)]
struct Cli {
    /// Input assembler file (.s)
    #[arg(value_name = "INPUT")]
    input: String,
    /// ISA (risc-iv-32, f32a, acc32, m68k, vliw-iv, wasm32); detected from the source if omitted
    #[arg(long, value_name = "ISA")]
    isa: Option<String>,
    /// Configuration file (.yaml)
    #[arg(short = 'c', long = "conf", value_name = "CONF")]
    conf: Option<String>,
    /// Only run preprocess and translation steps
    #[arg(short = 'S')]
    only_translation: bool,
    /// Append a built-in `Overview` report
    #[arg(long)]
    stats: bool,
    /// Verbose output
    #[arg(short, long)]
    verbose: bool,
    /// Maximum number of instructions to execute
    #[arg(long, value_name = "LIMIT", default_value_t = 8000000)]
    instruction_limit: i64,
    /// Maximum memory size in bytes
    #[arg(long, value_name = "SIZE", default_value_t = 0xFFFF)]
    memory_limit: i64,
    /// Maximum number of state records to log
    #[arg(long, value_name = "LIMIT", default_value_t = 10000)]
    state_log_limit: usize,
    /// Numeral format for -S dump addresses
    #[arg(long, value_name = "FORMAT", value_enum, default_value_t = AddrFormat::Hex)]
    dump_addr_format: AddrFormat,
    /// Show or hide the (N B) byte-count suffix on -S dump address ranges
    #[arg(long, value_name = "MODE", value_enum, default_value_t = DumpSize::Show)]
    dump_size: DumpSize,
}

fn fail(msg: &str) -> ExitCode {
    eprintln!("error: {}", msg);
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.verbose {
        println!("{:#?}", cli);
    }
    let conf = match &cli.conf {
        Some(f) => match Config::read(f) {
            Ok(c) if cli.stats => c.with_execution_stats(),
            Ok(c) => c,
            Err(e) => return fail(&e),
        },
        None => Config::default(),
    }
    .ensure_seed();
    if cli.verbose {
        println!("{:#?}\n---", conf);
    }
    if conf.limit > cli.instruction_limit {
        return fail("limit too high");
    }
    if conf.memory_size > cli.memory_limit {
        return fail("memory size too high");
    }
    let src = match std::fs::read(&cli.input) {
        Ok(b) => format!("{}\n", String::from_utf8_lossy(&b)),
        Err(e) => return fail(&format!("{}: {}", cli.input, e)),
    };
    let isa = match &cli.isa {
        Some(name) => match Isa::from_name(name) {
            Some(i) => i,
            None => return fail(&format!("unknown isa: {}", name)),
        },
        None => match detect_isa(&src) {
            Ok(i) => {
                if cli.verbose {
                    println!("detected isa: {}", i.name());
                }
                i
            }
            Err(reason) => return fail(&format!("cannot detect isa ({}), pass --isa", reason)),
        },
    };
    let opts = Options {
        input: cli.input.clone(),
        verbose: cli.verbose,
        max_state_log_limit: cli.state_log_limit,
    };
    let width = hex_addr_width(conf.memory_size);
    let show_addr = |a: i64| match cli.dump_addr_format {
        AddrFormat::Hex => hex_addr(width, a),
        AddrFormat::Dec => a.to_string(),
    };
    let outcome: Result<bool, String> = wrench::with_arch!(isa, A, {
        catch(|| wrench::<A>(&opts, &conf, &src)).map(|r| {
            for w in &r.warnings {
                eprintln!("warning ({}): {}", isa.name(), w);
            }
            let mut out = std::io::stdout().lock();
            if cli.only_translation {
                let _ = writeln!(out, "{}\n---", pretty_labels(&show_addr, &r.labels));
                let _ = write!(
                    out,
                    "{}",
                    pretty_dump(cli.dump_size == DumpSize::Show, &show_addr, &r.labels, &r.dump)
                );
                true
            } else {
                let _ = write!(out, "{}", r.trace);
                r.success
            }
        })
    });
    match outcome {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error ({}): {}", isa.name(), e);
            ExitCode::FAILURE
        }
    }
}
