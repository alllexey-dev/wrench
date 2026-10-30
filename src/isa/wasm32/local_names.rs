//! `.frame`: names for locals, replaced by their indices before the program
//! is parsed.

use std::collections::HashMap;

/// In `.text`, from each label to the next one or the section's end, blank
/// the `.frame ($a, $b) $c` line and write each name it lists as its index,
/// padded to the name's width. A name the `.frame` does not list stays in
/// the source for the parser to reject.
pub fn resolve_local_names(src: &str) -> Result<String, (usize, String)> {
    let mut out: Vec<String> = Vec::new();
    let mut offset = 0;
    let mut in_text = false;
    let mut names: HashMap<String, usize> = HashMap::new();
    for line in src.split('\n') {
        let first = code(line).split_whitespace().next();
        let resolved = match first {
            Some(".text") | Some(".data") => {
                in_text = first == Some(".text");
                names.clear();
                line.to_string()
            }
            Some(w) if in_text && w.ends_with(':') => {
                names.clear();
                line.to_string()
            }
            _ if !in_text => line.to_string(),
            _ => match frame_directive(line) {
                Some(after) => match frame_names(after) {
                    None => {
                        let indent = line.chars().take_while(|c| c.is_whitespace()).count();
                        return Err((
                            offset + indent,
                            "malformed .frame, expected distinct names as in .frame ($a, $b) $c".to_string(),
                        ));
                    }
                    Some(declared) => {
                        names = declared.into_iter().enumerate().map(|(k, n)| (n, k)).collect();
                        " ".repeat(line.chars().count())
                    }
                },
                None => substitute(&names, line),
            },
        };
        offset += line.chars().count() + 1;
        out.push(resolved);
    }
    Ok(out.join("\n"))
}

/// What follows `.frame` on the line, if the line is one.
fn frame_directive(line: &str) -> Option<&str> {
    let s = code(line).trim_start().strip_prefix(".frame")?;
    match s.chars().next() {
        Some(c) if c.is_whitespace() || c == '(' => Some(s),
        None => Some(s),
        _ => None,
    }
}

/// The names of `($a, $b) $c`: the parameters, then the reserved locals.
fn frame_names(s: &str) -> Option<Vec<String>> {
    let inside = s.trim_start().strip_prefix('(')?;
    let (params, locals) = inside.split_once(')')?;
    let mut declared = name_list(params)?;
    declared.extend(name_list(locals)?);
    let distinct: std::collections::HashSet<&String> = declared.iter().collect();
    (distinct.len() == declared.len()).then_some(declared)
}

fn name_list(t: &str) -> Option<Vec<String>> {
    if t.chars().all(char::is_whitespace) {
        return Some(Vec::new());
    }
    t.split(',')
        .map(|n| {
            let n = n.trim();
            let rest = n.strip_prefix('$')?;
            (!rest.is_empty() && rest.chars().all(is_name_char)).then(|| n.to_string())
        })
        .collect()
}

fn substitute(names: &HashMap<String, usize>, line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut k = 0;
    while k < chars.len() {
        if chars[k] == '$' {
            let len = chars[k + 1..].iter().take_while(|&&c| is_name_char(c)).count();
            let name: String = chars[k..k + 1 + len].iter().collect();
            if let Some(idx) = names.get(&name) {
                let index = idx.to_string();
                out.push_str(&index);
                out.push_str(&" ".repeat((len + 1).saturating_sub(index.len())));
                k += 1 + len;
                continue;
            }
        }
        out.push(chars[k]);
        k += 1;
    }
    out
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The line without its comment.
fn code(line: &str) -> &str {
    line.split(';').next().unwrap_or("")
}
