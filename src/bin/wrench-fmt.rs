//! asm-formatter - a simple assembly file formatter.

use std::process::ExitCode;

use clap::Parser;
use wrench::isa::{detect_isa, Isa};

/// Format assembly files
#[derive(Parser, Debug)]
#[command(version = wrench::version(), about, long_about = None)]
struct Cli {
    /// Assembly file name
    #[arg(value_name = "FILENAME", required = true)]
    file_names: Vec<String>,
    /// Instruction set architecture (acc32, f32a, risc-iv-32, vliw-iv, m68k, wasm32); detected from the source if omitted
    #[arg(long, value_name = "ISA")]
    isa: Option<String>,
    /// Modify the file in place
    #[arg(long)]
    inplace: bool,
    /// Verbose output
    #[arg(short, long)]
    verbose: bool,
    /// Check the formatting without modifying the file
    #[arg(long)]
    check: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum LineLayout {
    Standard,
    Wasm32,
    Vliw(Vec<usize>),
}

#[derive(Clone)]
struct FmtConfig {
    data_label_width: usize,
    data_type_width: usize,
    data_value_width: usize,
    text_command_indent: usize,
    text_command_token_widths: Vec<usize>,
    text_command_width: usize,
    comment_start: &'static str,
    line_layout: LineLayout,
}

impl Default for FmtConfig {
    fn default() -> Self {
        FmtConfig {
            data_label_width: 16,
            data_type_width: 6,
            data_value_width: 18,
            text_command_indent: 4,
            text_command_token_widths: vec![8, 0, 0, 0, 0, 0, 0],
            text_command_width: 40,
            comment_start: ";",
            line_layout: LineLayout::Standard,
        }
    }
}

fn fmt_config(isa: Isa) -> FmtConfig {
    let def = FmtConfig::default();
    match isa {
        Isa::RiscIv | Isa::M68k => def,
        Isa::F32a => FmtConfig {
            text_command_token_widths: vec![1; 10],
            text_command_width: 24,
            comment_start: "\\",
            ..def
        },
        Isa::Acc32 => FmtConfig {
            text_command_token_widths: vec![12, 8, 8, 8, 8, 8, 8],
            ..def
        },
        // ALU1 | ALU2 | Memory | Control
        Isa::VliwIv => FmtConfig {
            line_layout: LineLayout::Vliw(vec![30, 30, 12, 12]),
            ..def
        },
        Isa::Wasm32 => FmtConfig {
            text_command_token_widths: vec![],
            line_layout: LineLayout::Wasm32,
            ..def
        },
    }
}

#[derive(Debug)]
enum Statement {
    OutOfSection(Vec<String>),
    DataLine(Vec<String>),
    TextLine(Vec<String>),
}

fn is_space(c: char) -> bool {
    c.is_whitespace()
}

fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

fn width(n: usize, t: &str) -> String {
    format!("{}{}", t, " ".repeat(n.saturating_sub(t.chars().count())))
}

fn unwords(ts: &[String]) -> String {
    ts.join(" ")
}

fn tokenize(fmt: &FmtConfig, content: &str) -> Vec<String> {
    let vliw = matches!(fmt.line_layout, LineLayout::Vliw(_));
    let head = fmt.comment_start.chars().next().unwrap_or(';');
    let mut out = Vec::new();
    let mut txt = strip(content);
    while !txt.is_empty() {
        if txt.starts_with(fmt.comment_start) {
            out.push(txt.to_string());
            break;
        } else if let Some(body) = txt.strip_prefix('\'') {
            let (string, rest) = body.split_at(body.find('\'').unwrap_or(body.len()));
            out.push(format!("'{}'", string));
            txt = strip(rest.strip_prefix('\'').unwrap_or(rest));
        } else if vliw && txt.starts_with('/') {
            out.push("/".to_string());
            txt = strip(&txt[1..]);
        } else {
            let end = txt
                .find(|c: char| c == ' ' || c == '\t' || c == '\'' || c == head || (vliw && c == '/'))
                .unwrap_or(txt.len());
            out.push(txt[..end].to_string());
            txt = strip(&txt[end..]);
        }
    }
    out
}

fn split_comment(fmt: &FmtConfig, mut tokens: Vec<String>) -> (Vec<String>, String) {
    match tokens.last() {
        Some(last) if last.starts_with(fmt.comment_start) => {
            let c = tokens.pop().unwrap_or_default();
            (tokens, c)
        }
        _ => (tokens, String::new()),
    }
}

fn classify(source: Vec<Vec<String>>) -> Vec<Statement> {
    let mut section: fn(Vec<String>) -> Statement = Statement::OutOfSection;
    source
        .into_iter()
        .map(|tokens| match tokens.first().map(String::as_str) {
            Some(".data") => {
                section = Statement::DataLine;
                Statement::OutOfSection(tokens)
            }
            Some(".text") => {
                section = Statement::TextLine;
                Statement::OutOfSection(tokens)
            }
            _ => section(tokens),
        })
        .collect()
}

fn split_by_slash(tokens: &[String]) -> Vec<Vec<String>> {
    tokens.split(|t| t == "/").map(|s| s.to_vec()).collect()
}

fn vliw_slot_widths(config: &[usize], statements: &[Statement]) -> Vec<usize> {
    let slots_list: Vec<Vec<Vec<String>>> = statements
        .iter()
        .filter_map(|s| match s {
            Statement::TextLine(ts) if !ts.is_empty() && !ts[0].ends_with(':') => Some(split_by_slash(ts)),
            _ => None,
        })
        .collect();
    let num_slots = slots_list.iter().map(Vec::len).max().unwrap_or(0);
    let content: Vec<usize> = (0..num_slots)
        .map(|idx| {
            slots_list
                .iter()
                .filter_map(|slots| slots.get(idx))
                .map(|slot| unwords(slot).chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    let alu: Vec<usize> = config.iter().take(2).copied().collect();
    let mut out = alu.clone();
    out.extend(content.into_iter().skip(alu.len()));
    out
}

fn wasm32_depths(depth: usize, st: &Statement) -> (usize, usize) {
    let outer = depth.saturating_sub(1);
    match st {
        Statement::TextLine(ts) if !ts.is_empty() => match ts[0].as_str() {
            "block" | "loop" | "if" => (depth, depth + 1),
            "end" => (outer, outer),
            "else" => (outer, depth),
            _ => (depth, depth),
        },
        _ => (depth, depth),
    }
}

fn line_indents(fmt: &FmtConfig, statements: &[Statement]) -> Vec<usize> {
    if fmt.line_layout == LineLayout::Wasm32 {
        let mut depth = 0;
        statements
            .iter()
            .map(|s| {
                let (line, next) = wasm32_depths(depth, s);
                depth = next;
                fmt.text_command_indent * (1 + line)
            })
            .collect()
    } else {
        vec![fmt.text_command_indent; statements.len()]
    }
}

fn pprint(fmt: &FmtConfig, st: &Statement) -> Result<String, String> {
    match st {
        Statement::OutOfSection(ts) => Ok(format!("    {}", unwords(ts))),
        Statement::DataLine(ts) if ts.is_empty() => Ok(String::new()),
        Statement::DataLine(ts) if ts.len() >= 2 => Ok([
            width(fmt.data_label_width, &ts[0]),
            width(fmt.data_type_width, &ts[1]),
            width(fmt.data_value_width, &unwords(&ts[2..])),
        ]
        .join(" ")),
        Statement::TextLine(ts) if ts.is_empty() => Ok(String::new()),
        Statement::TextLine(ts) if ts[0].ends_with(':') => Ok(format!(
            "{}\n{}",
            ts[0],
            pprint(fmt, &Statement::TextLine(ts[1..].to_vec()))?
        )),
        Statement::TextLine(ts) => {
            let indent = " ".repeat(fmt.text_command_indent);
            match &fmt.line_layout {
                LineLayout::Vliw(widths) => {
                    let slots = split_by_slash(ts);
                    let formatted: Vec<String> = widths
                        .iter()
                        .zip(slots.iter())
                        .map(|(&w, slot)| {
                            if slot.is_empty() {
                                " ".repeat(w)
                            } else {
                                width(w, &unwords(slot))
                            }
                        })
                        .collect();
                    Ok(format!("{}{}", indent, formatted.join(" / ")))
                }
                _ => {
                    let n = fmt.text_command_token_widths.len();
                    let mut cmd: Vec<String> = fmt
                        .text_command_token_widths
                        .iter()
                        .zip(ts)
                        .map(|(&w, t)| width(w, t))
                        .collect();
                    cmd.extend(ts.iter().skip(n).cloned());
                    Ok(format!("{}{}", indent, width(fmt.text_command_width, &unwords(&cmd))))
                }
            }
        }
        st => Err(format!("Invalid statement: {:?}", st)),
    }
}

fn align_comment(st: &Statement, raw: &str, indent: usize, comment: &str) -> String {
    let indented = raw != raw.trim_start_matches(is_space);
    let bare = matches!(st, Statement::DataLine(ts) | Statement::TextLine(ts) if ts.is_empty());
    if comment.is_empty() || !indented || !bare {
        comment.to_string()
    } else {
        format!("{}{}", " ".repeat(indent), comment)
    }
}

fn format_lines(fmt: &FmtConfig, raw_lines: &[&str]) -> Result<Vec<String>, String> {
    let (source, comments): (Vec<Vec<String>>, Vec<String>) =
        raw_lines.iter().map(|l| split_comment(fmt, tokenize(fmt, l))).unzip();
    let statements = classify(source);
    let mut fmt = fmt.clone();
    if let LineLayout::Vliw(widths) = &fmt.line_layout {
        fmt.line_layout = LineLayout::Vliw(vliw_slot_widths(widths, &statements));
    }
    let indents = line_indents(&fmt, &statements);
    let mut out = Vec::new();
    for (k, st) in statements.iter().enumerate() {
        let indent = indents[k];
        let local = FmtConfig {
            text_command_indent: indent,
            text_command_width: (fmt.text_command_width + fmt.text_command_indent).saturating_sub(indent),
            ..fmt.clone()
        };
        let s = pprint(&local, st)?;
        let c = align_comment(st, raw_lines[k], indent, &comments[k]);
        let line = if s.is_empty() { c } else { format!("{} {}", s, c) };
        out.push(line.trim_end_matches(is_space).to_string());
    }
    Ok(out)
}

/// Haskell `lines`: a final newline does not start another line.
fn lines(content: &str) -> Vec<&str> {
    let mut ls: Vec<&str> = content.split('\n').collect();
    if ls.last() == Some(&"") {
        ls.pop();
    }
    ls
}

fn format_file(fmt: &FmtConfig, content: &str) -> Result<String, String> {
    Ok(format_lines(fmt, &lines(content))?
        .iter()
        .map(|l| format!("{}\n", l))
        .collect())
}

fn process(cli: &Cli, file: &str) -> Result<String, String> {
    let content = std::fs::read_to_string(file).map_err(|e| format!("{}: {}", file, e))?;
    let isa = match &cli.isa {
        Some(name) => {
            Isa::from_name(name).ok_or_else(|| format!("{}: Invalid ISA: {}", file, wrench::hs::show_str(name)))?
        }
        None => detect_isa(&content).map_err(|r| format!("{}: cannot detect ISA ({}), pass --isa", file, r))?,
    };
    let formatted = format_file(&fmt_config(isa), &content).map_err(|e| format!("{}: {}", file, e))?;
    let note = if cli.isa.is_some() {
        String::new()
    } else {
        format!(" ({})", isa.name())
    };
    let already = format!("{}{} already formatted", file, note);
    let reformatted = format!("{}{} reformatted", file, note);
    let same = content == formatted;
    if cli.check {
        return if same {
            Ok(already)
        } else {
            Err(format!("{} needs formatting", file))
        };
    }
    if cli.inplace {
        if !same {
            std::fs::write(file, &formatted).map_err(|e| format!("{}: {}", file, e))?;
        }
    } else {
        println!("{}", formatted);
    }
    Ok(if same { already } else { reformatted })
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let results: Vec<Result<String, String>> = cli.file_names.iter().map(|f| process(&cli, f)).collect();
    if cli.verbose {
        for r in &results {
            println!("{}", r.as_ref().unwrap_or_else(|e| e));
        }
    }
    if results.iter().all(Result::is_ok) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
