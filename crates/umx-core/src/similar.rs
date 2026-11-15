//! similar_to on disk (workshop 008 section 9; PRD req-65): the score every machine computes the same way, the line
//! format of `similar/<model@dims>/<shard>.tsv`, the `_meta.tsv` staleness line, and the per-graph settings with the
//! embeddings-expert's per-model defaults. Bytes in, bytes out; no I/O.
//!
//! One line per edge: `node id TAB other id TAB score`, the score with exactly 4 decimal places and no exponent.
//! Lines sort by (node id, other id) (ids are fixed-width text, so byte order is that order), not by score, so a node
//! entering or leaving a list is one added or removed line.

use crate::Value;
use crate::id::Id;
use crate::ports::ModelKey;
use crate::similarity::{FloorSource, Floors, Use};
use crate::vectors::{TextHash, round_half_even};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

/// Format version of the `_meta.tsv` line.
pub const META_FORMAT: u32 = 1;

/// The most similar_to edges a node may have, when graph.json sets none (008 section 9).
pub const DEFAULT_K: usize = 10;

/// The largest k a graph may set (architect, PR for ws004 s09); also fits the in-memory length byte.
pub const MAX_K: usize = 64;

/// `umx similarity pairs` (ws004 s31): the most pair comparisons (int8 dot products) one call may take, checked
/// before any vector is copied (graph-storage-expert and architect, 2026-11-12). 2 graphs of about 44k nodes each,
/// or 5 equal graphs of about 70k nodes in all, cross-graph only.
pub const MAX_PAIR_DOTS: u64 = 2_000_000_000;

/// The most bytes of vectors `umx similarity pairs` may copy out of the graphs: rows x (dims + 4 + 16).
pub const MAX_PAIR_BYTES: u64 = 512 << 20;

/// `umx similarity pairs`' k when none is given, and the largest it takes.
pub const PAIRS_DEFAULT_K: usize = 3;
pub const PAIRS_MAX_K: usize = DEFAULT_K;

/// Ten-thousandths: scores and floors are held as integers, so every machine ranks and writes them the same.
pub const SCALE: f64 = 10_000.0;

/// The score of `b` in `a`'s list, in ten-thousandths: the exact int8 dot product times `a`'s then `b`'s inverse
/// length, in f64 and in that order, rounded half to even at 4 places (embeddings-expert, 2026-11-10). The inputs
/// are the int8 rows and inverse lengths held in the graph's blocks, never a provider's f32 vector, so a rebuild from
/// committed shards gives the same scores as the lists kept current as vectors arrived.
pub fn score_units(dot: i32, inv_a: f32, inv_b: f32) -> i32 {
    let score = f64::from(dot) * f64::from(inv_a) * f64::from(inv_b);
    // rint rounds half to even (IEEE default rounding), the same as `round_half_even`, in one instruction.
    libm::rint(score * SCALE) as i32
}

/// A score or floor as the files write it: `-?D.DDDD` from the integer, so 0 is `0.0000`, never `-0.0000`.
pub fn format_units(units: i32) -> String {
    let sign = if units < 0 { "-" } else { "" };
    let abs = units.unsigned_abs();
    format!("{sign}{}.{:04}", abs / 10_000, abs % 10_000)
}

/// The integer of a `-?D.DDDD` score, exactly as [`format_units`] writes it; anything else (an exponent, more or
/// fewer places, `-0.0000`, a `+`) is `None`.
pub fn parse_units(text: &str) -> Option<i32> {
    let (negative, body) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let b = body.as_bytes();
    if b.len() != 6
        || b[1] != b'.'
        || !b[0].is_ascii_digit()
        || !b[2..].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    let units = i32::from(b[0] - b'0') * 10_000
        + b[2..]
            .iter()
            .fold(0i32, |n, d| n * 10 + i32::from(d - b'0'));
    let units = if negative { -units } else { units };
    (format_units(units) == text).then_some(units)
}

/// One similar_to edge: `other` is in `node`'s top k with this score.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SimilarLine {
    pub node: Id,
    pub other: Id,
    pub score: i32,
}

impl SimilarLine {
    pub fn encode(&self) -> Vec<u8> {
        format!(
            "{}\t{}\t{}",
            self.node.to_text(),
            self.other.to_text(),
            format_units(self.score)
        )
        .into_bytes()
    }

    /// A line in the exact form [`SimilarLine::encode`] writes, else `None`.
    fn decode(line: &[u8]) -> Option<SimilarLine> {
        let text = core::str::from_utf8(line).ok()?;
        let mut parts = text.split('\t');
        let (node, other, score) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() {
            return None;
        }
        let decoded = SimilarLine {
            node: node.parse().ok()?,
            other: other.parse().ok()?,
            score: parse_units(score)?,
        };
        (decoded.encode() == line).then_some(decoded)
    }
}

/// One line of a similarity shard: an edge, or a line this umx cannot read, kept as it is and written back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SimilarShardLine {
    Edge(SimilarLine),
    Kept(Vec<u8>),
}

impl SimilarShardLine {
    pub fn bytes(&self) -> Vec<u8> {
        match self {
            SimilarShardLine::Edge(e) => e.encode(),
            SimilarShardLine::Kept(raw) => raw.clone(),
        }
    }
}

/// Every non-empty line of a similarity shard, in file order.
pub fn decode_shard(bytes: &[u8]) -> Vec<SimilarShardLine> {
    bytes
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| match SimilarLine::decode(l) {
            Some(e) => SimilarShardLine::Edge(e),
            None => SimilarShardLine::Kept(l.to_vec()),
        })
        .collect()
}

/// A similarity shard's bytes: each line and a newline, sorted by bytes, which is (node id, other id) order.
pub fn encode_shard(lines: &[SimilarShardLine]) -> Vec<u8> {
    let mut encoded: Vec<Vec<u8>> = lines.iter().map(SimilarShardLine::bytes).collect();
    encoded.sort();
    let mut out = Vec::with_capacity(encoded.iter().map(|l| l.len() + 1).sum());
    for line in encoded {
        out.extend_from_slice(&line);
        out.push(b'\n');
    }
    out
}

/// What a similarity folder's lists were computed from: `_meta.tsv`, one tab-separated line and a newline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta {
    pub key: ModelKey,
    pub k: usize,
    pub floor: i32,
    /// The whole-node text version, the N of `@node.v<N>`.
    pub text_form: u32,
    /// [`digest`] of the whole-node vectors the lists were computed from.
    pub digest: [u8; 32],
    /// How many edges the lists hold (R102-3): a load that takes fewer (a shard missing, a line damaged or refused)
    /// knows the files are not the whole lists, though the vectors and settings still match.
    pub edges: u64,
}

impl Meta {
    pub fn encode(&self) -> Vec<u8> {
        let hex = digest_hex(&self.digest);
        format!(
            "{META_FORMAT}\t{}\t{}\t{}\t@node.v{}\t{hex}\t{}\n",
            self.key,
            self.k,
            format_units(self.floor),
            self.text_form,
            self.edges
        )
        .into_bytes()
    }

    /// The line in the exact form [`Meta::encode`] writes, with this format version; anything else is `None` (the
    /// lists are then stale and rebuilt).
    pub fn decode(bytes: &[u8]) -> Option<Meta> {
        let text = core::str::from_utf8(bytes).ok()?.strip_suffix('\n')?;
        let parts: Vec<&str> = text.split('\t').collect();
        let [format, key, k, floor, form, hex, edges] = parts.as_slice() else {
            return None;
        };
        if *format != "1" || hex.len() != 64 {
            return None;
        }
        let mut digest = [0u8; 32];
        for (i, pair) in hex.as_bytes().chunks(2).enumerate() {
            let pair = core::str::from_utf8(pair).ok()?;
            if pair.bytes().any(|b| b.is_ascii_uppercase()) {
                return None;
            }
            digest[i] = u8::from_str_radix(pair, 16).ok()?;
        }
        let meta = Meta {
            key: ModelKey::parse(key).ok()?,
            k: k.parse().ok()?,
            floor: parse_units(floor)?,
            text_form: form.strip_prefix("@node.v")?.parse().ok()?,
            digest,
            edges: edges.parse().ok()?,
        };
        (meta.encode() == bytes).then_some(meta)
    }
}

/// sha256 over the lines `node id TAB whole-node text hash`, one per node with a whole-node vector, in id order: what
/// the lists were computed from. Text hashes, not vector bytes, so a hosted model returning vectors that differ in
/// the last bits for the same text forces no rebuild. The lines are written into a fixed buffer, with nothing
/// allocated per node (ws004 s34b: `embeddings status` hashes 100k nodes under a read hold).
pub fn digest(nodes: impl Iterator<Item = (Id, TextHash)>) -> [u8; 32] {
    use sha2::Digest;
    const HEX: &[u8; 16] = b"0123456789abcdef";
    const LINE: usize = crate::id::ID_LEN + 1 + 32 + 1;
    let mut hasher = sha2::Sha256::new();
    let mut buf = [0u8; LINE * 64];
    let mut used = 0;
    for (id, hash) in nodes {
        let line = &mut buf[used..used + LINE];
        line[..crate::id::ID_LEN].copy_from_slice(&id.text_bytes());
        line[crate::id::ID_LEN] = b'\t';
        for (i, b) in hash.0.iter().enumerate() {
            line[crate::id::ID_LEN + 1 + 2 * i] = HEX[usize::from(b >> 4)];
            line[crate::id::ID_LEN + 2 + 2 * i] = HEX[usize::from(b & 15)];
        }
        line[LINE - 1] = b'\n';
        used += LINE;
        if used == buf.len() {
            hasher.update(&buf[..]);
            used = 0;
        }
    }
    hasher.update(&buf[..used]);
    hasher.finalize().into()
}

/// A digest as lowercase hex, as `_meta.tsv` writes it.
pub fn digest_hex(digest: &[u8; 32]) -> String {
    let mut hex = String::with_capacity(64);
    for b in digest {
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

/// `umx embeddings status`' `vectors_digest` (ws004 s34b, F194): sha256 over the line `<model key>\n` and then the
/// hex of `nodes_digest`, which is [`digest`] over the graph's whole-node vectors in that key, the very value
/// `_meta.tsv` records. The key names the space (model, dims, prefix), so vectors made in another space always
/// change it, even for the same texts. A cache key, not a check: it moves when the key, a node with a vector or a
/// vector's text hash moves, never when only vector bytes do (a provider swapping the model behind one key, or the
/// same text giving other floats, leaves it as it was), so it never proves two sets of vectors are the same.
pub fn vectors_digest(key: &ModelKey, nodes_digest: &[u8; 32]) -> [u8; 32] {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(format!("{key}\n{}", digest_hex(nodes_digest)).as_bytes());
    hasher.finalize().into()
}

/// The similar_to settings a graph uses: at most `k` edges per node, kept when a score is at or above `floor`
/// (ten-thousandths, compared as integers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub k: usize,
    pub floor: i32,
    /// Where the floor came from: graph.json, the machine config, the calibrated table, or none.
    pub source: FloorSource,
    /// The calibration run, for a calibrated floor.
    pub run: Option<&'static str>,
}

/// A setting in graph.json that is out of range or not a number: warned, and the default used instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BadSetting {
    K,
    Floor,
    /// `similarity` is not an object.
    Section,
}

impl Settings {
    /// Whether the floor is calibrated (only a default from the calibrated table is).
    pub fn calibrated(&self) -> bool {
        self.source == FloorSource::Calibrated
    }

    /// The settings for `key` from graph.json's `embedding.similarity` (`None` when absent): `k` a whole number in
    /// 1..=64, `floor` a number in [-1, 1], rounded half to even to ten-thousandths once here. A value out of range
    /// or of the wrong type is reported and its default used. The default floor is `floors`' for similar_to: the
    /// machine config's, else the calibrated table's, else none (0.0).
    pub fn of(key: &ModelKey, raw: Option<&Value>, floors: &Floors) -> (Settings, Vec<BadSetting>) {
        let floor = floors.get(key, Use::Similar);
        let mut out = Settings {
            k: DEFAULT_K,
            floor: round_half_even(f64::from(floor.value) * SCALE) as i32,
            source: floor.source,
            run: floor.run,
        };
        let mut bad = Vec::new();
        let Some(raw) = raw else {
            return (out, bad);
        };
        let Some(section) = raw.as_object() else {
            bad.push(BadSetting::Section);
            return (out, bad);
        };
        if let Some(k) = section.get("k") {
            match k.as_u64().map(|k| k as usize) {
                Some(k) if (1..=MAX_K).contains(&k) => out.k = k,
                _ => bad.push(BadSetting::K),
            }
        }
        if let Some(floor) = section.get("floor") {
            match floor.as_f64() {
                Some(f) if (-1.0..=1.0).contains(&f) => {
                    out.floor = round_half_even(f * SCALE) as i32;
                    out.source = FloorSource::Graph;
                    out.run = None;
                }
                _ => bad.push(BadSetting::Floor),
            }
        }
        (out, bad)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::borrow::ToOwned;
    use alloc::vec;
    use serde_json::json;

    #[test]
    fn scores_are_integers_written_with_four_places_and_parsed_back_exactly() {
        for (units, text) in [
            (0, "0.0000"),
            (8123, "0.8123"),
            (10_000, "1.0000"),
            (-10_000, "-1.0000"),
            (-5, "-0.0005"),
            (6000, "0.6000"),
        ] {
            assert_eq!(format_units(units), text);
            assert_eq!(parse_units(text), Some(units), "{text}");
        }
        for bad in [
            "-0.0000", "+0.5000", "0.500", "0.50000", "5e-1", ".5000", "10.0000", "0,5000",
            " 0.5000",
        ] {
            assert_eq!(parse_units(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_score_is_the_exact_dot_times_both_inverse_lengths_rounded_half_to_even() {
        // Same vector: dot = |a|^2, so the score is 1 whatever the length.
        let a: [i8; 4] = [3, 4, 0, 0];
        let inv = crate::similarity::inverse_length(&a);
        assert_eq!(
            score_units(crate::similarity::dot(&a, &a), inv, inv),
            10_000
        );
        // Orthogonal and opposite.
        assert_eq!(score_units(0, inv, inv), 0);
        assert_eq!(score_units(-25, 0.2, 0.2), -10_000);
        // Half to even at the fourth place, on values exact in binary: 0.12345 and 0.12355 are not, so check the
        // rounding step itself and one product that lands on a tie (2.5 / 10000 times 1 times 1).
        assert_eq!(round_half_even(1234.5), 1234);
        // The score's rounding (libm::rint) is the same half-to-even as the codec's own, everywhere it matters.
        for i in -40_000..=40_000 {
            for x in [f64::from(i) / 2.0, f64::from(i) / 3.0, f64::from(i) * 0.1] {
                assert_eq!(libm::rint(x) as i64, round_half_even(x), "{x}");
            }
        }
        assert_eq!(score_units(5, 0.5, 1.0 / 10_000.0 * 1.0), 2);
        assert_eq!(round_half_even(1235.5), 1236);
    }

    #[test]
    fn similarity_shards_sort_by_node_then_other_and_keep_what_they_cannot_read() {
        let line = |n: u128, o: u128, s: i32| {
            SimilarShardLine::Edge(SimilarLine {
                node: Id(n),
                other: Id(o),
                score: s,
            })
        };
        let lines = vec![
            line(2, 1, 9000),
            line(1, 3, 7000),
            line(1, 2, 8000),
            SimilarShardLine::Kept(b"<<<<<<< HEAD".to_vec()),
        ];
        let file = encode_shard(&lines);
        let order: Vec<(u128, u128)> = decode_shard(&file)
            .iter()
            .filter_map(|l| match l {
                SimilarShardLine::Edge(e) => Some((e.node.0, e.other.0)),
                SimilarShardLine::Kept(_) => None,
            })
            .collect();
        assert_eq!(order, [(1, 2), (1, 3), (2, 1)]);
        assert_eq!(
            encode_shard(&decode_shard(&file)),
            file,
            "decode then encode is byte identical"
        );
        let canonical = String::from_utf8(line(u128::MAX >> 3, 2, 8000).bytes()).unwrap();
        for raw in [
            canonical.replace("0.8000", "0.80"),
            canonical.replace("0.8000", "8.0e-1"),
            canonical.to_lowercase(),
            alloc::format!("{canonical}\textra"),
        ] {
            assert_eq!(
                decode_shard(raw.as_bytes()),
                vec![SimilarShardLine::Kept(raw.into_bytes())]
            );
        }
    }

    #[test]
    fn the_meta_line_round_trips_and_refuses_anything_else() {
        let meta = Meta {
            key: ModelKey::parse("embeddinggemma-2@256+p1").unwrap(),
            k: 10,
            floor: 6000,
            text_form: 1,
            digest: digest([(Id(1), TextHash([7; 16]))].into_iter()),
            edges: 12,
        };
        let bytes = meta.encode();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(
            text.starts_with("1\tembeddinggemma-2@256+p1\t10\t0.6000\t@node.v1\t"),
            "{text}"
        );
        assert!(text.ends_with("\t12\n"), "{text}");
        assert_eq!(Meta::decode(&bytes), Some(meta));
        for bad in [
            text.replacen('1', "2", 1),
            text.trim_end().to_owned(),
            text.replace("0.6000", "0.6"),
            text.replace("@node.v1", "node.v1"),
            text.to_uppercase(),
            text.replace("\t12\n", "\n"),
            text.replace("\t12\n", "\t012\n"),
            text.replace("\t12\n", "\t-1\n"),
        ] {
            assert_eq!(Meta::decode(bad.as_bytes()), None, "{bad:?}");
        }
    }

    #[test]
    fn the_digest_depends_on_ids_and_text_hashes_in_order() {
        let a = digest([(Id(1), TextHash([1; 16])), (Id(2), TextHash([2; 16]))].into_iter());
        assert_eq!(
            a,
            digest([(Id(1), TextHash([1; 16])), (Id(2), TextHash([2; 16]))].into_iter())
        );
        assert_ne!(
            a,
            digest([(Id(1), TextHash([1; 16])), (Id(2), TextHash([3; 16]))].into_iter())
        );
        assert_ne!(a, digest([(Id(1), TextHash([1; 16]))].into_iter()));
    }

    /// ws004 s34b: the buffered digest hashes exactly the lines `_meta.tsv` files on disk were made from
    /// (`<id text>\t<hash hex>\n`), across buffer boundaries (0, 1, 64, 65 and 130 nodes), so no stored list goes stale.
    #[test]
    fn the_digest_hashes_the_same_lines_as_before() {
        use sha2::Digest;
        let nodes = |n: u128| {
            (0..n).map(|i| {
                let mut h = [0u8; 16];
                h[0] = i as u8;
                h[15] = 0xf0 | (i as u8 & 15);
                (Id(u128::MAX - i * 7_919), TextHash(h))
            })
        };
        for n in [0, 1, 64, 65, 130] {
            let text: String = nodes(n)
                .map(|(id, h)| format!("{}\t{h}\n", id.to_text()))
                .collect();
            let reference: [u8; 32] = sha2::Sha256::digest(text.as_bytes()).into();
            assert_eq!(digest(nodes(n)), reference, "{n} nodes");
        }
    }

    #[test]
    fn the_vectors_digest_names_the_key_and_the_nodes_digest() {
        let nodes = digest([(Id(1), TextHash([1; 16]))].into_iter());
        let key = ModelKey::new("m", 8).unwrap();
        let a = vectors_digest(&key, &nodes);
        // The same texts in another space, or other texts in this one, give another digest.
        assert_ne!(a, vectors_digest(&ModelKey::new("m", 16).unwrap(), &nodes));
        assert_ne!(a, vectors_digest(&key, &digest(core::iter::empty())));
        use sha2::Digest;
        let line = format!("m@8\n{}", digest_hex(&nodes));
        assert_eq!(a, <[u8; 32]>::from(sha2::Sha256::digest(line.as_bytes())));
    }

    #[test]
    fn settings_default_per_model_and_bad_values_warn_and_fall_back() {
        let none = Floors::default();
        let other = ModelKey::new("other", 8).unwrap();
        // An uncalibrated key has no floor: only k limits.
        assert_eq!(
            Settings::of(&other, None, &none).0,
            Settings {
                k: 10,
                floor: 0,
                source: FloorSource::None,
                run: None
            }
        );
        assert_eq!(
            Settings::of(&ModelKey::new("fake", 8).unwrap(), None, &none)
                .0
                .floor,
            0,
            "test fakes keep every match"
        );
        // Every calibrated row is the similar_to default, marked with its run.
        for c in crate::similarity::CALIBRATED {
            let mut key = ModelKey::new(c.model, c.dims).unwrap();
            if let Some(p) = c.prompt_version {
                key = key.with_prompt_version(p);
            }
            let (s, _) = Settings::of(&key, None, &none);
            assert_eq!(
                s.floor,
                round_half_even(f64::from(c.similar) * SCALE) as i32
            );
            assert!(s.calibrated());
            assert_eq!(s.run, Some(c.run));
        }
        // The machine config's floor, then graph.json's, each replacing the one below.
        let mut config = Floors::default();
        config.set(&other, Some(0.2), Some(0.45));
        let (s, _) = Settings::of(&other, None, &config);
        assert_eq!(
            (s.floor, s.source, s.calibrated()),
            (4500, FloorSource::Config, false)
        );
        let set = json!({"k": 5, "floor": 0.75});
        assert_eq!(
            Settings::of(&other, Some(&set), &config),
            (
                Settings {
                    k: 5,
                    floor: 7500,
                    source: FloorSource::Graph,
                    run: None
                },
                vec![]
            )
        );
        let bad = json!({"k": 65, "floor": 1.5});
        let (s, warned) = Settings::of(&other, Some(&bad), &config);
        assert_eq!((s.k, s.floor, s.source), (10, 4500, FloorSource::Config));
        assert_eq!(warned, vec![BadSetting::K, BadSetting::Floor]);
        for raw in [json!({"k": 0}), json!({"k": "10"}), json!({"k": 2.5})] {
            assert_eq!(
                Settings::of(&other, Some(&raw), &none).1,
                vec![BadSetting::K],
                "{raw}"
            );
        }
        assert_eq!(
            Settings::of(&other, Some(&json!(null)), &none).1,
            vec![BadSetting::Section]
        );
    }
}
