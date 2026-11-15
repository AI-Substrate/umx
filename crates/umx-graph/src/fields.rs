//! Scalar field columns (s05 design note 5; workshop 008 sections 2.2 and 2.4): for each custom field name, the
//! nodes holding it, sorted by handle, one list per value kind. Numbers keep their integer or decimal form, strings
//! are a place in the node's line, and a list, object or null is only a handle (read from the line when asked).
//! The first entry over all four lists is the field's first use, which fixes an undeclared field's kind.
//!
//! Beside them, the typed time column (D71 step 10): for each value that `umx_core::time::date_span` reads as a
//! date, its span of UTC time and precision as numbers. It is derived from the value's text alone, so the text is
//! still the truth; the row functions keep it with the value it came from.

use crate::json::Num;
use crate::line::FieldVal;
use alloc::vec::Vec;
use core::mem::size_of;
use umx_core::time::{DateSpan, Precision};

/// A number: 16 bytes.
#[derive(Clone, Copy, Debug)]
struct NumAt {
    bits: u64,
    h: u32,
    tag: u8,
}

impl NumAt {
    fn new(h: u32, n: Num) -> NumAt {
        let (tag, bits) = match n {
            Num::U(u) => (0, u),
            Num::I(i) => (1, i as u64),
            Num::F(f) => (2, f.to_bits()),
        };
        NumAt { bits, h, tag }
    }

    fn num(&self) -> Num {
        match self.tag {
            0 => Num::U(self.bits),
            1 => Num::I(self.bits as i64),
            _ => Num::F(f64::from_bits(self.bits)),
        }
    }
}

/// A string: where its text sits in the node's line. The top bit of `len` says it holds an escape. 12 bytes.
#[derive(Clone, Copy, Debug)]
struct StrAt {
    h: u32,
    rel: u32,
    len: u32,
}

const ESC: u32 = 1 << 31;

impl StrAt {
    fn val(&self) -> FieldVal {
        FieldVal::Str {
            rel: self.rel,
            len: self.len & !ESC,
            esc: self.len & ESC != 0,
        }
    }
}

/// A date value as numbers: its UTC start in seconds (negative before 1970), and its length in seconds in the low 30
/// bits with its precision in the top 2. The longest span, a leap year, is 31,622,400 seconds, well below 2^30. 16
/// bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DateAt {
    start: i64,
    h: u32,
    prec_len: u32,
}

const LEN_BITS: u32 = 30;
const LEN_MASK: u32 = (1 << LEN_BITS) - 1;

impl DateAt {
    fn new(h: u32, span: DateSpan) -> DateAt {
        let len = span.end - span.start;
        debug_assert!(
            (1..=i64::from(LEN_MASK)).contains(&len),
            "a date span fits 30 bits"
        );
        let prec = match span.precision() {
            Precision::Second => 0,
            Precision::Day => 1,
            Precision::Month => 2,
            Precision::Year => 3,
        };
        DateAt {
            start: span.start,
            h,
            prec_len: (prec << LEN_BITS) | len as u32,
        }
    }

    fn span(&self) -> DateSpan {
        DateSpan {
            start: self.start,
            end: self.start + i64::from(self.prec_len & LEN_MASK),
        }
    }

    fn precision(&self) -> Precision {
        match self.prec_len >> LEN_BITS {
            0 => Precision::Second,
            1 => Precision::Day,
            2 => Precision::Month,
            _ => Precision::Year,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FieldCol {
    nums: Vec<NumAt>,
    strs: Vec<StrAt>,
    bools: Vec<(u32, bool)>,
    other: Vec<u32>,
    /// The holders whose value reads as a date, sorted by handle: the typed time column.
    dates: Vec<DateAt>,
}

/// Inserts at `h`'s place in a list sorted by handle (at the end when `h` is the largest, as in a load).
fn put<T>(list: &mut Vec<T>, h: u32, at: impl Fn(&T) -> u32, item: T) {
    let i = list.partition_point(|x| at(x) < h);
    list.insert(i, item);
}

fn take<T>(list: &mut Vec<T>, h: u32, at: impl Fn(&T) -> u32) -> bool {
    match list.binary_search_by_key(&h, at) {
        Ok(i) => {
            list.remove(i);
            true
        }
        Err(_) => false,
    }
}

fn at<T>(list: &[T], h: u32, key: impl Fn(&T) -> u32) -> Option<usize> {
    list.binary_search_by_key(&h, key).ok()
}

fn find<T: Copy>(list: &[T], h: u32, at: impl Fn(&T) -> u32) -> Option<T> {
    list.binary_search_by_key(&h, at).ok().map(|i| list[i])
}

impl FieldCol {
    /// Adds node `h`'s value and, when it reads as one, its date, keeping each list sorted by handle.
    pub fn insert(&mut self, h: u32, v: FieldVal, date: Option<DateSpan>) {
        self.insert_value(h, v);
        if let Some(d) = date {
            put(&mut self.dates, h, |x| x.h, DateAt::new(h, d));
        }
    }

    fn insert_value(&mut self, h: u32, v: FieldVal) {
        match v {
            FieldVal::Num(n) => put(&mut self.nums, h, |x| x.h, NumAt::new(h, n)),
            FieldVal::Str { rel, len, esc } => put(
                &mut self.strs,
                h,
                |x| x.h,
                StrAt {
                    h,
                    rel,
                    len: len | if esc { ESC } else { 0 },
                },
            ),
            FieldVal::Bool(b) => put(&mut self.bools, h, |x| x.0, (h, b)),
            FieldVal::Other => put(&mut self.other, h, |x| *x, h),
        }
    }

    /// Takes node `h`'s value and its date out.
    pub fn remove(&mut self, h: u32) {
        self.remove_value(h);
        take(&mut self.dates, h, |x| x.h);
    }

    fn remove_value(&mut self, h: u32) {
        let _ = take(&mut self.nums, h, |x| x.h)
            || take(&mut self.strs, h, |x| x.h)
            || take(&mut self.bools, h, |x| x.0)
            || take(&mut self.other, h, |x| *x);
    }

    /// Sets node `h`'s value (s26): in place when `h` already holds one of the same kind (number, string, bool or
    /// other), so an edit moves no list; otherwise out of its old kind's list, if any, and into `v`'s. Its date
    /// follows the new value: set in place, added, or dropped when the new value reads as no date (a kind change
    /// included).
    pub fn set(&mut self, h: u32, v: FieldVal, date: Option<DateSpan>) {
        let held = match v {
            FieldVal::Num(n) => at(&self.nums, h, |x| x.h).map(|i| self.nums[i] = NumAt::new(h, n)),
            FieldVal::Str { rel, len, esc } => at(&self.strs, h, |x| x.h).map(|i| {
                self.strs[i] = StrAt {
                    h,
                    rel,
                    len: len | if esc { ESC } else { 0 },
                }
            }),
            FieldVal::Bool(b) => at(&self.bools, h, |x| x.0).map(|i| self.bools[i] = (h, b)),
            FieldVal::Other => at(&self.other, h, |x| *x).map(drop),
        };
        if held.is_none() {
            self.remove_value(h);
            self.insert_value(h, v);
        }
        match (at(&self.dates, h, |x| x.h), date) {
            (Some(i), Some(d)) => self.dates[i] = DateAt::new(h, d),
            (Some(i), None) => {
                self.dates.remove(i);
            }
            (None, Some(d)) => put(&mut self.dates, h, |x| x.h, DateAt::new(h, d)),
            (None, None) => {}
        }
    }

    /// Node `h`'s date and its precision, when its value reads as one.
    pub fn date(&self, h: u32) -> Option<(DateSpan, Precision)> {
        find(&self.dates, h, |x| x.h).map(|x| (x.span(), x.precision()))
    }

    /// Every holder whose value reads as a date, in handle order.
    pub fn each_date(&self, mut f: impl FnMut(u32, DateSpan, Precision)) {
        for x in &self.dates {
            f(x.h, x.span(), x.precision());
        }
    }

    pub fn get(&self, h: u32) -> Option<FieldVal> {
        find(&self.nums, h, |x| x.h)
            .map(|x| FieldVal::Num(x.num()))
            .or_else(|| find(&self.strs, h, |x| x.h).map(|x| x.val()))
            .or_else(|| find(&self.bools, h, |x| x.0).map(|x| FieldVal::Bool(x.1)))
            .or_else(|| find(&self.other, h, |x| *x).map(|_| FieldVal::Other))
    }

    /// The node with the lowest handle that holds the field, and its value.
    pub fn first(&self) -> Option<(u32, FieldVal)> {
        [
            self.nums.first().map(|x| (x.h, FieldVal::Num(x.num()))),
            self.strs.first().map(|x| (x.h, x.val())),
            self.bools.first().map(|x| (x.0, FieldVal::Bool(x.1))),
            self.other.first().map(|&h| (h, FieldVal::Other)),
        ]
        .into_iter()
        .flatten()
        .min_by_key(|(h, _)| *h)
    }

    /// Every holder and its value, a kind at a time (each in handle order).
    pub fn each(&self, mut f: impl FnMut(u32, FieldVal)) {
        for x in &self.nums {
            f(x.h, FieldVal::Num(x.num()));
        }
        self.each_but_numbers(f);
    }

    /// The number holders, in handle order.
    #[inline]
    pub fn numbers(&self) -> impl ExactSizeIterator<Item = (u32, Num)> + '_ {
        self.nums.iter().map(|x| (x.h, x.num()))
    }

    /// `each` without the numbers.
    pub fn each_but_numbers(&self, mut f: impl FnMut(u32, FieldVal)) {
        for x in &self.strs {
            f(x.h, x.val());
        }
        for &(h, b) in &self.bools {
            f(h, FieldVal::Bool(b));
        }
        for &h in &self.other {
            f(h, FieldVal::Other);
        }
    }

    /// Every holder and its value, in handle order: the four lists merged, each row read once.
    pub fn each_in_order(&self, mut f: impl FnMut(u32, FieldVal)) {
        let (mut n, mut s, mut b, mut o) = (0, 0, 0, 0);
        loop {
            let next = [
                self.nums.get(n).map(|x| x.h),
                self.strs.get(s).map(|x| x.h),
                self.bools.get(b).map(|x| x.0),
                self.other.get(o).copied(),
            ];
            let Some(h) = next.into_iter().flatten().min() else {
                return;
            };
            if next[0] == Some(h) {
                f(h, FieldVal::Num(self.nums[n].num()));
                n += 1;
            } else if next[1] == Some(h) {
                f(h, self.strs[s].val());
                s += 1;
            } else if next[2] == Some(h) {
                f(h, FieldVal::Bool(self.bools[b].1));
                b += 1;
            } else {
                f(h, FieldVal::Other);
                o += 1;
            }
        }
    }

    /// The value lists' bytes; the dates are [`Self::date_bytes`].
    pub fn heap_bytes(&self) -> usize {
        self.nums.capacity() * size_of::<NumAt>()
            + self.strs.capacity() * size_of::<StrAt>()
            + self.bools.capacity() * size_of::<(u32, bool)>()
            + self.other.capacity() * size_of::<u32>()
    }

    /// The typed time column's bytes.
    pub fn date_bytes(&self) -> usize {
        self.dates.capacity() * size_of::<DateAt>()
    }

    pub fn shrink(&mut self) {
        self.nums.shrink_to_fit();
        self.strs.shrink_to_fit();
        self.bools.shrink_to_fit();
        self.other.shrink_to_fit();
        self.dates.shrink_to_fit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_column_keeps_holders_sorted_and_finds_the_first_use_over_every_kind() {
        let mut c = FieldCol::default();
        c.insert(5, FieldVal::Num(Num::F(1.0)), None);
        c.insert(3, FieldVal::Other, None);
        c.insert(
            9,
            FieldVal::Str {
                rel: 4,
                len: 2,
                esc: true,
            },
            None,
        );
        c.insert(1, FieldVal::Num(Num::I(-2)), None);
        assert!(matches!(c.first(), Some((1, FieldVal::Num(Num::I(-2))))));
        c.remove(1);
        assert!(matches!(c.first(), Some((3, FieldVal::Other))));
        assert!(matches!(
            c.get(9),
            Some(FieldVal::Str {
                rel: 4,
                len: 2,
                esc: true
            })
        ));
        assert!(matches!(c.get(5), Some(FieldVal::Num(Num::F(f))) if f == 1.0));
        assert!(c.get(1).is_none());
        let mut seen = alloc::vec![];
        c.each(|h, _| seen.push(h));
        assert_eq!(seen, [5, 9, 3]);
        assert_eq!(size_of::<NumAt>(), 16);
        assert_eq!(size_of::<StrAt>(), 12);
        assert_eq!(size_of::<DateAt>(), 16);
    }

    /// A date keeps its start, end and precision through the packing, before 1970 and in a leap year included, and
    /// follows its value through set: in place, dropped when the value stops reading as a date or changes kind.
    #[test]
    fn a_date_packs_into_16_bytes_and_follows_its_value() {
        let span = |s: &str| umx_core::time::date_span(s).unwrap();
        for s in [
            "2024",
            "0001",
            "9999",
            "1066-10",
            "2024-02",
            "1969-12-31",
            "2026-10-29T22:40:00Z",
            "1969-07-20T20:17:40Z",
            "0001-01-01T00:00:00Z",
            "9999-12-31T23:59:59Z",
        ] {
            let d = DateAt::new(7, span(s));
            assert_eq!(
                (d.span(), d.precision()),
                (span(s), span(s).precision()),
                "{s}"
            );
        }
        let s = |rel| FieldVal::Str {
            rel,
            len: 10,
            esc: false,
        };
        let mut c = FieldCol::default();
        c.insert(4, s(0), Some(span("2025-01")));
        c.insert(2, s(0), Some(span("1900")));
        c.insert(3, s(0), None);
        let held = |c: &FieldCol| {
            let mut v = alloc::vec![];
            c.each_date(|h, d, p| v.push((h, d, p)));
            v
        };
        assert_eq!(
            held(&c),
            [
                (2, span("1900"), Precision::Year),
                (4, span("2025-01"), Precision::Month)
            ]
        );
        c.set(4, s(1), Some(span("2025-01-02")));
        assert_eq!(c.date(4), Some((span("2025-01-02"), Precision::Day)));
        c.set(3, s(1), Some(span("2026-10-29T22:40:00Z")));
        assert_eq!(c.date(3).map(|d| d.1), Some(Precision::Second));
        c.set(4, s(2), None);
        c.set(2, FieldVal::Num(Num::U(1900)), None);
        assert_eq!(held(&c).len(), 1);
        c.remove(3);
        assert!(held(&c).is_empty());
    }
}
