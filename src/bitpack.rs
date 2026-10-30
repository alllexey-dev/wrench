//! Packing values into bits of a stream of machine words (used by wasm32).
//!
//! A layout is a sequence of fields, each a number of bits, read and written
//! most significant bit first; a field may cross a word boundary, and `word`
//! groups the fields that make up exactly one word. [`BitGet`] asks for the
//! next word only when a field reaches it, so the layout itself decides how
//! many words are read. A failure names where it happened through `label`.

use crate::word::W;

pub type Res<T> = Result<T, String>;

const WORD_BITS: u32 = W::BITS;

/// The widest field whose value fits a non-negative `i64`.
const MAX_FIELD_BITS: u32 = 63;

/// A type with a bit layout of its own, so layouts compose by type:
/// `let tag = Tag::unpack(g)?` inside a larger layout.
pub trait BitPack: Sized {
    fn unpack(g: &mut BitGet) -> Res<Self>;
    fn pack(&self, p: &mut BitPut) -> Res<()>;
}

/// A type whose constructors are numbered by their position in `ALL`, as
/// Haskell's `Enum` numbers them.
pub trait BitEnum: Copy + PartialEq + 'static {
    const ALL: &'static [Self];
}

/// Pack a value into whole words.
pub fn pack_words<A: BitPack>(a: &A) -> Res<Vec<W>> {
    run_bit_put(|p| a.pack(p))
}

/// Unpack a value from exactly these words; fails if words are left over.
pub fn unpack_words<A: BitPack>(words: &[W]) -> Res<A> {
    let (a, used) = run_bit_get_rest(A::unpack, words)?;
    let left = words.len() - used;
    if left > 0 {
        return Err(format!("words left over after decoding: {}", left));
    }
    Ok(a)
}

/// A decoder's position in the word stream; words come from `fetch` as the
/// fields reach them.
pub struct BitGet<'a> {
    fetch: &'a mut dyn FnMut(usize) -> Option<W>,
    fetched: usize,
    word: u32,
    used: u32,
}

impl BitGet<'_> {
    /// The next `n` bits, unsigned.
    pub fn get_field(&mut self, n: u32) -> Res<i64> {
        check_width(n)?;
        let mut k = n;
        let mut acc: u64 = 0;
        while k > 0 {
            if self.used == WORD_BITS {
                let w = (self.fetch)(self.fetched).ok_or("out of words")?;
                self.fetched += 1;
                self.word = w as u32;
                self.used = 0;
            }
            let taken = k.min(WORD_BITS - self.used);
            let bits = (self.word as u64 >> (WORD_BITS - self.used - taken)) & mask(taken);
            acc = (acc << taken) | bits;
            self.used += taken;
            k -= taken;
        }
        Ok(acc as i64)
    }

    /// The next `n` bits as a constructor of `E`; fails on a value no
    /// constructor has.
    pub fn get_enum<E: BitEnum>(&mut self, n: u32) -> Res<E> {
        let value = self.get_field(n)?;
        usize::try_from(value)
            .ok()
            .and_then(|k| E::ALL.get(k).copied())
            .ok_or_else(|| format!("unknown tag {}", value))
    }

    /// Skip `n` bits, any number of them.
    pub fn get_pad(&mut self, n: u32) -> Res<()> {
        pad_chunks(n).try_for_each(|k| self.get_field(k).map(|_| ()))
    }

    /// Run a layout that must cover exactly one word, starting on a word
    /// boundary.
    pub fn word<T>(&mut self, f: impl FnOnce(&mut Self) -> Res<T>) -> Res<T> {
        if self.used != WORD_BITS {
            return Err("word layout doesn't start on a word boundary".to_string());
        }
        let before = self.fetched;
        let a = f(self)?;
        if self.fetched - before == 1 && self.used == WORD_BITS {
            Ok(a)
        } else {
            Err(not_one_word())
        }
    }

    /// Prefix a failure with where it happened; nested labels read
    /// outermost first.
    pub fn label<T>(&mut self, name: &str, f: impl FnOnce(&mut Self) -> Res<T>) -> Res<T> {
        f(self).map_err(|e| format!("{}: {}", name, e))
    }
}

/// An encoder: the words written so far and the one being filled.
#[derive(Default)]
pub struct BitPut {
    written: Vec<W>,
    word: u32,
    used: u32,
}

impl BitPut {
    /// Write `value` into the next `n` bits.
    pub fn put_field(&mut self, n: u32, value: i64) -> Res<()> {
        check_width(n)?;
        if value < 0 || value as u64 > mask(n) {
            return Err(format!("{} doesn't fit in {} bits", value, n));
        }
        let mut k = n;
        while k > 0 {
            let taken = k.min(WORD_BITS - self.used);
            let bits = ((value as u64) >> (k - taken)) & mask(taken);
            self.word |= (bits as u32) << (WORD_BITS - self.used - taken);
            self.used += taken;
            if self.used == WORD_BITS {
                self.written.push(self.word as W);
                self.word = 0;
                self.used = 0;
            }
            k -= taken;
        }
        Ok(())
    }

    /// Write a constructor of `E` into the next `n` bits.
    pub fn put_enum<E: BitEnum>(&mut self, n: u32, e: E) -> Res<()> {
        let k = E::ALL.iter().position(|&x| x == e).expect("constructor listed in ALL");
        self.put_field(n, k as i64)
    }

    /// `n` zero bits, any number of them.
    pub fn put_pad(&mut self, n: u32) -> Res<()> {
        pad_chunks(n).try_for_each(|k| self.put_field(k, 0))
    }

    /// Run a layout that must cover exactly one word, starting on a word
    /// boundary.
    pub fn word<T>(&mut self, f: impl FnOnce(&mut Self) -> Res<T>) -> Res<T> {
        if self.used != 0 {
            return Err("word layout doesn't start on a word boundary".to_string());
        }
        let before = self.written.len();
        let a = f(self)?;
        if self.used == 0 && self.written.len() == before + 1 {
            Ok(a)
        } else {
            Err(not_one_word())
        }
    }

    /// Prefix a failure with where it happened; nested labels read
    /// outermost first.
    pub fn label<T>(&mut self, name: &str, f: impl FnOnce(&mut Self) -> Res<T>) -> Res<T> {
        f(self).map_err(|e| format!("{}: {}", name, e))
    }
}

/// Decode, fetching word `k` of the stream with `fetch(k)` when the layout
/// reaches it.
pub fn run_bit_get_with<T>(mut fetch: impl FnMut(usize) -> W, decoder: impl FnOnce(&mut BitGet) -> Res<T>) -> Res<T> {
    let mut f = |k| Some(fetch(k));
    decoder(&mut BitGet {
        fetch: &mut f,
        fetched: 0,
        word: 0,
        used: WORD_BITS,
    })
}

/// Decode a list of words; fails if the layout runs past them.
pub fn run_bit_get<T>(decoder: impl FnOnce(&mut BitGet) -> Res<T>, words: &[W]) -> Res<T> {
    run_bit_get_rest(decoder, words).map(|(a, _)| a)
}

/// The value and how many words it took.
fn run_bit_get_rest<T>(decoder: impl FnOnce(&mut BitGet) -> Res<T>, words: &[W]) -> Res<(T, usize)> {
    let mut f = |k: usize| words.get(k).copied();
    let mut g = BitGet {
        fetch: &mut f,
        fetched: 0,
        word: 0,
        used: WORD_BITS,
    };
    let a = decoder(&mut g)?;
    Ok((a, g.fetched))
}

/// Pack the fields into words; fails if a value doesn't fit its field or the
/// fields don't fill whole words.
pub fn run_bit_put(encoder: impl FnOnce(&mut BitPut) -> Res<()>) -> Res<Vec<W>> {
    let mut p = BitPut::default();
    encoder(&mut p)?;
    if p.used != 0 {
        return Err(format!(
            "{} bits don't fill whole {}-bit words",
            p.written.len() as u32 * WORD_BITS + p.used,
            WORD_BITS
        ));
    }
    Ok(p.written)
}

fn check_width(n: u32) -> Res<()> {
    if n > MAX_FIELD_BITS {
        Err(format!("field width {} is outside 0..{}", n, MAX_FIELD_BITS))
    } else {
        Ok(())
    }
}

fn not_one_word() -> String {
    format!("word layout doesn't fill exactly one {}-bit word", WORD_BITS)
}

fn pad_chunks(n: u32) -> impl Iterator<Item = u32> {
    let rest = n % MAX_FIELD_BITS;
    std::iter::repeat_n(MAX_FIELD_BITS, (n / MAX_FIELD_BITS) as usize).chain((rest > 0).then_some(rest))
}

fn mask(n: u32) -> u64 {
    (1u64 << n) - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Color {
        Red,
        Green,
        Blue,
    }

    impl BitEnum for Color {
        const ALL: &'static [Color] = &[Color::Red, Color::Green, Color::Blue];
    }

    impl BitPack for Color {
        fn unpack(g: &mut BitGet) -> Res<Self> {
            g.label("color", |g| g.get_enum(2))
        }

        fn pack(&self, p: &mut BitPut) -> Res<()> {
            p.label("color", |p| p.put_enum(2, *self))
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    struct Pixel {
        color: Color,
        level: i64,
    }

    impl BitPack for Pixel {
        fn unpack(g: &mut BitGet) -> Res<Self> {
            g.word(|g| {
                let color = Color::unpack(g)?;
                let level = g.get_field(30)?;
                Ok(Pixel { color, level })
            })
        }

        fn pack(&self, p: &mut BitPut) -> Res<()> {
            p.word(|p| {
                self.color.pack(p)?;
                p.label("level", |p| p.put_field(30, self.level))
            })
        }
    }

    fn w(x: u32) -> W {
        x as W
    }

    fn err<T>(s: &str) -> Res<T> {
        Err(s.to_string())
    }

    #[test]
    fn fields_pack_msb_first_and_may_cross_a_word() {
        let encode = |p: &mut BitPut| {
            p.put_field(4, 0xA)?;
            p.put_field(8, 0xBC)?;
            p.put_field(20, 0xDEF01)?;
            p.put_field(32, 0x12345678)
        };
        let decode = |g: &mut BitGet| Ok((g.get_field(4)?, g.get_field(8)?, g.get_field(20)?, g.get_field(32)?));
        assert_eq!(run_bit_put(encode), Ok(vec![w(0xABCDEF01), 0x12345678]));
        assert_eq!(
            run_bit_get(decode, &[w(0xABCDEF01), 0x12345678]),
            Ok((0xA, 0xBC, 0xDEF01, 0x12345678))
        );
    }

    #[test]
    fn a_field_spanning_two_words_reads_both() {
        let encode = |p: &mut BitPut| {
            p.put_pad(28)?;
            p.put_field(8, 0xFF)?;
            p.put_pad(28)
        };
        let decode = |g: &mut BitGet| {
            g.get_pad(28)?;
            g.get_field(8)
        };
        assert_eq!(run_bit_put(encode), Ok(vec![0x0000000F, w(0xF0000000)]));
        assert_eq!(run_bit_get(decode, &[0x0000000F, w(0xF0000000)]), Ok(0xFF));
    }

    #[test]
    fn a_42_bit_field_uses_two_words() {
        let encode = |p: &mut BitPut| {
            p.put_field(3, 0b101)?;
            p.put_field(42, 0x2ABCDEF0123)?;
            p.put_pad(19)
        };
        let decode = |g: &mut BitGet| {
            g.get_pad(3)?;
            g.get_field(42)
        };
        assert_eq!(run_bit_put(encode), Ok(vec![w(0xB55E6F78), 0x09180000]));
        assert_eq!(run_bit_get(decode, &[w(0xB55E6F78), 0x09180000]), Ok(0x2ABCDEF0123));
    }

    #[test]
    fn word_layouts_group_the_fields_of_one_word_each() {
        let encode = |p: &mut BitPut| {
            p.word(|p| {
                p.put_field(4, 0xA)?;
                p.put_field(28, 0xBCDEF01)
            })?;
            p.word(|p| {
                p.put_field(16, 0x1234)?;
                p.put_field(16, 0x5678)
            })
        };
        let decode = |g: &mut BitGet| {
            let (a, b) = g.word(|g| Ok((g.get_field(4)?, g.get_field(28)?)))?;
            let (c, d) = g.word(|g| Ok((g.get_field(16)?, g.get_field(16)?)))?;
            Ok((a, b, c, d))
        };
        assert_eq!(run_bit_put(encode), Ok(vec![w(0xABCDEF01), 0x12345678]));
        assert_eq!(
            run_bit_get(decode, &[w(0xABCDEF01), 0x12345678]),
            Ok((0xA, 0xBCDEF01, 0x1234, 0x5678))
        );
    }

    #[test]
    fn a_word_layout_fills_exactly_one_word_from_a_boundary() {
        let not_one = "word layout doesn't fill exactly one 32-bit word";
        let misaligned = "word layout doesn't start on a word boundary";
        assert_eq!(run_bit_put(|p| p.word(|p| p.put_pad(31))), err(not_one));
        assert_eq!(run_bit_put(|p| p.word(|p| p.put_pad(64))), err(not_one));
        let misaligned_put = |p: &mut BitPut| {
            p.put_pad(1)?;
            p.word(|p| p.put_pad(32))?;
            p.put_pad(31)
        };
        assert_eq!(run_bit_put(misaligned_put), err(misaligned));
        assert_eq!(run_bit_get(|g| g.word(|g| g.get_pad(31)), &[0, 0]), err(not_one));
        assert_eq!(run_bit_get(|g| g.word(|g| g.get_pad(33)), &[0, 0]), err(not_one));
        let misaligned_get = |g: &mut BitGet| {
            g.get_pad(1)?;
            g.word(|g| g.get_pad(32))
        };
        assert_eq!(run_bit_get(misaligned_get, &[0, 0]), err(misaligned));
    }

    #[test]
    fn a_value_that_doesnt_fit_its_field_is_rejected() {
        let encode = |x: i64| {
            move |p: &mut BitPut| {
                p.put_field(4, x)?;
                p.put_pad(28)
            }
        };
        assert_eq!(run_bit_put(encode(16)), err("16 doesn't fit in 4 bits"));
        assert_eq!(run_bit_put(encode(-1)), err("-1 doesn't fit in 4 bits"));
    }

    #[test]
    fn labels_name_where_a_layout_failed_outermost_first() {
        let encode = |p: &mut BitPut| {
            p.label("record", |p| {
                p.label("x", |p| p.put_field(4, 16))?;
                p.put_pad(28)
            })
        };
        assert_eq!(run_bit_put(encode), err("record: x: 16 doesn't fit in 4 bits"));
        let decode = |g: &mut BitGet| g.label("record", |g| g.label("x", |g| g.get_field(1)));
        assert_eq!(run_bit_get(decode, &[]), err("record: x: out of words"));
    }

    #[test]
    fn padding_may_be_any_width_a_field_at_most_63_bits() {
        assert_eq!(run_bit_put(|p| p.put_pad(96)), Ok(vec![0, 0, 0]));
        assert_eq!(run_bit_get(|g| g.get_pad(96), &[0, 0, 0]), Ok(()));
        assert_eq!(
            run_bit_put(|p| p.put_field(64, 0)),
            err("field width 64 is outside 0..63")
        );
        assert_eq!(
            run_bit_get(|g| g.get_field(64), &[0, 0]),
            err("field width 64 is outside 0..63")
        );
    }

    #[test]
    fn fields_must_fill_whole_words() {
        assert_eq!(
            run_bit_put(|p| p.put_pad(31)),
            err("31 bits don't fill whole 32-bit words")
        );
    }

    #[test]
    fn an_enum_field_rejects_a_value_no_constructor_has() {
        let encode = |width: u32| {
            move |p: &mut BitPut| {
                p.label("color", |p| p.put_enum(width, Color::Blue))?;
                p.put_pad(32 - width)
            }
        };
        assert_eq!(run_bit_put(encode(2)), Ok(vec![w(0x80000000)]));
        assert_eq!(run_bit_put(encode(1)), err("color: 2 doesn't fit in 1 bits"));
        assert_eq!(
            run_bit_get(|g| g.get_enum::<Color>(2), &[w(0x80000000)]),
            Ok(Color::Blue)
        );
        assert_eq!(
            run_bit_get(|g| g.label("color", |g| g.get_enum::<Color>(2)), &[w(0xC0000000)]),
            err("color: unknown tag 3")
        );
    }

    #[test]
    fn a_decoder_fetches_only_the_words_it_reaches() {
        fn fetched<T>(decoder: impl FnOnce(&mut BitGet) -> Res<T>) -> Vec<usize> {
            let mut log = Vec::new();
            let _ = run_bit_get_with(
                |k| {
                    log.push(k);
                    0
                },
                decoder,
            );
            log
        }
        assert_eq!(fetched(|g| g.get_field(2)), vec![0]);
        assert_eq!(fetched(|g| g.get_field(32)), vec![0]);
        assert_eq!(fetched(|g| g.get_field(33)), vec![0, 1]);
        assert_eq!(
            fetched(|g| {
                g.get_pad(32)?;
                g.get_field(32)
            }),
            vec![0, 1]
        );
        assert_eq!(
            fetched(|g| {
                let tag = g.get_field(2)?;
                if tag == 0 {
                    Ok(0)
                } else {
                    g.get_field(32)
                }
            }),
            vec![0]
        );
    }

    #[test]
    fn a_bitpack_instance_composes_inside_anothers_layout() {
        let pixel = Pixel {
            color: Color::Blue,
            level: 0x5A5,
        };
        assert_eq!(pack_words(&pixel), Ok(vec![w(0x800005A5)]));
        assert_eq!(
            unpack_words(&[w(0x800005A5)]),
            Ok(Pixel {
                color: Color::Blue,
                level: 0x5A5
            })
        );
        assert_eq!(unpack_words::<Pixel>(&[w(0xC0000000)]), err("color: unknown tag 3"));
        assert_eq!(
            pack_words(&Pixel {
                level: 1 << 30,
                ..pixel
            }),
            err("level: 1073741824 doesn't fit in 30 bits")
        );
    }

    #[test]
    fn unpack_words_rejects_words_left_over() {
        assert_eq!(
            unpack_words::<Pixel>(&[w(0x800005A5), 0]),
            err("words left over after decoding: 1")
        );
    }

    #[test]
    fn a_list_decoder_fails_past_the_end_of_its_words() {
        assert_eq!(run_bit_get(|g| g.get_field(33), &[0]), err("out of words"));
        assert_eq!(run_bit_get(|g| g.get_field(1), &[]), err("out of words"));
    }
}
