//! Word search (req-68; workshop 008 section 11): an inverted index over every node's title, note and declared text
//! fields, built in the load pass and kept by every write, never on a read and never on disk. A query is scored by
//! BM25F (k1 1.2, b 0.75; title 3, note 1.5, other text 1) over corpus figures summed across the graphs searched,
//! so a word's rarity is judged over everything searched and scores mean the same inside and outside a subtree.
//!
//! Determinism (embeddings-expert, architect): term and field numbers depend on load and write order, so no sum runs
//! in their order. Query terms are summed in string order and a node's fields in name order, the log is libm's, and
//! ties go to the lower handle; two daemons that loaded the same graphs in any order give bit-identical scores.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;
use core::hash::Hasher;
use core::mem::size_of;

/// FNV-1a: fixed by its definition, so the term table hashes the same on every run and machine. Term numbers never
/// order anything, but a fixed hash keeps the index free of any seed.
#[derive(Default)]
struct Fnv(u64);

impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        let mut h = if self.0 == 0 {
            0xcbf2_9ce4_8422_2325
        } else {
            self.0
        };
        for b in bytes {
            h = (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.0 = h;
    }
}

pub(crate) fn fnv(word: &str) -> u64 {
    let mut h = Fnv::default();
    h.write(word.as_bytes());
    h.finish()
}

/// No next member: the end of a stem class.
const NONE: u32 = u32::MAX;
/// New terms wait in an unsorted tail of at most this many before they merge into the sorted order, so a load or
/// a batch of new words never shifts the whole order once per word.
const TAIL: usize = 1024;

/// The term table: each live term's word by term number, a hash index over them, the words in string order (prefix
/// ranges) and the classes of words sharing a Porter2 stem (ws004 s21). Freed numbers hold an empty word.
#[derive(Clone, Debug, Default)]
struct Terms {
    names: Vec<Box<str>>,
    index: hashbrown::HashTable<u32>,
    /// Term numbers in string order, then a short unsorted tail of newer ones.
    sorted: Vec<u32>,
    tail: Vec<u32>,
    /// The first term of each stem class; only words [`crate::stem::stemmed`] takes have one. An entry is found by
    /// its stem, recomputed from its first term's word, so no stem string is kept (graph-storage-expert, S186-1).
    stems: hashbrown::HashTable<u32>,
    /// The next term in the same stem class, or [`NONE`].
    next: Vec<u32>,
}

/// The stem classes' hash of term `t`'s class: its word's stem, hashed.
fn stem_hash(names: &[Box<str>], t: u32) -> u64 {
    fnv(&crate::stem::stem(&names[t as usize]))
}

impl Terms {
    /// The first term of `stem`'s class.
    fn head(&self, stem: &str) -> Option<u32> {
        let names = &self.names;
        self.stems
            .find(fnv(stem), |&h| {
                crate::stem::stem(&names[h as usize]) == stem
            })
            .copied()
    }

    fn get(&self, word: &str) -> Option<u32> {
        let names = &self.names;
        self.index
            .find(fnv(word), |&t| *names[t as usize] == *word)
            .copied()
    }

    #[cfg(feature = "bench")]
    fn len(&self) -> usize {
        self.index.len()
    }

    /// Live terms as (word, number), in no particular order.
    fn iter(&self) -> impl Iterator<Item = (&str, u32)> {
        self.index.iter().map(|&t| (&*self.names[t as usize], t))
    }

    /// Adds `word` as term `t` (a new number or a freed one).
    fn insert(&mut self, word: &str, t: u32) {
        let i = t as usize;
        if i == self.names.len() {
            self.names.push(word.into());
            self.next.push(NONE);
        } else {
            self.names[i] = word.into();
            self.next[i] = NONE;
        }
        let names = &self.names;
        self.index
            .insert_unique(fnv(word), t, |&x| fnv(&names[x as usize]));
        self.tail.push(t);
        if self.tail.len() > TAIL {
            self.merge_tail();
        }
        if crate::stem::stemmed(word) {
            let s = crate::stem::stem(word);
            let names = &self.names;
            let same = |&h: &u32| crate::stem::stem(&names[h as usize]) == s;
            match self.stems.find_mut(fnv(&s), same) {
                Some(head) => {
                    self.next[i] = *head;
                    *head = t;
                }
                None => {
                    self.stems
                        .insert_unique(fnv(&s), t, |&x| stem_hash(names, x));
                }
            }
        }
    }

    /// Removes term `t` (`word`): out of the index, the order and its stem class; its number holds nothing.
    fn remove(&mut self, word: &str, t: u32) {
        let names = &self.names;
        if let Ok(e) = self
            .index
            .find_entry(fnv(word), |&x| *names[x as usize] == *word)
        {
            e.remove();
        }
        if let Some(at) = self.tail.iter().position(|&x| x == t) {
            self.tail.swap_remove(at);
        } else if let Ok(at) = self
            .sorted
            .binary_search_by(|&x| (*names[x as usize]).cmp(word))
        {
            self.sorted.remove(at);
        }
        if crate::stem::stemmed(word) {
            let s = crate::stem::stem(word);
            let after = self.next[t as usize];
            let names = &self.names;
            let same = |&h: &u32| crate::stem::stem(&names[h as usize]) == s;
            if let Ok(head) = self.stems.find_entry(fnv(&s), same) {
                if *head.get() == t {
                    if after == NONE {
                        head.remove();
                    } else {
                        *head.into_mut() = after;
                    }
                } else {
                    let head = *head.get();
                    let mut p = head;
                    while self.next[p as usize] != t {
                        p = self.next[p as usize];
                    }
                    self.next[p as usize] = after;
                }
            }
        }
        self.next[t as usize] = NONE;
        self.names[t as usize] = Box::default();
    }

    fn merge_tail(&mut self) {
        let names = &self.names;
        self.tail
            .sort_unstable_by(|&a, &b| names[a as usize].cmp(&names[b as usize]));
        let mut out = Vec::with_capacity(self.sorted.len() + self.tail.len());
        let (mut i, mut j) = (0, 0);
        while i < self.sorted.len() && j < self.tail.len() {
            if names[self.sorted[i] as usize] < names[self.tail[j] as usize] {
                out.push(self.sorted[i]);
                i += 1;
            } else {
                out.push(self.tail[j]);
                j += 1;
            }
        }
        out.extend_from_slice(&self.sorted[i..]);
        out.extend_from_slice(&self.tail[j..]);
        self.sorted = out;
        self.tail.clear();
    }

    /// Live terms starting with `prefix`, in string order.
    fn prefixed(&self, prefix: &str) -> Vec<(&str, u32)> {
        let name = |t: u32| &*self.names[t as usize];
        let lo = self.sorted.partition_point(|&t| name(t) < prefix);
        let mut out: Vec<(&str, u32)> = self.sorted[lo..]
            .iter()
            .map(|&t| (name(t), t))
            .take_while(|(w, _)| w.starts_with(prefix))
            .chain(
                self.tail
                    .iter()
                    .map(|&t| (name(t), t))
                    .filter(|(w, _)| w.starts_with(prefix)),
            )
            .collect();
        out.sort_unstable();
        out
    }

    /// The live terms whose stem is `stem`, in string order.
    fn class(&self, stem: &str) -> Vec<(&str, u32)> {
        let mut out = Vec::new();
        let mut t = self.head(stem).unwrap_or(NONE);
        while t != NONE {
            out.push((&*self.names[t as usize], t));
            t = self.next[t as usize];
        }
        out.sort_unstable();
        out
    }

    fn shrink(&mut self) {
        self.merge_tail();
        self.names.shrink_to_fit();
        let names = &self.names;
        self.index.shrink_to_fit(|&x| fnv(&names[x as usize]));
        self.sorted.shrink_to_fit();
        self.tail.shrink_to_fit();
        self.stems.shrink_to_fit(|&x| stem_hash(names, x));
        self.next.shrink_to_fit();
    }

    /// (the hash index and words, the order and stem classes): the second is s21's part. Tables count what they
    /// allocated: capacity() is items plus growth left, and a removal leaves a tombstone without returning growth,
    /// so it falls while the buckets stay (S186-1).
    fn heap_bytes(&self) -> (usize, usize) {
        let words = self.names.iter().map(|w| w.len()).sum::<usize>()
            + self.names.capacity() * size_of::<Box<str>>()
            + self.index.allocation_size();
        let s21 = (self.sorted.capacity() + self.tail.capacity() + self.next.capacity())
            * size_of::<u32>()
            + self.stems.allocation_size();
        (words, s21)
    }

    /// The order and the stem classes in a form that depends on no term number (the index image).
    fn image(&self, out: &mut String) {
        use core::fmt::Write;
        let name = |t: &u32| &*self.names[*t as usize];
        assert!(
            self.sorted.windows(2).all(|p| name(&p[0]) < name(&p[1])),
            "the order is sorted"
        );
        let mut all: Vec<&str> = self.sorted.iter().chain(&self.tail).map(name).collect();
        all.sort_unstable();
        let _ = writeln!(out, "order: {all:?}");
        let mut stems: Vec<Cow<'_, str>> = self
            .stems
            .iter()
            .map(|&h| crate::stem::stem(&self.names[h as usize]))
            .collect();
        stems.sort_unstable();
        for s in stems {
            let class: Vec<&str> = self.class(&s).into_iter().map(|(w, _)| w).collect();
            let _ = writeln!(out, "stem {s}: {class:?}");
        }
    }
}

/// A bigram's bucket in a [`TermGrams`] mask: a fixed function (Fibonacci hashing), so the masks are the same on
/// every run and machine. Any function keeps the bound sound; this one only decides how tight it is.
#[inline]
fn bucket(g: u64) -> usize {
    (g.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 56) as usize
}

/// The bucket masks of a bigram multiset (proof at [`WordIndex::title_terms`]): `m1`, the 256 buckets holding at
/// least one of its bigrams; `m2`, the 64 groups of four buckets (bucket / 4) with a bucket holding at least two.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Masks {
    m1: [u64; 4],
    m2: u64,
}

impl Masks {
    /// The masks of `grams`, and the slack e + d: e, the buckets holding two or more beyond one per `m2` group;
    /// d, the bigrams beyond two in each bucket.
    fn of(grams: &[u64]) -> (Masks, u32) {
        let mut count = [0u32; 256];
        for &g in grams {
            count[bucket(g)] += 1;
        }
        let (mut m, mut twos, mut d) = (Masks::default(), 0u32, 0u32);
        for (k, &c) in count.iter().enumerate() {
            if c >= 1 {
                m.m1[k >> 6] |= 1 << (k & 63);
            }
            if c >= 2 {
                m.m2 |= 1 << (k >> 2);
                twos += 1;
                d += c - 2;
            }
        }
        (m, twos - m.m2.count_ones() + d)
    }

    /// |M1_w & M1_Q| + |M2_w & M2_Q|.
    #[inline]
    fn shared(&self, q: &Masks) -> u32 {
        (0..4)
            .map(|i| (self.m1[i] & q.m1[i]).count_ones())
            .sum::<u32>()
            + (self.m2 & q.m2).count_ones()
    }
}

/// Each term's bigram figures by term number, made once when the word is interned (ws004 s18), so title find's
/// candidate step never computes or sorts a term's bigrams per query: the sorted bigrams of ` w `, their [`Masks`]
/// and slack. A free term number holds no bigrams and empty masks; reusing it overwrites all three.
#[derive(Clone, Debug, Default)]
struct TermGrams {
    masks: Vec<Masks>,
    slack: Vec<u32>,
    grams: Vec<Box<[u64]>>,
}

impl TermGrams {
    fn set(&mut self, t: u32, word: &str) {
        let mut g = Vec::new();
        crate::find::bigrams(word, &mut g);
        let (masks, slack) = Masks::of(&g);
        let t = t as usize;
        if t == self.grams.len() {
            self.masks.push(masks);
            self.slack.push(slack);
            self.grams.push(g.into_boxed_slice());
        } else {
            self.masks[t] = masks;
            self.slack[t] = slack;
            self.grams[t] = g.into_boxed_slice();
        }
    }

    fn clear(&mut self, t: u32) {
        let t = t as usize;
        self.masks[t] = Masks::default();
        self.slack[t] = 0;
        self.grams[t] = Box::default();
    }

    /// U(w) for term `t` against a query of `qlen` bigrams with masks `q`: at least the bigrams they share (proof at
    /// [`WordIndex::title_terms`]); 0 for a free term number.
    #[inline]
    fn upper(&self, t: usize, q: &Masks, qlen: u32) -> u32 {
        (self.masks[t].shared(q) + self.slack[t])
            .min(self.grams[t].len() as u32)
            .min(qlen)
    }

    /// U/n for term `t` against the query of `pass`, None when U is 0 (a free number, or no bucket shared).
    #[inline]
    fn ratio(&self, t: usize, pass: &TermPass<'_>) -> Option<f64> {
        let u = self.upper(t, &pass.qm, pass.q.len() as u32);
        (u > 0).then(|| u as f64 / self.grams[t].len() as f64)
    }

    fn shrink(&mut self) {
        self.masks.shrink_to_fit();
        self.slack.shrink_to_fit();
        self.grams.shrink_to_fit();
    }

    /// Heap bytes, and the bigrams held (for the bench's bytes per bigram).
    fn heap_bytes(&self) -> (usize, usize) {
        let held: usize = self.grams.iter().map(|g| g.len()).sum();
        let bytes = self.masks.capacity() * size_of::<Masks>()
            + self.slack.capacity() * size_of::<u32>()
            + self.grams.capacity() * size_of::<Box<[u64]>>()
            + held * size_of::<u64>();
        (bytes, held)
    }
}

/// Title words per group ([`TitleGroups`]).
const GROUP: usize = 3;
/// Title find walks groups from this cutoff up (a min of 2/3 or more: the near-duplicate check's 0.85); below it,
/// as at find's 0.4, a group's bound rarely rules its words out, so it walks every title word (ws003 s27 spike).
const GROUPED_CUTOFF: f64 = 0.5;
/// [`TitleGroups::at`] of a term held by no group or tail.
const UNHELD: u32 = u32::MAX;
/// [`TitleGroups::at`] of a term in the tail: this bit and its tail index.
const IN_TAIL: u32 = 1 << 31;
/// The most new title words the groups' tail holds, and places emptied, before every title word is grouped afresh:
/// [`TAIL`], lowered in tests so their indexes group afresh many times.
const GROUP_TAIL: usize = if cfg!(test) { 100 } else { TAIL };

/// Up to [`GROUP`] title words of one length n, with the OR of their [`Masks`] and their largest slack (ws003 s27).
/// Each member's U is at most the group's ([`Group::upper`]): its shared buckets are among the OR's and its slack at
/// most the largest. An emptied place holds [`NONE`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Group {
    masks: Masks,
    slack: u32,
    n: u32,
    members: [u32; GROUP],
}

impl Group {
    /// The group's U against a query of `qlen` bigrams with masks `q`: at least every member's.
    #[inline]
    fn upper(&self, q: &Masks, qlen: u32) -> u32 {
        (self.masks.shared(q) + self.slack).min(self.n).min(qlen)
    }

    fn live(&self) -> impl Iterator<Item = usize> + '_ {
        self.members
            .iter()
            .filter(|&&t| t != NONE)
            .map(|&t| t as usize)
    }

    /// Masks and slack from the members (empty when none is left).
    fn settle(&mut self, g: &TermGrams) {
        let (mut masks, mut slack) = (Masks::default(), 0);
        for t in self.live() {
            let m = &g.masks[t];
            for i in 0..4 {
                masks.m1[i] |= m.m1[i];
            }
            masks.m2 |= m.m2;
            slack = slack.max(g.slack[t]);
        }
        self.masks = masks;
        self.slack = slack;
    }
}

/// Lengths up to which [`Least`] holds its answer in a table (a 64-hex word has 65 bigrams).
const LEAST_N: usize = 80;

/// For each length n up to [`LEAST_N`], the least U for which `test(U, n)` holds (n + 1 when none), `test` rising in
/// U: the group walk's f64 tests made integer compares, the same answers (ws003 s27).
struct Least<F: Fn(u32, u32) -> bool> {
    at: [u32; LEAST_N + 1],
    test: F,
}

impl<F: Fn(u32, u32) -> bool> Least<F> {
    fn new(test: F) -> Least<F> {
        let mut at = [0; LEAST_N + 1];
        for (n, least) in at.iter_mut().enumerate().skip(1) {
            let n = n as u32;
            let (mut lo, mut hi) = (0, n + 1);
            while lo < hi {
                let mid = (lo + hi) / 2;
                if test(mid, n) {
                    hi = mid;
                } else {
                    lo = mid + 1;
                }
            }
            *least = lo;
        }
        Least { at, test }
    }

    /// `test(u, n)`.
    #[inline]
    fn holds(&self, u: u32, n: u32) -> bool {
        match self.at.get(n as usize) {
            Some(&least) if n > 0 => u >= least,
            _ => (self.test)(u, n),
        }
    }
}

/// The title words in groups (ws003 s27, graph-storage's design): runs of [`GROUP`] words of one length in the order
/// of their M1 masks, so a group's OR stays close to its members' masks and one test of its bound rules out all of
/// them. A word that becomes a title word waits in an unsorted tail of at most [`GROUP_TAIL`], looked at one by one; a
/// word that stops being one leaves its group, whose masks and slack are made again from the members left. When the
/// tail or the places emptied pass [`GROUP_TAIL`], every title word is grouped afresh, as a load's one build does. Which words
/// share a group depends on that history; the walk's answer does not.
#[derive(Clone, Debug, Default)]
struct TitleGroups {
    groups: Vec<Group>,
    /// Where each term number is: [`UNHELD`], [`IN_TAIL`] and its tail index, or group * [`GROUP`] + place.
    at: Vec<u32>,
    tail: Vec<u32>,
    /// Places emptied since the last grouping.
    holes: usize,
    /// A load is indexing: group once at its end.
    building: bool,
}

impl TitleGroups {
    fn holds(&self, t: u32) -> bool {
        self.at.get(t as usize).is_some_and(|&a| a != UNHELD)
    }

    /// Term `t` became a title word.
    fn add(&mut self, t: u32, g: &TermGrams) {
        let i = t as usize;
        if i >= self.at.len() {
            self.at.resize(i + 1, UNHELD);
        }
        debug_assert_eq!(self.at[i], UNHELD, "a title word is held once");
        self.at[i] = IN_TAIL | self.tail.len() as u32;
        self.tail.push(t);
        if !self.building && self.tail.len() > GROUP_TAIL {
            self.regroup(g);
        }
    }

    /// Term `t` is no longer a title word.
    fn remove(&mut self, t: u32, g: &TermGrams) {
        let a = core::mem::replace(&mut self.at[t as usize], UNHELD);
        debug_assert_ne!(a, UNHELD, "a title word is held");
        if a & IN_TAIL != 0 {
            let i = (a & !IN_TAIL) as usize;
            self.tail.swap_remove(i);
            if let Some(&moved) = self.tail.get(i) {
                self.at[moved as usize] = IN_TAIL | i as u32;
            }
            return;
        }
        let group = &mut self.groups[a as usize / GROUP];
        group.members[a as usize % GROUP] = NONE;
        group.settle(g);
        self.holes += 1;
        if !self.building && self.holes > GROUP_TAIL {
            self.regroup(g);
        }
    }

    /// Every title word into groups afresh: by (n, M1, term), [`GROUP`] at a time within one n.
    fn regroup(&mut self, g: &TermGrams) {
        // Keys made once, not per comparison: a key reads the term's masks and its bigrams' length.
        let key = |t: u32| (g.grams[t as usize].len() as u32, g.masks[t as usize].m1, t);
        let mut words: Vec<(u32, [u64; 4], u32)> = self
            .groups
            .iter()
            .flat_map(|gr| gr.members)
            .filter(|&t| t != NONE)
            .chain(self.tail.iter().copied())
            .map(key)
            .collect();
        words.sort_unstable();
        self.groups.clear();
        self.groups.reserve(words.len() / GROUP + 1);
        self.tail.clear();
        self.holes = 0;
        let mut k = 0;
        while k < words.len() {
            let n = words[k].0;
            let run = words[k..]
                .iter()
                .take(GROUP)
                .take_while(|w| w.0 == n)
                .count();
            let mut group = Group {
                n,
                members: [NONE; GROUP],
                ..Group::default()
            };
            for (place, &(_, m1, t)) in words[k..k + run].iter().enumerate() {
                group.members[place] = t;
                group.masks.m1.iter_mut().zip(m1).for_each(|(a, b)| *a |= b);
                group.masks.m2 |= g.masks[t as usize].m2;
                group.slack = group.slack.max(g.slack[t as usize]);
                self.at[t as usize] = (self.groups.len() * GROUP + place) as u32;
            }
            self.groups.push(group);
            k += run;
        }
    }

    fn begin_build(&mut self) {
        self.building = true;
    }

    fn finish_build(&mut self, g: &TermGrams) {
        self.building = false;
        if !self.tail.is_empty() || self.holes > 0 {
            self.regroup(g);
        }
    }

    fn shrink(&mut self) {
        self.groups.shrink_to_fit();
        self.at.shrink_to_fit();
        self.tail.shrink_to_fit();
    }

    fn heap_bytes(&self) -> usize {
        self.groups.capacity() * size_of::<Group>()
            + (self.at.capacity() + self.tail.capacity()) * size_of::<u32>()
    }

    /// Panics unless the groups are what their members make (graph-storage, s27): each title word (`titled`) held
    /// exactly once, by a group or the tail, and no place holding anything else; each group's n its members' length
    /// and its masks and slack equal to those its members give, not merely wider.
    fn check(&self, titled: &[u64], g: &TermGrams) {
        let is_titled = |t: usize| titled.get(t / 64).is_some_and(|&b| b >> (t % 64) & 1 == 1);
        for t in 0..self.at.len().max(titled.len() * 64) {
            let a = self.at.get(t).copied().unwrap_or(UNHELD);
            assert_eq!(
                is_titled(t),
                a != UNHELD,
                "term {t} is held iff it is a title word"
            );
            if a == UNHELD {
                continue;
            }
            let back = if a & IN_TAIL != 0 {
                self.tail.get((a & !IN_TAIL) as usize)
            } else {
                self.groups
                    .get(a as usize / GROUP)
                    .map(|gr| &gr.members[a as usize % GROUP])
            };
            assert_eq!(back, Some(&(t as u32)), "term {t}'s place holds it");
        }
        for (i, &t) in self.tail.iter().enumerate() {
            assert_eq!(
                self.at.get(t as usize),
                Some(&(IN_TAIL | i as u32)),
                "the tail's word {t} is held there"
            );
        }
        for (k, gr) in self.groups.iter().enumerate() {
            for (place, &t) in gr.members.iter().enumerate() {
                assert!(
                    t == NONE || self.at.get(t as usize) == Some(&((k * GROUP + place) as u32)),
                    "group {k}'s word {t} is held there"
                );
            }
            let mut made = *gr;
            made.settle(g);
            assert_eq!(made, *gr, "a group's masks and slack are its members'");
            assert!(
                gr.live().all(|t| g.grams[t].len() == gr.n as usize),
                "a group's members have its length"
            );
        }
    }
}

/// U/n below the cutoff is counted in 64 bins: bin b holds b/64 <= U/n < (b+1)/64 (x 64 is exact in f64).
const BINS: usize = 64;

fn bin(ru: f64) -> usize {
    ((ru * BINS as f64) as usize).min(BINS - 1)
}

/// Whether title find finds r_lo before testing the touched titles at r_lo = cutoff (see
/// [`WordIndex::title_candidates`]): when the words pass two would look at are under an eighth of the titles touched.
fn r_lo_first(pass: &TermPass<'_>, touched: usize) -> bool {
    (pass.counts.iter().map(|&c| c as usize).sum::<usize>() + pass.passed_over).saturating_mul(8)
        < touched
}

/// Title find's term step after pass one ([`WordIndex::title_walk`]).
pub(crate) struct TermPass<'q> {
    q: &'q [u64],
    qm: Masks,
    cutoff: f64,
    /// (term, c(w), n) of each word with r(w) >= cutoff, in term-number order.
    pub(crate) walk: Vec<(u32, u32, u32)>,
    /// The highest r below the cutoff among the words pass one merged.
    r_lo: f64,
    /// Words with 0 < U/n < cutoff, by bin, at index bin + 1.
    counts: [u32; BINS + 1],
    /// Each group's U, by group, when pass one walked groups ([`WordIndex::group_walk`]): pass two reads these
    /// rather than the groups' masks again.
    group_u: Vec<u32>,
    /// Places in groups pass one passed over ([`GROUP`] a group), counted with `counts` by [`r_lo_first`] only.
    passed_over: usize,
}

impl<'q> TermPass<'q> {
    fn new(q: &'q [u64], cutoff: f64) -> TermPass<'q> {
        TermPass {
            q,
            qm: Masks::of(q).0,
            cutoff,
            walk: Vec::new(),
            r_lo: 0.0,
            counts: [0; BINS + 1],
            group_u: Vec::new(),
            passed_over: 0,
        }
    }
}

pub const K1: f64 = 1.2;
pub const B: f64 = 0.75;
pub const TITLE: &str = "title";
pub const NOTE: &str = "note";

/// A field's weight: title above note above every other text field.
fn weight(field: &str) -> f64 {
    match field {
        TITLE => 3.0,
        NOTE => 1.5,
        _ => 1.0,
    }
}

/// `text`'s words in order, for index and query alike: lower case, split on anything that is not a letter or digit
/// (the rule title find uses). Numbers and mixed tokens (`iso27001`, `v2`) are words; nothing is stemmed or dropped.
pub fn words(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

/// [`words`] of text already in lower case, borrowed.
fn tokens(lower: &str) -> Vec<&str> {
    lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Counts in a posting saturate here (BM25 barely moves past it).
const MAX_COUNT: u32 = (1 << COUNT_BITS) - 1;
const COUNT_BITS: u32 = 10;
/// Field numbers below this pack with the count into a `u16`.
const NARROW_FIELDS: u32 = 1 << (16 - COUNT_BITS);

/// Field and count of each posting, `field << 10 | count`: `u16` while every field number fits (64 fields), widened
/// to `u32` for a term once one does not, the way a name column widens. Fields are never refused or merged.
#[derive(Clone, Debug)]
enum FieldCounts {
    Narrow(Vec<u16>),
    Wide(Vec<u32>),
}

impl FieldCounts {
    #[inline]
    fn get(&self, i: usize) -> (u32, u32) {
        let v = match self {
            FieldCounts::Narrow(v) => u32::from(v[i]),
            FieldCounts::Wide(v) => v[i],
        };
        (v >> COUNT_BITS, v & MAX_COUNT)
    }

    fn insert(&mut self, i: usize, field: u32, count: u32) {
        let v = field << COUNT_BITS | count.min(MAX_COUNT);
        if let FieldCounts::Narrow(n) = self
            && field >= NARROW_FIELDS
        {
            *self = FieldCounts::Wide(n.iter().map(|&x| u32::from(x)).collect());
        }
        match self {
            FieldCounts::Narrow(n) => n.insert(i, v as u16),
            FieldCounts::Wide(w) => w.insert(i, v),
        }
    }

    /// Sets the count of posting `i`, whose field stays `field`.
    fn set(&mut self, i: usize, field: u32, count: u32) {
        let v = field << COUNT_BITS | count.min(MAX_COUNT);
        match self {
            FieldCounts::Narrow(n) => n[i] = v as u16,
            FieldCounts::Wide(w) => w[i] = v,
        }
    }

    fn drain(&mut self, lo: usize, hi: usize) {
        match self {
            FieldCounts::Narrow(n) => drop(n.drain(lo..hi)),
            FieldCounts::Wide(w) => drop(w.drain(lo..hi)),
        }
    }

    fn shrink(&mut self) {
        match self {
            FieldCounts::Narrow(n) => n.shrink_to_fit(),
            FieldCounts::Wide(w) => w.shrink_to_fit(),
        }
    }

    fn heap_bytes(&self) -> usize {
        match self {
            FieldCounts::Narrow(n) => n.capacity() * size_of::<u16>(),
            FieldCounts::Wide(w) => w.capacity() * size_of::<u32>(),
        }
    }
}

/// One term's postings, sorted by (handle, field number), in two parallel arrays: 6 bytes a posting. Beside them,
/// kept by [`Postings::add`] and [`Postings::remove`] alone, so every write, rebuild and rollback keeps them (s21, C2
/// and C1): the nodes holding the term (its df, never recounted on a query) and the nodes holding it in their title.
#[derive(Clone, Debug)]
struct Postings {
    handles: Vec<u32>,
    fc: FieldCounts,
    nodes: u32,
    titles: u32,
}

impl Postings {
    fn new() -> Postings {
        Postings {
            handles: Vec::new(),
            fc: FieldCounts::Narrow(Vec::new()),
            nodes: 0,
            titles: 0,
        }
    }

    /// The postings of node `h`: `lo..hi`.
    #[inline]
    fn run(&self, h: u32) -> (usize, usize) {
        let lo = self.handles.partition_point(|&x| x < h);
        let hi = lo + self.handles[lo..].partition_point(|&x| x == h);
        (lo, hi)
    }

    /// Adds node `h`'s posting in `field` (at most one per node and field). True when it is the term's first title
    /// posting: the term has just become a title word.
    fn add(&mut self, h: u32, field: u32, count: u32) -> bool {
        // The load pass and appends: `h` is past every posting, so it goes last.
        if self.handles.last().is_none_or(|&x| x < h) {
            self.handles.push(h);
            let at = self.handles.len() - 1;
            self.fc.insert(at, field, count);
            self.nodes += 1;
        } else {
            let (lo, hi) = self.run(h);
            if lo == hi {
                self.nodes += 1;
            }
            let at = lo + (lo..hi).take_while(|&i| self.fc.get(i).0 < field).count();
            self.handles.insert(at, h);
            self.fc.insert(at, field, count);
        }
        if field != 0 {
            return false;
        }
        self.titles += 1;
        self.titles == 1
    }

    /// Removes node `h`'s postings. True when they held the term's last title posting: the term is no longer a title
    /// word.
    fn remove(&mut self, h: u32) -> bool {
        let (lo, hi) = self.run(h);
        if lo == hi {
            return false;
        }
        self.nodes -= 1;
        // A node's postings sort by field, so only its first can be the title (field 0).
        let title = self.fc.get(lo).0 == 0;
        if title {
            self.titles -= 1;
        }
        self.handles.drain(lo..hi);
        self.fc.drain(lo, hi);
        // Writes never leave a list holding far more than it needs.
        if self.handles.capacity() > 8 && self.handles.capacity() > 4 * self.handles.len() {
            self.handles.shrink_to_fit();
            self.fc.shrink();
        }
        title && self.titles == 0
    }

    /// Sets node `h`'s posting in `field` to `count`: in place when it has one, added when it has none, removed when
    /// `count` is 0. Keeps `nodes` and `titles` as [`Postings::add`] and [`Postings::remove`] do.
    fn set_field(&mut self, h: u32, field: u32, count: u32) {
        let (lo, hi) = self.run(h);
        let at = lo + (lo..hi).take_while(|&i| self.fc.get(i).0 < field).count();
        let held = at < hi && self.fc.get(at).0 == field;
        match (held, count) {
            (true, 0) => {
                self.handles.remove(at);
                self.fc.drain(at, at + 1);
                if hi - lo == 1 {
                    self.nodes -= 1;
                }
                if field == 0 {
                    self.titles -= 1;
                }
                if self.handles.capacity() > 8 && self.handles.capacity() > 4 * self.handles.len() {
                    self.handles.shrink_to_fit();
                    self.fc.shrink();
                }
            }
            (true, _) => self.fc.set(at, field, count),
            (false, 0) => {}
            (false, _) => {
                self.handles.insert(at, h);
                self.fc.insert(at, field, count);
                if lo == hi {
                    self.nodes += 1;
                }
                if field == 0 {
                    self.titles += 1;
                }
            }
        }
    }
}

/// A query word of at least this many characters also matches the terms that start with it (req-92,
/// embeddings-expert).
pub const PREFIX_MIN: usize = 4;
/// A prefix matches at most this many terms: the most common first (summed df, then string order). A capped prefix
/// is reported.
pub const PREFIX_CAP: usize = 64;
/// A stem match's weight against an exact match's 1 (embeddings-expert, s21).
pub const STEM_WEIGHT: f64 = 0.7;
/// A prefix match's weight, also its share of a word's coverage in hybrid search.
pub const PREFIX_WEIGHT: f64 = 0.4;

/// How a query word met a node: the word itself, another word with its Porter2 stem, or a word starting with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Form {
    Exact,
    Stem,
    Prefix,
}

impl Form {
    const ALL: [Form; 3] = [Form::Exact, Form::Stem, Form::Prefix];

    pub fn as_str(self) -> &'static str {
        match self {
            Form::Exact => "exact",
            Form::Stem => "stem",
            Form::Prefix => "prefix",
        }
    }

    pub fn weight(self) -> f64 {
        match self {
            Form::Exact => 1.0,
            Form::Stem => STEM_WEIGHT,
            Form::Prefix => PREFIX_WEIGHT,
        }
    }

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// A term a query word may match, over the graphs searched: nodes holding it, and whether it shares the word's stem
/// or starts with it (the word itself is the exact match).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Variant {
    pub df: u64,
    pub stem: bool,
    pub prefix: bool,
}

/// The figures BM25F needs from the graphs searched, summed across them before any scoring.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WordStats {
    /// Nodes in the graphs searched.
    pub nodes: u64,
    /// For each query term, in the query's order: nodes holding it.
    pub df: Vec<u64>,
    /// By field name: (nodes with the field non-empty, its words in all of them).
    pub fields: BTreeMap<String, (u64, u64)>,
    /// For each query term, in the query's order: the terms it may match (itself included), by word. Every graph
    /// scores the expansion these sums decide, so load order changes nothing.
    pub variants: Vec<BTreeMap<String, Variant>>,
}

/// One query word's matches as scored: its terms in string order, each with the tiers it is in, and each tier's
/// idf (its df is the largest of its members', as Lucene's SynonymQuery does).
#[derive(Clone, Debug)]
pub(crate) struct TermPlan<'s> {
    members: Vec<(&'s str, u8)>,
    idf: [f64; 3],
}

impl TermPlan<'_> {
    /// The word's score from each tier's tf' (0 where the tier missed): the best weighted tier, and that tier.
    fn score(&self, tf: &[f64; 3]) -> (f64, Form) {
        let mut best = (0.0, Form::Exact);
        for f in Form::ALL {
            let x = tf[f as usize];
            if x > 0.0 {
                let s = f.weight() * self.idf[f as usize] * x / (K1 + x);
                if s > best.0 {
                    best = (s, f);
                }
            }
        }
        best
    }
}

impl WordStats {
    /// Adds another graph's figures for the same query.
    pub fn add(&mut self, other: &WordStats) {
        self.nodes += other.nodes;
        if self.df.len() < other.df.len() {
            self.df.resize(other.df.len(), 0);
        }
        for (d, o) in self.df.iter_mut().zip(&other.df) {
            *d += o;
        }
        for (name, (docs, total)) in &other.fields {
            let e = self.fields.entry(name.clone()).or_default();
            e.0 += docs;
            e.1 += total;
        }
        if self.variants.len() < other.variants.len() {
            self.variants.resize(other.variants.len(), BTreeMap::new());
        }
        for (mine, theirs) in self.variants.iter_mut().zip(&other.variants) {
            for (word, v) in theirs {
                let e = mine.entry(word.clone()).or_default();
                e.df += v.df;
                e.stem |= v.stem;
                e.prefix |= v.prefix;
            }
        }
    }

    /// ln(1 + (N - df + 0.5) / (df + 0.5)): never negative, so a word in every node still counts a little.
    fn idf_of(&self, df: u64) -> f64 {
        let (n, df) = (self.nodes as f64, df as f64);
        libm::log(1.0 + (n - df + 0.5) / (df + 0.5))
    }

    /// The terms query term `i` (`word`) starts, the most common first, capped at [`PREFIX_CAP`]; and how many there
    /// were.
    fn prefix_kept(&self, i: usize) -> (Vec<(&str, u64)>, usize) {
        let mut all: Vec<(&str, u64)> = self
            .variants
            .get(i)
            .into_iter()
            .flatten()
            .filter(|(_, v)| v.prefix)
            .map(|(w, v)| (w.as_str(), v.df))
            .collect();
        let total = all.len();
        all.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        all.truncate(PREFIX_CAP);
        (all, total)
    }

    /// Each query word's plan: its tiers' members and idfs.
    pub(crate) fn plans<'s>(&'s self, q: &'s WordQuery) -> Vec<TermPlan<'s>> {
        q.terms
            .iter()
            .enumerate()
            .map(|(i, word)| {
                let (kept, _) = self.prefix_kept(i);
                let mut members: Vec<(&str, u8)> = Vec::new();
                let mut df = [0u64; 3];
                for (w, v) in self.variants.get(i).into_iter().flatten() {
                    let mut tiers = 0u8;
                    if w == word {
                        tiers |= Form::Exact.bit();
                        df[0] = df[0].max(v.df);
                    }
                    if v.stem {
                        tiers |= Form::Stem.bit();
                        df[1] = df[1].max(v.df);
                    }
                    if v.prefix && kept.iter().any(|k| k.0 == w) {
                        tiers |= Form::Prefix.bit();
                        df[2] = df[2].max(v.df);
                    }
                    if tiers != 0 {
                        members.push((w.as_str(), tiers));
                    }
                }
                TermPlan {
                    members,
                    idf: df.map(|d| self.idf_of(d)),
                }
            })
            .collect()
    }

    /// The df hybrid search's informative-term test uses for query term `i` (embeddings-expert, s21): the sum of
    /// its exact and stem matches' dfs, at most N, an upper bound on the nodes holding any of them. Prefix matches
    /// never count.
    pub fn informative_df(&self, i: usize) -> u64 {
        // A stemmed word that is a term is in its own class; an unstemmed one has no class and counts alone.
        let class: Vec<u64> = self
            .variants
            .get(i)
            .into_iter()
            .flatten()
            .filter(|(_, v)| v.stem)
            .map(|(_, v)| v.df)
            .collect();
        let sum = if class.is_empty() {
            self.df.get(i).copied().unwrap_or(0)
        } else {
            class.iter().sum()
        };
        sum.min(self.nodes)
    }

    /// Query words whose prefix matched more than [`PREFIX_CAP`] terms: (word, how many).
    pub fn capped<'q>(&self, q: &'q WordQuery) -> Vec<(&'q str, usize)> {
        q.terms
            .iter()
            .enumerate()
            .filter_map(|(i, w)| {
                let (_, total) = self.prefix_kept(i);
                (total > PREFIX_CAP).then_some((w.as_str(), total))
            })
            .collect()
    }

    fn avg_len(&self, field: &str) -> f64 {
        match self.fields.get(field) {
            Some(&(docs, total)) if docs > 0 && total > 0 => total as f64 / docs as f64,
            _ => 1.0,
        }
    }
}

/// A word query: its distinct words, in string order (the order every score is summed in).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WordQuery {
    terms: Vec<String>,
}

impl WordQuery {
    /// None if `text` has no words.
    pub fn new(text: &str) -> Option<WordQuery> {
        let mut terms = words(text);
        terms.sort_unstable();
        terms.dedup();
        (!terms.is_empty()).then_some(WordQuery { terms })
    }

    pub fn terms(&self) -> &[String] {
        &self.terms
    }
}

/// One node's texts, by field name: title, note, then the indexed custom fields.
pub(crate) type Texts<'a> = Vec<(Cow<'a, str>, Cow<'a, str>)>;

/// The word index of one graph.
#[derive(Clone, Debug)]
pub(crate) struct WordIndex {
    terms: Terms,
    grams: TermGrams,
    postings: Vec<Postings>,
    /// Term numbers that are title words (some title posting), one bit each (s21, C1): title find's term step walks
    /// only these. Set and cleared as [`Postings::add`] and [`Postings::remove`] say.
    titled: Vec<u64>,
    /// The title words in groups, for the near-duplicate check's walk (ws003 s27); kept by [`WordIndex::set_titled`].
    groups: TitleGroups,
    /// Term numbers whose last posting a write removed: out of the term table, reused by the next new word, so the
    /// index holds the graph's words, not its edit history.
    free: Vec<u32>,
    /// Field names by number: 0 title, 1 note, then custom text fields in first-use order.
    fields: Vec<Box<str>>,
    field_no: BTreeMap<Box<str>, u32>,
    /// By field number: (nodes with the field non-empty, words in all of them).
    totals: Vec<(u64, u64)>,
    title_len: Vec<u16>,
    /// |T| of each node's title: the bigrams of ` normalised title `, which is its normalised characters plus one
    /// (title find's bound, ws004 s15), saturating.
    title_grams: Vec<u16>,
    note_len: Vec<u16>,
    /// Custom text field lengths, (field, words); node h's run ends at `other_end[h]`.
    other: Vec<(u32, u16)>,
    other_end: Vec<u32>,
    /// The row an edit is replacing, and its (field, words) and (field, text) as indexed: noted by
    /// [`WordIndex::stash`] before its line changes, diffed by [`WordIndex::reindex`] (s26), with `was` and `now` the
    /// changed fields' (term, field, count)s. Scratch for one row, reused: it keeps its largest row's size, counted in
    /// `heap_bytes`.
    was_row: Option<u32>,
    was_lens: Vec<(u32, u64)>,
    was_texts: Vec<(u32, String)>,
    was: Vec<(u32, u32, u32)>,
    now: Vec<(u32, u32, u32)>,
}

/// One row's lengths as its texts give them.
#[derive(Default)]
struct Lens {
    title: u16,
    grams: u16,
    note: u16,
    other: Vec<(u32, u16)>,
}

/// Sorts (term, field, 1)s and sums the repeats into one count each.
fn counted(v: &mut Vec<(u32, u32, u32)>) {
    v.sort_unstable();
    v.dedup_by(|a, b| {
        let same = (a.0, a.1) == (b.0, b.1);
        if same {
            b.2 += a.2;
        }
        same
    });
}

impl Default for WordIndex {
    fn default() -> WordIndex {
        WordIndex::new()
    }
}

impl WordIndex {
    pub(crate) fn new() -> WordIndex {
        let mut w = WordIndex {
            terms: Terms::default(),
            grams: TermGrams::default(),
            postings: Vec::new(),
            titled: Vec::new(),
            groups: TitleGroups::default(),
            free: Vec::new(),
            fields: Vec::new(),
            field_no: BTreeMap::new(),
            totals: Vec::new(),
            title_len: Vec::new(),
            title_grams: Vec::new(),
            note_len: Vec::new(),
            other: Vec::new(),
            other_end: Vec::new(),
            was_row: None,
            was_lens: Vec::new(),
            was_texts: Vec::new(),
            was: Vec::new(),
            now: Vec::new(),
        };
        w.field(TITLE);
        w.field(NOTE);
        w
    }

    pub(crate) fn rows(&self) -> usize {
        self.title_len.len()
    }

    fn field(&mut self, name: &str) -> u32 {
        if let Some(&f) = self.field_no.get(name) {
            return f;
        }
        let f = self.fields.len() as u32;
        self.fields.push(name.into());
        self.field_no.insert(name.into(), f);
        self.totals.push((0, 0));
        f
    }

    fn term(&mut self, word: &str) -> u32 {
        if let Some(t) = self.terms.get(word) {
            return t;
        }
        let t = match self.free.pop() {
            Some(t) => t,
            None => {
                self.postings.push(Postings::new());
                if self.postings.len() > self.titled.len() * 64 {
                    self.titled.push(0);
                }
                self.postings.len() as u32 - 1
            }
        };
        self.terms.insert(word, t);
        self.grams.set(t, word);
        t
    }

    /// Marks term `t` a title word or not, and keeps the groups ([`TitleGroups`]) in step: every write, rebuild and
    /// rollback reaches the title bits only here.
    fn set_titled(&mut self, t: u32, on: bool) {
        let (word, bit) = (t as usize / 64, 1u64 << (t % 64));
        let was = self.titled[word] & bit != 0;
        if on {
            self.titled[word] |= bit;
        } else {
            self.titled[word] &= !bit;
        }
        match (was, on) {
            (false, true) => self.groups.add(t, &self.grams),
            (true, false) => self.groups.remove(t, &self.grams),
            _ => {}
        }
    }

    /// A load is about to index every row: the groups are made once, at [`WordIndex::finish_build`].
    pub(crate) fn begin_build(&mut self) {
        self.groups.begin_build();
    }

    pub(crate) fn finish_build(&mut self) {
        self.groups.finish_build(&self.grams);
    }

    /// Term `t` leaves the term table: its number is reused next (C3, graph-storage: no group or tail still holds
    /// it, so it cannot be walked under its next word's figures by mistake).
    fn free_term(&mut self, t: u32, word: &str) {
        debug_assert!(!self.groups.holds(t), "a freed term is in no group");
        self.postings[t as usize] = Postings::new();
        self.terms.remove(word, t);
        self.grams.clear(t);
        self.free.push(t);
    }

    /// The title words' term numbers, ascending.
    fn titled(&self) -> impl Iterator<Item = usize> + '_ {
        self.titled.iter().enumerate().flat_map(|(i, &bits)| {
            let mut bits = bits;
            core::iter::from_fn(move || {
                (bits != 0).then(|| {
                    let b = bits.trailing_zeros() as usize;
                    bits &= bits - 1;
                    i * 64 + b
                })
            })
        })
    }

    pub(crate) fn reserve_exact(&mut self, rows: usize) {
        self.title_len.reserve_exact(rows);
        self.title_grams.reserve_exact(rows);
        self.note_len.reserve_exact(rows);
        self.other_end.reserve_exact(rows);
    }

    /// Field `name` holds the words `toks`: counts them into the field's totals and the row's lengths, as indexing the
    /// row does.
    fn count_field(&mut self, name: &str, toks: &[&str], lens: &mut Lens) -> u32 {
        let f = self.field(name);
        let len = toks.len();
        self.totals[f as usize].0 += 1;
        self.totals[f as usize].1 += len as u64;
        let len = len.min(usize::from(u16::MAX)) as u16;
        match f {
            0 => {
                lens.title = len;
                let chars: usize = toks.iter().map(|w| w.chars().count() + 1).sum();
                lens.grams = chars.min(usize::from(u16::MAX)) as u16;
            }
            1 => lens.note = len,
            _ => lens.other.push((f, len)),
        }
        f
    }

    /// Sets row `h`'s lengths (pushes them for a new last row).
    fn set_lens(&mut self, h: u32, lens: Lens) {
        let h = h as usize;
        if h == self.rows() {
            self.title_len.push(lens.title);
            self.title_grams.push(lens.grams);
            self.note_len.push(lens.note);
            self.other.extend(lens.other);
            self.other_end.push(self.other.len() as u32);
        } else {
            self.title_len[h] = lens.title;
            self.title_grams[h] = lens.grams;
            self.note_len[h] = lens.note;
            self.splice_other(h, lens.other);
        }
    }

    /// Indexes node `h`'s texts: `h` is a new last row, or one `unindex` has just emptied.
    pub(crate) fn index(&mut self, h: u32, texts: &Texts<'_>) {
        debug_assert!(self.was_row.is_none(), "an edited row is reindexed");
        let mut lens = Lens::default();
        for (name, text) in texts {
            let lower = text.to_lowercase();
            let mut toks = tokens(&lower);
            if toks.is_empty() {
                continue;
            }
            let f = self.count_field(name, &toks, &mut lens);
            let mut ids: Vec<u32> = toks.drain(..).map(|w| self.term(w)).collect();
            ids.sort_unstable();
            let mut i = 0;
            while i < ids.len() {
                let run = ids[i..].iter().take_while(|&&t| t == ids[i]).count();
                if self.postings[ids[i] as usize].add(h, f, run as u32) {
                    self.set_titled(ids[i], true);
                }
                i += run;
            }
        }
        self.set_lens(h, lens);
    }

    /// Notes node `h`'s texts as indexed (its line before an edit), for the [`WordIndex::reindex`] that follows once
    /// the line has changed. Changes nothing in the index.
    pub(crate) fn stash(&mut self, h: u32, texts: &Texts<'_>) {
        debug_assert!(self.was_row.is_none(), "one edit at a time");
        self.was_lens.clear();
        self.was_texts.clear();
        for (name, text) in texts {
            let Some(&f) = self.field_no.get(&**name) else {
                continue;
            };
            let lower = text.to_lowercase();
            let len = lower
                .split(|c: char| !c.is_alphanumeric())
                .filter(|w| !w.is_empty())
                .count();
            if len > 0 {
                self.was_lens.push((f, len as u64));
                self.was_texts.push((f, String::from(&**text)));
            }
        }
        self.was_row = Some(h);
    }

    /// Indexes node `h`'s new texts against the ones [`WordIndex::stash`] noted (s26, review C4). A field whose text
    /// is unchanged is not looked at past its length. In a changed field, a posting is added, removed or has its count
    /// set only where the field's (term, count)s differ, and a term with no posting left is freed. The same index as
    /// `unindex` with the old texts then `index` with the new: postings, node and title counts, title bits, freed
    /// terms, totals and lengths.
    pub(crate) fn reindex(&mut self, h: u32, texts: &Texts<'_>) {
        assert_eq!(
            self.was_row.take(),
            Some(h),
            "stash the row's words before its line changes"
        );
        for i in 0..self.was_lens.len() {
            let (f, len) = self.was_lens[i];
            self.totals[f as usize].0 -= 1;
            self.totals[f as usize].1 -= len;
        }
        let was_texts = core::mem::take(&mut self.was_texts);
        let (mut was, mut now) = (
            core::mem::take(&mut self.was),
            core::mem::take(&mut self.now),
        );
        was.clear();
        now.clear();
        // Words no term holds yet: numbered once the row's lost terms are freed, as `index` after `unindex` would.
        let mut fresh: Vec<(Box<str>, u32)> = Vec::new();
        let mut same: Vec<u32> = Vec::new();
        let mut lens = Lens::default();
        for (name, text) in texts {
            let lower = text.to_lowercase();
            let toks = tokens(&lower);
            if toks.is_empty() {
                continue;
            }
            let f = self.count_field(name, &toks, &mut lens);
            if was_texts.iter().any(|(g, old)| *g == f && **old == **text) {
                same.push(f);
                continue;
            }
            for w in toks {
                match self.terms.get(w) {
                    Some(t) => now.push((t, f, 1)),
                    None => fresh.push((w.into(), f)),
                }
            }
        }
        for (f, old) in was_texts.iter().filter(|(f, _)| !same.contains(f)) {
            let lower = old.to_lowercase();
            was.extend(tokens(&lower).into_iter().filter_map(|w| {
                let t = self.terms.get(w);
                debug_assert!(t.is_some(), "a word the row was indexed with has a term");
                Some((t?, *f, 1))
            }));
        }
        counted(&mut was);
        counted(&mut now);
        let (mut i, mut j) = (0, 0);
        while i < was.len() || j < now.len() {
            let t = match (was.get(i), now.get(j)) {
                (Some(a), Some(b)) => a.0.min(b.0),
                (Some(a), None) => a.0,
                (None, Some(b)) => b.0,
                (None, None) => unreachable!(),
            };
            let mut touched = false;
            loop {
                let a = was.get(i).filter(|x| x.0 == t);
                let b = now.get(j).filter(|x| x.0 == t);
                let (f, old, new) = match (a, b) {
                    (None, None) => break,
                    (Some(a), Some(b)) if a.1 == b.1 => (a.1, a.2, b.2),
                    (Some(a), Some(b)) if a.1 < b.1 => (a.1, a.2, 0),
                    (Some(a), None) => (a.1, a.2, 0),
                    (_, Some(b)) => (b.1, 0, b.2),
                };
                if a.is_some_and(|a| a.1 == f) {
                    i += 1;
                }
                if b.is_some_and(|b| b.1 == f) {
                    j += 1;
                }
                if old.min(MAX_COUNT) != new.min(MAX_COUNT) {
                    self.postings[t as usize].set_field(h, f, new);
                    touched = true;
                }
            }
            if touched {
                self.settle_term(t);
            }
        }
        self.was = was;
        self.now = now;
        self.was_texts = was_texts;
        if !fresh.is_empty() {
            let mut ids: Vec<(u32, u32, u32)> =
                fresh.iter().map(|(w, f)| (self.term(w), *f, 1)).collect();
            counted(&mut ids);
            for (t, f, c) in ids {
                self.postings[t as usize].set_field(h, f, c);
                self.settle_term(t);
            }
        }
        self.set_lens(h, lens);
    }

    /// Term `t`'s title bit after its postings changed, and the term freed when none is left.
    fn settle_term(&mut self, t: u32) {
        let p = &self.postings[t as usize];
        let (titled, empty) = (p.titles > 0, p.handles.is_empty());
        self.set_titled(t, titled);
        if empty {
            let word = self.terms.names[t as usize].clone();
            self.free_term(t, &word);
        }
    }

    /// Removes node `h`'s words; `texts` are the texts it was indexed with. Its row stays, empty.
    pub(crate) fn unindex(&mut self, h: u32, texts: &Texts<'_>) {
        let mut seen = BTreeSet::new();
        for (_, text) in texts {
            for w in words(text) {
                if let Some(t) = self.terms.get(w.as_str())
                    && seen.insert(t)
                {
                    let p = &mut self.postings[t as usize];
                    let untitled = p.remove(h);
                    let empty = p.handles.is_empty();
                    if untitled {
                        self.set_titled(t, false);
                    }
                    if empty {
                        self.free_term(t, w.as_str());
                    }
                }
            }
        }
        let hu = h as usize;
        for (f, len) in [(0, self.title_len[hu]), (1, self.note_len[hu])]
            .into_iter()
            .chain(self.other_run(hu).iter().map(|&(f, l)| (f, l)))
            .collect::<Vec<_>>()
        {
            if len > 0 {
                self.totals[f as usize].0 -= 1;
                self.totals[f as usize].1 -= self.true_len(texts, f);
            }
        }
        self.title_len[hu] = 0;
        self.title_grams[hu] = 0;
        self.note_len[hu] = 0;
        self.splice_other(hu, Vec::new());
    }

    /// A field's word count as indexed, before the `u16` cap.
    fn true_len(&self, texts: &Texts<'_>, f: u32) -> u64 {
        let name = &*self.fields[f as usize];
        texts
            .iter()
            .find(|(n, _)| **n == *name)
            .map_or(0, |(_, t)| words(t).len() as u64)
    }

    /// Removes the last row; `unindex` it first.
    pub(crate) fn pop(&mut self) {
        self.title_len.pop();
        self.title_grams.pop();
        self.note_len.pop();
        self.other_end.pop();
        self.other
            .truncate(self.other_end.last().copied().unwrap_or(0) as usize);
    }

    fn other_run(&self, h: usize) -> &[(u32, u16)] {
        let start = if h == 0 {
            0
        } else {
            self.other_end[h - 1] as usize
        };
        &self.other[start..self.other_end[h] as usize]
    }

    fn splice_other(&mut self, h: usize, run: Vec<(u32, u16)>) {
        let start = if h == 0 {
            0
        } else {
            self.other_end[h - 1] as usize
        };
        let end = self.other_end[h] as usize;
        let delta = run.len() as i64 - (end - start) as i64;
        self.other.splice(start..end, run);
        if delta != 0 {
            for e in &mut self.other_end[h..] {
                *e = (i64::from(*e) + delta) as u32;
            }
        }
    }

    fn len_of(&self, h: u32, f: u32) -> u16 {
        match f {
            0 => self.title_len[h as usize],
            1 => self.note_len[h as usize],
            _ => self
                .other_run(h as usize)
                .iter()
                .find(|(x, _)| *x == f)
                .map_or(0, |&(_, l)| l),
        }
    }

    pub(crate) fn shrink(&mut self) {
        self.terms.shrink();
        self.postings.shrink_to_fit();
        self.grams.shrink();
        self.free.shrink_to_fit();
        self.titled.shrink_to_fit();
        for p in &mut self.postings {
            p.handles.shrink_to_fit();
            p.fc.shrink();
        }
        self.title_len.shrink_to_fit();
        self.title_grams.shrink_to_fit();
        self.note_len.shrink_to_fit();
        self.other.shrink_to_fit();
        self.other_end.shrink_to_fit();
        self.groups.shrink();
    }

    /// The title-word groups' heap bytes ([`TitleGroups`]), part of [`WordIndex::heap_bytes`]: what a batch and the
    /// same writes one by one may hold differently (their groupings follow their own histories).
    #[cfg(test)]
    pub(crate) fn group_bytes(&self) -> usize {
        self.groups.heap_bytes()
    }

    /// How many title-word groups there are (a test sees a grouping afresh by it).
    #[cfg(test)]
    pub(crate) fn group_count(&self) -> usize {
        self.groups.groups.len()
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        let (words, s21) = self.terms.heap_bytes();
        let terms = words + s21;
        let postings = self.postings.capacity() * size_of::<Postings>()
            + self
                .postings
                .iter()
                .map(|p| p.handles.capacity() * size_of::<u32>() + p.fc.heap_bytes())
                .sum::<usize>();
        let fields = self.fields.iter().map(|f| f.len() * 2).sum::<usize>()
            + crate::cols::btree_bytes::<Box<str>, u32>(self.field_no.len())
            + (self.fields.capacity() + self.totals.capacity()) * size_of::<(u64, u64)>();
        terms
            + self.grams.heap_bytes().0
            + postings
            + fields
            + (self.title_len.capacity() + self.title_grams.capacity() + self.note_len.capacity())
                * size_of::<u16>()
            + self.other.capacity() * size_of::<(u32, u16)>()
            + (self.other_end.capacity() + self.free.capacity()) * size_of::<u32>()
            + self.titled.capacity() * size_of::<u64>()
            + self.was_lens.capacity() * size_of::<(u32, u64)>()
            + self.was_texts.capacity() * size_of::<(u32, String)>()
            + self
                .was_texts
                .iter()
                .map(|(_, s)| s.capacity())
                .sum::<usize>()
            + (self.was.capacity() + self.now.capacity()) * size_of::<(u32, u32, u32)>()
            + self.groups.heap_bytes()
    }

    /// The index in a form that depends on no term or field number: terms in string order with their kept node and
    /// title counts and their postings as (node id, field name, count), the title words, each node's field lengths,
    /// and each field's totals. Two indexes of the same graph give the same image however they were built, so a count
    /// or title bit a write or rollback left stale differs from a rebuild's. The title-word groups depend on their
    /// history, so the image leaves them out and asserts what must hold of them instead ([`TitleGroups::check`]).
    pub(crate) fn image(&self, ids: &[umx_core::Id]) -> String {
        use core::fmt::Write;
        self.groups.check(&self.titled, &self.grams);
        let mut out = String::new();
        let mut terms: Vec<(&str, u32)> = self.terms.iter().collect();
        terms.sort_unstable();
        for (term, t) in terms {
            let p = &self.postings[t as usize];
            if p.handles.is_empty() {
                continue;
            }
            let mut row: Vec<(umx_core::Id, &str, u32)> = (0..p.handles.len())
                .map(|i| {
                    let (f, c) = p.fc.get(i);
                    (ids[p.handles[i] as usize], &*self.fields[f as usize], c)
                })
                .collect();
            row.sort();
            let _ = writeln!(
                out,
                "{term} ({} nodes, {} titles): {row:?}",
                p.nodes, p.titles
            );
        }
        let mut titled: Vec<&str> = self.titled().map(|t| &*self.terms.names[t]).collect();
        titled.sort_unstable();
        let _ = writeln!(out, "titled: {titled:?}");
        for (h, id) in ids.iter().enumerate() {
            let mut lens: Vec<(&str, u16)> = (0..self.fields.len() as u32)
                .map(|f| (&*self.fields[f as usize], self.len_of(h as u32, f)))
                .filter(|x| x.1 > 0)
                .collect();
            lens.sort();
            let _ = writeln!(out, "{id:?}: {lens:?}");
        }
        let mut totals: Vec<(&str, (u64, u64))> = self
            .fields
            .iter()
            .zip(&self.totals)
            .filter(|(_, t)| t.0 > 0)
            .map(|(f, t)| (&**f, *t))
            .collect();
        totals.sort();
        let _ = writeln!(out, "{totals:?}");
        self.terms.image(&mut out);
        out
    }

    /// Handles, ascending, of every node whose title could score `min` or more against the query whose sorted
    /// bigrams are `q` (ws004 s15, the graph-storage-expert's bound): complete by construction, so the caller, scoring
    /// these with s06's scorer, gives s06's answer.
    ///
    /// A title's bigrams are its words' ` w ` bigrams together, `|T|` of them (the normalised title's characters plus
    /// one), so the bigrams it shares with the query are at most the sum over its words of (count x c(w)), c(w) =
    /// |bigrams(" w ") ∩ q|; Dice is at most 2 x that / (|Q| + |T|). Write r(w) = c(w) / |bigrams(" w ")|. A title scoring
    /// m or more holds at least one word with r(w) >= m / (2 - m): with every r below some r', the shared
    /// bigrams are at most min(|Q|, r' x |T|), and Dice at most 2r' / (1 + r'). So only the postings of those words
    /// (title postings, field 0) are walked; each other word counts at most r_lo x its share of |T|, r_lo the highest
    /// r among them. When every word with c(w) > 0 is walked, this is exactly the bound above. A node is kept when
    /// 2 x bound >= m x (|Q| + |T|), with m a hair below `min` so a score that rounds up to `min` is never cut.
    ///
    /// Only title words are looked at (s21, review C1; agreed with the graph-storage-expert): a word in no title
    /// has no title posting to walk, and every word of a title that is not walked is itself a title word, so r_lo
    /// over title words alone still bounds it. That r_lo is at most s15's (over every word), so the candidates are
    /// s15's or fewer and every title that can reach `min` is still among them: the same hits, scores and order.
    ///
    /// The words to walk come from [`WordIndex::title_walk`] and r_lo from [`WordIndex::title_r_lo`], s15's set and
    /// r_lo over the title words without merging every term's bigrams with the query (ws004 s18). The keep test above
    /// only grows with r_lo, and r_lo < cutoff, so a node it drops at r_lo = cutoff is dropped at any r_lo: r_lo is
    /// found only when a node survives that test (a title with a common word and a unique token, say, rarely does),
    /// or at once when that is cheaper ([`r_lo_first`]).
    pub(crate) fn title_candidates(&self, q: &[u64], min: f64) -> Vec<u32> {
        self.candidates(q, min, true)
    }

    /// [`WordIndex::title_candidates`] by today's walk of every title word at any threshold, never by group: the
    /// reference the grouped walk is checked against (tests).
    #[cfg(test)]
    pub(crate) fn title_candidates_ungrouped(&self, q: &[u64], min: f64) -> Vec<u32> {
        self.candidates(q, min, false)
    }

    /// When `grouped` and [`WordIndex::takes_groups`], both passes take the title words by group ([`TitleGroups`],
    /// [`WordIndex::group_walk`]); the walk and r_lo, and so the candidates, are the same either way.
    fn candidates(&self, q: &[u64], min: f64, grouped: bool) -> Vec<u32> {
        let m = min - 0.001;
        let cutoff = m / (2.0 - m);
        let grouped = grouped && Self::takes_groups(min);
        let pass = if grouped {
            self.group_walk(q, cutoff)
        } else {
            self.title_walk(q, cutoff)
        };
        if pass.walk.is_empty() {
            return Vec::new();
        }
        let (acc, touched) = self.touch(&pass.walk);
        // Two orders give the same set. Pass two costs about one look at each word below the cutoff, the cutoff test
        // one keep per title touched: with far fewer such words than titles touched, find r_lo at once.
        let maybe: Vec<u32> = if r_lo_first(&pass, touched.len()) {
            touched
        } else {
            touched
                .into_iter()
                .filter(|&h| self.keep(&acc, h, q.len(), m, cutoff))
                .collect()
        };
        if maybe.is_empty() {
            return maybe;
        }
        let r_lo = if grouped {
            self.group_r_lo(&pass)
        } else {
            self.title_r_lo(&pass)
        };
        let mut out: Vec<u32> = maybe
            .into_iter()
            .filter(|&h| self.keep(&acc, h, q.len(), m, r_lo))
            .collect();
        out.sort_unstable();
        out
    }

    /// Whether title find at threshold `min` walks the title words by group: from a cutoff of [`GROUPED_CUTOFF`]
    /// (the near-duplicate check's 0.85), never at find's [`crate::find::MIN_SCORE`].
    fn takes_groups(min: f64) -> bool {
        let m = min - 0.001;
        m / (2.0 - m) >= GROUPED_CUTOFF
    }

    /// Pass one of title find's term step: the title words with r(w) >= `cutoff` as (term, c(w), |bigrams(" w ")|) in
    /// term-number order: the set s15's scan ([`WordIndex::title_terms_scan`]), which merged every term's bigrams
    /// with the query, gives when kept to the title words. [`WordIndex::title_r_lo`] then gives that scan's r_lo, the
    /// highest r(w) > 0 below `cutoff` (0 if none), to the bit (ws004 s18, agreed with the graph-storage-expert;
    /// title words only since s21).
    ///
    /// An upper bound on c(w) = |b(w) ∩ Q| (multisets) from figures made when the word was interned ([`TermGrams`]).
    /// Put bigrams in 256 buckets (any fixed function; [`bucket`] only sets how tight the bound is); cw(k) and cq(k)
    /// are the bigrams of w and of Q in bucket k. Merging bigrams into buckets cannot lower a min-sum, so
    /// c(w) <= sum over k of min(cw(k), cq(k)) = sum over k of ([cw >= 1][cq >= 1] + [cw >= 2][cq >= 2] + sum over
    /// j >= 3 of [cw >= j][cq >= j]). The first sum is |M1_w & M1_Q|, M1 the buckets holding one or more. The last is
    /// at most d_w = sum over k of max(cw(k) - 2, 0). The middle counts buckets with cw >= 2 whose group (bucket / 4)
    /// is in M2_Q, M2 the groups holding a bucket with two or more; per group of M2_w & M2_Q that is one plus the
    /// group's other such buckets, so at most |M2_w & M2_Q| + e_w, e_w = (buckets with cw >= 2) - |M2_w|. So
    /// U(w) = min(n, |Q|, |M1_w & M1_Q| + |M2_w & M2_Q| + e_w + d_w) >= c(w), n = |b(w)|; c(w) <= |Q| is the length
    /// filter (a word with n > |Q| / cutoff cannot reach the cutoff).
    ///
    /// Pass one merges the kept bigrams (exact c(w)) only for words with U/n >= cutoff: a word below has
    /// r = c/n <= U/n < cutoff (f64 division is monotone in c, same n), so it is not walked. Pass two
    /// ([`WordIndex::title_r_lo`], run only when r_lo can matter) finds r_lo, merging only words with U/n > r_lo so far: a word skipped there has r <= U/n <= r_lo so
    /// far <= the final r_lo and cannot raise it. Pass two takes words by U/n, highest first (64 bins, counted in pass
    /// one), and stops at a bin whose top is at most r_lo. r and cutoff are s15's expressions, so ties fall the same
    /// way, and neither the set nor r_lo depends on scan order.
    ///
    /// Cost: still one pass (two when r_lo is needed) over every title word per query, five ANDs and popcounts each
    /// instead of making, sorting and merging its bigrams, so a batch of titles made of unique tokens stays quadratic
    /// in principle, at a small constant. A rarity-ordered prefix index does not prune such titles: 64-hex has 256
    /// near-uniform bigrams and a hex query holds about a quarter of them, so a term's prefix almost always meets it
    /// (plan 059).
    pub(crate) fn title_walk<'q>(&self, q: &'q [u64], cutoff: f64) -> TermPass<'q> {
        let mut pass = TermPass::new(q, cutoff);
        for t in self.titled() {
            self.pass_word(t, &mut pass);
        }
        pass
    }

    /// Pass one's look at title word `t`: counted by its U/n bin below the cutoff, else merged with the query and
    /// walked or taken into r_lo.
    #[inline]
    fn pass_word(&self, t: usize, pass: &mut TermPass<'_>) {
        let grams = &self.grams.grams[t];
        let Some(ru) = self.grams.ratio(t, pass) else {
            return;
        };
        if ru < pass.cutoff {
            pass.counts[bin(ru) + 1] += 1;
            return;
        }
        let c = crate::find::common(pass.q, grams);
        if c == 0 {
            return;
        }
        let r = c as f64 / grams.len() as f64;
        if r >= pass.cutoff {
            pass.walk.push((t as u32, c as u32, grams.len() as u32));
        } else if r > pass.r_lo {
            pass.r_lo = r;
        }
    }

    /// [`WordIndex::title_walk`] by group (ws003 s27): a group whose U over n is below the cutoff is passed over with
    /// its members, each of whose U/n is at most the group's ([`Group::upper`], same n), so none of them would have
    /// been walked; the rest, and the tail, are looked at word by word as there. So the walk and pass one's r_lo are
    /// title_walk's. The places passed over only size [`r_lo_first`]'s test. Each test is [`Least`]'s integer one.
    pub(crate) fn group_walk<'q>(&self, q: &'q [u64], cutoff: f64) -> TermPass<'q> {
        let mut pass = TermPass::new(q, cutoff);
        let qlen = q.len() as u32;
        let walked = Least::new(|u, n| u as f64 / n as f64 >= cutoff);
        pass.group_u.reserve_exact(self.groups.groups.len());
        for gr in &self.groups.groups {
            let u = gr.upper(&pass.qm, qlen);
            pass.group_u.push(u);
            if !walked.holds(u, gr.n) {
                pass.passed_over += GROUP;
                continue;
            }
            for t in gr.live() {
                self.pass_word(t, &mut pass);
            }
        }
        for &t in &self.groups.tail {
            self.pass_word(t as usize, &mut pass);
        }
        pass.walk.sort_unstable();
        pass
    }

    /// [`WordIndex::title_r_lo`] by group: only a group whose U/n is above pass one's r_lo can hold a word that raises
    /// it (each member's r <= U/n <= the group's), so only those groups' words and the tail's are binned; then bins
    /// highest first, each in term order, as there. The same r_lo, to the bit. When a quarter of the groups or more
    /// pass (a hex graph, say), the words are taken in term order, as title_r_lo takes them, so their figures are
    /// read in the order they sit in memory; otherwise group by group.
    pub(crate) fn group_r_lo(&self, pass: &TermPass<'_>) -> f64 {
        let g = &self.grams;
        let groups = &self.groups;
        let above = Least::new(|u, n| u > 0 && u as f64 / n as f64 > pass.r_lo);
        let mut passing = alloc::vec![0u64; groups.groups.len().div_ceil(64)];
        let mut passed = 0;
        for (k, (gr, &u)) in groups.groups.iter().zip(&pass.group_u).enumerate() {
            if above.holds(u, gr.n) {
                passing[k / 64] |= 1 << (k % 64);
                passed += 1;
            }
        }
        let mut counts = [0u32; BINS + 1];
        let mut found: Vec<(u32, u32)> = Vec::with_capacity(passed * GROUP + groups.tail.len());
        let mut take = |t: usize| {
            if let Some(ru) = g
                .ratio(t, pass)
                .filter(|&ru| ru < pass.cutoff && ru > pass.r_lo)
            {
                let b = bin(ru);
                counts[b + 1] += 1;
                found.push((b as u32, t as u32));
            }
        };
        let by_term = passed * 4 >= groups.groups.len();
        if by_term {
            for t in self.titled() {
                let a = groups.at[t];
                let k = a as usize / GROUP;
                if a & IN_TAIL != 0 || passing[k / 64] >> (k % 64) & 1 == 1 {
                    take(t);
                }
            }
        } else {
            for (k, gr) in groups.groups.iter().enumerate() {
                if passing[k / 64] >> (k % 64) & 1 == 1 {
                    gr.live().for_each(&mut take);
                }
            }
            groups.tail.iter().for_each(|&t| take(t as usize));
        }
        for b in 0..BINS {
            counts[b + 1] += counts[b];
        }
        let mut at = counts;
        let mut order = alloc::vec![0u32; found.len()];
        for (b, t) in found {
            order[at[b as usize] as usize] = t;
            at[b as usize] += 1;
        }
        let mut r_lo = pass.r_lo;
        for b in (0..BINS).rev() {
            if (b + 1) as f64 / BINS as f64 <= r_lo {
                break;
            }
            let words = &mut order[counts[b] as usize..counts[b + 1] as usize];
            if !by_term {
                words.sort_unstable();
            }
            for &t in words.iter() {
                let t = t as usize;
                if !g.ratio(t, pass).is_some_and(|ru| ru > r_lo) {
                    continue;
                }
                let grams = &g.grams[t];
                let r = crate::find::common(pass.q, grams) as f64 / grams.len() as f64;
                if r > r_lo {
                    r_lo = r;
                }
            }
        }
        r_lo
    }

    /// Pass two (see [`WordIndex::title_walk`]): r_lo, taking words by U/n, highest first, so r_lo rises early; a bin
    /// whose top is at most r_lo holds no word that could raise it, nor does any bin below.
    pub(crate) fn title_r_lo(&self, pass: &TermPass<'_>) -> f64 {
        let g = &self.grams;
        let mut counts = pass.counts;
        for b in 0..BINS {
            counts[b + 1] += counts[b];
        }
        let mut at = counts;
        let mut order = alloc::vec![0u32; counts[BINS] as usize];
        for t in self.titled() {
            if let Some(ru) = g.ratio(t, pass).filter(|&ru| ru < pass.cutoff) {
                let b = bin(ru);
                order[at[b] as usize] = t as u32;
                at[b] += 1;
            }
        }
        let mut r_lo = pass.r_lo;
        for b in (0..BINS).rev() {
            if (b + 1) as f64 / BINS as f64 <= r_lo {
                break;
            }
            for &t in &order[counts[b] as usize..counts[b + 1] as usize] {
                let t = t as usize;
                if !g.ratio(t, pass).is_some_and(|ru| ru > r_lo) {
                    continue;
                }
                let grams = &g.grams[t];
                let r = crate::find::common(pass.q, grams) as f64 / grams.len() as f64;
                if r > r_lo {
                    r_lo = r;
                }
            }
        }
        r_lo
    }

    /// Both passes: the walk and r_lo (tests).
    #[cfg(test)]
    pub(crate) fn title_terms(&self, q: &[u64], cutoff: f64) -> (Vec<(u32, u32, u32)>, f64) {
        let pass = self.title_walk(q, cutoff);
        let r_lo = self.title_r_lo(&pass);
        (pass.walk, r_lo)
    }

    /// s15's term scan, kept as the reference [`WordIndex::title_terms`] is checked against (tests) and as the base
    /// the bench times it against: every term's bigrams made, sorted and merged with the query. Walk sorted by term
    /// number.
    #[cfg(any(test, feature = "bench"))]
    pub(crate) fn title_terms_scan(&self, q: &[u64], cutoff: f64) -> (Vec<(u32, u32, u32)>, f64) {
        self.terms_scan(q, cutoff, |_| true)
    }

    /// s15's scan over the terms `take` keeps.
    #[cfg(any(test, feature = "bench"))]
    fn terms_scan(
        &self,
        q: &[u64],
        cutoff: f64,
        take: impl Fn(u32) -> bool,
    ) -> (Vec<(u32, u32, u32)>, f64) {
        let mut grams = Vec::new();
        let mut walk = Vec::new();
        let mut r_lo = 0f64;
        for (term, t) in self.terms.iter().filter(|&(_, t)| take(t)) {
            crate::find::bigrams(term, &mut grams);
            let c = crate::find::common(q, &grams);
            if c == 0 {
                continue;
            }
            let r = c as f64 / grams.len() as f64;
            if r >= cutoff {
                walk.push((t, c as u32, grams.len() as u32));
            } else if r > r_lo {
                r_lo = r;
            }
        }
        walk.sort_unstable();
        (walk, r_lo)
    }

    /// The candidates from the walked words' title postings and r_lo, s15's step after its term scan (tests, bench).
    #[cfg(any(test, feature = "bench"))]
    pub(crate) fn gather(
        &self,
        q: &[u64],
        m: f64,
        walk: Vec<(u32, u32, u32)>,
        r_lo: f64,
    ) -> Vec<u32> {
        let (acc, touched) = self.touch(&walk);
        let mut out: Vec<u32> = touched
            .into_iter()
            .filter(|&h| self.keep(&acc, h, q.len(), m, r_lo))
            .collect();
        out.sort_unstable();
        out
    }

    /// Each title (by handle) the walked words' title postings reach, as (shared bound, walked bigrams), and the
    /// handles reached.
    fn touch(&self, walk: &[(u32, u32, u32)]) -> (Vec<(u32, u32)>, Vec<u32>) {
        let mut acc = alloc::vec![(0u32, 0u32); self.rows()];
        let mut touched = Vec::new();
        for &(t, c, len) in walk {
            let p = &self.postings[t as usize];
            // A node's postings are sorted by field, so only its first can be the title (field 0).
            let mut prev = u32::MAX;
            for (i, &h) in p.handles.iter().enumerate() {
                if h == prev {
                    continue;
                }
                prev = h;
                let (field, count) = p.fc.get(i);
                if field != 0 {
                    continue;
                }
                let a = &mut acc[h as usize];
                if *a == (0, 0) {
                    touched.push(h);
                }
                // A saturated count could undercount: such a node is always kept.
                if count == MAX_COUNT {
                    a.0 = u32::MAX;
                } else {
                    a.0 = a.0.saturating_add(count * c);
                    a.1 = a.1.saturating_add(count * len);
                }
            }
        }
        (acc, touched)
    }

    /// Handles, ascending, of every node whose title holds each of `words` (normalised query words, repeats kept) at
    /// least as often as `words` does (ws004 s24). Exact by construction: such a title is in each word's title
    /// postings (field 0) with a count of at least that word's repeats, and the walk keeps a handle only when every
    /// word's title posting says so. A count at the ceiling ([`MAX_COUNT`]) counts as enough; a word repeated more
    /// often than that in the query cannot be told apart, so it gives the empty set (such a query scores by Dice). A
    /// word not in the term table gives the empty set.
    pub(crate) fn title_containing<'w>(&self, words: impl Iterator<Item = &'w str>) -> Vec<u32> {
        let mut words: Vec<&str> = words.collect();
        words.sort_unstable();
        // (postings, repeats in the query), one per distinct word.
        let mut need: Vec<(&Postings, u32)> = Vec::new();
        let mut i = 0;
        while i < words.len() {
            let run = words[i..].iter().take_while(|&&w| w == words[i]).count();
            let Some(t) = self.terms.get(words[i]) else {
                return Vec::new();
            };
            if run > MAX_COUNT as usize {
                return Vec::new();
            }
            need.push((&self.postings[t as usize], run as u32));
            i += run;
        }
        let enough = |p: &Postings, at: usize, want: u32| {
            let (field, count) = p.fc.get(at);
            field == 0 && (count >= want || count == MAX_COUNT)
        };
        let Some(walk) = (0..need.len()).min_by_key(|&k| need[k].0.handles.len()) else {
            return Vec::new();
        };
        let (shortest, want) = need.swap_remove(walk);
        // The rarest of the rest first, so a handle most lists lack is dropped after one search.
        need.sort_unstable_by_key(|(p, _)| p.handles.len());
        // One cursor per other word: handles only grow along the walk, so each list is searched from where it was.
        let mut at = alloc::vec![0usize; need.len()];
        let mut out = Vec::new();
        'walk: for (i, &h) in shortest.handles.iter().enumerate() {
            // A node's title posting is the first of its run (postings sort by handle, then field; title is 0).
            if (i > 0 && shortest.handles[i - 1] == h) || !enough(shortest, i, want) {
                continue;
            }
            for (k, &(p, want)) in need.iter().enumerate() {
                // Galloping search from the cursor: common words' lists are dense, so the next handle is near.
                let rest = &p.handles[at[k]..];
                let (mut lo, mut step) = (0, 1);
                while lo + step < rest.len() && rest[lo + step] < h {
                    lo += step;
                    step *= 2;
                }
                let end = (lo + step + 1).min(rest.len());
                let j = at[k] + lo + rest[lo..end].partition_point(|&x| x < h);
                at[k] = j;
                if j == p.handles.len() || p.handles[j] != h || !enough(p, j, want) {
                    continue 'walk;
                }
            }
            out.push(h);
        }
        out
    }

    /// |T| of node `h`'s title (see `title_grams`), saturating at `u16::MAX`.
    pub(crate) fn title_grams(&self, h: u32) -> u16 {
        self.title_grams[h as usize]
    }

    /// Whether touched title `h` could score m or more: 2 x min(shared + r_lo x rest, |Q|) >= m x (|Q| + |T|).
    /// Grows with r_lo.
    fn keep(&self, acc: &[(u32, u32)], h: u32, qlen: usize, m: f64, r_lo: f64) -> bool {
        let (shared, walked) = acc[h as usize];
        let t = self.title_grams[h as usize];
        if shared == u32::MAX || t == u16::MAX {
            return true;
        }
        let qlen = qlen as f64;
        let t = f64::from(t);
        let rest = (t - f64::from(walked)).max(0.0);
        // The title can share no more than the query's |Q| bigrams.
        2.0 * (f64::from(shared) + r_lo * rest).min(qlen) >= m * (qlen + t)
    }

    /// The field number `name` has, if any (a test checks title is 0).
    #[cfg(test)]
    pub(crate) fn field_number(&self, name: &str) -> Option<u32> {
        self.field_no.get(name).copied()
    }

    /// The term-side bigram figures' heap bytes and the bigrams they hold (the bench reports bytes per bigram).
    #[cfg(feature = "bench")]
    pub(crate) fn term_grams_bytes(&self) -> (usize, usize) {
        self.grams.heap_bytes()
    }

    /// The term order's and stem classes' heap bytes, and the distinct terms (the bench reports them).
    #[cfg(feature = "bench")]
    pub(crate) fn order_and_stem_bytes(&self) -> (usize, usize) {
        (self.terms.heap_bytes().1, self.terms.len())
    }

    /// Distinct terms in the table (the bench reports it).
    #[cfg(feature = "bench")]
    pub(crate) fn term_count(&self) -> usize {
        self.terms.len()
    }

    /// Title words: terms some title holds (the bench reports them beside the terms).
    #[cfg(feature = "bench")]
    pub(crate) fn title_term_count(&self) -> usize {
        self.titled.iter().map(|b| b.count_ones() as usize).sum()
    }

    /// This graph's figures for `q`, to be summed with the other graphs searched.
    pub(crate) fn stats(&self, q: &WordQuery) -> WordStats {
        let df = |t: u32| u64::from(self.postings[t as usize].nodes);
        let exact: Vec<u64> = q
            .terms
            .iter()
            .map(|t| self.terms.get(t.as_str()).map_or(0, df))
            .collect();
        WordStats {
            nodes: self.rows() as u64,
            df: exact.clone(),
            fields: self
                .fields
                .iter()
                .zip(&self.totals)
                .filter(|(_, t)| t.0 > 0)
                .map(|(f, t)| (String::from(&**f), *t))
                .collect(),
            variants: q
                .terms
                .iter()
                .zip(&exact)
                .map(|(word, &own)| {
                    let mut m: BTreeMap<String, Variant> = BTreeMap::new();
                    let stemmed = crate::stem::stemmed(word);
                    if own > 0 {
                        m.insert(
                            word.clone(),
                            Variant {
                                df: own,
                                stem: stemmed,
                                prefix: false,
                            },
                        );
                    }
                    // Each word's df is counted once: the query word's own is already known.
                    let df_of = |w: &str, t: u32| if w == word { own } else { df(t) };
                    if stemmed {
                        for (w, t) in self.terms.class(&crate::stem::stem(word)) {
                            m.entry(w.into())
                                .or_insert_with(|| Variant {
                                    df: df_of(w, t),
                                    ..Variant::default()
                                })
                                .stem = true;
                        }
                        if word.len() >= PREFIX_MIN {
                            // Only words stemming takes (all ASCII letters) are prefix targets: a word with digits
                            // or other letters matches exactly only, as target as well as query (req-92).
                            let targets = self.terms.prefixed(word).into_iter();
                            for (w, t) in targets.filter(|(w, _)| crate::stem::stemmed(w)) {
                                m.entry(w.into())
                                    .or_insert_with(|| Variant {
                                        df: df_of(w, t),
                                        ..Variant::default()
                                    })
                                    .prefix = true;
                            }
                        }
                    }
                    m
                })
                .collect(),
        }
    }

    /// Per field number: (weight / avg length, rank of the name). The rank orders a node's fields by name.
    fn field_plan(&self, stats: &WordStats) -> Vec<(f64, f64, u32)> {
        let mut by_name: Vec<u32> = (0..self.fields.len() as u32).collect();
        by_name.sort_by(|a, b| self.fields[*a as usize].cmp(&self.fields[*b as usize]));
        let mut rank = alloc::vec![0u32; self.fields.len()];
        for (r, f) in by_name.into_iter().enumerate() {
            rank[f as usize] = r as u32;
        }
        self.fields
            .iter()
            .zip(rank)
            .map(|(name, r)| (weight(name), stats.avg_len(name), r))
            .collect()
    }

    /// Each field's part of tf' for node `h` and term postings `lo..hi`, in field-name order.
    fn parts(
        &self,
        p: &Postings,
        lo: usize,
        hi: usize,
        h: u32,
        plan: &[(f64, f64, u32)],
        out: &mut Vec<(u32, u32, f64)>,
    ) {
        out.clear();
        for i in lo..hi {
            let (f, count) = p.fc.get(i);
            let (w, avg, rank) = plan[f as usize];
            let norm = 1.0 - B + B * f64::from(self.len_of(h, f)) / avg;
            out.push((rank, f, w * f64::from(count) / norm));
        }
        if out.len() > 1 {
            out.sort_by_key(|x| x.0);
        }
    }

    /// BM25F over this graph: (handle, score, held), best first then by handle, at most `limit`. Each query word
    /// scores its best tier ([`TermPlan::score`]). `stats` are the summed figures of every graph searched and `plans`
    /// their [`WordStats::plans`]; `scope` limits which nodes are scored, never the figures. With `marks`, `held` is
    /// the marked words each node holds: 1 for an exact or stem match, [`PREFIX_WEIGHT`] for a prefix match only.
    pub(crate) fn search(
        &self,
        plans: &[TermPlan<'_>],
        stats: &WordStats,
        scope: Option<&dyn Fn(u32) -> bool>,
        limit: usize,
        marks: Option<&[bool]>,
    ) -> Vec<(u32, f64, f64)> {
        let plan = self.field_plan(stats);
        let mut acc = alloc::vec![0.0f64; self.rows()];
        let mut held = alloc::vec![0f64; if marks.is_some() { self.rows() } else { 0 }];
        // One word's tf' per tier, by node, for the nodes it reached (reset after each word); only a word with more
        // than one term needs it; a word with one term scores each posting as it goes.
        let several = plans.iter().any(|tp| tp.members.len() > 1);
        let mut tiers = alloc::vec![[0.0f64; 3]; if several { self.rows() } else { 0 }];
        let mut reached = Vec::new();
        let mut touched = Vec::new();
        let mut parts = Vec::new();
        for (i, tp) in plans.iter().enumerate() {
            let marked = marks.is_some_and(|m| m[i]);
            if let [(word, bits)] = tp.members[..] {
                let Some(t) = self.terms.get(word) else {
                    continue;
                };
                let p = &self.postings[t as usize];
                let mut lo = 0;
                while lo < p.handles.len() {
                    let h = p.handles[lo];
                    let hi = lo + p.handles[lo..].iter().take_while(|&&x| x == h).count();
                    if scope.is_none_or(|s| s(h)) {
                        self.parts(p, lo, hi, h, &plan, &mut parts);
                        let tf = parts.iter().fold(0.0, |s, x| s + x.2);
                        let e = Form::ALL.map(|f| if bits & f.bit() != 0 { tf } else { 0.0 });
                        if acc[h as usize] == 0.0 {
                            touched.push(h);
                        }
                        acc[h as usize] += tp.score(&e).0;
                        if marked {
                            held[h as usize] += if e[0] > 0.0 || e[1] > 0.0 {
                                1.0
                            } else {
                                PREFIX_WEIGHT
                            };
                        }
                    }
                    lo = hi;
                }
                continue;
            }
            for &(word, bits) in &tp.members {
                let Some(t) = self.terms.get(word) else {
                    continue;
                };
                let p = &self.postings[t as usize];
                let mut lo = 0;
                while lo < p.handles.len() {
                    let h = p.handles[lo];
                    let hi = lo + p.handles[lo..].iter().take_while(|&&x| x == h).count();
                    if scope.is_none_or(|s| s(h)) {
                        self.parts(p, lo, hi, h, &plan, &mut parts);
                        let tf = parts.iter().fold(0.0, |s, x| s + x.2);
                        let e = &mut tiers[h as usize];
                        if *e == [0.0; 3] {
                            reached.push(h);
                        }
                        for f in Form::ALL {
                            if bits & f.bit() != 0 {
                                e[f as usize] += tf;
                            }
                        }
                    }
                    lo = hi;
                }
            }
            for h in reached.drain(..) {
                let e = core::mem::take(&mut tiers[h as usize]);
                let (score, _) = tp.score(&e);
                if acc[h as usize] == 0.0 {
                    touched.push(h);
                }
                acc[h as usize] += score;
                if marked {
                    held[h as usize] += if e[0] > 0.0 || e[1] > 0.0 {
                        1.0
                    } else {
                        PREFIX_WEIGHT
                    };
                }
            }
        }
        let mut hits: Vec<(u32, f64, f64)> = touched
            .into_iter()
            .map(|h| {
                (
                    h,
                    acc[h as usize],
                    held.get(h as usize).copied().unwrap_or(0.0),
                )
            })
            .collect();
        // A total order (score, then the lower handle), so selecting the best `limit` first changes nothing.
        let order =
            |a: &(u32, f64, f64), b: &(u32, f64, f64)| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0));
        if limit == 0 {
            return Vec::new();
        }
        if hits.len() > limit {
            hits.select_nth_unstable_by(limit - 1, order);
            hits.truncate(limit);
        }
        hits.sort_by(order);
        hits
    }

    /// Why node `h` scored: the field that earned most of its score (each word's score shared among the fields of
    /// its best tier by their part of tf'; ties to the name first in order), and for each query word it holds, the
    /// tier that scored and that tier's term with the most tf' (ties to the first in string order).
    pub(crate) fn explain<'q>(
        &self,
        q: &'q WordQuery,
        plans: &[TermPlan<'q>],
        stats: &WordStats,
        h: u32,
    ) -> (&str, Vec<WordTerm>) {
        let plan = self.field_plan(stats);
        let mut share = alloc::vec![0.0f64; self.fields.len()];
        let mut by_tier = alloc::vec![[0.0f64; 3]; self.fields.len()];
        let mut matched = Vec::new();
        let mut parts = Vec::new();
        for (word, tp) in q.terms.iter().zip(plans) {
            let mut tf = [0.0f64; 3];
            let mut top: [Option<(&str, f64)>; 3] = [None; 3];
            by_tier.fill([0.0; 3]);
            for &(member, bits) in &tp.members {
                let Some(t) = self.terms.get(member) else {
                    continue;
                };
                let p = &self.postings[t as usize];
                let (lo, hi) = p.run(h);
                if lo == hi {
                    continue;
                }
                self.parts(p, lo, hi, h, &plan, &mut parts);
                let x = parts.iter().fold(0.0, |s, x| s + x.2);
                for f in Form::ALL {
                    if bits & f.bit() != 0 {
                        let k = f as usize;
                        tf[k] += x;
                        for &(_, field, part) in &parts {
                            by_tier[field as usize][k] += part;
                        }
                        if top[k].is_none_or(|(_, best)| x > best) {
                            top[k] = Some((member, x));
                        }
                    }
                }
            }
            let (score, form) = tp.score(&tf);
            if score == 0.0 {
                continue;
            }
            let k = form as usize;
            for (f, row) in by_tier.iter().enumerate() {
                share[f] += score * row[k] / tf[k];
            }
            let term = top[k].map_or("", |(m, _)| m);
            matched.push(WordTerm {
                query: word.clone(),
                term: term.into(),
                form: if term == word { Form::Exact } else { form },
            });
        }
        let best = (0..self.fields.len())
            .filter(|&f| share[f] > 0.0)
            .max_by(|&a, &b| {
                share[a]
                    .total_cmp(&share[b])
                    .then(self.fields[b].cmp(&self.fields[a]))
            });
        (best.map_or("", |f| &self.fields[f]), matched)
    }
}

/// A query word a hit holds, the term it met there and how (req-92: "matched: vendor ~ vendors (stem)").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WordTerm {
    pub query: String,
    pub term: String,
    pub form: Form,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::borrow::Cow;
    use alloc::format;

    /// ws004 s15 (graph-storage-expert): title find reads posting field 0 as the title, so title must be field 0
    /// whatever the index holds, also once a term's postings widen past 64 fields.
    #[test]
    fn title_is_posting_field_0_also_after_widening() {
        let mut w = WordIndex::new();
        assert_eq!(w.field_number(TITLE), Some(0));
        let mut texts: Texts<'_> = alloc::vec![
            (Cow::Borrowed(TITLE), Cow::Borrowed("alpha beta")),
            (Cow::Borrowed(NOTE), Cow::Borrowed("alpha")),
        ];
        for i in 0..70 {
            texts.push((Cow::Owned(format!("f{i:02}")), Cow::Borrowed("alpha")));
        }
        w.index(0, &texts);
        // Node 1 has alpha everywhere but its title.
        let mut untitled = texts.clone();
        untitled[0].1 = Cow::Borrowed("gamma");
        w.index(1, &untitled);
        let alpha = &w.postings[w.terms.get("alpha").expect("alpha") as usize];
        assert!(matches!(alpha.fc, FieldCounts::Wide(_)), "alpha widened");
        assert_eq!(w.field_number(TITLE), Some(0));
        let grams = |text: &str| {
            let mut g = alloc::vec::Vec::new();
            crate::find::bigrams(text, &mut g);
            g
        };
        assert_eq!(w.title_candidates(&grams("alpha"), 0.4), [0]);
        assert_eq!(w.title_candidates(&grams("gamma"), 0.4), [1]);
    }

    fn grams(text: &str) -> Vec<u64> {
        let mut norm = String::new();
        crate::find::normalise_into(text, &mut norm);
        let mut g = Vec::new();
        crate::find::bigrams(&norm, &mut g);
        g
    }

    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((self.0 >> 33) % n as u64) as usize
        }

        fn word(&mut self, alphabet: &[u8], lo: usize, hi: usize) -> String {
            (0..lo + self.below(hi - lo + 1))
                .map(|_| alphabet[self.below(alphabet.len())] as char)
                .collect()
        }
    }

    const HEX: &[u8] = b"0123456789abcdef";
    const LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
    /// Repeated bigrams (d > 0), one-letter words, non-ASCII, and words that become each other's typos.
    const ODD: &[&str] = &[
        "a",
        "ab",
        "ba",
        "aa",
        "aaaa",
        "aaaaaaaaaaaa",
        "abab",
        "abababab",
        "zzzzzzzzzz",
        "abcdefgh",
        "σίσυφος",
        "οδος",
        "straße",
        "strasse",
        "東京",
        "ǆungla",
        "x1",
        "1x",
        "launch",
        "bench",
        "lunch",
    ];

    /// Titles: 64-hex (the project manager's batch), short hex, vocabulary words, and the odd words, alone and mixed.
    fn titles(rng: &mut Rng, count: usize) -> Vec<String> {
        let vocab: Vec<String> = (0..300).map(|_| rng.word(LETTERS, 2, 10)).collect();
        (0..count)
            .map(|i| match i % 5 {
                0 => rng.word(HEX, 64, 64),
                1 => rng.word(HEX, 4, 12),
                2 => (0..1 + rng.below(4))
                    .map(|_| vocab[rng.below(vocab.len())].clone())
                    .collect::<Vec<_>>()
                    .join(" "),
                3 => ODD[rng.below(ODD.len())].into(),
                _ => alloc::format!(
                    "{} {} {}",
                    ODD[rng.below(ODD.len())],
                    vocab[rng.below(vocab.len())],
                    rng.word(HEX, 8, 40)
                ),
            })
            .collect()
    }

    /// Queries: titles whole, with typos, words joined and split, and others alone.
    fn queries(rng: &mut Rng, titles: &[String], count: usize) -> Vec<String> {
        let mut out: Vec<String> = ODD.iter().map(|&s| s.into()).collect();
        for _ in 0..count {
            let t = titles[rng.below(titles.len())].clone();
            let mut c: Vec<char> = t.chars().collect();
            for _ in 0..rng.below(3) {
                let i = rng.below(c.len());
                match rng.below(4) {
                    0 => c[i] = HEX[rng.below(16)] as char,
                    1 if c.len() > 1 => {
                        c.remove(i);
                    }
                    2 => c.insert(i, ' '),
                    _ => c.insert(i, LETTERS[rng.below(26)] as char),
                }
            }
            let q: String = c.into_iter().collect();
            out.push(if rng.below(4) == 0 {
                q.replace(' ', "")
            } else {
                q
            });
        }
        out
    }

    fn texts(title: &str) -> Texts<'_> {
        alloc::vec![(Cow::Borrowed(TITLE), Cow::Borrowed(title))]
    }

    fn with_note<'a>(title: &'a str, note: &'a str) -> Texts<'a> {
        alloc::vec![
            (Cow::Borrowed(TITLE), Cow::Borrowed(title)),
            (Cow::Borrowed(NOTE), Cow::Borrowed(note)),
        ]
    }

    /// Every other node's note: 1 to 6 words of a pool no title draws from, so most are note-only words (s21).
    fn note(rng: &mut Rng, pool: &[String], h: usize) -> String {
        if h % 2 == 1 {
            return String::new();
        }
        (0..1 + rng.below(6))
            .map(|_| pool[rng.below(pool.len())].as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The image of an index built afresh from `titles` and `notes`, handle by handle.
    fn rebuilt(titles: &[String], notes: &[String]) -> String {
        let mut w = WordIndex::new();
        for (h, (t, n)) in titles.iter().zip(notes).enumerate() {
            w.index(h as u32, &with_note(t, n));
        }
        w.image(&ids(titles.len()))
    }

    fn ids(n: usize) -> Vec<umx_core::Id> {
        (0..n as u128).map(umx_core::Id).collect()
    }

    /// An index with free term numbers reused by other words, some shorter than the word freed: titles in, every
    /// third out (freeing words only it held), then new titles in place of those. Every other node has a note of
    /// words no title holds (s21), so the title words are only part of the vocabulary. Its kept counts and title
    /// words equal a rebuild's.
    fn churned(seed: u64) -> (WordIndex, Rng, Vec<String>) {
        let mut rng = Rng(seed);
        let mut all = titles(&mut rng, 1500);
        let pool: Vec<String> = (0..400).map(|_| rng.word(LETTERS, 3, 12)).collect();
        let mut notes: Vec<String> = (0..1500).map(|h| note(&mut rng, &pool, h)).collect();
        let mut w = WordIndex::new();
        for (h, t) in all.iter().enumerate() {
            w.index(h as u32, &with_note(t, &notes[h]));
        }
        // A long word with no repeats freed, then a short one full of repeats: the reuse that a stale n, mask or d
        // would get wrong.
        w.index(1500, &texts("abcdefghijklmnopqrstuvwx"));
        all.push("abcdefghijklmnopqrstuvwx".into());
        notes.push(String::new());
        w.unindex(1500, &texts("abcdefghijklmnopqrstuvwx"));
        w.index(1500, &texts("aaaa"));
        all[1500] = "aaaa".into();
        let fresh = titles(&mut rng, 1500);
        for h in (0..1500).step_by(3) {
            w.unindex(h as u32, &with_note(&all[h], &notes[h]));
        }
        assert!(!w.free.is_empty(), "words were freed");
        for h in (0..1500).step_by(3) {
            all[h] = fresh[h].clone();
            notes[h] = note(&mut rng, &pool, h);
            w.index(h as u32, &with_note(&all[h], &notes[h]));
        }
        assert!(w.free.len() < 300, "freed words were reused");
        assert_eq!(w.image(&ids(all.len())), rebuilt(&all, &notes));
        (w, rng, all)
    }

    /// ws004 s21 (graph-storage-expert's plant): retiring a stem class's head moves the head on, a freed number
    /// reused for a word of another stem starts with no next, and the order drops and gains the words.
    #[test]
    fn a_retired_class_head_is_unlinked_and_its_number_reused_clean() {
        let mut w = WordIndex::new();
        w.index(0, &texts("vendors"));
        w.index(1, &texts("vendor"));
        w.index(2, &texts("vendored"));
        let class = |w: &WordIndex, s: &str| -> Vec<String> {
            w.terms
                .class(s)
                .into_iter()
                .map(|(x, _)| x.into())
                .collect()
        };
        assert_eq!(class(&w, "vendor"), ["vendor", "vendored", "vendors"]);
        // `vendored` went in last, so it heads the class.
        let head = w.terms.get("vendored").unwrap();
        assert_eq!(w.terms.head("vendor"), Some(head));
        w.unindex(2, &texts("vendored"));
        assert_eq!(class(&w, "vendor"), ["vendor", "vendors"]);
        assert_eq!(w.free, [head]);
        w.index(2, &texts("apples"));
        assert_eq!(
            w.terms.get("apples"),
            Some(head),
            "the freed number is reused"
        );
        assert_eq!(class(&w, "appl"), ["apples"]);
        assert_eq!(class(&w, "vendor"), ["vendor", "vendors"]);
        // A middle member out, then the last: the class and its stem entry go.
        w.unindex(0, &texts("vendors"));
        assert_eq!(class(&w, "vendor"), ["vendor"]);
        w.unindex(1, &texts("vendor"));
        assert!(class(&w, "vendor").is_empty());
        assert_eq!(w.terms.head("vendor"), None);
        let words: Vec<&str> = w.terms.prefixed("").into_iter().map(|(x, _)| x).collect();
        assert_eq!(words, ["apples"]);
        let mut out = String::new();
        w.terms.image(&mut out);
        assert_eq!(out, "order: [\"apples\"]\nstem appl: [\"apples\"]\n");
    }

    /// S186-1 (graph-storage-expert): retiring terms leaves tombstones, so capacity() falls while the buckets
    /// stay; the bytes counted are still the tables' allocations.
    #[test]
    fn retired_terms_keep_their_tables_counted_by_allocation() {
        let word = |i: usize| {
            let a = (b'a' + (i / 26 / 26 % 26) as u8) as char;
            let b = (b'a' + (i / 26 % 26) as u8) as char;
            let c = (b'a' + (i % 26) as u8) as char;
            format!("w{a}{b}{c}k{}", ["", "s", "ing"][i % 3])
        };
        let mut t = Terms::default();
        for i in 0..3_000 {
            t.insert(&word(i), i as u32);
        }
        let (index, stems) = (t.index.allocation_size(), t.stems.allocation_size());
        for i in (0..3_000).step_by(2) {
            t.remove(&word(i), i as u32);
        }
        assert_eq!(t.index.allocation_size(), index);
        assert_eq!(t.stems.allocation_size(), stems);
        let names = t.names.iter().map(|w| w.len()).sum::<usize>()
            + t.names.capacity() * size_of::<Box<str>>();
        let vecs = (t.sorted.capacity() + t.tail.capacity() + t.next.capacity()) * size_of::<u32>();
        assert_eq!(t.heap_bytes(), (names + index, vecs + stems));
    }

    /// The sorted order stays sorted across tail merges and retires out of both the order and the tail.
    #[test]
    fn the_order_holds_across_tail_merges_and_retires() {
        let mut w = WordIndex::new();
        let word = |i: usize| format!("w{:05}", (i * 7919) % 5000);
        for i in 0..3000 {
            w.index(i as u32, &texts(&word(i)));
        }
        for i in (0..3000).step_by(4) {
            w.unindex(i as u32, &texts(&word(i)));
        }
        let mut want: Vec<String> = (0..3000).filter(|i| i % 4 != 0).map(word).collect();
        want.sort();
        let got: Vec<String> = w
            .terms
            .prefixed("w")
            .into_iter()
            .map(|(x, _)| x.into())
            .collect();
        assert_eq!(got, want);
        w.shrink();
        assert!(w.terms.tail.is_empty());
        let mut out = String::new();
        w.terms.image(&mut out);
        let got: Vec<String> = w
            .terms
            .prefixed("w0")
            .into_iter()
            .map(|(x, _)| x.into())
            .collect();
        assert_eq!(
            got,
            want.iter()
                .filter(|x| x.starts_with("w0"))
                .cloned()
                .collect::<Vec<_>>()
        );
    }

    /// ws004 s18: U(w) is at least the bigrams w shares with the query, for every live term and query.
    #[test]
    fn the_term_bound_is_never_below_the_shared_bigrams() {
        let (w, mut rng, all) = churned(18);
        let mut tight = 0;
        for q in queries(&mut rng, &all, 300) {
            let q = grams(&q);
            let (qm, _) = Masks::of(&q);
            for (term, t) in w.terms.iter() {
                let mut g = Vec::new();
                crate::find::bigrams(term, &mut g);
                let c = crate::find::common(&q, &g) as u32;
                let u = w.grams.upper(t as usize, &qm, q.len() as u32);
                assert!(u >= c, "{term:?}: U {u} < c {c}");
                tight += usize::from(u == c && c > 0);
            }
        }
        // The check reaches the edge where a bound one lower would be wrong.
        assert!(tight > 1000, "{tight} tight pairs");
    }

    /// ws004 s18, s21: the walked words, r_lo (to the bit) and the candidates equal s15's term scan kept to the title
    /// words, and are among s15's full scan's candidates, on an index whose free term numbers were reused and whose
    /// notes hold words no title does, at find's and the near-duplicate check's thresholds and others.
    #[test]
    fn title_terms_equal_s15s_scan() {
        let (w, mut rng, all) = churned(1815);
        let (walked, lo, _, survivors, fewer) = equal_to_s15(&w, &queries(&mut rng, &all, 400));
        assert!(
            walked > 2000 && lo > 1000,
            "{walked} walked, {lo} with r_lo"
        );
        assert!(
            survivors > 500,
            "{survivors} queries needed r_lo after the cutoff test"
        );
        assert!(fewer > 5, "{fewer} queries kept fewer candidates than s15");
        // A small vocabulary under many titles: r_lo is found first (see r_lo_first).
        let vocab = [
            "approval", "delays", "launch", "date", "bench", "lunch", "risk", "plan", "budget", "a",
        ];
        let mut small = WordIndex::new();
        let titles: Vec<String> = (0..3000)
            .map(|_| {
                (0..1 + rng.below(4))
                    .map(|_| vocab[rng.below(vocab.len())])
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        for (h, t) in titles.iter().enumerate() {
            small.index(h as u32, &texts(t));
        }
        let (_, _, first, _, fewer) = equal_to_s15(&small, &queries(&mut rng, &titles, 200));
        assert!(first > 500, "{first} queries found r_lo first");
        assert_eq!(fewer, 0, "every word is a title word: s15's candidates");
    }

    /// Asserts, per query and threshold, the walk, r_lo and candidates equal s15's scan kept to the title words, and
    /// the candidates are among s15's full scan's; returns (words walked, queries with r_lo > 0, queries where r_lo
    /// was found first, queries with a title surviving the cutoff test, queries with fewer candidates than s15).
    fn equal_to_s15(w: &WordIndex, queries: &[String]) -> (usize, usize, usize, usize, usize) {
        let (mut walked, mut lo, mut first, mut survivors, mut fewer) = (0, 0, 0, 0, 0);
        let titled = |t: u32| w.postings[t as usize].titles > 0;
        for q in queries {
            let q = grams(q);
            for min in [0.4, 0.85, 0.3, 0.6, 0.95, 1.0] {
                let m = min - 0.001;
                let cutoff = m / (2.0 - m);
                let (walk, r_lo) = w.title_terms(&q, cutoff);
                let (scan, scan_lo) = w.terms_scan(&q, cutoff, titled);
                assert_eq!(walk, scan, "walk for {q:?} at {min}");
                assert_eq!(r_lo.to_bits(), scan_lo.to_bits(), "r_lo for {q:?} at {min}");
                let got = w.title_candidates(&q, min);
                assert_eq!(
                    got,
                    w.gather(&q, m, scan, scan_lo),
                    "candidates for {q:?} at {min}"
                );
                let (all, all_lo) = w.title_terms_scan(&q, cutoff);
                let s15 = w.gather(&q, m, all, all_lo);
                assert!(
                    got.iter().all(|h| s15.binary_search(h).is_ok()),
                    "candidates for {q:?} at {min} are among s15's"
                );
                fewer += usize::from(got.len() < s15.len());
                walked += walk.len();
                lo += usize::from(r_lo > 0.0);
                let pass = w.title_walk(&q, cutoff);
                let (acc, touched) = w.touch(&pass.walk);
                if r_lo_first(&pass, touched.len()) {
                    first += 1;
                } else if touched.iter().any(|&h| w.keep(&acc, h, q.len(), m, cutoff)) {
                    survivors += 1;
                }
            }
        }
        (walked, lo, first, survivors, fewer)
    }

    /// s21 (graph-storage-expert): a word leaving its last title but staying in a note leaves the title words, and a
    /// word in a note before any title joins them when a title takes it; the kept counts follow, as a rebuild's.
    #[test]
    fn title_words_follow_titles_not_notes() {
        let titled = |w: &WordIndex, word: &str| {
            w.terms
                .get(word)
                .is_some_and(|t| w.titled().any(|x| x == t as usize))
        };
        let df = |w: &WordIndex, word: &str| {
            let t = w.terms.get(word).unwrap() as usize;
            (w.postings[t].nodes, w.postings[t].titles)
        };
        let q = grams("alpha");
        let mut w = WordIndex::new();
        let (mut titles, mut notes) = (
            alloc::vec![String::from("alpha beta"), String::from("alpha")],
            alloc::vec![String::from("alpha"), String::new()],
        );
        for h in 0..2 {
            w.index(h as u32, &with_note(&titles[h], &notes[h]));
        }
        assert_eq!(df(&w, "alpha"), (2, 2));
        let mut edit = |w: &mut WordIndex, h: usize, title: &str, note: &str| {
            w.unindex(h as u32, &with_note(&titles[h], &notes[h]));
            titles[h] = title.into();
            notes[h] = note.into();
            w.index(h as u32, &with_note(&titles[h], &notes[h]));
            assert_eq!(w.image(&ids(2)), rebuilt(&titles, &notes));
        };
        edit(&mut w, 0, "beta", "alpha");
        assert!(titled(&w, "alpha"), "node 1's title still holds alpha");
        assert_eq!(df(&w, "alpha"), (2, 1));
        edit(&mut w, 1, "gamma", "alpha");
        assert!(!titled(&w, "alpha"), "alpha is in notes only");
        assert_eq!(df(&w, "alpha"), (2, 0));
        assert!(w.title_terms(&q, 0.7).0.is_empty());
        assert!(!w.title_terms_scan(&q, 0.7).0.is_empty(), "s15 walks it");
        assert!(w.title_candidates(&q, 0.4).is_empty());
        // A note word reaching a title.
        edit(&mut w, 0, "beta delta", "alpha");
        edit(&mut w, 1, "alpha", "delta");
        assert!(titled(&w, "alpha") && titled(&w, "delta"));
        assert_eq!(df(&w, "delta"), (2, 1));
        assert_eq!(w.title_candidates(&q, 0.4), [1]);
    }

    /// ws004 s18: a freed word is out of the scan: a query equal to it walks nothing it held.
    #[test]
    fn a_freed_word_is_not_walked() {
        let mut w = WordIndex::new();
        w.index(0, &texts("deadbeefcafe"));
        w.index(1, &texts("other"));
        w.unindex(0, &texts("deadbeefcafe"));
        let q = grams("deadbeefcafe");
        assert_eq!(w.title_terms(&q, 0.7).0, Vec::new());
    }

    /// ws003 s27 (graph-storage's plant): find's threshold keeps the walk of every title word, where groups rule out
    /// few words and cost more (the s27 spike); the near-duplicate check's 0.85 walks groups.
    #[test]
    fn only_the_near_duplicate_threshold_walks_groups() {
        assert!(!WordIndex::takes_groups(crate::find::MIN_SCORE));
        assert!(WordIndex::takes_groups(0.85));
    }

    /// Asserts, per query and threshold, that the grouped walk gives today's walk, r_lo (to the bit) and candidates,
    /// and that the groups are what their members make ([`WordIndex::image`]'s check); returns (groups passed over in
    /// pass one, queries whose r_lo is above 0).
    fn grouped_equals_today(w: &WordIndex, queries: &[String]) -> (usize, usize) {
        let _ = w.image(&ids(w.rows()));
        let (mut skipped, mut lo) = (0, 0);
        for q in queries {
            let q = grams(q);
            let qlen = q.len() as u32;
            for min in [0.7, 0.85, 0.95, 1.0] {
                let m = min - 0.001;
                let cutoff = m / (2.0 - m);
                let (today, grouped) = (w.title_walk(&q, cutoff), w.group_walk(&q, cutoff));
                assert_eq!(grouped.walk, today.walk, "walk for {q:?} at {min}");
                assert_eq!(
                    grouped.r_lo.to_bits(),
                    today.r_lo.to_bits(),
                    "pass one's r_lo"
                );
                let r_lo = w.title_r_lo(&today);
                assert_eq!(
                    w.group_r_lo(&grouped).to_bits(),
                    r_lo.to_bits(),
                    "r_lo for {q:?} at {min}"
                );
                assert_eq!(
                    w.title_candidates(&q, min),
                    w.title_candidates_ungrouped(&q, min),
                    "candidates for {q:?} at {min}"
                );
                skipped += w
                    .groups
                    .groups
                    .iter()
                    .filter(|g| {
                        let u = g.upper(&today.qm, qlen);
                        u > 0 && (u as f64 / g.n as f64) < cutoff
                    })
                    .count();
                lo += usize::from(r_lo > 0.0);
            }
        }
        (skipped, lo)
    }

    /// ws003 s27: through each write path (a load's one build, adds past a tail merge, the s26 reindex, unindex with
    /// freed numbers reused, enough removals to group afresh, pop), the groups stay what their members make and the
    /// grouped walk gives today's walk, r_lo and candidates; the index image equals a rebuild's at the end.
    #[test]
    fn grouped_walk_equals_todays_through_every_write() {
        let mut rng = Rng(27);
        let mut all = titles(&mut rng, 2400);
        let pool: Vec<String> = (0..400).map(|_| rng.word(LETTERS, 3, 12)).collect();
        let mut notes: Vec<String> = (0..2400).map(|h| note(&mut rng, &pool, h)).collect();
        let mut w = WordIndex::new();
        let mut checks = (0, 0);
        let mut check = |w: &WordIndex, all: &[String], rng: &mut Rng| {
            let live: Vec<String> = all[..w.rows()]
                .iter()
                .filter(|t| !t.is_empty())
                .cloned()
                .collect();
            let (s, l) = grouped_equals_today(w, &queries(rng, &live, 12));
            checks = (checks.0 + s, checks.1 + l);
        };
        // A load: one grouping at its end, nothing left in the tail.
        w.begin_build();
        for h in 0..400 {
            w.index(h as u32, &with_note(&all[h], &notes[h]));
        }
        w.finish_build();
        assert!(w.groups.tail.is_empty() && !w.groups.groups.is_empty());
        check(&w, &all, &mut rng);
        // Adds one by one: more new title words than the tail holds, so it merges.
        let (grouped, before) = (w.groups.groups.len(), w.titled().count());
        for h in 400..2400 {
            w.index(h as u32, &with_note(&all[h], &notes[h]));
            if h % 400 == 0 {
                check(&w, &all, &mut rng);
            }
        }
        let added = w.titled().count() - before;
        assert!(
            added > GROUP_TAIL && w.groups.groups.len() > grouped && w.groups.tail.len() < added,
            "the tail merged: {added} title words added"
        );
        check(&w, &all, &mut rng);
        // Edits through the s26 reindex: new titles, words gained and lost, some words freed and reused.
        let fresh = titles(&mut rng, 2400);
        for h in (0..2400).step_by(7) {
            w.stash(h as u32, &with_note(&all[h], &notes[h]));
            all[h] = fresh[h].clone();
            w.reindex(h as u32, &with_note(&all[h], &notes[h]));
        }
        check(&w, &all, &mut rng);
        // Removals: enough grouped title words leave for the places emptied to pass the tail's size.
        let title_words = |w: &WordIndex| w.titled().count();
        let (before, tail) = (title_words(&w), w.groups.tail.len());
        for h in (0..2400).filter(|h| h % 4 != 3) {
            w.unindex(h as u32, &with_note(&all[h], &notes[h]));
            all[h].clear();
            notes[h].clear();
        }
        let lost = before - title_words(&w);
        assert!(
            lost > tail + GROUP_TAIL && w.groups.holes <= GROUP_TAIL,
            "{lost} title words left, {tail} in the tail: more left groups than a grouping waits for"
        );
        check(&w, &all, &mut rng);
        for h in (0..2400).step_by(4) {
            all[h] = fresh[(h + 1) % 2400].clone();
            w.index(h as u32, &with_note(&all[h], &notes[h]));
        }
        check(&w, &all, &mut rng);
        // The last rows out, as a rolled-back append.
        for h in (2300..2400).rev() {
            w.unindex(h as u32, &with_note(&all[h], &notes[h]));
            w.pop();
        }
        all.truncate(2300);
        notes.truncate(2300);
        check(&w, &all, &mut rng);
        assert_eq!(w.image(&ids(all.len())), rebuilt(&all, &notes));
        assert!(checks.0 > 10_000, "{} groups passed over", checks.0);
        assert!(checks.1 > 100, "{} queries with r_lo", checks.1);
    }
}
