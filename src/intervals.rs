//! Sorted, non-overlapping address ranges; adjacent ranges merge.

/// Half-open `[lo, hi)` ranges, sorted and merged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Intervals(Vec<(i64, i64)>);

impl Intervals {
    pub fn new() -> Self {
        Intervals(Vec::new())
    }

    /// The inclusive range `[lo, hi]`.
    pub fn range(lo: i64, hi: i64) -> Self {
        if hi < lo {
            Intervals::new()
        } else {
            Intervals(vec![(lo, hi + 1)])
        }
    }

    /// Record `[addr .. addr+len-1]`.
    pub fn record(&mut self, addr: i64, len: i64) {
        if len <= 0 {
            return;
        }
        let (mut lo, mut hi) = (addr, addr + len);
        let mut out = Vec::with_capacity(self.0.len() + 1);
        let mut placed = false;
        for &(a, b) in &self.0 {
            if b < lo {
                out.push((a, b));
            } else if hi < a {
                if !placed {
                    out.push((lo, hi));
                    placed = true;
                }
                out.push((a, b));
            } else {
                lo = lo.min(a);
                hi = hi.max(b);
            }
        }
        if !placed {
            out.push((lo, hi));
        }
        self.0 = out;
    }

    pub fn with(mut self, addr: i64, len: i64) -> Self {
        self.record(addr, len);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn contains(&self, addr: i64) -> bool {
        self.0.iter().any(|&(a, b)| a <= addr && addr < b)
    }

    /// Bytes covered.
    pub fn size(&self) -> i64 {
        self.0.iter().map(|&(a, b)| b - a).sum()
    }

    /// Inclusive `(lo, hi)` pairs, ascending.
    pub fn to_list(&self) -> Vec<(i64, i64)> {
        self.0.iter().map(|&(a, b)| (a, b - 1)).collect()
    }

    pub fn union(&self, other: &Intervals) -> Intervals {
        let mut r = self.clone();
        for &(a, b) in &other.0 {
            r.record(a, b - a);
        }
        r
    }

    pub fn intersect(&self, other: &Intervals) -> Intervals {
        let mut out = Vec::new();
        for &(a, b) in &self.0 {
            for &(c, d) in &other.0 {
                let lo = a.max(c);
                let hi = b.min(d);
                if lo < hi {
                    out.push((lo, hi));
                }
            }
        }
        out.sort();
        let mut r = Intervals::new();
        for (a, b) in out {
            r.record(a, b - a);
        }
        r
    }

    pub fn difference(&self, other: &Intervals) -> Intervals {
        let mut out = Vec::new();
        for &(a, b) in &self.0 {
            let mut pieces = vec![(a, b)];
            for &(c, d) in &other.0 {
                let mut next = Vec::new();
                for (x, y) in pieces {
                    if d <= x || y <= c {
                        next.push((x, y));
                    } else {
                        if x < c {
                            next.push((x, c));
                        }
                        if d < y {
                            next.push((d, y));
                        }
                    }
                }
                pieces = next;
            }
            out.extend(pieces);
        }
        Intervals(out)
    }

    fn render_with(&self, fmt: impl Fn(i64) -> String) -> String {
        if self.0.is_empty() {
            return "-".to_string();
        }
        self.0
            .iter()
            .map(|&(a, b)| format!("{}..{} ({} B)", fmt(a), fmt(b - 1), b - a))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// `lo..hi (n B)` ranges in decimal, `-` when empty.
    pub fn render(&self) -> String {
        self.render_with(|n| n.to_string())
    }

    /// The same in hex, at least two digits.
    pub fn render_hex(&self) -> String {
        self.render_with(|n| format!("0x{:02x}", n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjacent_ranges_merge() {
        let i = Intervals::new().with(0, 4).with(4, 4).with(10, 2);
        assert_eq!(i.to_list(), vec![(0, 7), (10, 11)]);
        assert_eq!(i.size(), 10);
        assert_eq!(i.render_hex(), "0x00..0x07 (8 B), 0x0a..0x0b (2 B)");
    }

    #[test]
    fn set_operations() {
        let a = Intervals::range(0, 9);
        let b = Intervals::range(5, 14);
        assert_eq!(a.intersect(&b).to_list(), vec![(5, 9)]);
        assert_eq!(a.difference(&b).to_list(), vec![(0, 4)]);
        assert_eq!(a.union(&b).to_list(), vec![(0, 14)]);
    }
}
