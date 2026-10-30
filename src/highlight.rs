//! Line-by-line syntax highlighting for assembler sources and simulation
//! reports. Concatenating the token texts gives back the original line.

use crate::isa::Isa;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Plain,
    Comment,
    Label,
    Directive,
    Mnemonic,
    Register,
    Literal,
    Reloc,
    Ref,
    Call,
    Separator,
}

impl TokenKind {
    pub fn name(self) -> &'static str {
        match self {
            TokenKind::Plain => "Plain",
            TokenKind::Comment => "Comment",
            TokenKind::Label => "Label",
            TokenKind::Directive => "Directive",
            TokenKind::Mnemonic => "Mnemonic",
            TokenKind::Register => "Register",
            TokenKind::Literal => "Literal",
            TokenKind::Reloc => "Reloc",
            TokenKind::Ref => "Ref",
            TokenKind::Call => "Call",
            TokenKind::Separator => "Separator",
        }
    }
}

pub type Tokens = Vec<(TokenKind, String)>;

enum Lexeme {
    Space(String),
    Str(String),
    Label(String),
    Word(String),
    Punct(char),
}

pub fn tokenize_line(isa: Isa, line: &str) -> Tokens {
    let (code, comment) = split_comment(comment_char(isa), line);
    let chars: Vec<char> = code.chars().collect();
    let tokens = classify(isa, lex_code(isa, &chars));
    let mut out = if isa == Isa::F32a { f32a_calls(tokens) } else { tokens };
    if !comment.is_empty() {
        out.push((TokenKind::Comment, comment.to_string()));
    }
    out
}

fn comment_char(isa: Isa) -> char {
    if isa == Isa::F32a {
        '\\'
    } else {
        ';'
    }
}

fn is_quote(c: char) -> bool {
    c == '\'' || c == '"'
}

/// Split at the comment character outside quotes.
fn split_comment(c: char, line: &str) -> (&str, &str) {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let mut quote: Option<char> = None;
    let mut k = 0;
    while k < chars.len() {
        let (i, x) = chars[k];
        match quote {
            None if x == c => return line.split_at(i),
            None if is_quote(x) => quote = Some(x),
            None => {}
            Some(_) if x == '\\' && k + 1 < chars.len() => k += 1,
            Some(q) if x == q => quote = None,
            Some(_) => {}
        }
        k += 1;
    }
    (line, "")
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || "_.%$".contains(c)
}

fn span(s: &[char], pred: impl Fn(char) -> bool) -> usize {
    s.iter().position(|&c| !pred(c)).unwrap_or(s.len())
}

/// A quoted string's tail after the opening quote, and the rest.
fn quoted(q: char, s: &[char]) -> (String, usize) {
    let mut out = String::new();
    let mut k = 0;
    while k < s.len() {
        if s[k] == '\\' && k + 1 < s.len() {
            out.push('\\');
            out.push(s[k + 1]);
            k += 2;
        } else if s[k] == q {
            out.push(q);
            return (out, k + 1);
        } else {
            out.push(s[k]);
            k += 1;
        }
    }
    (out, k)
}

fn lex_code(isa: Isa, s: &[char]) -> Vec<Lexeme> {
    let mut out = Vec::new();
    let mut k = 0;
    while k < s.len() {
        let x = s[k];
        let rest = &s[k..];
        if x.is_whitespace() {
            let n = span(rest, char::is_whitespace);
            out.push(Lexeme::Space(rest[..n].iter().collect()));
            k += n;
        } else if is_quote(x) {
            let (st, n) = quoted(x, &rest[1..]);
            out.push(Lexeme::Str(format!("{}{}", x, st)));
            k += 1 + n;
        } else if isa == Isa::F32a {
            let n = span(rest, |c| !c.is_whitespace());
            let w: String = rest[..n].iter().collect();
            out.push(if n > 1 && w.ends_with(':') {
                Lexeme::Label(w)
            } else {
                Lexeme::Word(w)
            });
            k += n;
        } else if is_word_char(x) || (x == '-' && rest.get(1).is_some_and(|c| c.is_ascii_digit())) {
            let n = 1 + span(&rest[1..], is_word_char);
            let w: String = rest[..n].iter().collect();
            if rest.get(n) == Some(&':') {
                out.push(Lexeme::Label(format!("{}:", w)));
                k += n + 1;
            } else {
                out.push(Lexeme::Word(w));
                k += n;
            }
        } else {
            out.push(Lexeme::Punct(x));
            k += 1;
        }
    }
    out
}

fn classify(isa: Isa, ls: Vec<Lexeme>) -> Tokens {
    let mut start = true;
    let mut out = Vec::new();
    for l in ls {
        match l {
            Lexeme::Space(t) => out.push((TokenKind::Plain, t)),
            Lexeme::Str(t) => {
                out.push((TokenKind::Literal, t));
                start = false;
            }
            Lexeme::Label(t) => out.push((TokenKind::Label, t)),
            Lexeme::Punct('/') if isa == Isa::VliwIv => {
                out.push((TokenKind::Separator, "/".to_string()));
                start = true;
            }
            Lexeme::Punct(c) => out.push((TokenKind::Plain, c.to_string())),
            Lexeme::Word(w) => {
                let kind = if is_number(&w) {
                    TokenKind::Literal
                } else if w.starts_with('.') {
                    TokenKind::Directive
                } else if w.starts_with('%') {
                    TokenKind::Reloc
                } else if isa == Isa::Wasm32 && w.starts_with('$') {
                    TokenKind::Register
                } else if let Some(known) = mnemonics(isa) {
                    if known.contains(&w.as_str()) {
                        TokenKind::Mnemonic
                    } else {
                        TokenKind::Ref
                    }
                } else if start {
                    TokenKind::Mnemonic
                } else if is_register(isa, &w) {
                    TokenKind::Register
                } else {
                    TokenKind::Ref
                };
                out.push((kind, w));
                start = false;
            }
        }
    }
    out
}

fn mnemonics(isa: Isa) -> Option<&'static [&'static str]> {
    match isa {
        Isa::F32a => Some(crate::isa::f32a::MNEMONICS),
        Isa::Wasm32 => Some(crate::isa::wasm32::MNEMONICS),
        _ => None,
    }
}

/// A bare f32a word is a call unless it is the argument of a mnemonic.
fn f32a_calls(ts: Tokens) -> Tokens {
    let mut arg = false;
    ts.into_iter()
        .map(|(kind, t)| match kind {
            TokenKind::Plain => (kind, t),
            TokenKind::Ref if !arg => {
                arg = false;
                (TokenKind::Call, t)
            }
            _ => {
                arg = kind == TokenKind::Mnemonic && ["lit", "@p", "if", "-if", "next"].contains(&t.as_str());
                (kind, t)
            }
        })
        .collect()
}

fn is_number(w: &str) -> bool {
    let unsigned = w.strip_prefix('-').unwrap_or(w);
    match unsigned.strip_prefix("0x") {
        Some(ds) => {
            ds.chars().next().is_some_and(|c| c.is_ascii_hexdigit())
                && ds.chars().all(|c| c.is_ascii_hexdigit() || c == '_')
        }
        None => {
            unsigned.chars().next().is_some_and(|c| c.is_ascii_digit())
                && unsigned.chars().all(|c| c.is_ascii_digit() || c == '_')
        }
    }
}

fn is_register(isa: Isa, w: &str) -> bool {
    match isa {
        Isa::RiscIv | Isa::VliwIv => risc_registers().iter().any(|r| r == w),
        Isa::M68k => {
            let cs: Vec<char> = w.chars().collect();
            cs.len() == 2 && "AD".contains(cs[0]) && ('0'..='7').contains(&cs[1])
        }
        _ => false,
    }
}

fn risc_registers() -> Vec<String> {
    let mut v: Vec<String> = ["zero", "ra", "sp", "gp", "tp", "fp"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    for (p, n) in [("t", 6), ("s", 11), ("a", 7)] {
        v.extend((0..=n).map(|i| format!("{}{}", p, i)));
    }
    v
}

/// Tokens for simulation log, memory dump and statistics lines.
pub fn tokenize_report_line(isa: Isa, line: &str) -> Tokens {
    if line.starts_with('#') {
        return vec![(TokenKind::Directive, line.to_string())];
    }
    if ["---", "```"].contains(&line.trim()) {
        return vec![(TokenKind::Separator, line.to_string())];
    }
    if let Some((prefix, rest)) = line_prefix(line) {
        let bare = &prefix[..prefix.len() - prefix.chars().last().map_or(0, char::len_utf8)];
        if is_number(bare) || prefix.starts_with("mem[") {
            let mut out = vec![(TokenKind::Separator, prefix.to_string())];
            let words: Vec<&str> = rest.split_whitespace().collect();
            match words.as_slice() {
                [name] if !is_number(name) && name.chars().next().is_some_and(|c| c.is_lowercase() || c == '_') => {
                    let n = rest.len() - rest.trim_start().len();
                    out.push((TokenKind::Plain, rest[..n].to_string()));
                    out.push((TokenKind::Label, rest[n..].to_string()));
                }
                _ => out.extend(report_body(isa, rest)),
            }
            out
        } else {
            let mut out = vec![(TokenKind::Ref, prefix.to_string())];
            out.extend(report_body(isa, rest));
            out
        }
    } else {
        report_body(isa, line)
    }
}

fn line_prefix(line: &str) -> Option<(&str, &str)> {
    let replaced = line.replace(":\t", ": ");
    let before = replaced.split(": ").next()?;
    if !replaced.contains(": ")
        || before.is_empty()
        || (before.chars().any(char::is_whitespace) && !before.starts_with("mem["))
    {
        return None;
    }
    let n = before.chars().count() + 1;
    let idx = line.char_indices().nth(n).map_or(line.len(), |(i, _)| i);
    Some(line.split_at(idx))
}

fn report_body(isa: Isa, line: &str) -> Tokens {
    let s: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut k = 0;
    while k < s.len() {
        let x = s[k];
        let rest = &s[k..];
        if x.is_whitespace() {
            let n = span(rest, char::is_whitespace);
            out.push((TokenKind::Plain, rest[..n].iter().collect()));
            k += n;
        } else if x == '"' {
            let (st, n) = quoted('"', &rest[1..]);
            out.push((TokenKind::Literal, format!("\"{}", st)));
            k += 1 + n;
        } else if x == '@' && rest.get(1) == Some(&'"') {
            let (st, n) = quoted('"', &rest[2..]);
            out.push((TokenKind::Label, format!("@\"{}", st)));
            k += 2 + n;
        } else if x == '@' && rest.get(1).is_some_and(|&c| is_word_char(c)) {
            let n = 1 + span(&rest[1..], is_word_char);
            out.push((TokenKind::Label, rest[..n].iter().collect()));
            k += n;
        } else if x.is_alphanumeric() || (x == '-' && rest.get(1).is_some_and(|c| c.is_ascii_digit())) {
            let n = 1 + span(&rest[1..], |c| c.is_alphanumeric() || c == '_');
            let w: String = rest[..n].iter().collect();
            out.push((report_word(isa, &w), w));
            k += n;
        } else {
            out.push((TokenKind::Plain, x.to_string()));
            k += 1;
        }
    }
    out
}

fn report_word(isa: Isa, w: &str) -> TokenKind {
    if is_number(w) || (w.chars().count() == 2 && w.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase())) {
        TokenKind::Literal
    } else if w == "B" {
        TokenKind::Plain
    } else if w.chars().next().is_some_and(char::is_uppercase) {
        if is_register(isa, w)
            || is_register(isa, &w.to_lowercase())
            || ([Isa::RiscIv, Isa::VliwIv].contains(&isa) && w == "S0Fp")
        {
            TokenKind::Register
        } else {
            TokenKind::Mnemonic
        }
    } else {
        TokenKind::Plain
    }
}
