//! A small backtracking parser with megaparsec's commitment rule: an
//! alternative that fails after consuming input fails the whole choice,
//! unless it is wrapped in [`P::attempt`]. Errors carry an offset and are
//! rendered with the source line, as megaparsec's `errorBundlePretty` does.

use crate::word::{fit_signed, W};

#[derive(Clone, Debug, Default)]
pub struct PError {
    pub offset: usize,
    pub unexpected: Option<String>,
    pub expected: Vec<String>,
    pub messages: Vec<String>,
    /// Set when the failing parser consumed input but reports an earlier offset.
    pub consumed: bool,
}

impl PError {
    fn merge(mut self, other: PError) -> PError {
        use std::cmp::Ordering::*;
        match self.offset.cmp(&other.offset) {
            Greater => self,
            Less => other,
            Equal => {
                for e in other.expected {
                    if !self.expected.contains(&e) {
                        self.expected.push(e);
                    }
                }
                for m in other.messages {
                    if !self.messages.contains(&m) {
                        self.messages.push(m);
                    }
                }
                if self.unexpected.is_none() {
                    self.unexpected = other.unexpected;
                }
                self.consumed |= other.consumed;
                self
            }
        }
    }
}

pub type R<T> = Result<T, PError>;

/// Parser state over a source text, addressed by character offset.
pub struct P<'a> {
    pub src: &'a [char],
    pub pos: usize,
    pub file: &'a str,
}

/// Line and column (1-based, tabs to the next multiple of 8) of an offset.
pub fn line_col(src: &[char], offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for &c in &src[..offset.min(src.len())] {
        if c == '\n' {
            line += 1;
            col = 1;
        } else if c == '\t' {
            col += 8 - (col - 1) % 8;
        } else {
            col += 1;
        }
    }
    (line, col)
}

fn show_token(c: Option<char>) -> String {
    match c {
        None => "end of input".to_string(),
        Some('\n') => "newline".to_string(),
        Some('\t') => "tab".to_string(),
        Some(' ') => "space".to_string(),
        Some('\r') => "carriage return".to_string(),
        Some(c) => crate::hs::show_char(c),
    }
}

/// Render an error against the source, in megaparsec's layout.
pub fn error_pretty(file: &str, src: &[char], err: &PError) -> String {
    let (line, col) = line_col(src, err.offset);
    let line_start = src[..err.offset.min(src.len())]
        .iter()
        .rposition(|&c| c == '\n')
        .map_or(0, |i| i + 1);
    let line_end = src[line_start..]
        .iter()
        .position(|&c| c == '\n')
        .map_or(src.len(), |i| line_start + i);
    let mut text = String::new();
    let mut c = 1;
    for &ch in &src[line_start..line_end] {
        if ch == '\t' {
            let n = 8 - (c - 1) % 8;
            text.push_str(&" ".repeat(n));
            c += n;
        } else {
            text.push(ch);
            c += 1;
        }
    }
    let ln = line.to_string();
    let pad = " ".repeat(ln.len());
    let mut out = format!(
        "{}:{}:{}:\n{} |\n{} | {}\n{} | {}^\n",
        file,
        line,
        col,
        pad,
        ln,
        text,
        pad,
        " ".repeat(col - 1)
    );
    if err.messages.is_empty() || err.unexpected.is_some() || !err.expected.is_empty() {
        if let Some(u) = &err.unexpected {
            out.push_str(&format!("unexpected {}\n", u));
        } else if err.messages.is_empty() {
            out.push_str(&format!("unexpected {}\n", show_token(src.get(err.offset).copied())));
        }
        if !err.expected.is_empty() {
            let e = &err.expected;
            let list = match e.len() {
                1 => e[0].clone(),
                2 => format!("{} or {}", e[0], e[1]),
                n => format!("{}, or {}", e[..n - 1].join(", "), e[n - 1]),
            };
            out.push_str(&format!("expecting {}\n", list));
        }
    }
    for m in &err.messages {
        out.push_str(m);
        out.push('\n');
    }
    out
}

/// One alternative of [`P::choice`].
pub type Alt<'f, T> = &'f dyn Fn(&mut P) -> R<T>;

impl<'a> P<'a> {
    pub fn new(file: &'a str, src: &'a [char]) -> Self {
        P { src, pos: 0, file }
    }

    pub fn peek(&self) -> Option<char> {
        self.src.get(self.pos).copied()
    }

    pub fn at_end(&self) -> bool {
        self.pos >= self.src.len()
    }

    pub fn offset(&self) -> usize {
        self.pos
    }

    pub fn set_offset(&mut self, o: usize) {
        self.pos = o;
    }

    /// `file:line:col` of the current position.
    pub fn source_pos(&self) -> String {
        let (l, c) = line_col(self.src, self.pos);
        format!("{}:{}:{}", self.file, l, c)
    }

    pub fn expected<T>(&self, what: &str) -> R<T> {
        Err(PError {
            offset: self.pos,
            unexpected: Some(show_token(self.peek())),
            expected: vec![what.to_string()],
            ..Default::default()
        })
    }

    /// `fail`: an error with a message at the current position.
    pub fn fail<T>(&self, msg: impl Into<String>) -> R<T> {
        Err(PError {
            offset: self.pos,
            messages: vec![msg.into()],
            ..Default::default()
        })
    }

    /// `fail` after moving back over input already consumed: the choice
    /// around it still treats the input as consumed.
    pub fn fail_at<T>(&mut self, offset: usize, msg: impl Into<String>) -> R<T> {
        self.pos = offset;
        Err(PError {
            offset,
            messages: vec![msg.into()],
            consumed: true,
            ..Default::default()
        })
    }

    /// `try`: on failure, rewind and report no consumption.
    pub fn attempt<T>(&mut self, f: impl FnOnce(&mut P<'a>) -> R<T>) -> R<T> {
        let start = self.pos;
        match f(self) {
            Ok(v) => Ok(v),
            Err(mut e) => {
                self.pos = start;
                e.consumed = false;
                Err(e)
            }
        }
    }

    /// The first alternative that succeeds; stops at one that fails after consuming.
    pub fn choice<T>(&mut self, alts: &[Alt<T>]) -> R<T> {
        let start = self.pos;
        let mut err: Option<PError> = None;
        for alt in alts {
            match alt(self) {
                Ok(v) => return Ok(v),
                Err(e) => {
                    if self.pos != start || e.consumed {
                        return Err(e);
                    }
                    err = Some(match err {
                        None => e,
                        Some(prev) => prev.merge(e),
                    });
                }
            }
        }
        Err(err.unwrap_or_else(|| PError {
            offset: start,
            ..Default::default()
        }))
    }

    /// `p <|> q` for two alternatives.
    pub fn or<T>(&mut self, a: impl FnOnce(&mut P<'a>) -> R<T>, b: impl FnOnce(&mut P<'a>) -> R<T>) -> R<T> {
        let start = self.pos;
        match a(self) {
            Ok(v) => Ok(v),
            Err(e) if self.pos != start || e.consumed => Err(e),
            Err(e) => b(self).map_err(|e2| if self.pos == start { e.merge(e2) } else { e2 }),
        }
    }

    /// `optional`.
    pub fn optional<T>(&mut self, f: impl FnOnce(&mut P<'a>) -> R<T>) -> R<Option<T>> {
        let start = self.pos;
        match f(self) {
            Ok(v) => Ok(Some(v)),
            Err(e) if self.pos != start || e.consumed => Err(e),
            Err(_) => Ok(None),
        }
    }

    /// `many`.
    pub fn many<T>(&mut self, mut f: impl FnMut(&mut P<'a>) -> R<T>) -> R<Vec<T>> {
        let mut out = Vec::new();
        loop {
            let start = self.pos;
            match f(self) {
                Ok(v) => {
                    out.push(v);
                    if self.pos == start {
                        return Ok(out);
                    }
                }
                Err(e) if self.pos != start || e.consumed => return Err(e),
                Err(_) => return Ok(out),
            }
        }
    }

    /// `some`.
    pub fn some<T>(&mut self, mut f: impl FnMut(&mut P<'a>) -> R<T>) -> R<Vec<T>> {
        let first = f(self)?;
        let mut rest = self.many(f)?;
        rest.insert(0, first);
        Ok(rest)
    }

    /// `sepBy`.
    pub fn sep_by<T>(
        &mut self,
        mut item: impl FnMut(&mut P<'a>) -> R<T>,
        mut sep: impl FnMut(&mut P<'a>) -> R<()>,
    ) -> R<Vec<T>> {
        let first = match self.optional(&mut item)? {
            None => return Ok(Vec::new()),
            Some(v) => v,
        };
        let mut out = vec![first];
        out.extend(self.many(|p| {
            sep(p)?;
            item(p)
        })?);
        Ok(out)
    }

    /// `manyTill p end`.
    pub fn many_till<T>(
        &mut self,
        mut item: impl FnMut(&mut P<'a>) -> R<T>,
        mut end: impl FnMut(&mut P<'a>) -> R<()>,
    ) -> R<Vec<T>> {
        let mut out = Vec::new();
        loop {
            let start = self.pos;
            match end(self) {
                Ok(()) => return Ok(out),
                Err(e) if self.pos != start || e.consumed => return Err(e),
                Err(e) => match item(self) {
                    Ok(v) => out.push(v),
                    Err(e2) => return Err(if self.pos == start { e.merge(e2) } else { e2 }),
                },
            }
        }
    }

    pub fn satisfy(&mut self, pred: impl Fn(char) -> bool, label: &str) -> R<char> {
        match self.peek() {
            Some(c) if pred(c) => {
                self.pos += 1;
                Ok(c)
            }
            _ => self.expected(label),
        }
    }

    pub fn char(&mut self, c: char) -> R<char> {
        let label = crate::hs::show_char(c);
        self.satisfy(|x| x == c, &label)
    }

    pub fn any_char(&mut self) -> R<char> {
        self.satisfy(|_| true, "any character")
    }

    /// `string`: all or nothing.
    pub fn string(&mut self, s: &str) -> R<()> {
        let mut i = self.pos;
        for c in s.chars() {
            if self.src.get(i) != Some(&c) {
                return Err(PError {
                    offset: self.pos,
                    unexpected: Some(show_token(self.peek())),
                    expected: vec![crate::hs::show_str(s)],
                    ..Default::default()
                });
            }
            i += 1;
        }
        self.pos = i;
        Ok(())
    }

    /// The value of the first keyword that matches, as `choice` over `string`s.
    pub fn keyword<T: Copy>(&mut self, table: &[(&str, T)]) -> R<T> {
        let mut err: Option<PError> = None;
        for &(k, v) in table {
            match self.string(k) {
                Ok(()) => return Ok(v),
                Err(e) => err = Some(err.map_or(e.clone(), |prev| prev.merge(e))),
            }
        }
        Err(err.unwrap_or_else(|| PError {
            offset: self.pos,
            ..Default::default()
        }))
    }

    pub fn take_while(&mut self, pred: impl Fn(char) -> bool) -> String {
        let start = self.pos;
        while self.peek().is_some_and(&pred) {
            self.pos += 1;
        }
        self.src[start..self.pos].iter().collect()
    }

    pub fn hspace(&mut self) -> R<()> {
        self.take_while(is_hspace);
        Ok(())
    }

    pub fn hspace1(&mut self) -> R<()> {
        if !self.peek().is_some_and(is_hspace) {
            return self.expected("white space");
        }
        self.hspace()
    }

    pub fn space1(&mut self) -> R<()> {
        if !self.peek().is_some_and(char::is_whitespace) {
            return self.expected("white space");
        }
        self.take_while(char::is_whitespace);
        Ok(())
    }

    pub fn eof(&mut self) -> R<()> {
        if self.at_end() {
            Ok(())
        } else {
            self.expected("end of input")
        }
    }

    /// `eol`: `\n` or `\r\n`.
    pub fn eol(&mut self) -> R<()> {
        match self.peek() {
            Some('\n') => {
                self.pos += 1;
                Ok(())
            }
            Some('\r') if self.src.get(self.pos + 1) == Some(&'\n') => {
                self.pos += 2;
                Ok(())
            }
            _ => self.expected("end of line"),
        }
    }

    /// A comment up to and including the end of line.
    pub fn comment(&mut self, cstart: &str) -> R<()> {
        self.string(cstart)?;
        self.many_till(|p| p.any_char(), |p| p.eol())?;
        Ok(())
    }

    /// `eol'`: spaces, then the end of line or a comment.
    pub fn eol_(&mut self, cstart: &str) -> R<()> {
        self.hspace()?;
        self.or(|p| p.eol(), |p| p.comment(cstart))
    }

    pub fn comma(&mut self) -> R<()> {
        self.hspace()?;
        self.char(',')?;
        self.hspace()
    }

    pub fn letter(&mut self) -> R<char> {
        self.satisfy(char::is_alphabetic, "letter")
    }

    pub fn digit(&mut self) -> R<char> {
        self.satisfy(|c| c.is_ascii_digit(), "digit")
    }

    /// An identifier: a letter or `_`, then letters, digits and `_`.
    pub fn name(&mut self) -> R<String> {
        let first = self.satisfy(|c| c.is_alphabetic() || c == '_', "letter or '_'")?;
        let rest = self.take_while(|c| c.is_alphanumeric() || c == '_');
        Ok(format!("{}{}", first, rest))
    }

    /// `name:` as a label definition.
    pub fn label(&mut self) -> R<String> {
        self.attempt(|p| {
            let n = p.name()?;
            p.char(':')?;
            Ok(n)
        })
    }

    fn dec_digits(&mut self) -> R<String> {
        let first = self.digit()?;
        let rest = self.take_while(|c| c.is_ascii_digit() || c == '_');
        Ok(format!("{}{}", first, rest).replace('_', ""))
    }

    /// An integer literal: decimal (maybe negative) or `0x` hex, `_` allowed after the first digit.
    pub fn literal(&mut self) -> R<i128> {
        let hex = self.attempt(|p| {
            p.string("0x")?;
            let first = p.satisfy(|c| c.is_ascii_hexdigit(), "hexadecimal digit")?;
            let rest = p.take_while(|c| c.is_ascii_hexdigit() || c == '_');
            Ok(format!("{}{}", first, rest).replace('_', ""))
        });
        match hex {
            Ok(h) => Ok(i128::from_str_radix(&h, 16).unwrap_or(i128::MAX)),
            Err(e1) => {
                let num = self.or(
                    |p| {
                        p.attempt(|p| {
                            p.char('-')?;
                            p.dec_digits().map(|d| format!("-{}", d))
                        })
                    },
                    |p| p.dec_digits(),
                );
                match num {
                    Ok(d) => Ok(d
                        .parse::<i128>()
                        .unwrap_or(if d.starts_with('-') { i128::MIN } else { i128::MAX })),
                    Err(e2) => Err(e1.merge(e2)),
                }
            }
        }
    }

    fn narrowed(&mut self, what: &str, lo: i128, hi: i128) -> R<i128> {
        let at = self.pos;
        let v = self.literal()?;
        if lo <= v && v <= hi {
            Ok(v)
        } else {
            self.fail_at(
                at,
                format!("literal {} doesn't fit {}, expected {}..{}", v, what, lo, hi),
            )
        }
    }

    /// Any 32-bit pattern, read as signed or unsigned.
    pub fn word_literal(&mut self) -> R<W> {
        self.narrowed("a 32-bit machine word", -(1i128 << 31), (1i128 << 32) - 1)
            .map(|v| v as i64 as u32 as i32)
    }

    pub fn byte_literal(&mut self) -> R<u8> {
        self.narrowed("a byte", -128, 255).map(|v| v as i64 as u8)
    }

    pub fn int_literal(&mut self) -> R<i64> {
        self.narrowed("an Int", i64::MIN as i128, i64::MAX as i128)
            .map(|v| v as i64)
    }

    /// A character literal with Haskell escapes, as in `'...'` strings.
    pub fn char_literal(&mut self) -> R<char> {
        let c = self.any_char()?;
        if c != '\\' {
            return Ok(c);
        }
        let e = self.any_char()?;
        let simple = match e {
            'n' => Some('\n'),
            't' => Some('\t'),
            'r' => Some('\r'),
            'a' => Some('\x07'),
            'b' => Some('\x08'),
            'f' => Some('\x0c'),
            'v' => Some('\x0b'),
            '0' if !self.peek().is_some_and(|c| c.is_ascii_digit()) => Some('\0'),
            '\\' | '\'' | '"' => Some(e),
            _ => None,
        };
        if let Some(c) = simple {
            return Ok(c);
        }
        let radix_digits = |p: &mut P, radix: u32| -> R<char> {
            let s = p.take_while(|c| c.is_digit(radix));
            u32::from_str_radix(&s, radix)
                .ok()
                .and_then(char::from_u32)
                .map_or_else(|| p.fail("invalid escape sequence"), Ok)
        };
        match e {
            'x' => radix_digits(self, 16),
            'o' => radix_digits(self, 8),
            d if d.is_ascii_digit() => {
                self.pos -= 1;
                radix_digits(self, 10)
            }
            _ => {
                self.pos -= 1;
                for (i, n) in [
                    "NUL", "SOH", "STX", "ETX", "EOT", "ENQ", "ACK", "BEL", "BS", "HT", "LF", "VT", "FF", "CR",
                ]
                .iter()
                .enumerate()
                {
                    if self.attempt(|p| p.string(n)).is_ok() {
                        return Ok(char::from_u32(i as u32).unwrap());
                    }
                }
                if self.attempt(|p| p.string("DEL")).is_ok() {
                    return Ok('\x7f');
                }
                self.fail("invalid escape sequence")
            }
        }
    }
}

pub fn is_hspace(c: char) -> bool {
    c.is_whitespace() && c != '\n' && c != '\r'
}

/// A value written in the source: a label to resolve later or a constant,
/// with the transformation (`%hi`, `%lo`, ...) to apply once known.
#[derive(Clone)]
pub struct Ref {
    pub prepare: std::rc::Rc<dyn Fn(W) -> W>,
    /// `file:line:col` where it was written.
    pub pos: String,
    pub kind: RefKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefKind {
    Label(String),
    Value(W),
}

impl Ref {
    pub fn value(pos: String, v: W) -> Ref {
        Ref {
            prepare: std::rc::Rc::new(|x| x),
            pos,
            kind: RefKind::Value(v),
        }
    }

    pub fn map(self, f: impl Fn(W) -> W + 'static) -> Ref {
        let prev = self.prepare;
        Ref {
            prepare: std::rc::Rc::new(move |x| f(prev(x))),
            ..self
        }
    }

    /// Resolve against the labels and check against the field it is encoded into.
    pub fn resolve(
        &self,
        guard: impl Fn(W) -> Result<W, String>,
        labels: &dyn Fn(&str) -> Option<W>,
    ) -> Result<W, String> {
        let value = match &self.kind {
            RefKind::Label(l) => match labels(l) {
                Some(x) => (self.prepare)(x),
                None => return Err(format!("{}: can't resolve label: {}", self.pos, crate::hs::show_str(l))),
            },
            RefKind::Value(v) => (self.prepare)(*v),
        };
        guard(value).map_err(|e| format!("{}: {}", self.pos, e))
    }
}

impl std::fmt::Debug for Ref {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            RefKind::Label(l) => write!(f, "{}", l),
            RefKind::Value(v) => write!(f, "{}", (self.prepare)(*v)),
        }
    }
}

impl<'a> P<'a> {
    /// A constant or a label, transformed by `f` once resolved.
    pub fn reference_with_fn(&mut self, f: impl Fn(W) -> W + 'static) -> R<Ref> {
        let pos = self.source_pos();
        let r = self.or(
            |p| p.constant_at(pos.clone()),
            |p| {
                p.name().map(|l| Ref {
                    prepare: std::rc::Rc::new(|x| x),
                    pos: pos.clone(),
                    kind: RefKind::Label(l),
                })
            },
        )?;
        Ok(r.map(f))
    }

    pub fn reference(&mut self) -> R<Ref> {
        self.reference_with_fn(|x| x)
    }

    /// A constant only: a word literal or a quoted character.
    pub fn constant_ref(&mut self) -> R<Ref> {
        let pos = self.source_pos();
        self.constant_at(pos)
    }

    fn constant_at(&mut self, pos: String) -> R<Ref> {
        let pos2 = pos.clone();
        self.or(
            move |p| {
                p.char('\'')?;
                let c = p.satisfy(|c| c != '\'', "any character but '\\''")?;
                p.char('\'')?;
                Ok(Ref::value(pos, c as u32 as W))
            },
            move |p| p.word_literal().map(|v| Ref::value(pos2, v)),
        )
    }

    /// A reference, maybe wrapped in `%hi(...)` or `%lo(...)`.
    ///
    /// `%lo` keeps the low 12 bits sign-extended, as the consuming
    /// instructions do; `%hi` rounds up by half a field to compensate, so
    /// `lui rd, %hi(x)` + `addi rd, rd, %lo(x)` rebuilds every `x`.
    pub fn reference_with_directive(&mut self) -> R<Ref> {
        let hi = |p: &mut P| -> R<Ref> {
            p.string("%hi(")?;
            let r = p.reference_with_fn(|w| (w.wrapping_add(0x800) >> 12) & 0xFFFFF)?;
            p.string(")")?;
            Ok(r)
        };
        let lo = |p: &mut P| -> R<Ref> {
            p.string("%lo(")?;
            let r = p.reference_with_fn(|w| fit_signed(12, w))?;
            p.string(")")?;
            Ok(r)
        };
        self.choice(&[&hi, &lo, &|p| p.reference()])
    }

    /// `.org N` up to the end of line.
    pub fn org_directive(&mut self, cstart: &str) -> R<i64> {
        self.string(".org")?;
        self.hspace1()?;
        let v = self.int_literal()?;
        self.eol_(cstart)?;
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run<T>(src: &str, f: impl FnOnce(&mut P) -> R<T>) -> R<T> {
        let chars: Vec<char> = src.chars().collect();
        let mut p = P::new("t.s", &chars);
        f(&mut p)
    }

    #[test]
    fn literals() {
        assert_eq!(run("0x1_0", |p| p.literal()).unwrap(), 16);
        assert_eq!(run("-12", |p| p.literal()).unwrap(), -12);
        assert_eq!(run("4294967295", |p| p.word_literal()).unwrap(), -1);
        assert!(run("4294967296", |p| p.word_literal()).is_err());
    }

    #[test]
    fn choice_commits_after_consuming() {
        let r = run("abX", |p| {
            p.choice(&[
                &|p| {
                    p.char('a')?;
                    p.char('c').map(|_| 1)
                },
                &|p| p.string("ab").map(|_| 2),
            ])
        });
        assert!(r.is_err());
        let r = run("abX", |p| {
            p.choice(&[
                &|p| {
                    p.attempt(|p| {
                        p.char('a')?;
                        p.char('c').map(|_| 1)
                    })
                },
                &|p| p.string("ab").map(|_| 2),
            ])
        });
        assert_eq!(r.unwrap(), 2);
    }

    #[test]
    fn error_layout() {
        let src: Vec<char> = "a\n\tbc\n".chars().collect();
        let e = PError {
            offset: 3,
            messages: vec!["boom".into()],
            ..Default::default()
        };
        assert_eq!(
            error_pretty("f.s", &src, &e),
            "f.s:2:9:\n  |\n2 |         bc\n  |         ^\nboom\n"
        );
    }
}
