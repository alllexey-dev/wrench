//! Source and report text as HTML with line numbers and syntax highlighting.

use wrench::highlight::{tokenize_line, tokenize_report_line, TokenKind, Tokens};
use wrench::isa::Isa;

pub fn escape_html(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    for c in t.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

pub fn format_code_with_line_numbers(code: &str) -> String {
    format_code_lines(
        |line| format!("<div class=\"code-line\">{}</div>", escape_html(line)),
        code,
    )
}

pub fn format_asm(isa: Option<Isa>, code: &str) -> String {
    match isa {
        None => format_code_with_line_numbers(code),
        Some(isa) => format_code_lines(|l| highlight_line(&tokenize_line(isa, l)), code),
    }
}

pub fn format_report(isa: Option<Isa>, code: &str) -> String {
    match isa {
        None => format_code_with_line_numbers(code),
        Some(isa) => format_code_lines(|l| highlight_line(&tokenize_report_line(isa, l)), code),
    }
}

fn format_code_lines(render_line: impl Fn(&str) -> String, code: &str) -> String {
    let code: String = code.chars().filter(|&c| c != '\r').collect();
    let mut lines: Vec<&str> = code.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let numbers: String = (1..=lines.len())
        .map(|i| format!("<div class=\"line-number\">{}</div>", i))
        .collect();
    let content: String = lines.iter().map(|l| render_line(l)).collect();
    format!(
        "<div class=\"code-container\"><div class=\"line-numbers\">{}</div><div class=\"code-content\">{}</div></div>",
        numbers, content
    )
}

fn highlight_line(tokens: &Tokens) -> String {
    let comment_only = tokens.iter().any(|(k, _)| *k == TokenKind::Comment)
        && tokens
            .iter()
            .all(|(k, t)| *k == TokenKind::Comment || t.trim().is_empty());
    let cls = if comment_only {
        "code-line comment-only"
    } else {
        "code-line"
    };
    let body: String = tokens
        .iter()
        .map(|(k, t)| match k {
            TokenKind::Plain => escape_html(t),
            _ => format!(
                "<span class=\"tok-{}\">{}</span>",
                k.name().to_lowercase(),
                escape_html(t)
            ),
        })
        .collect();
    format!("<div class=\"{}\">{}</div>", cls, body)
}
