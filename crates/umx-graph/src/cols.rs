//! The column types behind `Graph` (workshop 008 sections 2.1 to 2.3): interned names with use counts, name
//! columns that widen past 65,536 names, whole-second times with an exceptions table, packed adjacency and the
//! edge field side table.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::mem::{size_of, size_of_val};
use umx_core::{Id, Value};

/// Every type, status, relation, author and custom field name in one graph, each stored once. Numbers never change while the
/// graph is held; a name no record uses any more stays in the table (not "in use") until the next load.
#[derive(Clone, Debug, Default)]
pub(crate) struct Names {
    text: Vec<Box<str>>,
    /// Name numbers in name order.
    sorted: Vec<u32>,
    /// Per name: how many nodes have it as their type, and how many edges as their relation.
    node_uses: Vec<u32>,
    edge_uses: Vec<u32>,
}

impl Names {
    fn search(&self, name: &str) -> Result<usize, usize> {
        self.sorted
            .binary_search_by(|&n| (*self.text[n as usize]).cmp(name))
    }

    pub fn get(&self, name: &str) -> Option<u32> {
        self.search(name).ok().map(|i| self.sorted[i])
    }

    pub fn intern(&mut self, name: &str) -> u32 {
        match self.search(name) {
            Ok(i) => self.sorted[i],
            Err(i) => {
                let n = self.text.len() as u32;
                self.text.push(name.into());
                self.node_uses.push(0);
                self.edge_uses.push(0);
                self.sorted.insert(i, n);
                n
            }
        }
    }

    /// How many names the table holds.
    pub fn len(&self) -> usize {
        self.text.len()
    }

    #[inline]
    pub fn text(&self, n: u32) -> &str {
        &self.text[n as usize]
    }

    pub fn node_use(&mut self, n: u32, add: bool) {
        bump(&mut self.node_uses[n as usize], add);
    }

    pub fn edge_use(&mut self, n: u32, add: bool) {
        bump(&mut self.edge_uses[n as usize], add);
    }

    /// Every node type count back to zero, before the node rows are derived again (edges keep theirs).
    pub fn clear_node_uses(&mut self) {
        self.node_uses.iter_mut().for_each(|c| *c = 0);
    }

    /// Back to the first `len` names, with no edge using any of them, sized to fit (`Graph::reset_edges`).
    pub fn truncate(&mut self, len: usize) {
        self.text.truncate(len);
        self.node_uses.truncate(len);
        self.edge_uses.clear();
        self.edge_uses.resize(len, 0);
        self.sorted.retain(|&n| (n as usize) < len);
        self.shrink();
    }

    /// Tests only: how many nodes have name `n` as their type, and how many edges as their relation.
    #[cfg(test)]
    pub fn uses(&self, n: u32) -> (u32, u32) {
        (self.node_uses[n as usize], self.edge_uses[n as usize])
    }

    pub fn used_by_node(&self, n: u32) -> bool {
        self.node_uses[n as usize] > 0
    }

    pub fn node_uses(&self, n: u32) -> usize {
        self.node_uses[n as usize] as usize
    }

    pub fn used_by_edge(&self, n: u32) -> bool {
        self.edge_uses[n as usize] > 0
    }

    /// Every name number, in name order.
    pub fn in_name_order(&self) -> impl Iterator<Item = u32> + '_ {
        self.sorted.iter().copied()
    }

    pub fn heap_bytes(&self) -> usize {
        self.text.capacity() * size_of::<Box<str>>()
            + self.text.iter().map(|t| t.len()).sum::<usize>()
            + (self.sorted.capacity() + self.node_uses.capacity() + self.edge_uses.capacity())
                * size_of::<u32>()
    }

    pub fn shrink(&mut self) {
        self.text.shrink_to_fit();
        self.sorted.shrink_to_fit();
        self.node_uses.shrink_to_fit();
        self.edge_uses.shrink_to_fit();
    }
}

fn bump(count: &mut u32, add: bool) {
    if add {
        *count += 1;
    } else {
        *count -= 1;
    }
}

/// A column of name numbers: `u16` until a number needs more, then `u32` (workshop 008 section 2.2). Widening
/// happens on the push or set that needs it; nothing is refused.
#[derive(Clone, Debug)]
pub(crate) enum NameCol {
    Narrow(Vec<u16>),
    Wide(Vec<u32>),
}

impl Default for NameCol {
    fn default() -> NameCol {
        NameCol::Narrow(Vec::new())
    }
}

impl NameCol {
    #[inline]
    pub fn get(&self, i: usize) -> u32 {
        match self {
            NameCol::Narrow(v) => u32::from(v[i]),
            NameCol::Wide(v) => v[i],
        }
    }

    fn widen_for(&mut self, n: u32) {
        if let NameCol::Narrow(v) = self
            && n > u32::from(u16::MAX)
        {
            *self = NameCol::Wide(v.iter().map(|&x| u32::from(x)).collect());
        }
    }

    pub fn push(&mut self, n: u32) {
        self.widen_for(n);
        match self {
            NameCol::Narrow(v) => v.push(n as u16),
            NameCol::Wide(v) => v.push(n),
        }
    }

    pub fn set(&mut self, i: usize, n: u32) {
        self.widen_for(n);
        match self {
            NameCol::Narrow(v) => v[i] = n as u16,
            NameCol::Wide(v) => v[i] = n,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            NameCol::Narrow(v) => v.len(),
            NameCol::Wide(v) => v.len(),
        }
    }

    /// Replaces the numbers in `at` with `with`.
    pub fn splice(&mut self, at: core::ops::Range<usize>, with: &[u32]) {
        if let Some(&top) = with.iter().max() {
            self.widen_for(top);
        }
        match self {
            NameCol::Narrow(v) => {
                v.splice(at, with.iter().map(|&n| n as u16));
            }
            NameCol::Wide(v) => {
                v.splice(at, with.iter().copied());
            }
        }
    }

    pub fn truncate(&mut self, len: usize) {
        match self {
            NameCol::Narrow(v) => v.truncate(len),
            NameCol::Wide(v) => v.truncate(len),
        }
    }

    pub fn remove(&mut self, i: usize) {
        match self {
            NameCol::Narrow(v) => {
                v.remove(i);
            }
            NameCol::Wide(v) => {
                v.remove(i);
            }
        }
    }

    #[cfg(test)]
    pub fn is_wide(&self) -> bool {
        matches!(self, NameCol::Wide(_))
    }

    pub fn heap_bytes(&self) -> usize {
        match self {
            NameCol::Narrow(v) => v.capacity() * 2,
            NameCol::Wide(v) => v.capacity() * 4,
        }
    }

    pub fn shrink(&mut self) {
        match self {
            NameCol::Narrow(v) => v.shrink_to_fit(),
            NameCol::Wide(v) => v.shrink_to_fit(),
        }
    }
}

/// Marks a time held in the exceptions table.
const ODD: u32 = u32::MAX;

/// A time column: whole seconds since 1970 for the form umx writes (`YYYY-MM-DDTHH:MM:SSZ`), and any other text
/// (fractions, another zone form, before 1970, empty, not a time) kept exactly in an exceptions table by id.
#[derive(Clone, Debug, Default)]
pub(crate) struct Times {
    secs: Vec<u32>,
    odd: BTreeMap<Id, Box<str>>,
}

impl Times {
    fn encode(&mut self, id: Id, text: &str) -> u32 {
        match canonical_secs(text) {
            Some(s) => s,
            None => {
                self.odd.insert(id, text.into());
                ODD
            }
        }
    }

    pub fn push(&mut self, id: Id, text: &str) {
        let s = self.encode(id, text);
        self.secs.push(s);
    }

    pub fn set(&mut self, i: usize, id: Id, text: &str) {
        self.odd.remove(&id);
        self.secs[i] = self.encode(id, text);
    }

    pub fn remove(&mut self, i: usize, id: Id) {
        self.secs.remove(i);
        self.odd.remove(&id);
    }

    /// The time as a JSON string, quotes included. An exception's text can hold any character, so it is escaped.
    pub fn write(&self, i: usize, id: Id, out: &mut Vec<u8>) {
        match self.secs[i] {
            ODD => crate::encode::push_str(out, &self.odd[&id]),
            s => {
                out.push(b'"');
                out.extend_from_slice(&iso(s));
                out.push(b'"');
            }
        }
    }

    pub fn get(&self, i: usize, id: Id) -> String {
        match self.secs[i] {
            ODD => self.odd[&id].as_ref().into(),
            s => core::str::from_utf8(&iso(s))
                .expect("iso writes ASCII")
                .into(),
        }
    }

    /// Row `i`'s time as whole seconds since 1970 when it is in the form umx writes; `None` for an exception (any
    /// other text), which `get` returns as written.
    #[inline]
    pub fn secs(&self, i: usize) -> Option<u32> {
        Some(self.secs[i]).filter(|&s| s != ODD)
    }

    pub fn column_bytes(&self) -> usize {
        self.secs.capacity() * size_of::<u32>()
    }

    pub fn exception_bytes(&self) -> usize {
        btree_bytes::<Id, Box<str>>(self.odd.len())
            + self.odd.values().map(|t| t.len()).sum::<usize>()
    }

    pub fn shrink(&mut self) {
        self.secs.shrink_to_fit();
    }
}

/// The text umx writes for `secs`.
fn iso(secs: u32) -> [u8; 20] {
    let days = i64::from(secs / 86_400);
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    let mut out = *b"0000-00-00T00:00:00Z";
    let mut put = |at: usize, width: usize, mut v: u32| {
        for i in (at..at + width).rev() {
            out[i] = b'0' + (v % 10) as u8;
            v /= 10;
        }
    };
    put(0, 4, y as u32);
    put(5, 2, m);
    put(8, 2, d);
    put(11, 2, rem / 3600);
    put(14, 2, rem % 3600 / 60);
    put(17, 2, rem % 60);
    out
}

/// Seconds since 1970 if `text` is exactly the form `iso` writes back, so encoding gives the same bytes.
fn canonical_secs(text: &str) -> Option<u32> {
    let b = text.as_bytes();
    if b.len() != 20 {
        return None;
    }
    let num = |r: core::ops::Range<usize>| -> Option<u32> {
        b[r].iter().try_fold(0u32, |acc, &c| {
            c.is_ascii_digit().then(|| acc * 10 + u32::from(c - b'0'))
        })
    };
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, s) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 59 || y < 1970
    {
        return None;
    }
    let days = days_from_civil(i64::from(y), mo, d);
    let secs = days * 86_400 + i64::from(h * 3600 + mi * 60 + s);
    let secs = u32::try_from(secs).ok().filter(|&s| s != ODD)?;
    (iso(secs) == *b).then_some(secs)
}

// Howard Hinnant's civil-from-days and days-from-civil, as in umx_core::time.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// One adjacency entry: the edge and the node at its other end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Entry {
    pub edge: u32,
    pub other: u32,
}

/// Packed adjacency for one direction: node h's entries are `ent[off[h]..off[h + 1]]`, in edge id order.
#[derive(Clone, Debug, Default)]
pub(crate) struct Csr {
    off: Vec<u32>,
    ent: Vec<Entry>,
}

impl Csr {
    /// Rebuilds in full from the edge columns (`key[e]` owns edge e in this direction, `other[e]` is the far
    /// end). Edges are in id order, so each node's entries come out in edge id order. Reuses the arrays.
    pub fn build(&mut self, nodes: usize, key: &[u32], other: &[u32]) {
        self.off.clear();
        self.off.resize(nodes + 1, 0);
        for (&k, &o) in key.iter().zip(other) {
            if (k | o) & crate::EXTERNAL == 0 {
                self.off[k as usize + 1] += 1;
            }
        }
        for i in 0..nodes {
            self.off[i + 1] += self.off[i];
        }
        self.ent.clear();
        self.ent.resize(self.off[nodes] as usize, Entry::default());
        // Use off[k] as node k's cursor; afterwards it holds k's end, which is off[k + 1] before the shift back.
        for (e, (&k, &o)) in key.iter().zip(other).enumerate() {
            if (k | o) & crate::EXTERNAL != 0 {
                continue;
            }
            let at = &mut self.off[k as usize];
            self.ent[*at as usize] = Entry {
                edge: e as u32,
                other: o,
            };
            *at += 1;
        }
        self.off.copy_within(0..nodes, 1);
        self.off[0] = 0;
    }

    /// A node with no edges was appended: what a rebuild would give, without one.
    pub fn push_node(&mut self) {
        let end = self.off.last().copied().unwrap_or(0);
        if self.off.is_empty() {
            self.off.push(0);
        }
        self.off.push(end);
    }

    /// The last node, which has no edges, was removed.
    pub fn pop_node(&mut self) {
        self.off.pop();
    }

    #[inline]
    pub fn of(&self, h: u32) -> &[Entry] {
        &self.ent[self.off[h as usize] as usize..self.off[h as usize + 1] as usize]
    }

    pub fn heap_bytes(&self) -> usize {
        self.off.capacity() * size_of::<u32>() + self.ent.capacity() * size_of::<Entry>()
    }

    pub fn shrink(&mut self) {
        self.off.shrink_to_fit();
        self.ent.shrink_to_fit();
    }
}

/// Custom fields of one record, in name order.
pub(crate) type Fields = Box<[(Box<str>, Value)]>;

pub(crate) fn fields_from(map: BTreeMap<String, Value>) -> Fields {
    map.into_iter()
        .map(|(k, v)| (k.into_boxed_str(), v))
        .collect()
}

pub(crate) fn fields_to(fields: &[(Box<str>, Value)]) -> BTreeMap<String, Value> {
    // Already in name order: inserting one by one skips `collect`'s sort and its temporary Vec.
    let mut map = BTreeMap::new();
    for (k, v) in fields {
        map.insert(String::from(&**k), v.clone());
    }
    map
}

pub(crate) fn field_in<'a>(fields: &'a [(Box<str>, Value)], name: &str) -> Option<&'a Value> {
    fields
        .binary_search_by(|(k, _)| (**k).cmp(name))
        .ok()
        .map(|i| &fields[i].1)
}

pub(crate) fn fields_bytes(fields: &[(Box<str>, Value)]) -> usize {
    size_of_val(fields)
        + fields
            .iter()
            .map(|(k, v)| k.len() + value_bytes(v))
            .sum::<usize>()
}

/// Heap bytes held by one JSON value (serde_json's map is a `BTreeMap<String, Value>`).
pub(crate) fn value_bytes(v: &Value) -> usize {
    match v {
        Value::String(s) => s.capacity(),
        Value::Array(a) => {
            a.capacity() * size_of::<Value>() + a.iter().map(value_bytes).sum::<usize>()
        }
        Value::Object(m) => {
            btree_bytes::<String, Value>(m.len())
                + m.iter()
                    .map(|(k, v)| k.capacity() + value_bytes(v))
                    .sum::<usize>()
        }
        _ => 0,
    }
}

/// Heap bytes of a `BTreeMap<K, V>` with `len` entries: std's nodes hold up to 11 entries; a leaf is a parent
/// pointer, two `u16`s and the key and value arrays, and an internal node adds 12 child pointers. Exact up to
/// 11 entries, close above (tested against the allocator in umx-bench).
pub(crate) fn btree_bytes<K, V>(len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let align = core::mem::align_of::<K>()
        .max(core::mem::align_of::<V>())
        .max(8);
    let leaf = (8 + 4 + 11 * (size_of::<K>() + size_of::<V>())).next_multiple_of(align);
    if len <= 11 {
        return leaf;
    }
    // Keys arriving in order (a load) split full nodes 6 and 5, so about 6 entries per leaf and one key per leaf
    // in the level above.
    let leaves = len.div_ceil(7);
    leaves * leaf + leaves.div_ceil(7) * (leaf + 12 * 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_is_held_as_seconds_only_when_it_writes_back_the_same() {
        assert_eq!(canonical_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(canonical_secs("2026-11-09T00:00:00Z"), Some(1_794_182_400));
        assert_eq!(&iso(1_794_182_400), b"2026-11-09T00:00:00Z");
        assert_eq!(&iso(u32::MAX - 1), b"2106-02-07T06:28:14Z");
        for odd in [
            "",
            "2026-11-09T00:00:00.5Z",
            "2026-11-09T00:00:00+00:00",
            "2026-11-09 00:00:00Z",
            "1969-12-31T23:59:59Z",
            "2026-02-30T00:00:00Z",
            "2026-11-09T24:00:00Z",
            "2106-02-07T06:28:15Z",
            "2200-01-01T00:00:00Z",
            "２026-11-09T00:00:0Z",
        ] {
            assert_eq!(canonical_secs(odd), None, "{odd}");
        }
        let mut t = Times::default();
        t.push(Id(1), "2026-11-09T00:00:00.5Z");
        t.push(Id(2), "2026-11-09T00:00:01Z");
        assert_eq!(
            (t.get(0, Id(1)).as_str(), t.get(1, Id(2)).as_str()),
            ("2026-11-09T00:00:00.5Z", "2026-11-09T00:00:01Z")
        );
        t.set(0, Id(1), "2026-11-09T00:00:02Z");
        assert!(t.odd.is_empty());
        // secs: the second date_span reads from the text written back, or none for an exception (D71 step 12).
        t.push(Id(3), "1969-12-31T23:59:59Z");
        for (i, id) in [(0, Id(1)), (1, Id(2))] {
            let s = i64::from(t.secs(i).unwrap());
            let d = umx_core::time::date_span(&t.get(i, id)).unwrap();
            assert_eq!((d.start, d.end), (s, s + 1));
        }
        assert_eq!(t.secs(2), None);
    }

    #[test]
    fn a_name_column_widens_instead_of_refusing() {
        let mut c = NameCol::default();
        c.push(7);
        c.push(u32::from(u16::MAX));
        assert!(!c.is_wide());
        c.push(u32::from(u16::MAX) + 1);
        assert!(c.is_wide());
        assert_eq!((c.get(0), c.get(1), c.get(2)), (7, 65_535, 65_536));
        let mut c = NameCol::default();
        c.push(1);
        c.set(0, 70_000);
        assert_eq!(c.get(0), 70_000);
    }

    #[test]
    fn names_keep_use_counts_per_side() {
        let mut n = Names::default();
        let (b, a) = (n.intern("blocks"), n.intern("actor"));
        assert_eq!(n.intern("blocks"), b);
        n.edge_use(b, true);
        n.node_use(a, true);
        assert!(n.used_by_edge(b) && !n.used_by_node(b) && n.used_by_node(a));
        n.edge_use(b, false);
        assert!(!n.used_by_edge(b));
        let order: Vec<&str> = n.in_name_order().map(|x| n.text(x)).collect();
        assert_eq!(order, ["actor", "blocks"]);
    }
}
