//! A scanner for one JSON line (s05, workshop 008 section 5): it walks a record line once, checks all of it, and
//! hands back where each top-level value is, without building the values. Load reads the hot fields this way and
//! skips the note, evidence and history.
//!
//! It accepts a subset of what `serde_json` accepts and reads it the same way: anything it is unsure of (a lone
//! surrogate escape, deep nesting, a repeated key) is refused, and the caller falls back to `umx_core::codec`, which
//! gives serde's answer or its exact error. A differential test (tests/scan.rs) holds the two together.

use alloc::borrow::Cow;
use alloc::string::String;
use core::str::FromStr;
use umx_core::Value;

/// serde_json's own limit is 128 levels; deeper than this is left to it.
const MAX_DEPTH: u32 = 100;

/// The scanner refused the line; the caller asks the codec instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Refused;

pub(crate) type Scanned<T> = Result<T, Refused>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Str,
    Num,
    Bool(bool),
    Null,
    Arr,
    Obj,
}

/// One value: its kind and byte range in the line. For a string the range is the text between the quotes, and
/// `esc` says whether it holds a backslash escape.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Val {
    pub kind: Kind,
    pub start: usize,
    pub end: usize,
    pub esc: bool,
}

/// A top-level key: the text between its quotes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Key {
    pub start: usize,
    pub end: usize,
    pub esc: bool,
}

const ONES: u64 = 0x0101_0101_0101_0101;
const HIGHS: u64 = 0x8080_8080_8080_8080;

/// A high bit set for each byte of `v` (little endian) that is zero. Only bytes above a true zero can be marked
/// falsely, so the lowest mark is always a true one.
#[inline]
fn zeros(v: u64) -> u64 {
    v.wrapping_sub(ONES) & !v & HIGHS
}

/// Marks the bytes of `x` that are `"`, `\` or below 0x20 (the bytes that end a plain run inside a string); the
/// lowest mark is the first such byte.
#[inline]
fn special(x: u64) -> u64 {
    let below_space = x.wrapping_sub(ONES * 0x20) & !x & HIGHS;
    zeros(x ^ (ONES * u64::from(b'"'))) | zeros(x ^ (ONES * u64::from(b'\\'))) | below_space
}

#[inline]
fn word(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("8 bytes"))
}

/// The position of the first `\n` at or after `from`, or `b.len()`.
pub(crate) fn newline(b: &[u8], from: usize) -> usize {
    let mut i = from;
    while i + 8 <= b.len() {
        let m = zeros(word(b, i) ^ (ONES * u64::from(b'\n')));
        if m != 0 {
            return i + m.trailing_zeros() as usize / 8;
        }
        i += 8;
    }
    b[i..]
        .iter()
        .position(|&c| c == b'\n')
        .map_or(b.len(), |p| i + p)
}

/// Same answer as `codec`'s blank-line test.
pub(crate) fn blank(line: &[u8]) -> bool {
    line.iter().all(u8::is_ascii_whitespace)
}

struct Cur<'a> {
    b: &'a [u8],
    i: usize,
}

impl Cur<'_> {
    #[inline]
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    #[inline]
    fn ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.peek() {
            self.i += 1;
        }
    }

    #[inline]
    fn eat(&mut self, c: u8) -> Scanned<()> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(Refused)
        }
    }

    fn hex4(&mut self) -> Scanned<u32> {
        let digits = self.b.get(self.i..self.i + 4).ok_or(Refused)?;
        let mut n = 0;
        for &d in digits {
            n = n * 16 + char::from(d).to_digit(16).ok_or(Refused)?;
        }
        self.i += 4;
        Ok(n)
    }

    /// After the opening quote: up to and past the closing quote. Returns (start, end, escaped).
    fn string(&mut self) -> Scanned<(usize, usize, bool)> {
        self.eat(b'"')?;
        let start = self.i;
        let mut esc = false;
        loop {
            while self.i + 8 <= self.b.len() {
                let m = special(word(self.b, self.i));
                if m != 0 {
                    self.i += m.trailing_zeros() as usize / 8;
                    break;
                }
                self.i += 8;
            }
            match self.peek().ok_or(Refused)? {
                b'"' => {
                    let end = self.i;
                    self.i += 1;
                    return Ok((start, end, esc));
                }
                b'\\' => {
                    esc = true;
                    self.i += 1;
                    match self.peek().ok_or(Refused)? {
                        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => self.i += 1,
                        b'u' => {
                            self.i += 1;
                            let n = self.hex4()?;
                            if (0xD800..0xDC00).contains(&n) {
                                // Only a well-formed pair; serde's handling of a lone half differs by version.
                                if self.b.get(self.i..self.i + 2) != Some(b"\\u") {
                                    return Err(Refused);
                                }
                                self.i += 2;
                                if !(0xDC00..0xE000).contains(&self.hex4()?) {
                                    return Err(Refused);
                                }
                            } else if (0xDC00..0xE000).contains(&n) {
                                return Err(Refused);
                            }
                        }
                        _ => return Err(Refused),
                    }
                }
                0..=0x1f => return Err(Refused),
                _ => self.i += 1,
            }
        }
    }

    fn digits(&mut self) -> usize {
        let from = self.i;
        while let Some(b'0'..=b'9') = self.peek() {
            self.i += 1;
        }
        self.i - from
    }

    /// A number token by JSON's grammar. One that is not a short plain integer is also parsed by serde, which
    /// refuses what does not fit an f64.
    fn number(&mut self, s: &str) -> Scanned<()> {
        let start = self.i;
        let mut plain = true;
        if self.peek() == Some(b'-') {
            self.i += 1;
            plain = false;
        }
        match self.peek() {
            Some(b'0') => self.i += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(Refused),
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            plain = false;
            if self.digits() == 0 {
                return Err(Refused);
            }
        }
        if let Some(b'e' | b'E') = self.peek() {
            self.i += 1;
            plain = false;
            if let Some(b'+' | b'-') = self.peek() {
                self.i += 1;
            }
            if self.digits() == 0 {
                return Err(Refused);
            }
        }
        if !plain || self.i - start > 18 {
            Value::from_str(&s[start..self.i]).map_err(|_| Refused)?;
        }
        Ok(())
    }

    fn literal(&mut self, word: &[u8]) -> Scanned<()> {
        if self.b.get(self.i..self.i + word.len()) == Some(word) {
            self.i += word.len();
            Ok(())
        } else {
            Err(Refused)
        }
    }

    /// One value of any kind, checked through; whitespace before it is the caller's.
    fn value(&mut self, s: &str, depth: u32) -> Scanned<Val> {
        let start = self.i;
        let (kind, start, end, esc) = match self.peek().ok_or(Refused)? {
            b'"' => {
                let (a, b, esc) = self.string()?;
                (Kind::Str, a, b, esc)
            }
            b'-' | b'0'..=b'9' => {
                self.number(s)?;
                (Kind::Num, start, self.i, false)
            }
            b't' => {
                self.literal(b"true")?;
                (Kind::Bool(true), start, self.i, false)
            }
            b'f' => {
                self.literal(b"false")?;
                (Kind::Bool(false), start, self.i, false)
            }
            b'n' => {
                self.literal(b"null")?;
                (Kind::Null, start, self.i, false)
            }
            b'[' => {
                if depth >= MAX_DEPTH {
                    return Err(Refused);
                }
                self.i += 1;
                self.ws();
                if self.peek() == Some(b']') {
                    self.i += 1;
                } else {
                    loop {
                        self.value(s, depth + 1)?;
                        self.ws();
                        match self.peek() {
                            Some(b',') => {
                                self.i += 1;
                                self.ws();
                            }
                            Some(b']') => {
                                self.i += 1;
                                break;
                            }
                            _ => return Err(Refused),
                        }
                    }
                }
                (Kind::Arr, start, self.i, false)
            }
            b'{' => {
                if depth >= MAX_DEPTH {
                    return Err(Refused);
                }
                self.i += 1;
                self.ws();
                if self.peek() == Some(b'}') {
                    self.i += 1;
                } else {
                    loop {
                        self.string()?;
                        self.ws();
                        self.eat(b':')?;
                        self.ws();
                        self.value(s, depth + 1)?;
                        self.ws();
                        match self.peek() {
                            Some(b',') => {
                                self.i += 1;
                                self.ws();
                            }
                            Some(b'}') => {
                                self.i += 1;
                                break;
                            }
                            _ => return Err(Refused),
                        }
                    }
                }
                (Kind::Obj, start, self.i, false)
            }
            _ => return Err(Refused),
        };
        Ok(Val {
            kind,
            start,
            end,
            esc,
        })
    }
}

/// Walks a line that must be one JSON object, calling `each` with every top-level key and value in line order.
/// The whole line is checked, nested values included; nothing but whitespace may follow the object.
pub(crate) fn object<'a>(
    line: &'a [u8],
    mut each: impl FnMut(&'a str, Key, Val) -> Scanned<()>,
) -> Scanned<&'a str> {
    let s = core::str::from_utf8(line).map_err(|_| Refused)?;
    let mut c = Cur { b: line, i: 0 };
    c.ws();
    c.eat(b'{')?;
    c.ws();
    if c.peek() == Some(b'}') {
        c.i += 1;
    } else {
        loop {
            let (start, end, esc) = c.string()?;
            c.ws();
            c.eat(b':')?;
            c.ws();
            let v = c.value(s, 1)?;
            each(s, Key { start, end, esc }, v)?;
            c.ws();
            match c.peek() {
                Some(b',') => {
                    c.i += 1;
                    c.ws();
                }
                Some(b'}') => {
                    c.i += 1;
                    break;
                }
                _ => return Err(Refused),
            }
        }
    }
    c.ws();
    if c.i != line.len() {
        return Err(Refused);
    }
    Ok(s)
}

/// Each item's `kind` in an evidence array the scanner checked (its text, brackets included): for an item that is an
/// object, its `kind` member when that is a string, the last one when the key repeats (serde's map keeps the last).
/// Any other item adds nothing. In item order, repeats kept. No `Value` is built.
pub(crate) fn evidence_kinds<'a>(array: &'a str, mut each: impl FnMut(Cow<'a, str>)) {
    const CHECKED: &str = "an evidence array the scanner checked";
    let mut c = Cur {
        b: array.as_bytes(),
        i: 0,
    };
    c.eat(b'[').expect(CHECKED);
    c.ws();
    if c.peek() == Some(b']') {
        return;
    }
    loop {
        // The depths `object` gave: the array is a top-level value (1), its items 2, their members 3.
        if c.peek() == Some(b'{') {
            c.i += 1;
            c.ws();
            let mut kind = None;
            if c.peek() == Some(b'}') {
                c.i += 1;
            } else {
                loop {
                    let (start, end, esc) = c.string().expect(CHECKED);
                    c.ws();
                    c.eat(b':').expect(CHECKED);
                    c.ws();
                    let v = c.value(array, 3).expect(CHECKED);
                    if text(&array[start..end], esc) == "kind" {
                        kind = Some(v);
                    }
                    c.ws();
                    if c.peek() == Some(b',') {
                        c.i += 1;
                        c.ws();
                    } else {
                        c.eat(b'}').expect(CHECKED);
                        break;
                    }
                }
            }
            if let Some(v) = kind
                && v.kind == Kind::Str
            {
                each(text(&array[v.start..v.end], v.esc));
            }
        } else {
            c.value(array, 2).expect(CHECKED);
        }
        c.ws();
        if c.peek() == Some(b',') {
            c.i += 1;
            c.ws();
        } else {
            c.eat(b']').expect(CHECKED);
            return;
        }
    }
}

/// The text of a string the scanner checked (between its quotes), escapes undone as serde undoes them.
pub(crate) fn text(raw: &str, esc: bool) -> Cow<'_, str> {
    if !esc {
        return Cow::Borrowed(raw);
    }
    let b = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    let hex =
        |at: usize| u32::from_str_radix(&raw[at..at + 4], 16).expect("checked by the scanner");
    while i < b.len() {
        let next = b[i..]
            .iter()
            .position(|&c| c == b'\\')
            .map_or(b.len(), |p| i + p);
        out.push_str(&raw[i..next]);
        if next == b.len() {
            break;
        }
        i = next + 2;
        match b[next + 1] {
            b'"' => out.push('"'),
            b'\\' => out.push('\\'),
            b'/' => out.push('/'),
            b'b' => out.push('\u{8}'),
            b'f' => out.push('\u{c}'),
            b'n' => out.push('\n'),
            b'r' => out.push('\r'),
            b't' => out.push('\t'),
            _ => {
                let n = hex(i);
                i += 4;
                let c = if (0xD800..0xDC00).contains(&n) {
                    let low = hex(i + 2);
                    i += 6;
                    0x10000 + ((n - 0xD800) << 10) + (low - 0xDC00)
                } else {
                    n
                };
                out.push(char::from_u32(c).expect("checked by the scanner"));
            }
        }
    }
    Cow::Owned(out)
}

/// A number as serde_json holds it: a non-negative integer, a negative one, or a float. Keeping which one keeps
/// `1` and `1.0` apart.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Num {
    U(u64),
    I(i64),
    F(f64),
}

impl Num {
    /// A number token the scanner checked.
    pub fn parse(token: &str) -> Num {
        if token.len() <= 18 && token.bytes().all(|b| b.is_ascii_digit()) {
            return Num::U(token.parse().expect("checked digits"));
        }
        let v = Value::from_str(token).expect("checked by the scanner");
        Num::of(&v).expect("a number token is a number")
    }

    pub fn of(v: &Value) -> Option<Num> {
        let Value::Number(n) = v else { return None };
        Some(if let Some(u) = n.as_u64() {
            Num::U(u)
        } else if let Some(i) = n.as_i64() {
            Num::I(i)
        } else {
            Num::F(n.as_f64()?)
        })
    }

    /// `serde_json::Number::as_f64`'s answer, which `--where` compares.
    #[inline]
    pub fn as_f64(self) -> f64 {
        match self {
            Num::U(u) => u as f64,
            Num::I(i) => i as f64,
            Num::F(f) => f,
        }
    }

    pub fn value(self) -> Value {
        match self {
            Num::U(u) => Value::from(u),
            Num::I(i) => Value::from(i),
            Num::F(f) => Value::from(f),
        }
    }
}

/// The value of a checked span, as serde builds it.
pub(crate) fn value(s: &str, v: Val) -> Value {
    match v.kind {
        Kind::Str => Value::String(text(&s[v.start..v.end], v.esc).into_owned()),
        Kind::Num => Num::parse(&s[v.start..v.end]).value(),
        Kind::Bool(b) => Value::Bool(b),
        Kind::Null => Value::Null,
        Kind::Arr if v.end - v.start == 2 => Value::Array(alloc::vec::Vec::new()),
        Kind::Arr | Kind::Obj => {
            Value::from_str(&s[v.start..v.end]).expect("checked by the scanner")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newline_and_special_find_the_first_match_at_any_position() {
        for len in 0..40 {
            for at in 0..=len {
                let mut b = alloc::vec![b'a'; len];
                if at < len {
                    b[at] = b'\n';
                }
                assert_eq!(newline(&b, 0), at, "{len} {at}");
            }
        }
        for c in [b'"', b'\\', 0, 0x1f] {
            for at in 0..8 {
                let mut b = [b'x'; 8];
                b[at] = c;
                assert_eq!(
                    special(u64::from_le_bytes(b)).trailing_zeros() / 8,
                    at as u32,
                    "{c} at {at}"
                );
            }
        }
        assert_eq!(special(u64::from_le_bytes(*b" ~\x7f\xc3\xa9azz")), 0);
        // The first match is found below later ones.
        assert_eq!(
            special(u64::from_le_bytes(*b"a\"\x00\\\"bcd")).trailing_zeros() / 8,
            1
        );
    }

    #[test]
    fn numbers_keep_their_form() {
        assert_eq!(Num::parse("3"), Num::U(3));
        assert_eq!(Num::parse("-3"), Num::I(-3));
        assert_eq!(Num::parse("1.0").value().to_string(), "1.0");
        assert_eq!(Num::parse("18446744073709551615"), Num::U(u64::MAX));
        assert_eq!(Num::parse("1e2"), Num::F(100.0));
    }

    use alloc::string::ToString;
}
