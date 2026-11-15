//! Vector shards on disk (workshop 008 section 2.5, PRD req-34 to req-36): which texts of a node are embedded,
//! the hash that says a vector is current, and the line format of `embeddings/<model@dims>/<shard>.tsv`.
//! Bytes in, bytes out; no I/O.
//!
//! One line per vector: `node id TAB field TAB text hash TAB base64 of int8 values`. Lines sort by their bytes,
//! which is (node id, field) order because a tab sorts below every printable character. The shard is the last two
//! characters of the node id, so every vector of one node is in one file.

use crate::id::Id;
use crate::model::Node;
use crate::ontology::{FieldKind, Ontology, Role, never_embedded_field};
use crate::ports::ModelKey;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// The version of the whole-node text form. Its field name carries it, so bumping it changes only whole-node vectors
/// (req-35). 2 (D56, req-97): data fields are left out. A whole-node line of an older version whose text hash equals
/// the node's text now is still that node's vector (same text, same key, same vector; embeddings-expert, s22).
pub const WHOLE_NODE_VERSION: u32 = 2;

/// The field name of a node's whole-node vector: `@node.v<version>`. No real field starts with `@`.
pub fn whole_node_field() -> String {
    format!("@node.v{WHOLE_NODE_VERSION}")
}

/// The version of a whole-node field name `@node.v<N>`, of any version; None for any other field.
pub fn whole_node_version(field: &str) -> Option<u32> {
    let n = field.strip_prefix("@node.v")?;
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        .then(|| n.parse().ok())
        .flatten()
}

/// The texts of `node` that get a vector, by field name, sorted by field: the title, the note and each content field
/// (req-34, req-97: a field the node's type declares as a string whose role is content), when not blank; then the
/// whole node. Nothing when the node has no text at all, or is an actual provenance record.
///
/// Text form v2 (embeddings-expert; the adapter adds its model's own prompt): a field vector embeds
/// `title: none | text: <value>`; the whole node embeds `title: <title> | text: <other fields>`, the other fields
/// as `name: value` lines in name order. Data fields are in neither. A text's hash covers its whole form, so changing
/// a form re-embeds what it changes; the whole-node field name carries [`WHOLE_NODE_VERSION`] as well.
pub fn node_texts(node: &Node, ontology: &Ontology) -> Vec<(String, String)> {
    let values = field_values(node, ontology);
    let Some(whole) = whole_text(node, &values) else {
        return values;
    };
    let mut out: Vec<(String, String)> = values
        .into_iter()
        .map(|(f, v)| (f, format!("{FIELD_FORM}{v}")))
        .collect();
    out.push((whole_node_field(), whole));
    out.sort();
    out
}

/// The whole-node text of `node` from its embedded `values`, None when there are none.
fn whole_text(node: &Node, values: &[(String, String)]) -> Option<String> {
    if values.is_empty() {
        return None;
    }
    let others: Vec<String> = values
        .iter()
        .filter(|(f, _)| f != "title")
        .map(|(f, v)| format!("{f}: {v}"))
        .collect();
    let title = if node.title.trim().is_empty() {
        "none"
    } else {
        node.title.as_str()
    };
    Some(format!("title: {title} | text: {}", others.join("\n")))
}

/// The whole-node text of `node` in form v1, which had no roles: every field its type declares as a string. Only to
/// tell a committed v1 vector made from text that included a field now data (it goes pending, `text_form_outdated`)
/// from one whose node changed since (req-97).
pub fn whole_text_v1(node: &Node, ontology: &Ontology) -> Option<String> {
    if crate::provenance::is_provenance(node) {
        return None;
    }
    whole_text(node, &text_values(node, ontology))
}

/// Up to this many data fields with a value, [`whole_texts_with_data`] tries every set of them; past it, each alone
/// and all together.
const FORMER_FIELDS: usize = 6;

/// The whole-node texts `node` would have had if some of its data fields that could be content (string fields that
/// are not provenance or identity keys) had been content: one per non-empty set of those with a value. Only to tell a
/// committed whole-node vector made from text that included a field now data, whatever its text-form version
/// (R189-2: a pulled graph.json can mark a field data under `@node.v2` too), from one whose node changed since.
pub fn whole_texts_with_data(node: &Node, ontology: &Ontology) -> Vec<String> {
    if crate::provenance::is_provenance(node) {
        return Vec::new();
    }
    let content = field_values(node, ontology);
    let data: Vec<(String, String)> = values_where(node, ontology, |f| {
        f.kind == FieldKind::String
            && f.effective_role() == Role::Data
            && !never_embedded_field(&f.name)
    })
    .into_iter()
    .filter(|(f, _)| f != "title" && f != "note")
    .collect();
    let n = data.len();
    let sets: Vec<Vec<usize>> = if n <= FORMER_FIELDS {
        (1..(1usize << n))
            .map(|bits| (0..n).filter(|i| bits & (1 << i) != 0).collect())
            .collect()
    } else {
        (0..n)
            .map(|i| alloc::vec![i])
            .chain([(0..n).collect()])
            .collect()
    };
    sets.into_iter()
        .filter_map(|set| {
            let mut values = content.clone();
            values.extend(set.into_iter().map(|i| data[i].clone()));
            values.sort();
            whole_text(node, &values)
        })
        .collect()
}

/// What a field vector's text starts with, before the field's value (text forms v1 and v2).
pub const FIELD_FORM: &str = "title: none | text: ";

/// The raw values of `node`'s texts that get a vector, by field name, sorted by field: the title, the note and each
/// content field ([`crate::ontology::FieldDecl::effective_role`]), when not blank. The fields [`node_texts`] embeds,
/// and the one source the secret hold judges (s08). Nothing for an actual provenance record.
pub fn field_values(node: &Node, ontology: &Ontology) -> Vec<(String, String)> {
    if crate::provenance::is_provenance(node) {
        return Vec::new();
    }
    values_where(node, ontology, |f| {
        f.effective_role() == Role::Content && !never_embedded_field(&f.name)
    })
}

/// The raw values of `node`'s text fields whatever their role: the title, the note and each field its type declares
/// as a string, when not blank. What a write scans for secrets (`secret_suspected`): a data field is never sent, but
/// it lands in tracked files and agents' context (security, s22).
pub fn text_values(node: &Node, ontology: &Ontology) -> Vec<(String, String)> {
    values_where(node, ontology, |f| f.kind == FieldKind::String)
}

fn values_where(
    node: &Node,
    ontology: &Ontology,
    take: impl Fn(&crate::ontology::FieldDecl) -> bool,
) -> Vec<(String, String)> {
    let mut values: Vec<(String, String)> = Vec::new();
    let mut push = |field: &str, text: &str| {
        if !text.trim().is_empty() {
            values.push((field.to_string(), text.to_string()));
        }
    };
    push("title", &node.title);
    push("note", &node.note);
    if let Some(decl) = ontology.type_decl(&node.kind) {
        for f in decl.fields.iter().filter(|f| take(f)) {
            if let Some(text) = node.fields.get(&f.name).and_then(|v| v.as_str()) {
                push(&f.name, text);
            }
        }
    }
    values.sort();
    values
}

/// The first 16 bytes of the SHA-256 of an embedded text, written as 32 lower-case hex digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TextHash(pub [u8; 16]);

impl TextHash {
    pub fn of(text: &str) -> TextHash {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(text.as_bytes());
        let mut out = [0u8; 16];
        out.copy_from_slice(&digest[..16]);
        TextHash(out)
    }

    pub fn parse(hex: &str) -> Option<TextHash> {
        let bytes = hex.as_bytes();
        if bytes.len() != 32 {
            return None;
        }
        let digit = |b: u8| match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            _ => None,
        };
        let mut out = [0u8; 16];
        for (i, pair) in bytes.chunks(2).enumerate() {
            out[i] = digit(pair[0])? << 4 | digit(pair[1])?;
        }
        Some(TextHash(out))
    }
}

impl fmt::Display for TextHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

/// Quantises a vector to int8 with no stored scale (req-36): `round_half_even(x * 127 / max|x|)`. Scaling by the
/// largest component keeps the direction, so the result is the same whether or not `v` was L2-normalised, and the
/// same input always gives the same bytes. A zero vector gives zeros.
pub fn quantise(v: &[f32]) -> Vec<i8> {
    let max = v.iter().fold(0f64, |m, x| m.max(f64::from(x.abs())));
    if max == 0.0 || !max.is_finite() {
        return alloc::vec![0; v.len()];
    }
    v.iter()
        .map(|x| round_half_even(f64::from(*x) * 127.0 / max).clamp(-127, 127) as i8)
        .collect()
}

/// Rounds to the nearest integer, ties to even. No `f64::round` in no_std, so it is spelled out.
pub(crate) fn round_half_even(x: f64) -> i64 {
    let floor = {
        let t = x as i64;
        if (t as f64) > x { t - 1 } else { t }
    };
    let diff = x - floor as f64;
    if diff > 0.5 || (diff == 0.5 && floor % 2 != 0) {
        floor + 1
    } else {
        floor
    }
}

/// The shard of a node: the last two characters of its id, in lower case. A ULID's first characters are a
/// timestamp, so the last two spread new nodes across shards instead of making one merge hotspot.
pub fn shard_of(id: Id) -> String {
    let text = id.to_text();
    text[text.len() - 2..].to_ascii_lowercase()
}

/// Which sharded folder of a vector space a shard lives in: its vectors (`embeddings/`, workshop 008 2.5) or its
/// stored similar_to lists (`similar/`, 008 section 9). Both shard the same way, by the node id's last two characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ShardKind {
    Vectors,
    Similar,
}

impl ShardKind {
    /// The top folder inside the graph folder: `embeddings` or `similar`.
    pub const fn top(self) -> &'static str {
        match self {
            ShardKind::Vectors => "embeddings",
            ShardKind::Similar => "similar",
        }
    }

    /// `<top>/<model@dims>`: the folder of one vector space's shards of this kind.
    pub fn folder(self, key: &ModelKey) -> String {
        format!("{}/{key}", self.top())
    }
}

/// One shard file of one vector space: `embeddings/<model@dims>/<shard>.tsv` (vectors) or
/// `similar/<model@dims>/<shard>.tsv` (similar_to lists) in a graph folder.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShardFile {
    kind: ShardKind,
    key: ModelKey,
    shard: String,
}

impl ShardFile {
    /// A vector shard. `shard` must be `[0-9a-z]{2}` (so no path part can leave the graph folder), as [`shard_of`]
    /// gives.
    pub fn new(key: ModelKey, shard: &str) -> Option<ShardFile> {
        ShardFile::of_kind(ShardKind::Vectors, key, shard)
    }

    /// A shard of either kind; `shard` as for [`ShardFile::new`].
    pub fn of_kind(kind: ShardKind, key: ModelKey, shard: &str) -> Option<ShardFile> {
        let ok = shard.len() == 2
            && shard
                .bytes()
                .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase());
        ok.then(|| ShardFile {
            kind,
            key,
            shard: shard.to_string(),
        })
    }

    /// The shard file holding `id`'s vectors.
    pub fn of(key: ModelKey, id: Id) -> ShardFile {
        ShardFile::of_node(ShardKind::Vectors, key, id)
    }

    /// The shard file of this kind holding `id`'s lines.
    pub fn of_node(kind: ShardKind, key: ModelKey, id: Id) -> ShardFile {
        ShardFile::of_kind(kind, key, &shard_of(id))
            .expect("an id's last two characters are a shard")
    }

    pub fn kind(&self) -> ShardKind {
        self.kind
    }

    pub fn key(&self) -> &ModelKey {
        &self.key
    }

    pub fn shard(&self) -> &str {
        &self.shard
    }

    /// The folder of this shard inside the graph folder: `embeddings/<model@dims>` or `similar/<model@dims>`.
    pub fn folder(&self) -> String {
        self.kind.folder(&self.key)
    }

    /// `<shard>.tsv`.
    pub fn file_name(&self) -> String {
        format!("{}.tsv", self.shard)
    }

    /// The vector shard named by a file name in a vector space folder, if it is one.
    pub fn from_file_name(key: &ModelKey, name: &str) -> Option<ShardFile> {
        ShardFile::from_file_name_in(ShardKind::Vectors, key, name)
    }

    /// The shard of `kind` named by a file name in its folder, if it is one.
    pub fn from_file_name_in(kind: ShardKind, key: &ModelKey, name: &str) -> Option<ShardFile> {
        ShardFile::of_kind(kind, key.clone(), name.strip_suffix(".tsv")?)
    }
}

/// `embeddings/<model@dims>`: the folder of a vector space inside a graph folder.
pub fn folder_of(key: &ModelKey) -> String {
    ShardKind::Vectors.folder(key)
}

impl fmt::Display for ShardFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.folder(), self.file_name())
    }
}

/// One vector line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VectorLine {
    pub node: Id,
    pub field: String,
    pub hash: TextHash,
    pub values: Vec<i8>,
}

impl VectorLine {
    pub fn encode(&self) -> Vec<u8> {
        let bytes: Vec<u8> = self.values.iter().map(|v| *v as u8).collect();
        format!(
            "{}\t{}\t{}\t{}",
            self.node.to_text(),
            self.field,
            self.hash,
            base64_encode(&bytes)
        )
        .into_bytes()
    }

    /// A line in the exact form [`VectorLine::encode`] writes, else `None`. Canonical form is checked part by part
    /// (upper-case id text, lower-case hash, base64 with zero padding bits), so decode then encode is byte identical
    /// without encoding the line again.
    fn decode(line: &[u8]) -> Option<VectorLine> {
        let mut parts = line.split(|b| *b == b'\t');
        let (node, field, hash, values) =
            (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        let field = core::str::from_utf8(field).ok()?;
        if parts.next().is_some() || !valid_field(field) {
            return None;
        }
        let id: Id = core::str::from_utf8(node).ok()?.parse().ok()?;
        if id.to_text().as_bytes() != node {
            return None;
        }
        let hash = TextHash::parse(core::str::from_utf8(hash).ok()?)?;
        if !base64_padding_is_zero(values) {
            return None;
        }
        let values: Vec<i8> = base64_decode(core::str::from_utf8(values).ok()?)?
            .into_iter()
            .map(|b| b as i8)
            .collect();
        Some(VectorLine {
            node: id,
            field: field.to_string(),
            hash,
            values,
        })
    }
}

/// Whether the bits after the last byte of padded base64 are zero, as [`base64_encode`] writes them. Decoding
/// ignores them, so this is what makes a decoded line encode back to the same bytes.
fn base64_padding_is_zero(text: &[u8]) -> bool {
    let pad = text.iter().rev().take_while(|&&c| c == b'=').count();
    let unused = match pad {
        0 => return true,
        1 => 2,
        2 => 4,
        _ => return false,
    };
    text.len() > pad && B64_VALUE[text[text.len() - pad - 1] as usize] & ((1 << unused) - 1) == 0
}

/// A field name a line may carry: not empty, no whitespace or control characters.
fn valid_field(field: &str) -> bool {
    !field.is_empty() && !field.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// One line of a shard file: a vector, or a line this umx cannot read (a hand edit, a merge marker), kept as it is
/// and written back so nothing is erased.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShardLine {
    Vector(VectorLine),
    Kept(Vec<u8>),
}

impl ShardLine {
    pub fn bytes(&self) -> Vec<u8> {
        match self {
            ShardLine::Vector(v) => v.encode(),
            ShardLine::Kept(raw) => raw.clone(),
        }
    }

    pub fn vector(&self) -> Option<&VectorLine> {
        match self {
            ShardLine::Vector(v) => Some(v),
            ShardLine::Kept(_) => None,
        }
    }
}

/// Every non-empty line of a shard file, in file order, each a vector or kept as it is. Blank lines carry nothing
/// and are dropped (the next write of the shard leaves them out).
pub fn decode_shard(bytes: &[u8]) -> Vec<ShardLine> {
    decode_shard_numbered(bytes)
        .into_iter()
        .map(|(_, l)| l)
        .collect()
}

/// [`decode_shard`] with each line's physical 1-based line number in the file, so a warning names the line a
/// person sees: blank lines count, and the newline that ends the last line does not start another.
pub fn decode_shard_numbered(bytes: &[u8]) -> Vec<(usize, ShardLine)> {
    bytes
        .split(|b| *b == b'\n')
        .enumerate()
        .filter(|(_, l)| !l.is_empty())
        .map(|(i, l)| {
            let line = match VectorLine::decode(l) {
                Some(v) => ShardLine::Vector(v),
                None => ShardLine::Kept(l.to_vec()),
            };
            (i + 1, line)
        })
        .collect()
}

/// The shard file for `lines`: each line's bytes and a newline, sorted by bytes, so equal content gives equal
/// files (req-36) and lines merge in git.
pub fn encode_shard(lines: &[ShardLine]) -> Vec<u8> {
    let mut encoded: Vec<Vec<u8>> = lines.iter().map(ShardLine::bytes).collect();
    encoded.sort();
    let mut out = Vec::with_capacity(encoded.iter().map(|l| l.len() + 1).sum());
    for line in encoded {
        out.extend_from_slice(&line);
        out.push(b'\n');
    }
    out
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding (RFC 4648 section 4).
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Each byte's base64 value, or 0xff for a byte that is not in the alphabet. A table, because shard loads decode
/// every character of every vector.
const B64_VALUE: [u8; 256] = {
    let mut table = [0xffu8; 256];
    let mut i = 0;
    while i < 64 {
        table[B64[i] as usize] = i as u8;
        i += 1;
    }
    table
};

/// Decodes standard padded base64; `None` for anything else.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let Some((body, last)) = bytes.split_last_chunk::<4>() else {
        return Some(Vec::new());
    };
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    // Every chunk but the last: four characters, none of them padding (its value 0xff fails the test like any
    // character outside the alphabet).
    for chunk in body.as_chunks::<4>().0 {
        let [a, b, c, d] = [0, 1, 2, 3].map(|i| u32::from(B64_VALUE[chunk[i] as usize]));
        if (a | b | c | d) > 63 {
            return None;
        }
        let n = a << 18 | b << 12 | c << 6 | d;
        out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8]);
    }
    let pad = last.iter().rev().take_while(|&&c| c == b'=').count();
    if pad > 2 {
        return None;
    }
    let mut n = 0u32;
    for &c in &last[..4 - pad] {
        match B64_VALUE[c as usize] {
            0xff => return None,
            v => n = n << 6 | u32::from(v),
        }
    }
    n <<= 6 * pad as u32;
    out.extend_from_slice(&[(n >> 16) as u8, (n >> 8) as u8, n as u8][..3 - pad]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ontology::{FieldDecl, TypeDecl};
    use alloc::collections::BTreeMap;
    use alloc::vec;

    #[test]
    fn a_text_hash_is_the_first_half_of_its_sha256() {
        // FIPS 180-2 appendix B.1: SHA-256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a3...
        assert_eq!(
            TextHash::of("abc").to_string(),
            "ba7816bf8f01cfea414140de5dae2223"
        );
        let h = TextHash::of("Slow builds");
        assert_eq!(TextHash::parse(&h.to_string()), Some(h));
        assert_eq!(
            TextHash::parse("BA7816BF8F01CFEA414140DE5DAE2223"),
            None,
            "lower case only"
        );
    }

    #[test]
    fn base64_round_trips_and_refuses_junk() {
        for (raw, text) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (&[0xff, 0x80, 0x00][..], "/4AA"),
        ] {
            assert_eq!(base64_encode(raw), text);
            assert_eq!(base64_decode(text).as_deref(), Some(raw));
        }
        for junk in ["Zg=", "Z===", "Zg==Zg==", "Zm9v!A==", "Zm-v"] {
            assert_eq!(base64_decode(junk), None, "{junk}");
        }
    }

    #[test]
    fn quantisation_is_deterministic_and_rounds_half_to_even() {
        let v = [0.5f32, -1.0, 0.25, 0.0];
        assert_eq!(quantise(&v), vec![64, -127, 32, 0]);
        assert_eq!(quantise(&v), quantise(&v));
        // Scaling does not change the bytes: normalised or not, the same direction quantises the same.
        assert_eq!(quantise(&[1.0, 2.0]), quantise(&[0.5, 1.0]));
        assert_eq!(quantise(&[0.0, 0.0]), vec![0, 0]);
        assert_eq!(round_half_even(0.5), 0);
        assert_eq!(round_half_even(1.5), 2);
        assert_eq!(round_half_even(2.5), 2);
        assert_eq!(round_half_even(-0.5), 0);
        assert_eq!(round_half_even(-1.5), -2);
        assert_eq!(round_half_even(-1.2), -1);
    }

    fn line(node: u128, field: &str, value: i8) -> VectorLine {
        VectorLine {
            node: Id(node),
            field: field.into(),
            hash: TextHash::of(field),
            values: vec![value; 256],
        }
    }

    #[test]
    fn a_line_at_256_dims_fits_the_budget_and_round_trips() {
        let l = line(u128::MAX >> 2, "a_field_name", -3);
        let bytes = l.encode();
        assert!(bytes.len() < 460, "{} bytes and a newline", bytes.len());
        assert_eq!(VectorLine::decode(&bytes), Some(l));
    }

    #[test]
    fn shards_sort_by_node_then_field_and_decode_then_encode_is_identical() {
        let lines = vec![
            ShardLine::Vector(line(2, "title", 1)),
            ShardLine::Vector(line(1, "title", 2)),
            ShardLine::Vector(line(1, "note", 3)),
            ShardLine::Vector(line(1, "note_x", 4)),
            ShardLine::Kept(b"<<<<<<< HEAD".to_vec()),
        ];
        let file = encode_shard(&lines);
        let order: Vec<String> = decode_shard(&file)
            .iter()
            .filter_map(|l| l.vector())
            .map(|v| format!("{}:{}", v.node.0, v.field))
            .collect();
        assert_eq!(order, ["1:note", "1:note_x", "1:title", "2:title"]);
        assert_eq!(encode_shard(&decode_shard(&file)), file);
        assert_eq!(
            decode_shard(&file)
                .iter()
                .filter(|l| matches!(l, ShardLine::Kept(_)))
                .count(),
            1,
            "a line umx cannot read is kept"
        );
    }

    #[test]
    fn numbered_lines_are_physical_lines() {
        let good = line(1, "title", 1).encode();
        let mut file = b"\nnot-a-vector\n\n".to_vec();
        file.extend_from_slice(&good);
        file.push(b'\n');
        let numbers: Vec<usize> = decode_shard_numbered(&file)
            .iter()
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(numbers, [2, 4]);
        assert_eq!(decode_shard(&file).len(), 2);
    }

    #[test]
    fn a_non_canonical_line_is_kept_not_rewritten() {
        let id = Id(u128::MAX >> 3);
        let canonical = line(id.0, "title", 5).encode();
        let mut lower = String::from_utf8(canonical.clone()).unwrap();
        lower.replace_range(0..26, &id.to_text().to_lowercase());
        // The values end `BQ==`; `BR==` decodes to the same bytes but its padding bits are not zero.
        let mut padding = String::from_utf8(canonical).unwrap();
        assert!(padding.ends_with("BQ=="));
        padding.replace_range(padding.len() - 3..padding.len() - 2, "R");
        for raw in [
            lower.as_str(),
            padding.as_str(),
            "too\tfew\tparts",
            "00000000000000000000000007\ttitle\tnot-hex\tAA==",
        ] {
            assert_eq!(
                decode_shard(raw.as_bytes()),
                vec![ShardLine::Kept(raw.as_bytes().to_vec())]
            );
        }
    }

    #[test]
    fn a_node_lives_in_the_shard_of_its_last_two_characters() {
        let id: Id = "01JC0000000000000000000AB7".parse().unwrap();
        assert_eq!(shard_of(id), "b7");
        let key = ModelKey::new("m", 4).unwrap();
        let shard = ShardFile::of(key.clone(), id);
        assert_eq!(shard.to_string(), "embeddings/m@4/b7.tsv");
        assert_eq!(ShardFile::from_file_name(&key, "b7.tsv"), Some(shard));
        for bad in ["B7.tsv", "b.tsv", "b7x.tsv", "b7.txt", "../.tsv", "-7.tsv"] {
            assert_eq!(ShardFile::from_file_name(&key, bad), None, "{bad}");
        }
    }

    fn node(note: &str, fields: &[(&str, &str)]) -> Node {
        Node {
            id: Id(1),
            kind: "problem".into(),
            title: "Slow builds".into(),
            status: "open".into(),
            note: note.into(),
            evidence: Vec::new(),
            status_history: Vec::new(),
            created: String::new(),
            updated: String::new(),
            created_by: String::new(),
            locator: None,
            authored_by_session: None,
            edited_by_session: None,
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    fn ontology() -> Ontology {
        let field = |name: &str, kind| FieldDecl::new(name, kind);
        Ontology {
            types: vec![TypeDecl {
                name: "problem".into(),
                fields: vec![
                    field("impact", FieldKind::String),
                    field("due", FieldKind::Date),
                ],
                columns: Vec::new(),
                description: None,
                extra: BTreeMap::new(),
            }],
            ..Ontology::default()
        }
    }

    #[test]
    fn provenance_has_no_text_and_reserved_keys_are_never_embedded_on_content() {
        let mut ontology = ontology();
        for key in crate::ontology::PROVENANCE_KEYS {
            ontology.types[0]
                .fields
                .push(FieldDecl::new(*key, FieldKind::String));
        }
        let mut content = node(
            "Ordinary note",
            &[
                ("impact", "Ordinary impact"),
                ("prov_key", "agent/pij/pij-calm-otter"),
                ("authored_by_session", "unindexedmarker"),
                ("edited_by_session", "unindexedmarker"),
            ],
        );
        let texts = node_texts(&content, &ontology);
        assert!(!texts.is_empty());
        assert!(texts.iter().all(|(field, text)| {
            !crate::ontology::PROVENANCE_KEYS.contains(&field.as_str())
                && !text.contains("unindexedmarker")
                && !text.contains("agent/pij")
        }));
        content.kind = "agent".into();
        assert!(node_texts(&content, &ontology).is_empty());
        content.fields.remove("prov_key");
        assert!(!node_texts(&content, &ontology).is_empty());
    }

    #[test]
    fn a_node_with_three_text_fields_has_four_vectors() {
        let n = node(
            "Builds take an hour.",
            &[("impact", "Teams wait"), ("due", "2026-12-01")],
        );
        let texts = node_texts(&n, &ontology());
        let fields: Vec<&str> = texts.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(fields, ["@node.v2", "impact", "note", "title"]);
        assert_eq!(
            texts[0].1,
            "title: Slow builds | text: impact: Teams wait\nnote: Builds take an hour."
        );
        assert_eq!(texts[1].1, "title: none | text: Teams wait");
        assert_eq!(texts[3].1, "title: none | text: Slow builds");
        // A blank note and an undeclared field are not embedded.
        let fields: Vec<String> = node_texts(&node(" ", &[("other", "x")]), &ontology())
            .into_iter()
            .map(|(f, _)| f)
            .collect();
        assert_eq!(fields, ["@node.v2", "title"]);
    }

    #[test]
    fn a_data_field_has_no_vector_and_is_left_out_of_the_whole_node_text() {
        let mut o = ontology();
        let mut file = FieldDecl::new("file", FieldKind::String);
        file.role = Some(Role::Data);
        o.types[0].fields.push(file);
        let n = node("", &[("impact", "Teams wait"), ("file", "src/build.rs")]);
        let texts = node_texts(&n, &o);
        let fields: Vec<&str> = texts.iter().map(|(f, _)| f.as_str()).collect();
        assert_eq!(fields, ["@node.v2", "impact", "title"]);
        assert!(texts.iter().all(|(_, t)| !t.contains("src/build.rs")));
        // Form v1 had no roles: its whole-node text held the field, which is how a committed v1 vector that did is
        // told apart.
        let v1 = whole_text_v1(&n, &o).unwrap();
        assert!(v1.contains("file: src/build.rs"), "{v1}");
        assert_ne!(Some(v1.as_str()), texts.first().map(|(_, t)| t.as_str()));
        // Written values keep the data field, so a write still scans it for secrets.
        assert!(text_values(&n, &o).iter().any(|(f, _)| f == "file"));
    }

    #[test]
    fn the_former_whole_texts_put_back_each_set_of_now_data_fields() {
        let mut o = ontology();
        for name in ["file", "seat", "prov_key"] {
            let mut f = FieldDecl::new(name, FieldKind::String);
            f.role = Some(Role::Data);
            o.types[0].fields.push(f);
        }
        let n = node(
            "",
            &[
                ("impact", "Teams wait"),
                ("file", "src/build.rs"),
                ("seat", "zzseat"),
                ("prov_key", "agent/x"),
            ],
        );
        let former = whole_texts_with_data(&n, &o);
        // file, seat, and both: never prov_key, which was never embedded.
        assert_eq!(former.len(), 3, "{former:?}");
        assert!(former.iter().all(|t| !t.contains("agent/x")));
        let mut file_only = field_values(&n, &o);
        file_only.push(("file".into(), "src/build.rs".into()));
        file_only.sort();
        assert!(former.contains(&whole_text(&n, &file_only).unwrap()));
        // No data field with a value: nothing.
        assert!(whole_texts_with_data(&node("", &[("impact", "x")]), &o).is_empty());
    }

    #[test]
    fn a_content_field_embeds_and_a_kind_that_cannot_be_content_never_does() {
        let mut o = ontology();
        o.types[0].fields[0].role = Some(Role::Content);
        // A pulled graph.json may say content for a date: umx acts on data.
        o.types[0].fields[1].role = Some(Role::Content);
        let n = node("", &[("impact", "Teams wait"), ("due", "2026-12-01")]);
        let fields: Vec<String> = node_texts(&n, &o).into_iter().map(|(f, _)| f).collect();
        assert_eq!(fields, ["@node.v2", "impact", "title"]);
    }

    #[test]
    fn provenance_is_never_embedded_whatever_graph_json_declares() {
        let mut o = ontology();
        let never: Vec<&str> = crate::ontology::PROVENANCE_KEYS
            .iter()
            .chain(crate::ontology::IDENTITY_KEYS)
            .copied()
            .chain([crate::entity::ENTITY_KEY, crate::evidence::EVIDENCE_REMOVED])
            .collect();
        for name in &never {
            let mut f = FieldDecl::new(*name, FieldKind::String);
            f.role = Some(Role::Content);
            o.types[0].fields.push(f);
        }
        // entity_key (ws008 E1) is a list by std, but a pulled graph.json may declare it a content string.
        let values: Vec<(&str, &str)> =
            never.iter().map(|k| (*k, "s3cret-looking-value")).collect();
        let n = node("", &values);
        let texts = node_texts(&n, &o);
        assert_eq!(
            texts.len(),
            2,
            "the title and the whole node only: {texts:?}"
        );
        assert!(texts.iter().all(|(_, t)| !t.contains("s3cret")));
        for kind in crate::ontology::PROVENANCE_TYPES {
            let mut n = node("A note", &[]);
            n.kind = (*kind).into();
            o.types.push(TypeDecl::new(
                *kind,
                vec![FieldDecl::new("impact", FieldKind::String)],
            ));
            assert!(
                !node_texts(&n, &o).is_empty(),
                "a user-made {kind} stays content"
            );
            let key = match *kind {
                "platform" => "platform/pij",
                "agent" => "agent/unknown",
                "model" => "model/test",
                "harness" => "harness/copilot",
                "session" => "session/unknown/unknown",
                _ => unreachable!(),
            };
            n.fields.insert("prov_key".into(), serde_json::json!(key));
            assert!(node_texts(&n, &o).is_empty(), "{kind}");
            assert!(field_values(&n, &o).is_empty(), "{kind}");
            assert!(whole_text_v1(&n, &o).is_none());
            assert!(whole_texts_with_data(&n, &o).is_empty());
        }
    }

    #[test]
    fn whole_node_versions_parse_from_their_field_names() {
        assert_eq!(whole_node_field(), "@node.v2");
        assert_eq!(whole_node_version("@node.v1"), Some(1));
        assert_eq!(whole_node_version("@node.v2"), Some(2));
        for not in [
            "@node.v",
            "@node.vx",
            "@node.v+1",
            "node.v1",
            "title",
            "@node.v1 ",
        ] {
            assert_eq!(whole_node_version(not), None, "{not}");
        }
    }
}
