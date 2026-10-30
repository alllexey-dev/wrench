//! Report views, the `{...}` keys of a report template, as data. A view's
//! spec says how its key is spelled and what it reads (numbers, a format);
//! parsing a key against it yields those values and the renderer takes them.
//! Usage text, the docs table and key matching all read the same spec.

use std::rc::Rc;

use super::Env;
use crate::machine::Machine;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Dec,
    Hex,
    Sym,
    Bin,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Dec => "dec",
            Format::Hex => "hex",
            Format::Sym => "sym",
            Format::Bin => "bin",
        }
    }
}

/// One colon-separated part of a key.
#[derive(Clone, Debug)]
pub enum Piece {
    Lit(String),
    /// A number, decimal or `0x` hex, named for messages.
    Arg(String),
    OptArg(String),
    /// Allowed formats and the default; `None` makes it required.
    Format(Vec<Format>, Option<Format>),
}

/// What a non-literal piece read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Val {
    Int(i64),
    OptInt(Option<i64>),
    Fmt(Format),
}

impl Val {
    pub fn int(self) -> i64 {
        match self {
            Val::Int(i) => i,
            _ => panic!("view argument is not a number"),
        }
    }

    pub fn opt_int(self) -> Option<i64> {
        match self {
            Val::OptInt(i) => i,
            _ => panic!("view argument is not an optional number"),
        }
    }

    pub fn fmt(self) -> Format {
        match self {
            Val::Fmt(f) => f,
            _ => panic!("view argument is not a format"),
        }
    }
}

/// How a key is spelled.
#[derive(Clone, Debug, Default)]
pub struct Spec(pub Vec<Piece>);

impl Spec {
    /// Literal parts: `lit("mem:instr-ranges")` is two of them.
    pub fn lit(name: &str) -> Spec {
        Spec(name.split(':').map(|t| Piece::Lit(t.to_string())).collect())
    }

    pub fn arg(mut self, name: &str) -> Spec {
        self.0.push(Piece::Arg(name.to_string()));
        self
    }

    pub fn opt_arg(mut self, name: &str) -> Spec {
        self.0.push(Piece::OptArg(name.to_string()));
        self
    }

    pub fn format(mut self, fs: &[Format], def: Format) -> Spec {
        self.0.push(Piece::Format(fs.to_vec(), Some(def)));
        self
    }

    pub fn format_required(mut self, fs: &[Format]) -> Spec {
        self.0.push(Piece::Format(fs.to_vec(), None));
        self
    }

    pub fn then_lit(mut self, name: &str) -> Spec {
        self.0.extend(Spec::lit(name).0);
        self
    }

    /// The leading literal parts.
    pub fn literals(&self) -> Vec<&str> {
        self.0
            .iter()
            .map_while(|p| match p {
                Piece::Lit(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    }

    pub fn name(&self) -> String {
        self.literals().join(":")
    }

    fn drop_literals(&self) -> Spec {
        let n = self.literals().len();
        Spec(self.0[n..].to_vec())
    }

    pub fn usage(&self) -> String {
        let mut out = String::from("{");
        for (n, p) in self.0.iter().enumerate() {
            let sep = if n == 0 { "" } else { ":" };
            match p {
                Piece::Lit(t) => out.push_str(&format!("{}{}", sep, t)),
                Piece::Arg(a) => out.push_str(&format!("{}<{}>", sep, a)),
                Piece::OptArg(a) => out.push_str(&format!("[{}<{}>]", sep, a)),
                Piece::Format(fs, None) => out.push_str(&format!("{}{}", sep, formats(fs, "|"))),
                Piece::Format(fs, Some(_)) => out.push_str(&format!("[{}{}]", sep, formats(fs, "|"))),
            }
        }
        out.push('}');
        out
    }

    fn parse(&self, parts: &[&str]) -> Result<Vec<Val>, String> {
        parse_pieces(&self.0, parts)
    }
}

fn formats(fs: &[Format], sep: &str) -> String {
    fs.iter().map(|f| f.name()).collect::<Vec<_>>().join(sep)
}

fn parse_pieces(pieces: &[Piece], parts: &[&str]) -> Result<Vec<Val>, String> {
    let Some((p, rest)) = pieces.split_first() else {
        return if parts.is_empty() {
            Ok(Vec::new())
        } else {
            Err(format!("unexpected '{}'", parts.join(":")))
        };
    };
    match parse_piece(p, parts) {
        Ok((v, consumed)) => {
            let mut tail = parse_pieces(rest, &parts[consumed..])?;
            if let Some(v) = v {
                tail.insert(0, v);
            }
            Ok(tail)
        }
        // An optional format that does not match may be left out: the part
        // is then the next piece's. If that fails too, the format error is
        // the one to report.
        Err(err) => match p {
            Piece::Format(_, Some(def)) if !rest.is_empty() => {
                let mut tail = parse_pieces(rest, parts).map_err(|_| err)?;
                tail.insert(0, Val::Fmt(*def));
                Ok(tail)
            }
            _ => Err(err),
        },
    }
}

fn number(name: &str, x: &str) -> Result<i64, String> {
    crate::hs::read_int(x).ok_or_else(|| format!("<{}> should be a number, got '{}'", name, x))
}

fn parse_piece(p: &Piece, parts: &[&str]) -> Result<(Option<Val>, usize), String> {
    match (p, parts.first()) {
        (Piece::Lit(t), Some(x)) if t == x => Ok((None, 1)),
        (Piece::Lit(t), _) => Err(format!("expected '{}'", t)),
        (Piece::Arg(a), None) => Err(format!("missing <{}>", a)),
        (Piece::Arg(a), Some(x)) => Ok((Some(Val::Int(number(a, x)?)), 1)),
        (Piece::OptArg(_), None) => Ok((Some(Val::OptInt(None)), 0)),
        (Piece::OptArg(a), Some(x)) => Ok((Some(Val::OptInt(Some(number(a, x)?))), 1)),
        (Piece::Format(_, Some(f)), None) => Ok((Some(Val::Fmt(*f)), 0)),
        (Piece::Format(fs, None), None) => Err(format!("missing format, one of: {}", formats(fs, ", "))),
        (Piece::Format(fs, _), Some(x)) => match fs.iter().find(|f| f.name() == *x) {
            Some(f) => Ok((Some(Val::Fmt(*f)), 1)),
            None => Err(format!("unknown format '{}', one of: {}", x, formats(fs, ", "))),
        },
    }
}

/// What a key's arguments are checked against: the configuration.
#[derive(Clone, Debug)]
pub struct CheckEnv {
    pub memory_size: i64,
    pub io_ports: Vec<i64>,
}

pub type Render<S> = Rc<dyn Fn(&Env<S>, &[Val]) -> String>;
pub type Check = Rc<dyn Fn(&CheckEnv, &[Val]) -> Result<(), String>>;
/// A view closed over its arguments.
pub type Hole<S> = Rc<dyn Fn(&Env<S>) -> String>;

pub struct View<S: Machine> {
    pub spec: Spec,
    pub doc: String,
    pub check: Check,
    pub render: Render<S>,
    pub renamed_to: Option<Vec<String>>,
}

impl<S: Machine> Clone for View<S> {
    fn clone(&self) -> Self {
        View {
            spec: self.spec.clone(),
            doc: self.doc.clone(),
            check: self.check.clone(),
            render: self.render.clone(),
            renamed_to: self.renamed_to.clone(),
        }
    }
}

impl<S: Machine> View<S> {
    pub fn new(spec: Spec, doc: &str, render: impl Fn(&Env<S>, &[Val]) -> String + 'static) -> Self {
        View::checked(spec, doc, |_, _| Ok(()), render)
    }

    /// A view whose arguments must also fit the configuration.
    pub fn checked(
        spec: Spec,
        doc: &str,
        check: impl Fn(&CheckEnv, &[Val]) -> Result<(), String> + 'static,
        render: impl Fn(&Env<S>, &[Val]) -> String + 'static,
    ) -> Self {
        View {
            spec,
            doc: doc.to_string(),
            check: Rc::new(check),
            render: Rc::new(render),
            renamed_to: None,
        }
    }

    pub fn name(&self) -> String {
        self.spec.name()
    }

    pub fn usage(&self) -> String {
        self.spec.usage()
    }

    /// Keys that name this view, one per spelling: every format, with and
    /// without optional parts, `0` for each number.
    pub fn examples(&self) -> Vec<String> {
        let mut acc: Vec<Vec<String>> = vec![Vec::new()];
        for p in &self.spec.0 {
            let spellings: Vec<Vec<String>> = match p {
                Piece::Lit(t) => vec![vec![t.clone()]],
                Piece::Arg(_) => vec![vec!["0".to_string()]],
                Piece::OptArg(_) => vec![vec![], vec!["0".to_string()]],
                Piece::Format(fs, d) => {
                    let mut v: Vec<Vec<String>> = if d.is_some() { vec![vec![]] } else { vec![] };
                    v.extend(fs.iter().map(|f| vec![f.name().to_string()]));
                    v
                }
            };
            acc = acc
                .iter()
                .flat_map(|x| {
                    spellings.iter().map(move |s| {
                        let mut y = x.clone();
                        y.extend(s.iter().cloned());
                        y
                    })
                })
                .collect();
        }
        acc.into_iter().map(|x| x.join(":")).collect()
    }
}

/// Old names kept as aliases of current views; using one warns.
pub fn with_renames<S: Machine>(renames: &[(&str, &str)], mut vs: Vec<View<S>>) -> Vec<View<S>> {
    let aliases: Vec<View<S>> = renames
        .iter()
        .map(|(old, new)| {
            let v = vs
                .iter()
                .find(|v| v.name() == *new)
                .unwrap_or_else(|| panic!("renamed view not found: {}", new));
            let mut spec = Spec::lit(old);
            spec.0.extend(v.spec.drop_literals().0);
            View {
                spec,
                doc: v.doc.clone(),
                check: v.check.clone(),
                render: v.render.clone(),
                renamed_to: Some(v.spec.literals().iter().map(|s| s.to_string()).collect()),
            }
        })
        .collect();
    vs.extend(aliases);
    vs
}

/// A run-wide stat of an ISA: a view of its own and a line of `{isa-specific}`.
pub struct Stat<S: Machine> {
    pub name: String,
    pub doc: String,
    pub render: Hole<S>,
}

impl<S: Machine> Stat<S> {
    pub fn new(name: &str, doc: &str, render: impl Fn(&Env<S>) -> String + 'static) -> Self {
        Stat {
            name: name.to_string(),
            doc: doc.to_string(),
            render: Rc::new(render),
        }
    }

    pub fn view(&self) -> View<S> {
        let r = self.render.clone();
        View::new(
            Spec::lit(&self.name),
            &format!("{} Listed in `{{isa-specific}}`.", self.doc),
            move |e, _| r(e),
        )
    }
}

/// Find the view a key names, read and check its arguments. The view with
/// the longest literal prefix wins, so `stack:layout` is not read as `stack`
/// with an unknown format. Returns a rename warning, if any, and the hole.
fn match_view<S: Machine>(
    check_env: &CheckEnv,
    views: &[View<S>],
    key: &str,
) -> Result<(Option<String>, Hole<S>), String> {
    let parts: Vec<&str> = key.split(':').collect();
    let mut candidates: Vec<&View<S>> = views
        .iter()
        .filter(|v| {
            let ls = v.spec.literals();
            !ls.is_empty() && ls.len() <= parts.len() && ls.iter().zip(&parts).all(|(a, b)| a == b)
        })
        .collect();
    candidates.sort_by_key(|v| std::cmp::Reverse(v.spec.literals().len()));
    let Some(first) = candidates.first() else {
        return Err(format!("unknown view {{{}}}{}", key, suggestions(views, &parts)));
    };
    let best = first.spec.literals().len();
    let mut errs = Vec::new();
    for v in candidates.iter().filter(|v| v.spec.literals().len() == best) {
        let attempt = v
            .spec
            .parse(&parts)
            .and_then(|vals| (v.check)(check_env, &vals).map(|_| vals));
        match attempt {
            Ok(vals) => {
                let n = v.spec.literals().len();
                let warn = v.renamed_to.as_ref().map(|new| {
                    let mut name: Vec<String> = new.clone();
                    name.extend(parts[n..].iter().map(|s| s.to_string()));
                    format!("{{{}}} is renamed, use {{{}}}", key, name.join(":"))
                });
                let render = v.render.clone();
                return Ok((warn, Rc::new(move |e: &Env<S>| render(e, &vals))));
            }
            Err(err) => errs.push(format!("{{{}}}: {}, usage: {}", key, err, v.usage())),
        }
    }
    Err(errs.join("\n"))
}

/// Views whose name is a small edit away from the key's leading parts.
fn suggestions<S: Machine>(views: &[View<S>], parts: &[&str]) -> String {
    let mut close: Vec<(usize, String)> = views
        .iter()
        .filter(|v| v.renamed_to.is_none())
        .filter_map(|v| {
            let ls = v.spec.literals();
            let name = ls.join(":");
            let given = parts.iter().take(ls.len()).copied().collect::<Vec<_>>().join(":");
            let d = edit_distance(&name.to_lowercase(), &given.to_lowercase());
            (d <= 1.max(name.chars().count() / 3)).then(|| (d, v.usage()))
        })
        .collect();
    close.sort_by_key(|(d, _)| *d);
    let us: Vec<String> = close.into_iter().take(3).map(|(_, u)| u).collect();
    if us.is_empty() {
        String::new()
    } else {
        format!(", did you mean {}?", us.join(" or "))
    }
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let mut prev: Vec<usize> = (0..=a.len()).collect();
    for c in b.chars() {
        let mut row = vec![prev[0] + 1];
        for (k, &ca) in a.iter().enumerate() {
            let left = row[k];
            let v = (left + 1).min(prev[k + 1] + 1).min(prev[k] + usize::from(ca != c));
            row.push(v);
        }
        prev = row;
    }
    *prev.last().unwrap_or(&0)
}

fn markdown_table(header: (&str, &str), rows: &[(String, String)]) -> String {
    let w_a = rows
        .iter()
        .map(|r| r.0.chars().count())
        .chain([header.0.len()])
        .max()
        .unwrap_or(0);
    let w_b = rows
        .iter()
        .map(|r| r.1.chars().count())
        .chain([header.1.len()])
        .max()
        .unwrap_or(0);
    let line = |a: &str, b: &str| {
        format!(
            "| {}{} | {}{} |\n",
            a,
            " ".repeat(w_a - a.chars().count()),
            b,
            " ".repeat(w_b - b.chars().count())
        )
    };
    let mut out = line(header.0, header.1);
    out.push_str(&line(&"-".repeat(w_a), &"-".repeat(w_b)));
    for (a, b) in rows {
        out.push_str(&line(a, b));
    }
    out
}

fn code(u: &str) -> String {
    format!("`{}`", u.replace('|', "\\|"))
}

/// A Markdown table of the current views. Neighbours with the same
/// description, such as a register under its two names, share a row.
pub fn views_table<S: Machine>(vs: &[View<S>]) -> String {
    let mut rows: Vec<(Vec<String>, String)> = Vec::new();
    for v in vs.iter().filter(|v| v.renamed_to.is_none()) {
        match rows.last_mut() {
            Some((names, doc)) if *doc == v.doc => names.push(code(&v.usage())),
            _ => rows.push((vec![code(&v.usage())], v.doc.clone())),
        }
    }
    let rows: Vec<(String, String)> = rows.into_iter().map(|(n, d)| (n.join(", "), d)).collect();
    markdown_table(("View", "Description"), &rows)
}

pub fn renames_table<S: Machine>(vs: &[View<S>]) -> String {
    let rows: Vec<(String, String)> = vs
        .iter()
        .filter_map(|v| {
            v.renamed_to.as_ref().map(|new| {
                let mut spec = Spec::lit(&new.join(":"));
                spec.0.extend(v.spec.drop_literals().0);
                (code(&v.usage()), code(&spec.usage()))
            })
        })
        .collect();
    if rows.is_empty() {
        String::new()
    } else {
        markdown_table(("Old", "New"), &rows)
    }
}

/// A report template with every key matched to its view.
pub struct Template<S: Machine>(Vec<Chunk<S>>);

enum Chunk<S: Machine> {
    Literal(String),
    Hole(Hole<S>),
}

/// Split a template into text and `{key}` holes, matching every key.
/// Returns rename warnings and the template, or every error.
pub fn compile_template<S: Machine>(
    check_env: &CheckEnv,
    views: &[View<S>],
    input: &str,
) -> Result<(Vec<String>, Template<S>), Vec<String>> {
    let mut chunks = Vec::new();
    let mut warnings = Vec::new();
    let mut errs = Vec::new();
    let mut t = input;
    while !t.is_empty() {
        // A '{' without a later '}' is text, as is everything outside braces.
        let Some(open) = t.find('{') else {
            chunks.push(Chunk::Literal(t.to_string()));
            break;
        };
        let Some(close) = t[open + 1..].find('}') else {
            chunks.push(Chunk::Literal(t.to_string()));
            break;
        };
        if open > 0 {
            chunks.push(Chunk::Literal(t[..open].to_string()));
        }
        let key = &t[open + 1..open + 1 + close];
        match match_view(check_env, views, key) {
            Ok((warn, hole)) => {
                warnings.extend(warn);
                chunks.push(Chunk::Hole(hole));
            }
            Err(e) => errs.push(e),
        }
        t = &t[open + 1 + close + 1..];
    }
    if errs.is_empty() {
        Ok((warnings, Template(chunks)))
    } else {
        Err(errs)
    }
}

pub fn render_template<S: Machine>(template: &Template<S>, env: &Env<S>) -> String {
    template
        .0
        .iter()
        .map(|c| match c {
            Chunk::Literal(t) => t.clone(),
            Chunk::Hole(f) => f(env),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_distance_is_levenshtein() {
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "ab"), 2);
    }

    #[test]
    fn usage_renders_pieces() {
        let s = Spec::lit("mem:dump")
            .arg("from")
            .opt_arg("to")
            .format(&[Format::Dec, Format::Hex], Format::Dec);
        assert_eq!(s.usage(), "{mem:dump:<from>[:<to>][:dec|hex]}");
    }

    #[test]
    fn optional_format_may_be_skipped() {
        let s = Spec::lit("stack")
            .format(&[Format::Dec, Format::Hex], Format::Dec)
            .arg("n");
        assert_eq!(
            s.parse(&["stack", "3"]).unwrap(),
            vec![Val::Fmt(Format::Dec), Val::Int(3)]
        );
        assert_eq!(
            s.parse(&["stack", "hex", "3"]).unwrap(),
            vec![Val::Fmt(Format::Hex), Val::Int(3)]
        );
    }
}
