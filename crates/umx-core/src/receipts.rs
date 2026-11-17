//! Batch idempotency receipts (ws003 s09): what `umx batch --idempotency-key` keeps in a graph folder's
//! `receipts.jsonl`, so a re-sent batch gets the receipt of its first run back and nothing is written. Pure: the
//! caller reads and writes the file through the GraphFiles port and passes the time in; nothing here reads a clock.
//!
//! The file holds no caller text. A key is kept as its digest and a batch's input as its digest; a receipt is the ids
//! its refs bound, in the order the batch defined them (a replay has the same input, so it takes the ref names from
//! its own lines), and a count per op. Line 1 names the format and the graph; then one record per key, sorted by when
//! it was recorded, then by key, so the same records always encode to the same bytes (req-02).

use crate::id::Id;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use serde::{Deserialize, Serialize};

const DAY_MS: u64 = 86_400_000;

/// The receipts format this umx writes, and the newest it reads.
pub const RECEIPTS_FORMAT: u32 = 1;

/// Every live receipt younger than this is kept (graph-storage, 2026-11-17). Receipts serve a retry after a lost
/// reply: a timeout, a daemon restart, an agent crash or compaction, a fleet resuming after a reboot. Those come
/// within minutes or hours, and a resumed agent may come back after a weekend; 7 days covers a weekend and a resume.
pub const KEEP_AGE_MS: u64 = 7 * DAY_MS;
/// At least the most recent this many live receipts are kept, whatever their age: the union with [`KEEP_AGE_MS`],
/// never the fewer of the two, so a quiet graph keeps its last receipts past a week (graph-storage, 2026-11-17).
pub const KEEP_COUNT: usize = 1_000;
/// The most bytes umx writes to the file: past it, the oldest live receipts are dropped first, whatever the two
/// bounds above keep. Every keyed batch, one that changed nothing included, reads, parses and rewrites the whole file
/// and adds one git blob; 4 MiB parses in about 10 to 15 ms, small next to a batch, and holds about 13,000 typical
/// receipts, over a week of a busy fleet (graph-storage, 2026-11-17).
pub const WRITE_TARGET: usize = 4 << 20;
/// The most bytes umx reads (the port's cap for the file): 4 times [`WRITE_TARGET`], room for a merge of two
/// machines' files or a newer writer's larger records. A bigger file is unreadable for keyed batches.
pub const READ_CAP: u64 = 16 << 20;
/// A dropped key is remembered this long after it was dropped, only so the reply can say the key expired.
pub const DROPPED_AGE_MS: u64 = 30 * DAY_MS;
/// And at most this many dropped keys (about 130 bytes each, 1.3 MB at most).
pub const DROPPED_COUNT: usize = 10_000;

/// The longest idempotency key.
pub const MAX_KEY_LEN: usize = 128;
/// The key rule, as a usage error states it (never quoting the key).
pub const KEY_RULE: &str = "an idempotency key is 1 to 128 of A-Z a-z 0-9 . _ : -";
/// The op labels a receipt counts: a count under any other label is a file umx did not write.
pub const OP_LABELS: [&str; 9] = [
    "node add",
    "edge add",
    "node edit",
    "node note",
    "node status",
    "edge delete",
    "edge edit",
    "evidence add",
    "evidence remove",
];

/// True when `key` is 1 to [`MAX_KEY_LEN`] of `A-Z a-z 0-9 . _ : -`.
pub fn check_key(key: &str) -> bool {
    (1..=MAX_KEY_LEN).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

fn sha256_hex(tag: &[u8], body: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut h = Sha256::new();
    h.update(tag);
    h.update(body);
    let mut out = String::with_capacity(64);
    for byte in h.finalize() {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 15)]));
    }
    out
}

/// The digest the file keeps for a key: never the key itself, which is caller text. Not secrecy: a short key can be
/// guessed from it.
pub fn key_digest(key: &str) -> String {
    sha256_hex(b"umx-batch-key/1\n", key.as_bytes())
}

/// The digest of a batch's canonical input (umx-protocol builds the bytes from the wire request).
pub fn input_digest(canonical: &[u8]) -> String {
    sha256_hex(b"umx-batch-input/1\n", canonical)
}

/// True when `s` is a digest as [`key_digest`] and [`input_digest`] write it: 64 lower-case hex digits.
pub fn is_digest(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// One op's lines in a batch, and how many of them found what they would add already there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpCount {
    pub lines: u32,
    pub existed: u32,
}

/// What a keyed batch did, as kept: the ids its refs bound in definition order, and its counts by op label.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Receipt {
    pub refs: Vec<Id>,
    pub counts: BTreeMap<String, OpCount>,
}

/// A key's record: a live receipt, or only the time it was dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Live { input: String, receipt: Receipt },
    Dropped { dropped: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// The key's digest ([`key_digest`]).
    pub key: String,
    /// When the receipt was recorded, in epoch milliseconds.
    pub at: u64,
    pub state: State,
}

/// What a key finds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lookup<'a> {
    None,
    Live {
        at: u64,
        input: &'a str,
        receipt: &'a Receipt,
    },
    Dropped {
        at: u64,
        dropped: u64,
    },
}

/// Why a file is not read. No variant quotes the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// Its format is newer than [`RECEIPTS_FORMAT`].
    Newer(u32),
    /// Its header names no format umx writes.
    Unknown,
    /// Its header names another graph id (a file copied in from another graph).
    OtherGraph,
    /// This line (1-based) is not a record umx writes.
    Invalid(usize),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::Newer(v) => write!(
                f,
                "it is receipts format {v}, newer than this umx reads ({RECEIPTS_FORMAT})"
            ),
            ReadError::Unknown => f.write_str("its first line is not a receipts header"),
            ReadError::OtherGraph => f.write_str("it names another graph"),
            ReadError::Invalid(line) => write!(f, "line {line} is not a receipt record"),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    umx_receipts: u32,
    graph_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Line {
    key: String,
    at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refs: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    counts: Option<BTreeMap<String, OpCount>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dropped: Option<u64>,
}

impl Record {
    fn line(&self) -> Line {
        match &self.state {
            State::Live { input, receipt } => Line {
                key: self.key.clone(),
                at: self.at,
                input: Some(input.clone()),
                refs: Some(receipt.refs.iter().map(|id| id.to_text()).collect()),
                counts: Some(receipt.counts.clone()),
                dropped: None,
            },
            State::Dropped { dropped } => Line {
                key: self.key.clone(),
                at: self.at,
                input: None,
                refs: None,
                counts: None,
                dropped: Some(*dropped),
            },
        }
    }

    fn encoded_len(&self) -> usize {
        serde_json::to_vec(&self.line()).map_or(0, |v| v.len()) + 1
    }

    fn from_line(line: Line) -> Option<Record> {
        if !is_digest(&line.key) {
            return None;
        }
        let state = match line {
            Line {
                input: Some(input),
                refs: Some(refs),
                counts: Some(counts),
                dropped: None,
                ..
            } => {
                let refs = refs
                    .iter()
                    .map(|t| t.parse::<Id>().ok().filter(|id| id.to_text() == *t))
                    .collect::<Option<Vec<Id>>>()?;
                if !is_digest(&input) || counts.keys().any(|k| !OP_LABELS.contains(&k.as_str())) {
                    return None;
                }
                State::Live {
                    input,
                    receipt: Receipt { refs, counts },
                }
            }
            Line {
                input: None,
                refs: None,
                counts: None,
                dropped: Some(dropped),
                ..
            } => State::Dropped { dropped },
            _ => return None,
        };
        Some(Record {
            key: line.key,
            at: line.at,
            state,
        })
    }
}

/// One graph folder's receipts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipts {
    graph_id: Id,
    records: Vec<Record>,
}

impl Receipts {
    /// No receipts yet (the file is absent).
    pub fn new(graph_id: Id) -> Receipts {
        Receipts {
            graph_id,
            records: Vec::new(),
        }
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// Reads the file of the graph `graph_id`. Records may come in any order (a merge); one key twice is refused.
    pub fn decode(bytes: &[u8], graph_id: Id) -> Result<Receipts, ReadError> {
        let text = core::str::from_utf8(bytes).map_err(|_| ReadError::Invalid(1))?;
        let body = text.strip_suffix('\n').ok_or(ReadError::Invalid(1))?;
        let mut lines = body.split('\n');
        let first = lines.next().unwrap_or_default();
        let version = serde_json::from_str::<serde_json::Value>(first)
            .ok()
            .and_then(|v| v.get("umx_receipts").and_then(serde_json::Value::as_u64))
            .ok_or(ReadError::Unknown)?;
        if version > u64::from(RECEIPTS_FORMAT) {
            return Err(ReadError::Newer(u32::try_from(version).unwrap_or(u32::MAX)));
        }
        let header: Header = serde_json::from_str(first).map_err(|_| ReadError::Unknown)?;
        if header.umx_receipts != RECEIPTS_FORMAT {
            return Err(ReadError::Unknown);
        }
        if header.graph_id != graph_id.to_text() {
            return Err(ReadError::OtherGraph);
        }
        let mut records: Vec<Record> = Vec::new();
        // The keys seen so far, so a repeated key is found in log time: a file at the read cap holds over 160,000
        // records, and a scan of every earlier record per line would cost billions of comparisons (review B1).
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for (i, line) in lines.enumerate() {
            let n = i + 2;
            let record = serde_json::from_str::<Line>(line)
                .ok()
                .and_then(Record::from_line)
                .ok_or(ReadError::Invalid(n))?;
            if !seen.insert(record.key.clone()) {
                return Err(ReadError::Invalid(n));
            }
            records.push(record);
        }
        let mut out = Receipts { graph_id, records };
        out.sort();
        Ok(out)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = serde_json::to_vec(&Header {
            umx_receipts: RECEIPTS_FORMAT,
            graph_id: self.graph_id.to_text(),
        })
        .expect("a header always serialises");
        out.push(b'\n');
        for r in &self.records {
            out.extend(serde_json::to_vec(&r.line()).expect("a record always serialises"));
            out.push(b'\n');
        }
        out
    }

    fn sort(&mut self) {
        self.records
            .sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.key.cmp(&b.key)));
    }

    /// What `key` (a digest) finds.
    pub fn find(&self, key: &str) -> Lookup<'_> {
        match self.records.iter().find(|r| r.key == key) {
            None => Lookup::None,
            Some(Record {
                at,
                state: State::Live { input, receipt },
                ..
            }) => Lookup::Live {
                at: *at,
                input,
                receipt,
            },
            Some(Record {
                at,
                state: State::Dropped { dropped },
                ..
            }) => Lookup::Dropped {
                at: *at,
                dropped: *dropped,
            },
        }
    }

    /// Records a receipt for `key` (a digest), replacing any record it had.
    pub fn record(&mut self, key: String, at: u64, input: String, receipt: Receipt) {
        self.records.retain(|r| r.key != key);
        self.records.push(Record {
            key,
            at,
            state: State::Live { input, receipt },
        });
        self.sort();
    }

    /// Applies the bounds at `now`: a live receipt older than [`KEEP_AGE_MS`] and outside the most recent
    /// [`KEEP_COUNT`] is dropped; then the oldest live ones until the file fits [`WRITE_TARGET`]; dropped keys older
    /// than [`DROPPED_AGE_MS`] are forgotten, and past [`DROPPED_COUNT`] the longest dropped go first. A record dated
    /// after `now` (another machine's clock) counts as age 0.
    pub fn evict(&mut self, now: u64) {
        let age = |t: u64| now.saturating_sub(t);
        let mut live: Vec<usize> = (0..self.records.len())
            .filter(|i| matches!(self.records[*i].state, State::Live { .. }))
            .collect();
        // Most recent first: records are sorted oldest first.
        live.reverse();
        for (rank, i) in live.into_iter().enumerate() {
            if rank >= KEEP_COUNT && age(self.records[i].at) >= KEEP_AGE_MS {
                self.records[i].state = State::Dropped { dropped: now };
            }
        }
        self.forget(now);
        let mut size = self.encode_len();
        for i in 0..self.records.len() {
            if size <= WRITE_TARGET {
                break;
            }
            if matches!(self.records[i].state, State::Live { .. }) {
                let before = self.records[i].encoded_len();
                self.records[i].state = State::Dropped { dropped: now };
                size = size - before + self.records[i].encoded_len();
            }
        }
        self.forget(now);
    }

    fn encode_len(&self) -> usize {
        self.encode().len()
    }

    fn forget(&mut self, now: u64) {
        self.records.retain(|r| match r.state {
            State::Dropped { dropped } => now.saturating_sub(dropped) < DROPPED_AGE_MS,
            State::Live { .. } => true,
        });
        let mut dropped: Vec<(u64, u64, String)> = self
            .records
            .iter()
            .filter_map(|r| match r.state {
                State::Dropped { dropped } => Some((dropped, r.at, r.key.clone())),
                State::Live { .. } => None,
            })
            .collect();
        if dropped.len() > DROPPED_COUNT {
            dropped.sort();
            let excess = dropped.len() - DROPPED_COUNT;
            let gone: BTreeSet<String> = dropped
                .into_iter()
                .take(excess)
                .map(|(_, _, k)| k)
                .collect();
            self.records.retain(|r| !gone.contains(&r.key));
        }
    }
}

/// The text of a time a receipt carries, for a reply: `2026-11-17T00:42:00.123Z`.
pub fn time_text(ms: u64) -> String {
    let secs = crate::time::iso_from_ms(ms);
    let stem = secs.strip_suffix('Z').unwrap_or(&secs);
    format!("{stem}.{:03}Z", ms % 1000)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    fn graph() -> Id {
        "01JC8XA4H7B3KT9M2NZ5QW6RDE".parse().unwrap()
    }

    fn id(n: u32) -> Id {
        Id(u128::from(n) + (1u128 << 100))
    }

    fn receipt(refs: usize) -> Receipt {
        let mut counts = BTreeMap::new();
        counts.insert(
            "node add".to_string(),
            OpCount {
                lines: refs as u32,
                existed: 0,
            },
        );
        Receipt {
            refs: (0..refs as u32).map(id).collect(),
            counts,
        }
    }

    #[test]
    fn a_key_is_one_to_128_of_the_rule_characters() {
        for good in ["a", "window-08", "run:2026-11-17.batch_3", &"Z".repeat(128)] {
            assert!(check_key(good), "{good}");
        }
        for bad in ["", &"a".repeat(129), "a b", "a/b", "é", "a\n", "a,b"] {
            assert!(!check_key(bad), "{bad}");
        }
    }

    #[test]
    fn digests_are_tagged_hex_and_differ_by_kind() {
        let k = key_digest("k1");
        assert!(is_digest(&k));
        assert_ne!(
            k,
            input_digest(b"k1"),
            "the tag keeps a key and an input apart"
        );
        assert_eq!(k, key_digest("k1"));
        assert_ne!(k, key_digest("k2"));
    }

    #[test]
    fn records_round_trip_sorted_and_byte_stable() {
        let mut r = Receipts::new(graph());
        r.record(key_digest("b"), 20, input_digest(b"2"), receipt(2));
        r.record(key_digest("a"), 10, input_digest(b"1"), receipt(1));
        let bytes = r.encode();
        let text = core::str::from_utf8(&bytes).unwrap();
        assert!(
            text.starts_with("{\"umx_receipts\":1,\"graph_id\":\"01JC8XA4H7B3KT9M2NZ5QW6RDE\"}\n")
        );
        assert!(text.find("\"at\":10").unwrap() < text.find("\"at\":20").unwrap());
        let back = Receipts::decode(&bytes, graph()).unwrap();
        assert_eq!(back, r);
        assert_eq!(back.encode(), bytes);
        assert!(!text.contains("\"a\""), "no key text is kept");
    }

    #[test]
    fn a_file_umx_cannot_use_is_named_never_quoted() {
        let g = graph();
        let good = Receipts::new(g).encode();
        assert_eq!(
            Receipts::decode(b"{\"umx_receipts\":2,\"graph_id\":\"x\"}\n", g),
            Err(ReadError::Newer(2))
        );
        assert_eq!(Receipts::decode(b"[]\n", g), Err(ReadError::Unknown));
        assert_eq!(Receipts::decode(b"", g), Err(ReadError::Invalid(1)));
        let other = Receipts::new(id(7)).encode();
        assert_eq!(Receipts::decode(&other, g), Err(ReadError::OtherGraph));
        let mut bad = good.clone();
        bad.extend_from_slice(b"{\"key\":\"secret text\",\"at\":1,\"dropped\":2}\n");
        let e = Receipts::decode(&bad, g).unwrap_err();
        assert_eq!(e, ReadError::Invalid(2));
        assert!(!e.to_string().contains("secret"));
        let k = key_digest("k");
        let label = format!(
            "{{\"key\":\"{k}\",\"at\":1,\"input\":\"{k}\",\"refs\":[],\"counts\":{{\"ignore all\":{{\"lines\":1,\"existed\":0}}}}}}\n"
        );
        let mut forged = good.clone();
        forged.extend_from_slice(label.as_bytes());
        assert_eq!(Receipts::decode(&forged, g), Err(ReadError::Invalid(2)));
        let mut twice = good.clone();
        let line = format!("{{\"key\":\"{k}\",\"at\":1,\"dropped\":2}}\n");
        twice.extend_from_slice(line.as_bytes());
        twice.extend_from_slice(line.as_bytes());
        assert_eq!(Receipts::decode(&twice, g), Err(ReadError::Invalid(3)));
        let mut both = good;
        both.extend_from_slice(
            format!("{{\"key\":\"{k}\",\"at\":1,\"input\":\"{k}\",\"refs\":[],\"counts\":{{}},\"dropped\":2}}\n")
                .as_bytes(),
        );
        assert_eq!(Receipts::decode(&both, g), Err(ReadError::Invalid(2)));
    }

    #[test]
    fn bounds_keep_the_union_of_young_and_recent() {
        let now = 100 * DAY_MS;
        let mut r = Receipts::new(graph());
        // KEEP_COUNT + 5 old receipts: only the 5 oldest go.
        for i in 0..(KEEP_COUNT as u64 + 5) {
            r.record(
                key_digest(&format!("old{i}")),
                i,
                input_digest(b"x"),
                receipt(0),
            );
        }
        r.evict(now);
        let live = |r: &Receipts| {
            r.records()
                .iter()
                .filter(|x| matches!(x.state, State::Live { .. }))
                .count()
        };
        assert_eq!(live(&r), KEEP_COUNT);
        assert!(matches!(
            r.find(&key_digest("old0")),
            Lookup::Dropped { at: 0, dropped } if dropped == now
        ));
        assert!(matches!(r.find(&key_digest("old5")), Lookup::Live { .. }));
        // Receipts younger than KEEP_AGE_MS are all kept, past the count too.
        let mut young = Receipts::new(graph());
        for i in 0..(KEEP_COUNT as u64 + 5) {
            young.record(
                key_digest(&format!("young{i}")),
                now - KEEP_AGE_MS + 1 + i,
                input_digest(b"x"),
                receipt(0),
            );
        }
        young.evict(now);
        assert_eq!(live(&young), KEEP_COUNT + 5);
    }

    #[test]
    fn a_future_record_counts_as_age_zero() {
        let mut r = Receipts::new(graph());
        for i in 0..(KEEP_COUNT as u64 + 1) {
            r.record(
                key_digest(&format!("k{i}")),
                10 * DAY_MS + i,
                input_digest(b"x"),
                receipt(0),
            );
        }
        r.evict(DAY_MS);
        assert!(
            r.records()
                .iter()
                .all(|x| matches!(x.state, State::Live { .. }))
        );
    }

    #[test]
    fn the_write_target_drops_the_oldest_live_first() {
        let now = 1_000;
        let mut r = Receipts::new(graph());
        // Each receipt with 10,000 refs is about 290 KB: 20 of them are past 4 MiB.
        for i in 0..20u64 {
            r.record(
                key_digest(&format!("big{i}")),
                i,
                input_digest(b"x"),
                receipt(10_000),
            );
        }
        r.evict(now);
        assert!(r.encode().len() <= WRITE_TARGET);
        assert!(matches!(
            r.find(&key_digest("big0")),
            Lookup::Dropped { .. }
        ));
        assert!(matches!(r.find(&key_digest("big19")), Lookup::Live { .. }));
    }

    #[test]
    fn dropped_keys_are_forgotten_by_age_and_count() {
        let mut r = Receipts::new(graph());
        r.records.push(Record {
            key: key_digest("gone"),
            at: 0,
            state: State::Dropped { dropped: 0 },
        });
        r.records.push(Record {
            key: key_digest("kept"),
            at: 0,
            state: State::Dropped { dropped: 1 },
        });
        r.evict(DROPPED_AGE_MS);
        assert_eq!(r.find(&key_digest("gone")), Lookup::None);
        assert!(matches!(
            r.find(&key_digest("kept")),
            Lookup::Dropped { .. }
        ));
        let mut many = Receipts::new(graph());
        for i in 0..(DROPPED_COUNT as u64 + 3) {
            many.records.push(Record {
                key: key_digest(&format!("d{i}")),
                at: i,
                state: State::Dropped { dropped: i },
            });
        }
        many.evict(DROPPED_COUNT as u64 + 3);
        assert_eq!(many.records().len(), DROPPED_COUNT);
        assert_eq!(many.find(&key_digest("d2")), Lookup::None);
        assert!(matches!(
            many.find(&key_digest("d3")),
            Lookup::Dropped { .. }
        ));
    }

    #[test]
    fn a_record_replaces_the_keys_old_one() {
        let mut r = Receipts::new(graph());
        let k = key_digest("k");
        r.records.push(Record {
            key: k.clone(),
            at: 1,
            state: State::Dropped { dropped: 2 },
        });
        r.record(k.clone(), 5, input_digest(b"y"), receipt(1));
        assert_eq!(r.records().len(), 1);
        assert!(matches!(r.find(&k), Lookup::Live { at: 5, .. }));
        assert_eq!(time_text(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(time_text(86_401_500), "1970-01-02T00:00:01.500Z");
    }
}
