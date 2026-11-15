//! This machine's secret allows (req-49, ws004 s08): `$UMX_HOME/allows/secret-allows.json`, read and written through a
//! LocalState port instance of its own, so it can never break the policy record (security, s08). Its own folder too:
//! a file in `state/` would create that folder before the policy record exists, and a state folder without its
//! record is refused (every graph secured). Each allow says
//! that one field's current text, which the secret scan flags, may go to a hosted embedder on this machine anyway.
//! It is keyed by the hash of that text: any change to the text holds it again.
//!
//! Format: `{"umx_secret_allows": 1, "allows": [{"graph", "node", "field", "hash", "allowed_at"}]}`, sorted. A file
//! that does not parse, or is newer, means no allows (held); an entry that does not parse is dropped.

use crate::id::Id;
use crate::vectors::TextHash;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ALLOWS_VERSION: u32 = 1;
/// The owner-only folder under `$UMX_HOME` that holds the allows file.
pub const ALLOWS_DIR: &str = "allows";
pub const ALLOWS_FILE: &str = "secret-allows.json";
/// At most this many allows (security, s08). Past it, or past [`MAX_ALLOWS_BYTES`], a new allow is refused.
pub const MAX_ALLOWS: usize = 256;
/// The file never grows past this (R120-2): 4 KiB under the small-file reader's cap, so a full file still reads.
pub const MAX_ALLOWS_BYTES: usize = crate::ports::config_files::SMALL_FILE_CAP as usize - 4096;
/// Longest field name an allow records (longer ones are not umx field names).
const MAX_FIELD: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SecretAllow {
    pub graph: Id,
    pub node: Id,
    pub field: String,
    /// [`TextHash`] of the field's raw value, as 32 hex digits.
    pub hash: String,
    pub allowed_at: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SecretAllows {
    pub allows: Vec<SecretAllow>,
}

impl SecretAllows {
    /// The allows in `bytes`, or None when the file cannot be used (does not parse, or a newer or missing version).
    /// Entries that do not parse are dropped, never fatal.
    pub fn decode(bytes: &[u8]) -> Option<SecretAllows> {
        let v: Value = serde_json::from_slice(bytes).ok()?;
        let version = v.get("umx_secret_allows")?.as_u64()?;
        if version == 0 || version > u64::from(ALLOWS_VERSION) {
            return None;
        }
        let allows = v
            .get("allows")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|e| serde_json::from_value::<SecretAllow>(e.clone()).ok())
                    .filter(|e| {
                        TextHash::parse(&e.hash).is_some()
                            && !e.field.is_empty()
                            && e.field.len() <= MAX_FIELD
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(SecretAllows { allows })
    }

    /// Sorted and without repeats, so the same allows always give the same bytes. Compact, one allow a line.
    pub fn encode(&self) -> Vec<u8> {
        let mut allows = self.allows.clone();
        allows.sort();
        allows.dedup_by(|a, b| {
            (a.graph, a.node, &a.field, &a.hash) == (b.graph, b.node, &b.field, &b.hash)
        });
        let lines: Vec<String> = allows
            .iter()
            .map(|a| serde_json::to_string(a).expect("an allow always serialises"))
            .collect();
        alloc::format!(
            "{{\"umx_secret_allows\": {ALLOWS_VERSION}, \"allows\": [\n{}\n]}}\n",
            lines.join(",\n")
        )
        .into_bytes()
    }

    /// The bytes to write, or None when the allows are more than the file may hold ([`MAX_ALLOWS`] entries or
    /// [`MAX_ALLOWS_BYTES`] bytes): a file the reader would refuse is never written.
    pub fn encode_within_caps(&self) -> Option<Vec<u8>> {
        let bytes = self.encode();
        (self.allows.len() <= MAX_ALLOWS && bytes.len() <= MAX_ALLOWS_BYTES).then_some(bytes)
    }

    /// Whether `field` of `node` is allowed with exactly the text hashed `hash`.
    pub fn allows(&self, graph: Id, node: Id, field: &str, hash: TextHash) -> bool {
        let hex = alloc::format!("{hash}");
        self.allows
            .iter()
            .any(|a| a.graph == graph && a.node == node && a.field == field && a.hash == hex)
    }

    /// Records an allow, replacing any for the same field (an older text's allow is dead anyway). False if it was
    /// already there.
    pub fn add(&mut self, graph: Id, node: Id, field: &str, hash: TextHash, at: &str) -> bool {
        if self.allows(graph, node, field, hash) {
            return false;
        }
        self.allows
            .retain(|a| !(a.graph == graph && a.node == node && a.field == field));
        self.allows.push(SecretAllow {
            graph,
            node,
            field: field.into(),
            hash: alloc::format!("{hash}"),
            allowed_at: at.into(),
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_round_trip_and_bad_entries_or_files_hold() {
        let (g, n) = (Id(1), Id(2));
        let h = TextHash::of("x");
        let mut a = SecretAllows::default();
        assert!(a.add(g, n, "note", h, "2026-11-11T00:00:00Z"));
        assert!(!a.add(g, n, "note", h, "2026-11-11T00:00:01Z"));
        let back = SecretAllows::decode(&a.encode()).unwrap();
        assert!(back.allows(g, n, "note", h));
        assert!(
            !back.allows(g, n, "note", TextHash::of("y")),
            "another text is held"
        );
        assert!(!back.allows(g, n, "title", h));
        // A new text's allow replaces the old one.
        a.add(g, n, "note", TextHash::of("y"), "t");
        assert_eq!(a.allows.len(), 1);
        // A malformed entry is dropped; the rest stand.
        let text = alloc::format!(
            r#"{{"umx_secret_allows":1,"allows":[{{"graph":"{g}","node":"{n}","field":"note","hash":"zz","allowed_at":"t"}},{{"graph":"{g}","node":"{n}","field":"title","hash":"{h}","allowed_at":"t"}}]}}"#
        );
        let mixed = SecretAllows::decode(text.as_bytes()).unwrap();
        assert_eq!(mixed.allows.len(), 1);
        assert!(mixed.allows(g, n, "title", h));
        assert_eq!(SecretAllows::decode(b"{"), None);
        assert_eq!(
            SecretAllows::decode(br#"{"umx_secret_allows":2,"allows":[]}"#),
            None
        );
        assert_eq!(SecretAllows::decode(br#"{"allows":[]}"#), None);
    }

    #[test]
    fn caps_hold_by_count_and_by_bytes() {
        let mut a = SecretAllows::default();
        for n in 0..MAX_ALLOWS as u128 {
            a.add(
                Id(1),
                Id(n),
                "note",
                TextHash::of("x"),
                "2026-11-11T00:00:00.000Z",
            );
        }
        assert!(
            a.encode_within_caps()
                .is_some_and(|b| b.len() <= MAX_ALLOWS_BYTES)
        );
        a.add(Id(1), Id(9999), "note", TextHash::of("x"), "t");
        assert_eq!(a.encode_within_caps(), None, "one past the count");
        // Fewer entries, each with the longest field a file may hold: over the bytes first.
        let long = "f".repeat(MAX_FIELD);
        let mut b = SecretAllows::default();
        let mut n = 0;
        while b.encode().len() <= MAX_ALLOWS_BYTES {
            b.add(
                Id(1),
                Id(n),
                &long,
                TextHash::of("x"),
                "2026-11-11T00:00:00.000Z",
            );
            n += 1;
        }
        assert!(b.allows.len() < MAX_ALLOWS);
        assert_eq!(b.encode_within_caps(), None);
    }
}
