//! A term index (ws003 s22 and s29, lifted for ws008 tags): each term text has a number; the inverted index holds each
//! term's rows, in handle order, and the forward column each row's term numbers. Its users are the evidence kinds
//! ([`crate::evidence_kinds`]) and the std tags; each reads a row's terms through [`TermSrc`]. Inside,
//! a term is called a kind, the name of its first user.

use crate::cols::NameCol;
use crate::words::fnv;
use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::mem::{size_of, take};
use core::slice;

/// Where a row's terms are read from: each one through `f`, repeats kept.
pub(crate) trait TermSrc<'a> {
    fn each_term(self, f: impl FnMut(Cow<'a, str>));
}

/// The most kinds a mask holds (graph-storage-expert's s29 spec).
pub(crate) const MASK_MAX: usize = 64;
/// A CSR goes back to a mask only at the end of a row operation that leaves this many kinds or fewer (graph-storage,
/// s29 C1), so a graph at 64 or 65 kinds does not rebuild the forward column on every write.
pub(crate) const MASK_BACK: usize = 56;

/// Each row's kind numbers.
#[derive(Clone, Debug)]
pub(crate) enum Forward {
    /// At most [`MASK_MAX`] kinds, numbered by rank in name order: row `h`'s kinds are the set bits of its `stride`
    /// bytes (little-endian, `ceil(kinds / 8)` bytes a row), so they read out in name order.
    Mask {
        stride: usize,
        bits: Vec<u8>,
        len: usize,
    },
    /// Past [`MASK_MAX`] kinds, numbered in arrival order with freed numbers reused: row `h`'s numbers are
    /// `nums[ends[h - 1]..ends[h]]`, in name order of their texts. `u16` until a number passes 65,535.
    Csr { ends: Vec<u32>, nums: NameCol },
}

impl Default for Forward {
    fn default() -> Forward {
        Forward::Mask {
            stride: 0,
            bits: Vec::new(),
            len: 0,
        }
    }
}

fn stride_for(kinds: usize) -> usize {
    kinds.div_ceil(8)
}

/// Row `h`'s mask. Strides of 1, 2, 4 and 8 bytes are one load each (every read of a row's kinds comes here; a copy of
/// a run-time length is a `memcpy` call).
#[inline]
fn read_mask(bits: &[u8], stride: usize, h: usize) -> u64 {
    let at = h * stride;
    match stride {
        1 => u64::from(bits[h]),
        2 => u64::from(u16::from_le_bytes([bits[at], bits[at + 1]])),
        4 => u64::from(u32::from_le_bytes(
            bits[at..at + 4].try_into().expect("4 bytes"),
        )),
        8 => u64::from_le_bytes(bits[at..at + 8].try_into().expect("8 bytes")),
        _ => {
            let mut b = [0; 8];
            b[..stride].copy_from_slice(&bits[at..at + stride]);
            u64::from_le_bytes(b)
        }
    }
}

fn write_mask(bits: &mut [u8], stride: usize, h: usize, m: u64) {
    debug_assert!(stride == 8 || m >> (8 * stride) == 0);
    bits[h * stride..(h + 1) * stride].copy_from_slice(&m.to_le_bytes()[..stride]);
}

/// Row `h`'s range in a CSR.
fn span(ends: &[u32], h: usize) -> core::ops::Range<usize> {
    let start = if h == 0 { 0 } else { ends[h - 1] as usize };
    start..ends[h] as usize
}

/// The kind texts by number, each kind's rows (each list ascending with a row at most once, a number whose list is
/// empty is free) and each row's kind numbers.
#[derive(Clone, Debug, Default)]
pub(crate) struct Terms {
    /// Kind text by number; a free number holds an empty text.
    texts: Vec<Box<str>>,
    /// The inverted index: each kind's rows, by number.
    rows: Vec<Vec<u32>>,
    /// Live numbers, found by their text's FNV hash.
    pub(crate) index: hashbrown::HashTable<u32>,
    /// Free numbers (a CSR only; a mask's numbers are its ranks, `0..kinds`).
    free: Vec<u32>,
    pub(crate) fwd: Forward,
    /// A row's numbers while a row function works on it.
    scratch: Vec<u32>,
    /// Tests only: how many times the forward column changed between a mask and a CSR.
    #[cfg(test)]
    pub(crate) switches: usize,
}

impl Terms {
    fn find(&self, kind: &str) -> Option<u32> {
        let texts = &self.texts;
        self.index
            .find(fnv(kind), |&n| *texts[n as usize] == *kind)
            .copied()
    }

    /// How many rows the forward column holds.
    fn len(&self) -> usize {
        match &self.fwd {
            Forward::Mask { len, .. } => *len,
            Forward::Csr { ends, .. } => ends.len(),
        }
    }

    /// Every mask row through `f`, laid out again at `stride` bytes.
    fn remask(&mut self, stride: usize, f: impl Fn(u64) -> u64) {
        let Forward::Mask {
            stride: old,
            bits,
            len,
        } = &mut self.fwd
        else {
            unreachable!("remask needs a mask")
        };
        let mut out = vec![0; *len * stride];
        for h in 0..*len {
            write_mask(&mut out, stride, h, f(read_mask(bits, *old, h)));
        }
        *bits = out;
        *old = stride;
    }

    /// `kind`'s number, added when no row holds it. A new kind in a mask moves every rank at or above its own up one,
    /// in every row and in `pending` (the numbers a row function has taken so far); the 65th turns the mask into a CSR
    /// first.
    fn number(&mut self, kind: &str, pending: &mut [u32]) -> u32 {
        if let Some(n) = self.find(kind) {
            return n;
        }
        if matches!(self.fwd, Forward::Mask { .. }) && self.index.len() == MASK_MAX {
            self.make_csr();
        }
        let n = match self.fwd {
            Forward::Mask { .. } => {
                let r = self.texts.partition_point(|t| **t < *kind);
                self.texts.insert(r, kind.into());
                self.rows.insert(r, Vec::new());
                let r = r as u32;
                for n in self.index.iter_mut().chain(pending.iter_mut()) {
                    if *n >= r {
                        *n += 1;
                    }
                }
                let low = (1u64 << r) - 1;
                self.remask(stride_for(self.texts.len()), |m| {
                    (m & low) | ((m & !low) << 1)
                });
                r
            }
            Forward::Csr { .. } => match self.free.pop() {
                Some(n) => {
                    self.texts[n as usize] = kind.into();
                    n
                }
                None => {
                    self.texts.push(kind.into());
                    self.rows.push(Vec::new());
                    (self.texts.len() - 1) as u32
                }
            },
        };
        let texts = &self.texts;
        self.index
            .insert_unique(fnv(kind), n, |&x| fnv(&texts[x as usize]));
        n
    }

    /// Kind `n`, whose list just emptied, goes. In a mask every rank above it moves down one in every row.
    fn drop_kind(&mut self, n: u32) {
        let i = n as usize;
        debug_assert!(self.rows[i].is_empty());
        match self.index.find_entry(fnv(&self.texts[i]), |&x| x == n) {
            Ok(e) => {
                e.remove();
            }
            Err(_) => unreachable!("a live kind is in the index"),
        }
        match self.fwd {
            Forward::Mask { .. } => {
                self.texts.remove(i);
                self.rows.remove(i);
                for x in self.index.iter_mut() {
                    if *x > n {
                        *x -= 1;
                    }
                }
                let low = (1u64 << n) - 1;
                self.remask(stride_for(self.texts.len()), |m| {
                    (m & low) | ((m >> 1) & !low)
                });
            }
            Forward::Csr { .. } => {
                self.texts[i] = Box::default();
                self.rows[i] = Vec::new();
                self.free.push(n);
            }
        }
    }

    /// The mask becomes a CSR with the same numbers (its ranks), so each row's numbers stay in name order.
    fn make_csr(&mut self) {
        let Forward::Mask { stride, bits, len } = take(&mut self.fwd) else {
            unreachable!("make_csr needs a mask")
        };
        let mut ends = Vec::with_capacity(len);
        let mut nums = NameCol::default();
        for h in 0..len {
            let mut m = read_mask(&bits, stride, h);
            while m != 0 {
                nums.push(m.trailing_zeros());
                m &= m - 1;
            }
            ends.push(nums.len() as u32);
        }
        self.fwd = Forward::Csr { ends, nums };
        #[cfg(test)]
        {
            self.switches += 1;
        }
    }

    /// The CSR becomes a mask: the live kinds are numbered again by rank in name order.
    fn make_mask(&mut self) {
        let mut order: Vec<u32> = self.index.iter().copied().collect();
        order.sort_unstable_by(|&a, &b| self.texts[a as usize].cmp(&self.texts[b as usize]));
        let mut rank = vec![u32::MAX; self.texts.len()];
        for (r, &n) in order.iter().enumerate() {
            rank[n as usize] = r as u32;
        }
        self.texts = order
            .iter()
            .map(|&n| take(&mut self.texts[n as usize]))
            .collect();
        self.rows = order
            .iter()
            .map(|&n| take(&mut self.rows[n as usize]))
            .collect();
        self.free.clear();
        self.index.clear();
        for n in 0..self.texts.len() {
            let texts = &self.texts;
            self.index
                .insert_unique(fnv(&texts[n]), n as u32, |&x| fnv(&texts[x as usize]));
        }
        let Forward::Csr { ends, nums } = take(&mut self.fwd) else {
            unreachable!("make_mask needs a CSR")
        };
        let stride = stride_for(order.len());
        let mut bits = vec![0; ends.len() * stride];
        for h in 0..ends.len() {
            let m = span(&ends, h).fold(0u64, |m, i| m | 1 << rank[nums.get(i) as usize]);
            write_mask(&mut bits, stride, h, m);
        }
        self.fwd = Forward::Mask {
            stride,
            bits,
            len: ends.len(),
        };
        #[cfg(test)]
        {
            self.switches += 1;
        }
    }

    /// The end of every row function: a CSR left with [`MASK_BACK`] kinds or fewer goes back to a mask.
    fn settle(&mut self) {
        if matches!(self.fwd, Forward::Csr { .. }) && self.index.len() <= MASK_BACK {
            self.make_mask();
        }
    }

    /// Row `h`'s numbers into `out`, in name order.
    fn row_nums(&self, h: usize, out: &mut Vec<u32>) {
        match &self.fwd {
            Forward::Mask { stride, bits, .. } => {
                let mut m = read_mask(bits, *stride, h);
                while m != 0 {
                    out.push(m.trailing_zeros());
                    m &= m - 1;
                }
            }
            Forward::Csr { ends, nums } => out.extend(span(ends, h).map(|i| nums.get(i))),
        }
    }

    /// A new last row's kinds: `h` is above every handle held, so it goes at the end of each list.
    pub fn push<'a>(&mut self, h: u32, src: impl TermSrc<'a>) {
        debug_assert_eq!(h as usize, self.len());
        let mut nums = take(&mut self.scratch);
        nums.clear();
        src.each_term(|kind| {
            let n = self.number(&kind, &mut nums);
            let rows = &mut self.rows[n as usize];
            if rows.last() != Some(&h) {
                rows.push(h);
            }
            nums.push(n);
        });
        match &mut self.fwd {
            Forward::Mask { stride, bits, len } => {
                let m = nums.iter().fold(0u64, |m, &n| m | 1 << n);
                bits.extend_from_slice(&m.to_le_bytes()[..*stride]);
                *len += 1;
            }
            Forward::Csr { ends, nums: col } => {
                let texts = &self.texts;
                nums.sort_unstable_by(|&a, &b| texts[a as usize].cmp(&texts[b as usize]));
                nums.dedup();
                for &n in &nums {
                    col.push(n);
                }
                ends.push(col.len() as u32);
            }
        }
        self.scratch = nums;
    }

    /// Row `h`'s kinds from its new line. Nothing moves when they are the same. Otherwise the row leaves the kinds it
    /// drops (highest number first, so a mask's emptied kind moves only ranks not yet visited), then joins the ones it
    /// gains, and its forward row is written: a mask's bytes, or a CSR's numbers in place when their count is the same,
    /// else spliced with the later ends shifted.
    pub fn set<'a>(&mut self, h: u32, src: impl TermSrc<'a>) {
        let mut kinds = Vec::new();
        src.each_term(|kind| kinds.push(kind));
        kinds.sort_unstable();
        kinds.dedup();
        if kinds.iter().map(|k| &**k).eq(self.kinds(h)) {
            return;
        }
        let mut nums = take(&mut self.scratch);
        nums.clear();
        self.row_nums(h as usize, &mut nums);
        let texts = &self.texts;
        nums.retain(|&n| {
            kinds
                .binary_search_by(|k| (**k).cmp(&*texts[n as usize]))
                .is_err()
        });
        nums.sort_unstable_by(|a, b| b.cmp(a));
        for &n in &nums {
            let rows = &mut self.rows[n as usize];
            let at = rows
                .binary_search(&h)
                .expect("a row is in its kinds' lists");
            rows.remove(at);
            if rows.is_empty() {
                self.drop_kind(n);
            }
        }
        for kind in &kinds {
            let n = self.number(kind, &mut []);
            let rows = &mut self.rows[n as usize];
            if let Err(at) = rows.binary_search(&h) {
                rows.insert(at, h);
            }
        }
        nums.clear();
        nums.extend(
            kinds
                .iter()
                .map(|k| self.find(k).expect("a row's kind has a number")),
        );
        let h = h as usize;
        match &mut self.fwd {
            Forward::Mask { stride, bits, .. } => {
                let m = nums.iter().fold(0u64, |m, &n| m | 1 << n);
                write_mask(bits, *stride, h, m);
            }
            Forward::Csr { ends, nums: col } => {
                let at = span(ends, h);
                if at.len() == nums.len() {
                    for (i, &n) in at.zip(&nums) {
                        col.set(i, n);
                    }
                } else {
                    let delta = nums.len() as i64 - at.len() as i64;
                    col.splice(at, &nums);
                    for e in &mut ends[h..] {
                        *e = (i64::from(*e) + delta) as u32;
                    }
                }
            }
        }
        self.scratch = nums;
        self.settle();
    }

    /// The last row (`h`) is removed: it is the last entry of each of its kinds' lists.
    pub fn pop(&mut self, h: u32) {
        debug_assert_eq!(h as usize + 1, self.len());
        let mut nums = take(&mut self.scratch);
        nums.clear();
        self.row_nums(h as usize, &mut nums);
        match &mut self.fwd {
            Forward::Mask { stride, bits, len } => {
                bits.truncate(bits.len() - *stride);
                *len -= 1;
            }
            Forward::Csr { ends, nums: col } => {
                ends.pop();
                col.truncate(ends.last().map_or(0, |&e| e as usize));
            }
        }
        // Highest number first: a mask's emptied kind moves only the ranks above it.
        nums.sort_unstable_by(|a, b| b.cmp(a));
        for &n in &nums {
            let rows = &mut self.rows[n as usize];
            debug_assert_eq!(rows.last(), Some(&h));
            rows.pop();
            if rows.is_empty() {
                self.drop_kind(n);
            }
        }
        self.scratch = nums;
        self.settle();
    }

    pub fn clear(&mut self) {
        self.texts.clear();
        self.rows.clear();
        self.index.clear();
        self.free.clear();
        self.fwd = Forward::default();
    }

    /// Row `h`'s kinds, in name order, from the forward column: O(kinds of the row).
    #[inline]
    pub fn kinds(&self, h: u32) -> TermsOf<'_> {
        let (h, texts) = (h as usize, &self.texts[..]);
        match &self.fwd {
            Forward::Mask { stride, bits, .. } => TermsOf::Mask {
                bits: read_mask(bits, *stride, h),
                texts,
            },
            Forward::Csr { ends, nums } => match nums {
                NameCol::Narrow(v) => TermsOf::Narrow(v[span(ends, h)].iter(), texts),
                NameCol::Wide(v) => TermsOf::Wide(v[span(ends, h)].iter(), texts),
            },
        }
    }

    /// The rows holding `kind`, ascending; empty for a kind no row holds.
    pub fn rows(&self, kind: &str) -> &[u32] {
        self.find(kind).map_or(&[], |n| &self.rows[n as usize][..])
    }

    /// Every kind some row holds, with its rows, in number order (the `=` filter's walk over every kind: the texts in
    /// memory order, not the hash index's).
    pub fn each(&self) -> impl Iterator<Item = (&str, &[u32])> + '_ {
        self.texts
            .iter()
            .zip(&self.rows)
            .filter(|(_, rows)| !rows.is_empty())
            .map(|(text, rows)| (&**text, &rows[..]))
    }

    /// The kind texts, the inverted index and the hash index over the texts.
    pub fn heap_bytes(&self) -> usize {
        self.texts.capacity() * size_of::<Box<str>>()
            + self.texts.iter().map(|t| t.len()).sum::<usize>()
            + self.rows.capacity() * size_of::<Vec<u32>>()
            + self
                .rows
                .iter()
                .map(|r| r.capacity() * size_of::<u32>())
                .sum::<usize>()
            + self.index.allocation_size()
            + self.free.capacity() * size_of::<u32>()
    }

    /// The forward column (ws003 s29).
    pub fn forward_bytes(&self) -> usize {
        self.scratch.capacity() * size_of::<u32>()
            + match &self.fwd {
                Forward::Mask { bits, .. } => bits.capacity(),
                Forward::Csr { ends, nums } => {
                    ends.capacity() * size_of::<u32>() + nums.heap_bytes()
                }
            }
    }

    pub fn shrink(&mut self) {
        self.texts.shrink_to_fit();
        self.rows.iter_mut().for_each(Vec::shrink_to_fit);
        self.rows.shrink_to_fit();
        let texts = &self.texts;
        self.index.shrink_to_fit(|&x| fnv(&texts[x as usize]));
        self.free.shrink_to_fit();
        self.scratch = Vec::new();
        match &mut self.fwd {
            Forward::Mask { bits, .. } => bits.shrink_to_fit(),
            Forward::Csr { ends, nums } => {
                ends.shrink_to_fit();
                nums.shrink();
            }
        }
    }

    /// The forward column's form, for tests: `mask`, `narrow` (a `u16` CSR) or `wide`.
    pub fn layout(&self) -> &'static str {
        match &self.fwd {
            Forward::Mask { .. } => "mask",
            Forward::Csr {
                nums: NameCol::Narrow(_),
                ..
            } => "narrow",
            Forward::Csr { .. } => "wide",
        }
    }

    /// Panics unless this column answers what `fresh` (built from the same image) answers: each row's kinds and each
    /// kind's rows by text (a CSR's numbers follow its own history), and unless its own parts agree. A mask equals the
    /// fresh one byte for byte when both are masks.
    pub fn check(&self, fresh: &Terms) {
        let rows = self.len();
        assert_eq!(rows, fresh.len(), "terms: rows");
        for h in 0..rows as u32 {
            assert!(self.kinds(h).eq(fresh.kinds(h)), "terms of row {h}");
        }
        assert_eq!(self.index.len(), fresh.index.len(), "terms: kinds");
        for (kind, held) in fresh.each() {
            assert_eq!(self.rows(kind), held, "term {kind:?}: rows");
        }
        // Its own parts: the index over every live number, free numbers empty, each row in the lists of its kinds.
        let live = self.rows.iter().filter(|r| !r.is_empty()).count();
        assert_eq!(live, self.index.len(), "terms: live numbers");
        for (n, text) in self.texts.iter().enumerate() {
            if !self.rows[n].is_empty() {
                assert_eq!(self.find(text), Some(n as u32), "term {text:?}: number");
            }
        }
        let mut nums = Vec::new();
        let mut held = 0;
        for h in 0..rows {
            nums.clear();
            self.row_nums(h, &mut nums);
            held += nums.len();
            for w in nums.windows(2) {
                assert!(
                    self.texts[w[0] as usize] < self.texts[w[1] as usize],
                    "terms of row {h}: name order"
                );
            }
            for &n in &nums {
                assert!(
                    self.rows[n as usize].binary_search(&(h as u32)).is_ok(),
                    "row {h} in the list of kind {n}"
                );
            }
        }
        assert_eq!(
            held,
            self.rows.iter().map(Vec::len).sum::<usize>(),
            "terms: forward and inverted entries"
        );
        match &self.fwd {
            Forward::Mask { stride, .. } => {
                assert!(self.index.len() <= MASK_MAX, "a mask past 64 kinds");
                assert!(self.free.is_empty(), "a mask with free numbers");
                assert_eq!(
                    self.texts.len(),
                    self.index.len(),
                    "a mask's numbers are its ranks"
                );
                assert!(self.texts.is_sorted(), "a mask's numbers are its ranks");
                assert_eq!(*stride, stride_for(self.texts.len()), "a mask's stride");
                if let Forward::Mask {
                    bits: fresh_bits, ..
                } = &fresh.fwd
                    && let Forward::Mask { bits, .. } = &self.fwd
                {
                    assert_eq!(bits, fresh_bits, "terms: mask bytes");
                }
            }
            Forward::Csr { .. } => {
                assert!(self.index.len() > MASK_BACK, "a CSR at 56 kinds or fewer");
                let mut free = self.free.clone();
                free.sort_unstable();
                let empty: Vec<u32> = (0..self.rows.len() as u32)
                    .filter(|&n| self.rows[n as usize].is_empty())
                    .collect();
                assert_eq!(free, empty, "terms: free numbers");
            }
        }
    }
}

/// A row's kinds, in name order, from the forward column.
pub(crate) enum TermsOf<'g> {
    Mask { bits: u64, texts: &'g [Box<str>] },
    Narrow(slice::Iter<'g, u16>, &'g [Box<str>]),
    Wide(slice::Iter<'g, u32>, &'g [Box<str>]),
}

impl<'g> Iterator for TermsOf<'g> {
    type Item = &'g str;

    #[inline]
    fn next(&mut self) -> Option<&'g str> {
        match self {
            TermsOf::Mask { bits, texts } => {
                if *bits == 0 {
                    return None;
                }
                let n = bits.trailing_zeros();
                *bits &= *bits - 1;
                Some(&texts[n as usize])
            }
            TermsOf::Narrow(nums, texts) => nums.next().map(|&n| &*texts[usize::from(n)]),
            TermsOf::Wide(nums, texts) => nums.next().map(|&n| &*texts[n as usize]),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = match self {
            TermsOf::Mask { bits, .. } => bits.count_ones() as usize,
            TermsOf::Narrow(nums, _) => nums.len(),
            TermsOf::Wide(nums, _) => nums.len(),
        };
        (n, Some(n))
    }
}

impl ExactSizeIterator for TermsOf<'_> {}
