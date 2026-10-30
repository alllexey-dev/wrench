//! Machine-word helpers: fixed-width truncation, shifts with the semantics of
//! the original simulator, arithmetic that also reports overflow and carry,
//! and range guards for instruction fields.

/// The machine word of every ISA.
pub type W = i32;

/// Keep the low `n` bits and sign-extend from bit `n-1`.
pub fn fit_signed(n: u32, x: W) -> W {
    if n >= 32 {
        return x;
    }
    let mask: i32 = ((1i64 << n) - 1) as i32;
    let low = x & mask;
    if n > 0 && low & (1 << (n - 1)) != 0 {
        low | !mask
    } else {
        low
    }
}

/// Logical shift right; a shift of 32 or more gives 0.
pub fn lshr(x: W, n: i64) -> W {
    if !(0..32).contains(&n) {
        0
    } else {
        ((x as u32) >> n) as i32
    }
}

/// Shift left; a shift of 32 or more gives 0.
pub fn shl(x: W, n: i64) -> W {
    if !(0..32).contains(&n) {
        0
    } else {
        ((x as u32) << n) as i32
    }
}

/// Arithmetic shift right; a shift of 32 or more gives 0 or -1.
pub fn sar(x: W, n: i64) -> W {
    if n < 0 {
        0
    } else if n >= 32 {
        if x < 0 {
            -1
        } else {
            0
        }
    } else {
        x >> n
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ext {
    pub value: W,
    pub overflow: bool,
    pub carry: bool,
}

pub fn add_ext(x: W, y: W) -> Ext {
    let value = x.wrapping_add(y);
    let overflow = (x > 0 && y > 0 && value < 0) || (x < 0 && y < 0 && value > 0);
    let carry = (x as u32 as u64 + y as u32 as u64) >> 32 & 1 == 1;
    Ext { value, overflow, carry }
}

pub fn sub_ext(x: W, y: W) -> Ext {
    let value = x.wrapping_sub(y);
    let overflow = (x > 0 && y < 0 && value < 0) || (x < 0 && y > 0 && value > 0);
    let carry = (x as u32) < (y as u32);
    Ext { value, overflow, carry }
}

pub fn mul_ext(x: W, y: W) -> Ext {
    let value = x.wrapping_mul(y);
    let overflow = x != 0 && y != 0 && (x == -1 && value == W::MIN || value.div_euclid_floor(x) != y);
    // The original compares a wrapped 64-bit product against its own maximum, so it never sets carry.
    Ext {
        value,
        overflow,
        carry: false,
    }
}

trait FloorDiv {
    fn div_euclid_floor(self, d: Self) -> Self;
}

impl FloorDiv for i32 {
    fn div_euclid_floor(self, d: i32) -> i32 {
        let q = self.wrapping_div(d);
        if (self % d != 0) && ((self < 0) != (d < 0)) {
            q - 1
        } else {
            q
        }
    }
}

/// Floored division (Haskell `div`), defined for the corner cases.
pub fn div_floor(x: W, y: W) -> W {
    if y == -1 {
        return x.wrapping_neg();
    }
    x.div_euclid_floor(y)
}

/// Floored modulo (Haskell `mod`).
pub fn mod_floor(x: W, y: W) -> W {
    if y == -1 {
        return 0;
    }
    let r = x % y;
    if r != 0 && ((r < 0) != (y < 0)) {
        r + y
    } else {
        r
    }
}

/// A field guard: the value must lie in `lo..=hi`, otherwise the message is built from it.
pub fn guard(lo: i64, hi: i64, msg: impl Fn(&str) -> String, x: W) -> Result<W, String> {
    let v = x as i64;
    if lo <= v && v <= hi {
        Ok(x)
    } else {
        Err(msg(&x.to_string()))
    }
}

macro_rules! field_guard {
    ($name:ident, $lo:expr, $hi:expr) => {
        pub fn $name(msg: impl Fn(&str) -> String, x: W) -> Result<W, String> {
            guard($lo, $hi, msg, x)
        }
    };
}

field_guard!(b8, -128, 255);
field_guard!(u5, 0, 31);
field_guard!(u8, 0, 255);
field_guard!(u16, 0, 65535);
field_guard!(u20, 0, 1048575);
field_guard!(i8, -128, 127);
field_guard!(i10, -512, 511);
field_guard!(i12, -2048, 2047);
field_guard!(i13, -4096, 4095);
field_guard!(i15, -16384, 16383);
field_guard!(i16, -32768, 32767);
field_guard!(i17, -65536, 65535);
field_guard!(i20, -524288, 524287);
field_guard!(i21, -1048576, 1048575);

/// Little-endian bytes of a word, lowest first.
pub fn word_split(w: W) -> [u8; 4] {
    w.to_le_bytes()
}

pub fn word_combine(bytes: [u8; 4]) -> W {
    W::from_le_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_signed_sign_extends() {
        assert_eq!(fit_signed(12, 0xFFF), -1);
        assert_eq!(fit_signed(12, 0x7FF), 0x7FF);
        assert_eq!(fit_signed(12, 0x1800), -2048);
    }

    #[test]
    fn ext_flags() {
        assert!(add_ext(W::MAX, 1).overflow);
        assert!(add_ext(-1, 1).carry);
        assert!(sub_ext(0, 1).carry);
        assert!(mul_ext(0x10000, 0x10000).overflow);
        assert!(!mul_ext(3, -4).overflow);
    }

    #[test]
    fn floor_ops() {
        assert_eq!(div_floor(-7, 2), -4);
        assert_eq!(mod_floor(-7, 2), 1);
        assert_eq!(div_floor(W::MIN, -1), W::MIN);
    }
}
