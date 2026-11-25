//! Source hashes (usage 220): when a write adds an evidence pointer or a resource locator whose source is a local
//! file, umx stores the file's sha256, its length in bytes and, for a transcript, the highest sequence number it
//! holds, so `umx check --sources` can tell later whether the file changed. Pure: the SourceReader port streams the
//! bytes into a [`Digester`]; nothing here reads a file, and nothing keeps its content.

use crate::models::FileHasher;
use alloc::string::String;
use alloc::vec::Vec;
use serde_json::Value;

/// The keys a hashed source adds to a stored evidence pointer (a locator has its own fields). Two pointers that
/// differ only in these are the same pointer.
pub const HASH_KEYS: &[&str] = &["hash", "hash_state", "length", "seq_max", "hash_accepted"];

/// The largest source umx reads to hash: past it the state is `too_large` and nothing more is read.
pub const SOURCE_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// The longest line read for its sequence number; a longer one is still hashed, but its number is not read.
pub const SEQ_LINE_MAX: usize = 1 << 20;

/// Why a source has, or has not, a hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashState {
    /// Read and hashed when the reference was added (or last refreshed).
    Hashed,
    /// A URL or a umx address: never fetched.
    Remote,
    /// A local path with no file there.
    Missing,
    /// A local path umx could not read: a folder, or the system refused.
    Unreadable,
    /// A local path that leads outside the roots umx may read (a symlink out): never read.
    Outside,
    /// A file over [`SOURCE_MAX_BYTES`]: not read past the cap.
    TooLarge,
}

impl HashState {
    pub const ALL: [HashState; 6] = [
        HashState::Hashed,
        HashState::Remote,
        HashState::Missing,
        HashState::Unreadable,
        HashState::Outside,
        HashState::TooLarge,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            HashState::Hashed => "hashed",
            HashState::Remote => "remote",
            HashState::Missing => "missing",
            HashState::Unreadable => "unreadable",
            HashState::Outside => "outside",
            HashState::TooLarge => "too_large",
        }
    }

    pub fn parse(text: &str) -> Option<HashState> {
        HashState::ALL.into_iter().find(|s| s.as_str() == text)
    }

    /// A local file umx tried to read: `check --sources` reads it again.
    pub fn is_local(self) -> bool {
        self != HashState::Remote
    }
}

/// A transcript format whose `seq=` selectors umx can count, from the source's extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transcript {
    /// One JSON object per line; `seq_max` is the largest whole-number `seq` among them.
    Ndjson,
    /// WebVTT captions; `seq_max` is the number of cues (a cue is a line holding `-->`).
    Vtt,
}

impl Transcript {
    /// The transcript format of a path or `root:` source, by its last part's extension (any case); None otherwise.
    pub fn of(source: &str) -> Option<Transcript> {
        let last = source.rsplit('/').next().unwrap_or(source);
        let (stem, ext) = last.rsplit_once('.')?;
        if stem.is_empty() {
            return None;
        }
        match ext.to_ascii_lowercase().as_str() {
            "ndjson" => Some(Transcript::Ndjson),
            "vtt" => Some(Transcript::Vtt),
            _ => None,
        }
    }
}

/// What reading a source gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceDigest {
    /// `sha256:<64 lower-case hex digits>`.
    pub hash: String,
    pub length: u64,
    /// For a transcript only: see [`Transcript`]. None when no line held a number.
    pub seq_max: Option<u64>,
}

/// Takes a source's bytes in any chunks, holding at most one line (up to [`SEQ_LINE_MAX`]) of a transcript.
pub struct Digester {
    hasher: FileHasher,
    length: u64,
    transcript: Option<Transcript>,
    line: Vec<u8>,
    long_line: bool,
    seq_max: Option<u64>,
}

impl Digester {
    pub fn new(transcript: Option<Transcript>) -> Digester {
        Digester {
            hasher: FileHasher::default(),
            length: 0,
            transcript,
            line: Vec::new(),
            long_line: false,
            seq_max: None,
        }
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.hasher.update(bytes);
        self.length += bytes.len() as u64;
        if self.transcript.is_none() {
            return;
        }
        let mut rest = bytes;
        while !rest.is_empty() {
            match rest.iter().position(|b| *b == b'\n') {
                Some(end) => {
                    self.take(&rest[..end]);
                    self.end_line();
                    rest = &rest[end + 1..];
                }
                None => {
                    self.take(rest);
                    rest = &[];
                }
            }
        }
    }

    fn take(&mut self, part: &[u8]) {
        if self.long_line {
            return;
        }
        if self.line.len() + part.len() > SEQ_LINE_MAX {
            self.long_line = true;
            self.line = Vec::new();
            return;
        }
        self.line.extend_from_slice(part);
    }

    fn end_line(&mut self) {
        if !self.long_line {
            let line = core::mem::take(&mut self.line);
            match self.transcript {
                Some(Transcript::Ndjson) => {
                    let seq = serde_json::from_slice::<Value>(&line)
                        .ok()
                        .and_then(|v| v.get("seq").and_then(Value::as_u64));
                    if let Some(seq) = seq {
                        self.seq_max = Some(self.seq_max.map_or(seq, |m| m.max(seq)));
                    }
                }
                Some(Transcript::Vtt) if line.windows(3).any(|w| w == b"-->") => {
                    self.seq_max = Some(self.seq_max.unwrap_or(0) + 1);
                }
                Some(Transcript::Vtt) | None => {}
            }
        }
        self.line.clear();
        self.long_line = false;
    }

    /// The bytes taken so far.
    pub fn length(&self) -> u64 {
        self.length
    }

    pub fn finish(mut self) -> SourceDigest {
        if self.transcript.is_some() && (!self.line.is_empty() || self.long_line) {
            self.end_line();
        }
        if self.transcript == Some(Transcript::Vtt) && self.seq_max.is_none() {
            self.seq_max = Some(0);
        }
        SourceDigest {
            hash: alloc::format!("sha256:{}", self.hasher.hex()),
            length: self.length,
            seq_max: self.seq_max,
        }
    }
}

/// The digest of `bytes` read at once (the fake reader and tests).
pub fn digest(bytes: &[u8], transcript: Option<Transcript>) -> SourceDigest {
    let mut d = Digester::new(transcript);
    d.update(bytes);
    d.finish()
}

/// Sets a stored evidence pointer's hash keys: `hash_state` always, `hash`, `length` and `seq_max` when it was read.
/// `accepted` (a refresh) records a state without a hash as seen, so `check --sources` stops reporting it until it
/// changes; a write never sets it.
pub fn stamp_pointer(
    pointer: &mut Value,
    state: HashState,
    digest: Option<&SourceDigest>,
    accepted: bool,
) {
    let Some(map) = pointer.as_object_mut() else {
        return;
    };
    map.insert(
        "hash".into(),
        digest.map_or(Value::Null, |d| Value::String(d.hash.clone())),
    );
    map.insert("hash_state".into(), Value::String(state.as_str().into()));
    map.remove("length");
    map.remove("seq_max");
    map.remove("hash_accepted");
    if accepted && state != HashState::Hashed && state != HashState::Remote {
        map.insert("hash_accepted".into(), Value::Bool(true));
    }
    if let Some(d) = digest {
        map.insert("length".into(), Value::from(d.length));
        if let Some(seq) = d.seq_max {
            map.insert("seq_max".into(), Value::from(seq));
        }
    }
}

/// Whether a refresh accepted the stored pointer's state without a hash.
pub fn pointer_accepted(pointer: &Value) -> bool {
    pointer.get("hash_accepted") == Some(&Value::Bool(true))
}

/// A stored pointer's hash state; None for one stored before umx hashed sources.
pub fn pointer_state(pointer: &Value) -> Option<HashState> {
    pointer
        .get("hash_state")
        .and_then(Value::as_str)
        .and_then(HashState::parse)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_digest_is_the_same_in_any_chunks() {
        let text = b"{\"seq\":1,\"t\":\"a\"}\n{\"seq\":7}\n{\"seq\":3}\nnot json\n{\"seq\":-2}";
        let whole = digest(text, Some(Transcript::Ndjson));
        for size in [1, 2, 5, 64] {
            let mut d = Digester::new(Some(Transcript::Ndjson));
            text.chunks(size).for_each(|c| d.update(c));
            assert_eq!(d.finish(), whole, "{size}");
        }
        assert_eq!(whole.length, text.len() as u64);
        assert_eq!(whole.seq_max, Some(7));
        assert!(whole.hash.starts_with("sha256:") && whole.hash.len() == 7 + 64);
    }

    #[test]
    fn vtt_counts_cues_and_other_files_have_no_seq() {
        let vtt = b"WEBVTT\n\n1\n00:00.000 --> 00:01.000\nhi\n\n2\n00:01.000 --> 00:02.000\nthere";
        assert_eq!(digest(vtt, Some(Transcript::Vtt)).seq_max, Some(2));
        assert_eq!(digest(b"WEBVTT\n", Some(Transcript::Vtt)).seq_max, Some(0));
        assert_eq!(digest(b"{\"seq\":4}\n", None).seq_max, None);
        assert_eq!(
            digest(b"{\"x\":4}\n", Some(Transcript::Ndjson)).seq_max,
            None
        );
        assert_eq!(
            digest(b"abc", None).hash,
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_long_line_is_hashed_but_not_read_for_its_number() {
        let mut text = alloc::format!("{{\"seq\":99,\"t\":\"{}\"}}\n", "x".repeat(SEQ_LINE_MAX));
        text.push_str("{\"seq\":2}\n");
        let d = digest(text.as_bytes(), Some(Transcript::Ndjson));
        assert_eq!((d.seq_max, d.length), (Some(2), text.len() as u64));
    }

    #[test]
    fn transcripts_are_known_by_extension() {
        assert_eq!(
            Transcript::of("root:acme/t/016.NDJSON"),
            Some(Transcript::Ndjson)
        );
        assert_eq!(Transcript::of("calls/3-nov.vtt"), Some(Transcript::Vtt));
        for s in ["a.md", ".vtt", "vtt", "calls.vtt/x"] {
            assert_eq!(Transcript::of(s), None, "{s}");
        }
    }

    #[test]
    fn stamping_sets_and_clears_the_hash_keys() {
        let mut p = json!({ "source": "a.ndjson", "hash": null });
        let d = digest(b"{\"seq\":3}\n", Some(Transcript::Ndjson));
        stamp_pointer(&mut p, HashState::Hashed, Some(&d), true);
        assert_eq!(p["hash"], json!(d.hash));
        assert_eq!(
            (p["length"].as_u64(), p["seq_max"].as_u64()),
            (Some(10), Some(3))
        );
        assert_eq!(pointer_state(&p), Some(HashState::Hashed));
        assert!(
            !pointer_accepted(&p),
            "a hashed state is never marked accepted"
        );
        stamp_pointer(&mut p, HashState::Missing, None, false);
        assert_eq!(
            p,
            json!({ "source": "a.ndjson", "hash": null, "hash_state": "missing" })
        );
        stamp_pointer(&mut p, HashState::Missing, None, true);
        assert!(pointer_accepted(&p));
        assert_eq!(pointer_state(&json!({ "hash": null })), None);
    }
}
