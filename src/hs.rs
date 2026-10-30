//! Text renderings that reports inherit from the original implementation:
//! Haskell's `show` for strings, characters and numbers.

/// `show` of a string: double quotes and Haskell escapes.
pub fn show_str(s: &str) -> String {
    let mut out = String::from("\"");
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '"' => out.push_str("\\\""),
            _ => {
                let esc = escape_char(c);
                out.push_str(&esc);
                // A numeric escape followed by a digit needs the empty escape `\&`,
                // and so does `\SO` followed by `H`.
                let next = chars.get(i + 1).copied();
                let numeric = esc.len() > 1 && esc[1..].chars().all(|d| d.is_ascii_digit());
                if numeric && next.is_some_and(|n| n.is_ascii_digit()) || esc == "\\SO" && next == Some('H') {
                    out.push_str("\\&");
                }
            }
        }
    }
    out.push('"');
    out
}

/// `show` of a character.
pub fn show_char(c: char) -> String {
    match c {
        '\'' => "'\\''".to_string(),
        '"' => "'\"'".to_string(),
        _ => format!("'{}'", escape_char(c)),
    }
}

const ASCII_NAMES: [&str; 32] = [
    "NUL", "SOH", "STX", "ETX", "EOT", "ENQ", "ACK", "a", "b", "t", "n", "v", "f", "r", "SO", "SI", "DLE", "DC1",
    "DC2", "DC3", "DC4", "NAK", "SYN", "ETB", "CAN", "EM", "SUB", "ESC", "FS", "GS", "RS", "US",
];

fn escape_char(c: char) -> String {
    let code = c as u32;
    match c {
        '\\' => "\\\\".to_string(),
        _ if code < 32 => format!("\\{}", ASCII_NAMES[code as usize]),
        '\x7f' => "\\DEL".to_string(),
        _ if code > 127 => format!("\\{}", code),
        _ => c.to_string(),
    }
}

/// `showsPrec 11` of a number: negative numbers in parentheses, as a
/// constructor argument.
pub fn show_arg<T: std::fmt::Display + PartialOrd + Default>(x: T) -> String {
    if x < T::default() {
        format!("({})", x)
    } else {
        x.to_string()
    }
}

/// `show` of a list of numbers: `[1,-2,3]`.
pub fn show_list<T: std::fmt::Display>(xs: impl IntoIterator<Item = T>) -> String {
    format!(
        "[{}]",
        xs.into_iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")
    )
}

/// `read :: String -> Int` for the spellings configs and view keys use:
/// decimal, `0x` hex, `0o` octal, optionally negative and padded with spaces.
pub fn read_int(s: &str) -> Option<i64> {
    let t = s.trim();
    let (neg, body) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t),
    };
    let v = if let Some(h) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        i64::from_str_radix(h, 16).ok()?
    } else if let Some(o) = body.strip_prefix("0o").or_else(|| body.strip_prefix("0O")) {
        i64::from_str_radix(o, 8).ok()?
    } else if !body.is_empty() && body.chars().all(|c| c.is_ascii_digit()) {
        body.parse().ok()?
    } else {
        return None;
    };
    Some(if neg { -v } else { v })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_show_like_haskell() {
        assert_eq!(show_str("a\nb"), "\"a\\nb\"");
        assert_eq!(show_str("\0"), "\"\\NUL\"");
        assert_eq!(show_str("\u{1}2"), "\"\\SOH2\"");
        assert_eq!(show_str("\u{80}1"), "\"\\128\\&1\"");
        assert_eq!(show_str("\u{e}H"), "\"\\SO\\&H\"");
    }

    #[test]
    fn ints_read_like_haskell() {
        assert_eq!(read_int("0x80"), Some(128));
        assert_eq!(read_int(" -12 "), Some(-12));
        assert_eq!(read_int("12a"), None);
    }
}
