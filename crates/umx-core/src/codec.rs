//! Deterministic file encoding (req-02): one record per line, records sorted by id, fields in a fixed order, and
//! encoding an unchanged graph gives byte-identical files. Bytes in, bytes out; no I/O.

use crate::id::Id;
use crate::model::{Edge, FORMAT_VERSION, GraphMeta, Node};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::fmt;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The files a graph folder may hold that umx reads or writes: the three the codec encodes, and `.gitignore`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GraphFile {
    Graph,
    Nodes,
    Edges,
    /// The graph folder's `.gitignore`, where umx keeps the line for its temp and marker files. A user file as
    /// much as umx's: never in `ALL` (so a folder holding only it is not a graph), never removed.
    GitIgnore,
}

impl GraphFile {
    /// The graph's data files, in list order.
    pub const ALL: [GraphFile; 3] = [GraphFile::Graph, GraphFile::Nodes, GraphFile::Edges];

    pub fn file_name(self) -> &'static str {
        match self {
            GraphFile::Graph => "graph.json",
            GraphFile::Nodes => "nodes.jsonl",
            GraphFile::Edges => "edges.jsonl",
            GraphFile::GitIgnore => ".gitignore",
        }
    }
}

/// A file that does not decode: which file, which line and column (1-based) and why. Never quotes file content
/// (it reaches an agent's error message, which must hold no graph text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeError {
    pub file: GraphFile,
    pub line: usize,
    pub column: usize,
    pub reason: DecodeReason,
}

/// Why a file does not decode, in umx's own words and numbers only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeReason {
    Syntax,
    Data,
    Eof,
    Io,
    /// graph.json's `umx` format version is newer than [`FORMAT_VERSION`]: a newer umx wrote it.
    NewerFormat(u32),
    /// graph.json's `umx` format version is one no umx ever wrote.
    UnknownFormat(u32),
    /// An end names its author's graph explicitly instead of using a local end.
    OwnGraphEnd(&'static str),
    /// graph.json's `visibility` and `embedder` keys sit beside `policy: "open"` (an older umx may have rewritten
    /// `policy`), or state a pair `policy` names alone: refused, never read as either (D53, plan 071).
    PolicyPair,
    /// The file is past what umx holds in memory: 4 GiB of node lines, or 4,294,967,295 records (u32 handles and
    /// offsets).
    TooLarge,
}

impl fmt::Display for DecodeReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeReason::Syntax => f.write_str("not valid JSON"),
            DecodeReason::Data => f.write_str("a field is missing, unknown or of the wrong type"),
            DecodeReason::Eof => f.write_str("the record ends early"),
            DecodeReason::Io => f.write_str("read error"),
            DecodeReason::NewerFormat(v) => write!(
                f,
                "graph format {v} is newer than this umx reads ({FORMAT_VERSION})"
            ),
            DecodeReason::UnknownFormat(v) => write!(f, "graph format {v} is not one umx writes"),
            DecodeReason::OwnGraphEnd(end) => write!(f, "{end}_graph names this graph (write the end as local)"),
            DecodeReason::PolicyPair => f.write_str(
                "its embedding section has visibility and embedder keys beside a policy that does not match them (an older umx may have set policy to open): remove both keys or set policy to secured",
            ),
            DecodeReason::TooLarge => f.write_str(
                "the graph is too large for this umx (over 4 GiB of node lines or 4294967295 records)",
            ),
        }
    }
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} line {} column {}: {}",
            self.file.file_name(),
            self.line,
            self.column,
            self.reason
        )
    }
}

/// Writes `umx` as the higher of the version the graph had and the version its content needs, so a write never
/// relabels a file older than it was.
pub fn encode_graph(meta: &GraphMeta) -> Vec<u8> {
    let umx = meta.umx.max(meta.needed_format());
    let mut out = if umx == meta.umx {
        serde_json::to_vec_pretty(meta)
    } else {
        serde_json::to_vec_pretty(&GraphMeta {
            umx,
            ..meta.clone()
        })
    }
    .expect("graph metadata always serialises");
    out.push(b'\n');
    out
}

/// Metadata for the same write as `edges`: validates the owner's graph keys and raises, never lowers, its format.
/// The caller persists these bytes and the edge file in one write set.
pub fn encode_graph_with_edges<'a>(
    meta: &GraphMeta,
    edges: impl IntoIterator<Item = &'a Edge>,
) -> Result<Vec<u8>, DecodeError> {
    let edges: Vec<&Edge> = edges.into_iter().collect();
    check_edge_graphs(meta.id, edges.iter().copied())?;
    let mut meta = meta.clone();
    meta.umx = meta.umx.max(meta.needed_format_for_edges(edges));
    Ok(encode_graph(&meta))
}

/// Edge bytes for a graph-aware write. An explicit reference to the author's graph is refused, not normalised.
pub fn encode_edges_for_graph<'a>(
    graph: Id,
    edges: impl IntoIterator<Item = &'a Edge>,
) -> Result<Vec<u8>, DecodeError> {
    let edges: Vec<&Edge> = edges.into_iter().collect();
    check_edge_graphs(graph, edges.iter().copied())?;
    Ok(encode_edges(edges))
}

fn check_edge_graphs<'a>(
    graph: Id,
    edges: impl IntoIterator<Item = &'a Edge>,
) -> Result<(), DecodeError> {
    for (i, edge) in edges.into_iter().enumerate() {
        for (end, owner) in [("from", edge.from_graph), ("to", edge.to_graph)] {
            if owner == Some(graph) {
                return Err(DecodeError {
                    file: GraphFile::Edges,
                    line: i + 1,
                    column: 1,
                    reason: DecodeReason::OwnGraphEnd(end),
                });
            }
        }
    }
    Ok(())
}

/// Checks the format version before anything else, so a newer graph.json (which may add fields) is reported as
/// newer, not as invalid.
pub fn decode_graph(bytes: &[u8]) -> Result<GraphMeta, DecodeError> {
    decode_graph_with_version(bytes, FORMAT_VERSION)
}

fn decode_graph_with_version(bytes: &[u8], supported: u32) -> Result<GraphMeta, DecodeError> {
    #[derive(Deserialize)]
    struct Version {
        umx: u32,
    }
    if let Ok(Version { umx }) = serde_json::from_slice::<Version>(bytes) {
        let reason = match umx {
            v if v > supported => Some(DecodeReason::NewerFormat(v)),
            0 => Some(DecodeReason::UnknownFormat(0)),
            _ => None,
        };
        if let Some(reason) = reason {
            return Err(DecodeError {
                file: GraphFile::Graph,
                line: 1,
                column: 1,
                reason,
            });
        }
    }
    let meta: GraphMeta = serde_json::from_slice(bytes).map_err(|e| DecodeError {
        file: GraphFile::Graph,
        line: e.line(),
        column: e.column(),
        reason: reason(&e),
    })?;
    // Format 1 has no `embedding` key: one there is a hand edit, refused like any other unknown key in format 1.
    // Stored spines umx could not have written (a hand edit, R366-3) are refused the same way.
    // So are former aliases and paths it could not have written (ws006 S23).
    if meta.umx < meta.needed_format()
        || !crate::spine::stored_ok(&meta.spines)
        || !meta.former_ok()
    {
        return Err(DecodeError {
            file: GraphFile::Graph,
            line: 1,
            column: 1,
            reason: DecodeReason::Data,
        });
    }
    Ok(meta)
}

pub fn encode_nodes<'a>(nodes: impl IntoIterator<Item = &'a Node>) -> Vec<u8> {
    let mut nodes: Vec<&Node> = nodes.into_iter().collect();
    for node in &nodes {
        node.assert_provenance_fields();
    }
    nodes.sort_by_key(|n| n.id);
    lines(nodes)
}

pub fn encode_edges<'a>(edges: impl IntoIterator<Item = &'a Edge>) -> Vec<u8> {
    let mut edges: Vec<&Edge> = edges.into_iter().collect();
    for edge in &edges {
        edge.assert_provenance_fields();
    }
    edges.sort_by_key(|e| e.id);
    lines(edges)
}

/// A line not in use because its id appears again on a later line (req-67). Kept byte for byte, without its
/// newline, and written back by every encode, so nothing is erased.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeptLine {
    pub id: Id,
    pub line: Vec<u8>,
}

/// An id on more than one line of a file: every line number (1-based) in file order. The last is in use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repeat {
    pub file: GraphFile,
    pub id: Id,
    pub lines: Vec<usize>,
}

/// A decoded nodes or edges file: the records in use (the last line for each id), the earlier lines for repeated
/// ids, and one [`Repeat`] per repeated id, in id order.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded<T> {
    pub records: Vec<T>,
    /// The line number (1-based) of each record in `records`, in the same order.
    pub record_lines: Vec<usize>,
    pub kept: Vec<KeptLine>,
    pub repeats: Vec<Repeat>,
}

pub fn decode_nodes(bytes: &[u8]) -> Result<Decoded<Node>, DecodeError> {
    decode_lines(bytes, GraphFile::Nodes, |n: &Node| n.id)
}

pub fn decode_edges(bytes: &[u8]) -> Result<Decoded<Edge>, DecodeError> {
    decode_lines(bytes, GraphFile::Edges, |e: &Edge| e.id)
}

/// [`encode_nodes`] with kept lines: each goes just before the line of the record with its id, in the order given.
pub fn encode_nodes_keeping<'a>(
    nodes: impl IntoIterator<Item = &'a Node>,
    kept: &[KeptLine],
) -> Vec<u8> {
    lines_keeping(nodes.into_iter().map(|n| (n.id, n)).collect(), kept)
}

/// [`encode_edges`] with kept lines, placed as in [`encode_nodes_keeping`].
pub fn encode_edges_keeping<'a>(
    edges: impl IntoIterator<Item = &'a Edge>,
    kept: &[KeptLine],
) -> Vec<u8> {
    lines_keeping(edges.into_iter().map(|e| (e.id, e)).collect(), kept)
}

fn lines_keeping<T: Serialize>(records: Vec<(Id, &T)>, kept: &[KeptLine]) -> Vec<u8> {
    if kept.is_empty() {
        let mut records = records;
        records.sort_by_key(|(id, _)| *id);
        return lines(records.into_iter().map(|(_, r)| r).collect());
    }
    // (id, 0 = kept before 1 = in use, position among kept lines)
    let mut order: Vec<(Id, u8, usize, Option<&T>)> = kept
        .iter()
        .enumerate()
        .map(|(i, k)| (k.id, 0, i, None))
        .chain(records.into_iter().map(|(id, r)| (id, 1, 0, Some(r))))
        .collect();
    order.sort_by_key(|(id, rank, i, _)| (*id, *rank, *i));
    let mut out = Vec::new();
    for (_, _, i, record) in order {
        match record {
            Some(r) => out.extend(serde_json::to_vec(r).expect("records always serialise")),
            None => out.extend(&kept[i].line),
        }
        out.push(b'\n');
    }
    out
}

fn lines<T: Serialize>(records: Vec<&T>) -> Vec<u8> {
    let mut out = Vec::new();
    for r in records {
        out.extend(serde_json::to_vec(r).expect("records always serialise"));
        out.push(b'\n');
    }
    out
}

fn decode_lines<T: DeserializeOwned>(
    bytes: &[u8],
    file: GraphFile,
    id: impl Fn(&T) -> Id,
) -> Result<Decoded<T>, DecodeError> {
    let mut parsed: Vec<(Id, usize, &[u8], T)> = Vec::new();
    let mut lines: BTreeMap<Id, Vec<usize>> = BTreeMap::new();
    let mut last: BTreeMap<Id, usize> = BTreeMap::new();
    for (i, line) in bytes.split(|&b| b == b'\n').enumerate() {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let record = serde_json::from_slice(line).map_err(|e| DecodeError {
            file,
            line: i + 1,
            column: e.column(),
            reason: reason(&e),
        })?;
        let rid = id(&record);
        lines.entry(rid).or_default().push(i + 1);
        last.insert(rid, parsed.len());
        parsed.push((rid, i + 1, line, record));
    }
    let mut out = Decoded {
        records: Vec::new(),
        record_lines: Vec::new(),
        kept: Vec::new(),
        repeats: Vec::new(),
    };
    for (i, (rid, n, line, record)) in parsed.into_iter().enumerate() {
        if last[&rid] == i {
            out.records.push(record);
            out.record_lines.push(n);
        } else {
            out.kept.push(KeptLine {
                id: rid,
                line: line.to_vec(),
            });
        }
    }
    out.repeats = lines
        .into_iter()
        .filter(|(_, l)| l.len() > 1)
        .map(|(id, lines)| Repeat { file, id, lines })
        .collect();
    Ok(out)
}

fn reason(e: &serde_json::Error) -> DecodeReason {
    use alloc::string::ToString;
    use serde_json::error::Category;
    match e.classify() {
        Category::Syntax => DecodeReason::Syntax,
        Category::Data if e.to_string().contains(crate::model::POLICY_PAIR_REFUSED) => {
            DecodeReason::PolicyPair
        }
        Category::Data => DecodeReason::Data,
        Category::Eof => DecodeReason::Eof,
        Category::Io => DecodeReason::Io,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::Id;
    use crate::model::GraphEmbedding;
    use crate::ontology::{FieldDecl, FieldKind, Ontology};
    use alloc::collections::BTreeMap;
    use alloc::string::String;
    use alloc::string::ToString;
    use alloc::vec;
    use serde_json::json;

    fn node(id: u128, title: &str) -> Node {
        let mut fields = BTreeMap::new();
        fields.insert("zeta".to_string(), json!(1));
        fields.insert("alpha".to_string(), json!(["a", "b"]));
        Node {
            id: Id(id),
            kind: "problem".into(),
            title: title.into(),
            status: "open".into(),
            note: "line one\nline \"two\"".into(),
            evidence: vec![],
            status_history: vec![],
            created: "2026-11-09T05:50:12Z".into(),
            updated: "2026-11-09T05:50:12Z".into(),
            created_by: "tester".into(),
            locator: None,
            authored_by_session: None,
            edited_by_session: None,
            fields,
        }
    }

    /// ws006 S3: graph.json written at the locator format (as the writer raises it once a node has a locator) is
    /// refused by a reader one format older, never read as if the locator were not there.
    #[test]
    fn the_locator_format_is_written_for_locators_and_refused_by_an_older_reader() {
        use crate::model::FORMAT_LOCATORS;
        let mut meta = decode_graph(V1.as_bytes()).unwrap();
        meta.umx = meta.needed_format_for(false, true);
        let bytes = encode_graph(&meta);
        assert_eq!(decode_graph(&bytes).unwrap().umx, FORMAT_LOCATORS);
        assert_eq!(
            decode_graph_with_version(&bytes, FORMAT_LOCATORS - 1)
                .unwrap_err()
                .reason,
            DecodeReason::NewerFormat(FORMAT_LOCATORS)
        );
    }

    /// ws008 TM5 (architect): graph.json with a declared spine is written at the spines format and refused by a reader
    /// one format older (graph_format_newer), never read without its spines; with none, the key is left out and the
    /// format is not raised.
    #[test]
    fn the_spines_format_is_written_for_a_spine_and_refused_by_an_older_reader() {
        use crate::model::FORMAT_SPINES;
        let mut meta = decode_graph(V1.as_bytes()).unwrap();
        let plain = encode_graph(&meta);
        assert!(!core::str::from_utf8(&plain).unwrap().contains("spines"));
        assert_eq!(decode_graph(&plain).unwrap().umx, meta.umx);
        meta.spines.push(crate::spine::SpineDecl {
            name: "day".into(),
            title: "segment by starts_on".into(),
            main: true,
            anchors: crate::spine::Anchors {
                kind: "segment".into(),
                inside: Some("part_of".into()),
            },
            order: alloc::vec!["starts_on".into()],
            place: alloc::vec!["raised_in".into()],
        });
        let bytes = encode_graph(&meta);
        let read = decode_graph(&bytes).unwrap();
        assert_eq!((read.umx, &read.spines), (FORMAT_SPINES, &meta.spines));
        assert_eq!(
            decode_graph_with_version(&bytes, FORMAT_SPINES - 1)
                .unwrap_err()
                .reason,
            DecodeReason::NewerFormat(FORMAT_SPINES)
        );
        // A hand edit that adds spines without the format is refused, as any content above its format is.
        let mut low = read.clone();
        low.umx = FORMAT_SPINES - 1;
        let text = serde_json::to_vec(&low).unwrap();
        assert_eq!(decode_graph(&text).unwrap_err().reason, DecodeReason::Data);
        // R366-3: stored spines umx could not have written are refused too: a bad name, two mains, a repeated
        // name, a padded title, an order past its cap.
        let first = read.spines[0].clone();
        let bad: [fn(&mut GraphMeta); 5] = [
            |m| m.spines[0].name = "Day One".into(),
            |m| {
                m.spines.push(crate::spine::SpineDecl {
                    name: "two".into(),
                    ..m.spines[0].clone()
                })
            },
            |m| {
                m.spines.push(crate::spine::SpineDecl {
                    main: false,
                    ..m.spines[0].clone()
                })
            },
            |m| m.spines[0].title = " padded".into(),
            |m| m.spines[0].order = alloc::vec!["starts_on".into(); crate::spine::MAX_ORDER + 1],
        ];
        for (i, edit) in bad.iter().enumerate() {
            let mut m = read.clone();
            m.spines = alloc::vec![first.clone()];
            edit(&mut m);
            let text = serde_json::to_vec(&m).unwrap();
            assert_eq!(
                decode_graph(&text).unwrap_err().reason,
                DecodeReason::Data,
                "{i}"
            );
        }
    }

    /// ws006 S23 (architect): graph.json with a former alias or path is written at the former format and refused by a
    /// reader one format older; with neither, both keys are left out and the format is not raised; a hand edit that
    /// adds one below the format, or one umx could not have written, is refused as Data. It is this build's format
    /// (the last row of model.rs's format table).
    #[test]
    fn the_former_format_is_written_for_a_redirect_and_refused_by_an_older_reader() {
        use crate::model::{FORMAT_FORMER, MAX_FORMER};
        assert_eq!(FORMAT_VERSION, FORMAT_FORMER);
        let meta = decode_graph(V1.as_bytes()).unwrap();
        let plain = encode_graph(&meta);
        let text = core::str::from_utf8(&plain).unwrap();
        assert!(!text.contains("former_aliases") && !text.contains("former_paths"));
        assert_eq!(decode_graph(&plain).unwrap().umx, meta.umx);
        for (aliases, paths) in [
            (alloc::vec!["alpha".to_string()], Vec::new()),
            (Vec::new(), alloc::vec!["graphs/alpha".to_string()]),
        ] {
            let mut m = meta.clone();
            m.former_aliases = aliases;
            m.former_paths = paths;
            let bytes = encode_graph(&m);
            let read = decode_graph(&bytes).unwrap();
            assert_eq!(
                (read.umx, &read.former_aliases, &read.former_paths),
                (FORMAT_FORMER, &m.former_aliases, &m.former_paths)
            );
            assert_eq!(
                decode_graph_with_version(&bytes, FORMAT_FORMER - 1)
                    .unwrap_err()
                    .reason,
                DecodeReason::NewerFormat(FORMAT_FORMER)
            );
            let mut low = read.clone();
            low.umx = FORMAT_FORMER - 1;
            let text = serde_json::to_vec(&low).unwrap();
            assert_eq!(decode_graph(&text).unwrap_err().reason, DecodeReason::Data);
        }
        let mut ok = meta.clone();
        ok.umx = FORMAT_FORMER;
        ok.former_aliases = alloc::vec!["alpha".into()];
        ok.former_paths = alloc::vec!["graphs/alpha".into()];
        assert!(decode_graph(&serde_json::to_vec(&ok).unwrap()).is_ok());
        let live = ok.alias.clone();
        let bad: [&dyn Fn(&mut GraphMeta); 7] = [
            &|m| m.former_aliases.push("Not An Alias".into()),
            &|m| m.former_aliases.push(live.clone()),
            &|m| m.former_aliases.push("alpha".into()),
            &|m| m.former_aliases = (0..=MAX_FORMER).map(|i| alloc::format!("a{i}")).collect(),
            &|m| m.former_paths.push("/abs/alpha".into()),
            &|m| m.former_paths.push("./graphs/beta".into()),
            &|m| m.former_paths.push("../beta".into()),
        ];
        for (i, edit) in bad.iter().enumerate() {
            let mut m = ok.clone();
            edit(&mut m);
            let text = serde_json::to_vec(&m).unwrap();
            assert_eq!(
                decode_graph(&text).unwrap_err().reason,
                DecodeReason::Data,
                "{i}"
            );
        }
    }

    /// ws006 S3 (graph-storage-expert, workshop 010 9a): `locator` sits right after `created_by` and before the
    /// session keys, its own keys in kind, format, source, hash, length, seq_max order, then any key a newer umx
    /// added; a node without one keeps its bytes.
    #[test]
    fn a_locator_sits_after_created_by_before_the_session_keys() {
        let plain = node(1, "A");
        let before = encode_nodes([&plain]);
        assert!(!core::str::from_utf8(&before).unwrap().contains("locator"));
        let mut n = plain.clone();
        n.locator = Some(crate::Locator {
            hash: Some("sha256:9f2c".into()),
            length: Some(1843302),
            seq_max: Some(3511),
            ..crate::Locator::parse("t/016.ndjson").unwrap()
        });
        n.authored_by_session = Some(Id(2));
        n.edited_by_session = Some(Id(3));
        let bytes = encode_nodes([&n]);
        let text = core::str::from_utf8(&bytes).unwrap();
        assert!(
            text.contains(&alloc::format!(
                "\"created_by\":\"tester\",\"locator\":{{\"kind\":\"file\",\"format\":\"transcript_ndjson\",\"source\":\"t/016.ndjson\",\"hash\":\"sha256:9f2c\",\"length\":1843302,\"seq_max\":3511}},\"authored_by_session\":\"{}\",\"edited_by_session\":\"{}\",\"alpha\"",
                Id(2),
                Id(3)
            )),
            "{text}"
        );
        assert_eq!(decode_nodes(&bytes).unwrap().records, [n.clone()]);
        assert_eq!(encode_nodes(&decode_nodes(&bytes).unwrap().records), bytes);
        let locator = text
            .split("\"locator\":")
            .nth(1)
            .and_then(|rest| rest.split_once("},").map(|(l, _)| alloc::format!("{l}}}")))
            .unwrap();
        let newer = text.replace(&locator, "{\"kind\":\"file\",\"source\":\"a\",\"later\":1}");
        let read = decode_nodes(newer.as_bytes()).unwrap().records;
        assert_eq!(read[0].locator.as_ref().unwrap().extra["later"], json!(1));
        assert_eq!(
            encode_nodes(&read),
            newer.as_bytes(),
            "a newer umx's locator key is kept"
        );
    }

    /// graph-storage-expert, S241-1: a `locator` that is not a locator (a custom field from before the name was
    /// reserved, `null`, a newer kind or format) never fails the load. It loads as that custom field, and a line an
    /// older umx wrote, with the field in its custom-field place, keeps its bytes.
    #[test]
    fn a_locator_that_is_not_a_locator_loads_as_a_kept_custom_field() {
        for bad in [
            json!(null),
            json!("t/016.ndjson"),
            json!({ "kind": "passage", "of": "x", "selector": "seq=1" }),
            json!({ "kind": "file", "format": "csv", "source": "a" }),
            json!({ "source": "a" }),
        ] {
            let mut n = node(1, "A");
            n.fields.insert("locator".into(), bad.clone());
            let old = encode_nodes([&n]);
            let text = core::str::from_utf8(&old).unwrap();
            assert!(
                text.contains(&alloc::format!(
                    "\"alpha\":[\"a\",\"b\"],\"locator\":{bad},\"zeta\":1"
                )),
                "{text}"
            );
            let read = decode_nodes(&old).unwrap().records;
            assert_eq!(read[0].locator, None, "{bad}");
            assert!(read[0].has_kept_locator_field());
            assert_eq!(read[0].fields["locator"], bad);
            assert_eq!(encode_nodes(&read), old, "{bad}");
            let pinned = text
                .replace(&alloc::format!(",\"locator\":{bad}"), "")
                .replace(
                    "\"created_by\":\"tester\",",
                    &alloc::format!("\"created_by\":\"tester\",\"locator\":{bad},"),
                );
            assert_eq!(
                decode_nodes(pinned.as_bytes()).unwrap().records,
                read,
                "{bad}"
            );
        }
    }

    #[test]
    fn additive_session_keys_survive_old_readers_and_keep_the_pinned_order() {
        #[derive(Serialize, Deserialize)]
        struct OldNode {
            id: Id,
            #[serde(rename = "type")]
            kind: String,
            title: String,
            status: String,
            note: String,
            evidence: Vec<serde_json::Value>,
            status_history: Vec<serde_json::Value>,
            created: String,
            updated: String,
            created_by: String,
            #[serde(flatten)]
            fields: BTreeMap<String, serde_json::Value>,
        }
        let old = alloc::format!(
            "{{\"id\":\"{}\",\"type\":\"problem\",\"title\":\"A\",\"status\":\"open\",\"note\":\"\",\"evidence\":[],\"status_history\":[],\"created\":\"now\",\"updated\":\"now\",\"created_by\":\"t\",\"z\":1}}\n",
            Id(1)
        );
        let mut n = decode_nodes(old.as_bytes()).unwrap().records.pop().unwrap();
        assert_eq!(encode_nodes([&n]), old.as_bytes());
        n.authored_by_session = Some(Id(2));
        n.edited_by_session = Some(Id(3));
        let bytes = encode_nodes([&n]);
        let text = core::str::from_utf8(&bytes).unwrap();
        assert!(text.contains(&alloc::format!(
            "\"created_by\":\"t\",\"authored_by_session\":\"{}\",\"edited_by_session\":\"{}\",\"z\":1",
            Id(2), Id(3)
        )));
        let old_reader: OldNode = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(old_reader.fields["authored_by_session"], json!(Id(2)));
        assert_eq!(old_reader.fields["edited_by_session"], json!(Id(3)));
        let old_written = serde_json::to_vec(&old_reader).unwrap();
        assert_eq!(decode_nodes(&old_written).unwrap().records, [n]);
        assert_eq!(encode_nodes(&decode_nodes(&bytes).unwrap().records), bytes);
        for value in ["null", "7", "\"not-an-id\""] {
            let bad = text.replace(
                &alloc::format!("\"authored_by_session\":\"{}\"", Id(2)),
                &alloc::format!("\"authored_by_session\":{value}"),
            );
            let record = decode_nodes(bad.as_bytes()).unwrap().records.pop().unwrap();
            assert_eq!(record.authored_by_session, None);
            assert_eq!(
                record.fields.get("authored_by_session"),
                Some(&serde_json::from_str::<serde_json::Value>(value).unwrap())
            );
            let encoded = encode_nodes([&record]);
            assert_eq!(decode_nodes(&encoded).unwrap().records, [record]);
        }
        let edge = Edge {
            id: Id(5),
            kind: "supports".into(),
            from_graph: None,
            from: Id(1),
            to_graph: None,
            to: Id(2),
            created: "now".into(),
            created_by: "t".into(),
            authored_by_session: Some(Id(0)),
            edited_by_session: None,
            evidence: Vec::new(),
            fields: [("alpha".into(), json!(true))].into(),
        };
        let bytes = encode_edges([&edge]);
        assert!(
            core::str::from_utf8(&bytes)
                .unwrap()
                .contains(&alloc::format!(
                    "\"created_by\":\"t\",\"authored_by_session\":\"{}\",\"alpha\":true",
                    Id(0)
                ))
        );
        assert_eq!(decode_edges(&bytes).unwrap().records, [edge]);
        assert_eq!(encode_edges(&decode_edges(&bytes).unwrap().records), bytes);
    }

    #[test]
    fn edge_edit_sessions_decode_old_fields_and_keep_invalid_values_until_edited() {
        let old = alloc::format!(
            "{{\"id\":\"{}\",\"type\":\"supports\",\"from\":\"{}\",\"to\":\"{}\",\"created\":\"now\",\"created_by\":\"t\",\"z\":true,\"edited_by_session\":\"{}\",\"authored_by_session\":\"{}\",\"evidence\":[]}}\n",
            Id(5),
            Id(1),
            Id(2),
            Id(3),
            Id(0),
        );
        let edge = decode_edges(old.as_bytes()).unwrap().records.pop().unwrap();
        assert_eq!(edge.edited_by_session, Some(Id(3)));
        assert!(!edge.fields.contains_key("edited_by_session"));
        let encoded = encode_edges([&edge]);
        let text = core::str::from_utf8(&encoded).unwrap();
        assert!(text.contains(&alloc::format!(
            "\"authored_by_session\":\"{}\",\"edited_by_session\":\"{}\",\"evidence\":[],\"z\":true",
            Id(0), Id(3),
        )));
        assert_eq!(decode_edges(&encoded).unwrap().records, [edge]);
        for value in [json!(null), json!(7), json!("not-an-id")] {
            let bad = text.replace(
                &alloc::format!("\"edited_by_session\":\"{}\"", Id(3)),
                &alloc::format!("\"edited_by_session\":{value}"),
            );
            let mut kept = decode_edges(bad.as_bytes()).unwrap().records.pop().unwrap();
            assert_eq!(kept.edited_by_session, None);
            assert_eq!(kept.fields["edited_by_session"], value);
            assert_eq!(kept.authored_by_session, Some(Id(0)));
            let raw = encode_edges([&kept]);
            assert_eq!(decode_edges(&raw).unwrap().records, [kept.clone()]);
            kept.record_edit(Id(4));
            assert_eq!(kept.edited_by_session, Some(Id(4)));
            assert!(!kept.fields.contains_key("edited_by_session"));
            let updated = encode_edges([&kept]);
            assert_eq!(
                core::str::from_utf8(&updated)
                    .unwrap()
                    .matches("\"edited_by_session\":")
                    .count(),
                1
            );
        }
    }

    #[test]
    fn nodes_sort_by_id_with_fixed_field_order_and_round_trip_byte_identical() {
        let bytes = encode_nodes(&[node(2, "B"), node(1, "A")]);
        let text = core::str::from_utf8(&bytes).unwrap();
        let first = text.lines().next().unwrap();
        assert!(first.starts_with(r#"{"id":"00000000000000000000000001","type":"problem","title":"A","status":"open","note":"#));
        assert!(first.ends_with(r#""created_by":"tester","alpha":["a","b"],"zeta":1}"#));
        assert_eq!(text.lines().count(), 2);
        let decoded = decode_nodes(&bytes).unwrap().records;
        assert_eq!(decoded[0].title, "A");
        assert_eq!(encode_nodes(&decoded), bytes);
    }

    #[test]
    fn edges_and_graph_round_trip_byte_identical() {
        let edge = Edge {
            id: Id(9),
            kind: "blocks".into(),
            from_graph: None,
            from: Id(1),
            to_graph: None,
            to: Id(2),
            created: "2026-11-09T05:50:12Z".into(),
            created_by: "t".into(),
            authored_by_session: None,
            edited_by_session: None,
            evidence: Vec::new(),
            fields: BTreeMap::new(),
        };
        let bytes = encode_edges(&[edge]);
        assert_eq!(encode_edges(&decode_edges(&bytes).unwrap().records), bytes);

        let mut ontology = Ontology::starter();
        ontology.types[0]
            .fields
            .push(FieldDecl::new("likelihood", FieldKind::Float));
        let meta = GraphMeta {
            spines: Vec::new(),
            former_aliases: Vec::new(),
            former_paths: Vec::new(),
            umx: 1,
            id: Id(7),
            alias: "demo".into(),
            title: "Demo".into(),
            created: "2026-11-09T05:50:12Z".into(),
            embedding: None,
            ontology,
        };
        let bytes = encode_graph(&meta);
        assert!(bytes.ends_with(b"}\n"));
        assert_eq!(decode_graph(&bytes).unwrap(), meta);
        assert_eq!(encode_graph(&decode_graph(&bytes).unwrap()), bytes);
    }

    /// Plan 079: graph.json files written before the guide keys are written back byte for byte.
    #[test]
    fn graph_files_without_guide_keys_are_unchanged() {
        for bytes in [
            &include_bytes!("../../umx/tests/fixtures/query-graph/graph.json")[..],
            &include_bytes!("../../umx-cypher/fixtures/design/graph.json")[..],
        ] {
            assert_eq!(encode_graph(&decode_graph(bytes).unwrap()), bytes);
        }
    }

    fn described_meta() -> GraphMeta {
        let mut ontology = Ontology::starter();
        ontology.types[0]
            .fields
            .push(FieldDecl::new("likelihood", FieldKind::Float));
        ontology.types[0].description = Some("Something in the way".into());
        ontology.types[0].fields[0].description = Some("0 to 1".into());
        ontology.purpose = Some("Plan the launch".into());
        ontology.conventions = Some("Short titles".into());
        ontology
            .status_descriptions
            .insert("open".into(), "Not worked on yet".into());
        ontology
            .relation_descriptions
            .insert("blocks".into(), "Must be done first".into());
        ontology.extra.insert("a_newer_key".into(), json!(1));
        GraphMeta {
            spines: Vec::new(),
            former_aliases: Vec::new(),
            former_paths: Vec::new(),
            umx: 1,
            id: Id(7),
            alias: "demo".into(),
            title: "Demo".into(),
            created: "2026-11-09T05:50:12Z".into(),
            embedding: None,
            ontology,
        }
    }

    /// The order graph-storage-expert ruled (plan 079): the guide keys after the vocabulary, before kept keys; a
    /// description after a decl's known keys. No format bump: the file stays format 1.
    #[test]
    fn guide_keys_are_written_in_their_place_without_a_format_bump() {
        let bytes = encode_graph(&described_meta());
        let text = core::str::from_utf8(&bytes).unwrap();
        let at = |key: &str| text.find(&alloc::format!("\"{key}\"")).unwrap();
        let order = [
            "types",
            "statuses",
            "relations",
            "purpose",
            "conventions",
            "status_descriptions",
            "relation_descriptions",
            "a_newer_key",
        ];
        assert!(order.windows(2).all(|w| at(w[0]) < at(w[1])), "{text}");
        assert!(
            text.contains("\"kind\": \"float\",\n            \"description\": \"0 to 1\""),
            "{text}"
        );
        assert!(text.contains("\"description\": \"Something in the way\""));
        assert!(text.starts_with("{\n  \"umx\": 1,"));
        assert_eq!(encode_graph(&decode_graph(&bytes).unwrap()), bytes);
    }

    /// Architect note 2: an older umx (Ontology, TypeDecl and FieldDecl as before plan 079, each keeping unknown keys)
    /// reads a graph.json with the guide keys and writes every one of them back.
    #[test]
    fn an_older_reader_keeps_the_guide_keys() {
        use serde::{Deserialize, Serialize};
        use serde_json::Value;
        #[derive(Serialize, Deserialize)]
        struct OldField {
            name: String,
            kind: FieldKind,
            #[serde(flatten)]
            extra: BTreeMap<String, Value>,
        }
        #[derive(Serialize, Deserialize)]
        struct OldType {
            name: String,
            fields: Vec<OldField>,
            #[serde(flatten)]
            extra: BTreeMap<String, Value>,
        }
        #[derive(Serialize, Deserialize)]
        struct OldOntology {
            types: Vec<OldType>,
            statuses: Vec<String>,
            relations: Vec<String>,
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            symmetric: Vec<String>,
            #[serde(flatten)]
            extra: BTreeMap<String, Value>,
        }
        #[derive(Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct OldMeta {
            umx: u32,
            id: Id,
            alias: String,
            title: String,
            created: String,
            ontology: OldOntology,
        }
        let meta = described_meta();
        let old: OldMeta = serde_json::from_slice(&encode_graph(&meta)).unwrap();
        let written = serde_json::to_vec_pretty(&old).unwrap();
        assert_eq!(decode_graph(&written).unwrap(), meta);
    }

    #[test]
    fn bad_lines_name_the_file_and_line() {
        let good = encode_nodes(&[node(1, "A")]);
        let mut bytes = good.clone();
        bytes.extend(b"{not json}\n");
        let err = decode_nodes(&bytes).unwrap_err();
        assert_eq!((err.file, err.line), (GraphFile::Nodes, 2));
        let secret = br#"{"id":"00000000000000000000000001","type":"SECRET TEXT"}"#;
        let err = decode_nodes(secret).unwrap_err();
        assert!(!err.to_string().contains("SECRET"), "{err}");
        assert!(decode_nodes(b"").unwrap().records.is_empty());
    }

    #[test]
    fn a_repeated_id_keeps_every_line_and_the_last_is_in_use() {
        // A git merge that kept both sides of an edit to node 1, plus an exact copy of node 2's line.
        let one = encode_nodes(&[node(1, "A")]);
        let two = encode_nodes(&[node(2, "B")]);
        let bytes = [
            one.clone(),
            two.clone(),
            two.clone(),
            encode_nodes(&[node(1, "A edited")]),
        ]
        .concat();
        let d = decode_nodes(&bytes).unwrap();
        let titles: Vec<&str> = d.records.iter().map(|n| n.title.as_str()).collect();
        assert_eq!(titles, ["B", "A edited"]);
        assert_eq!(
            d.repeats,
            vec![
                Repeat {
                    file: GraphFile::Nodes,
                    id: Id(1),
                    lines: vec![1, 4]
                },
                Repeat {
                    file: GraphFile::Nodes,
                    id: Id(2),
                    lines: vec![2, 3]
                },
            ]
        );
        assert_eq!(
            d.kept,
            vec![
                KeptLine {
                    id: Id(1),
                    line: one[..one.len() - 1].to_vec()
                },
                KeptLine {
                    id: Id(2),
                    line: two[..two.len() - 1].to_vec()
                },
            ]
        );
        // Writing back keeps every line, each kept one just before the line in use; a sorted file round-trips.
        let again = encode_nodes_keeping(&d.records, &d.kept);
        let sorted = [
            one.clone(),
            encode_nodes(&[node(1, "A edited")]),
            two.clone(),
            two.clone(),
        ]
        .concat();
        assert_eq!(again, sorted);
        let d2 = decode_nodes(&sorted).unwrap();
        assert_eq!(encode_nodes_keeping(&d2.records, &d2.kept), sorted);
        let edge = Edge {
            id: Id(9),
            kind: "blocks".into(),
            from_graph: None,
            from: Id(1),
            to_graph: None,
            to: Id(2),
            created: "2026-11-09T05:50:12Z".into(),
            created_by: "t".into(),
            authored_by_session: None,
            edited_by_session: None,
            evidence: Vec::new(),
            fields: BTreeMap::new(),
        };
        let e = encode_edges(&[edge]);
        let d = decode_edges(&[e.clone(), b"\n".to_vec(), e.clone()].concat()).unwrap();
        assert_eq!((d.records.len(), d.kept.len()), (1, 1));
        assert_eq!(d.repeats[0].lines, vec![1, 3]);
    }

    #[test]
    fn graph_json_format_version_is_checked_before_its_fields() {
        let meta = GraphMeta {
            spines: Vec::new(),
            former_aliases: Vec::new(),
            former_paths: Vec::new(),
            umx: FORMAT_VERSION,
            id: Id(7),
            alias: "demo".into(),
            title: "Demo".into(),
            created: "2026-11-09T05:50:12Z".into(),
            embedding: None,
            ontology: Ontology::default(),
        };
        let mut v: serde_json::Value = serde_json::from_slice(&encode_graph(&meta)).unwrap();
        v["umx"] = (FORMAT_VERSION + 1).into();
        v["a_field_from_the_future"] = true.into();
        let err = decode_graph(&serde_json::to_vec(&v).unwrap()).unwrap_err();
        assert_eq!(err.reason, DecodeReason::NewerFormat(FORMAT_VERSION + 1));
        v["umx"] = 0.into();
        let err = decode_graph(&serde_json::to_vec(&v).unwrap()).unwrap_err();
        assert_eq!(err.reason, DecodeReason::UnknownFormat(0));
        v["umx"] = FORMAT_VERSION.into();
        let err = decode_graph(&serde_json::to_vec(&v).unwrap()).unwrap_err();
        assert_eq!(
            err.reason,
            DecodeReason::Data,
            "today's format still denies unknown top-level keys"
        );
    }

    #[test]
    fn unknown_ontology_keys_survive_decode_and_encode() {
        let text = br#"{"umx":1,"id":"00000000000000000000000007","alias":"demo","title":"Demo","created":"2026-11-09T05:50:12Z",
            "ontology":{"types":[{"name":"risk","fields":[{"name":"likelihood","kind":"int","unit":"percent"}],"colour":"red"}],
            "statuses":[],"relations":[],"descriptions":{"risk":"a thing that may happen"}}}"#;
        let meta = decode_graph(text).unwrap();
        let again: serde_json::Value = serde_json::from_slice(&encode_graph(&meta)).unwrap();
        let o = &again["ontology"];
        assert_eq!(o["descriptions"]["risk"], "a thing that may happen");
        assert_eq!(o["types"][0]["colour"], "red");
        assert_eq!(o["types"][0]["fields"][0]["unit"], "percent");
        assert_eq!(decode_graph(&encode_graph(&meta)).unwrap(), meta);
    }

    const V1: &str = r#"{
  "umx": 1,
  "id": "00000000000000000000000007",
  "alias": "demo",
  "title": "Demo",
  "created": "2026-11-09T05:50:12Z",
  "ontology": {
    "types": [],
    "statuses": [],
    "relations": []
  }
}
"#;

    const V2: &str = r#"{
  "umx": 2,
  "id": "00000000000000000000000007",
  "alias": "demo",
  "title": "Demo",
  "created": "2026-11-09T05:50:12Z",
  "embedding": {
    "key": "nomic-embed-text-v1.5@256+p1",
    "text_form": 1,
    "policy": "secured",
    "colour": "red"
  },
  "ontology": {
    "types": [],
    "statuses": [],
    "relations": []
  }
}
"#;

    #[test]
    fn graph_json_round_trips_byte_for_byte_in_both_formats() {
        for text in [V1, V2] {
            let meta = decode_graph(text.as_bytes()).unwrap();
            assert_eq!(String::from_utf8(encode_graph(&meta)).unwrap(), text);
        }
        let e = decode_graph(V2.as_bytes()).unwrap().embedding.unwrap();
        assert_eq!(e.key.unwrap().to_string(), "nomic-embed-text-v1.5@256+p1");
        assert_eq!((e.text_form, e.policy), (1, crate::model::Policy::SECURED));
        assert_eq!(e.extra["colour"], "red");
    }

    #[test]
    fn relation_fields_are_written_only_when_declared_and_round_trip() {
        // F92 (graph-addressing-expert): no format bump. A graph with no relation fields keeps its bytes; one with them
        // round-trips.
        let mut meta = decode_graph(V1.as_bytes()).unwrap();
        assert!(meta.ontology.relation_fields.is_empty());
        assert_eq!(String::from_utf8(encode_graph(&meta)).unwrap(), V1);
        meta.ontology.relation_fields.insert(
            "works_for".into(),
            vec![
                FieldDecl::new("starts_on", FieldKind::Date),
                FieldDecl::new("role_title", FieldKind::String),
            ],
        );
        let bytes = encode_graph(&meta);
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(
            text.contains(
                "\"relations\": [],\n    \"relation_fields\": {\n      \"works_for\": [\n        {\n          \"name\": \"starts_on\",\n          \"kind\": \"date\"\n        },"
            ),
            "{text}"
        );
        let again = decode_graph(&bytes).unwrap();
        assert_eq!(again.ontology, meta.ontology);
        assert_eq!(
            again
                .ontology
                .relation_field_kind("works_for", "role_title"),
            Some(FieldKind::String)
        );
        assert_eq!(
            again.ontology.relation_field_kind("member_of", "starts_on"),
            None
        );
        assert_eq!(encode_graph(&again), bytes);
        meta.ontology.relation_fields.clear();
        assert_eq!(String::from_utf8(encode_graph(&meta)).unwrap(), V1);
    }

    #[test]
    fn graph_json_version_follows_the_embedding_section_and_never_drops() {
        let mut meta = decode_graph(V1.as_bytes()).unwrap();
        let mut section = GraphEmbedding::new(None, crate::model::Policy::OPEN);
        assert_eq!(
            section.text_form, 2,
            "a new section records the current form"
        );
        section.text_form = 1;
        meta.embedding = Some(section);
        let text = String::from_utf8(encode_graph(&meta)).unwrap();
        assert!(text.contains("\"umx\": 2"), "a section needs format 2");
        assert!(
            text.contains("\"key\": null") && text.contains("\"policy\": \"open\""),
            "all three known keys are written"
        );
        let mut v2 = decode_graph(V2.as_bytes()).unwrap();
        v2.embedding = None;
        let text = String::from_utf8(encode_graph(&v2)).unwrap();
        assert!(
            text.contains("\"umx\": 2"),
            "dropping the section keeps format 2"
        );
        assert_eq!(decode_graph(text.as_bytes()).unwrap(), v2);
    }

    #[test]
    fn graph_json_embedding_rules() {
        let v1_with_section = V2.replace("\"umx\": 2", "\"umx\": 1");
        assert_eq!(
            decode_graph(v1_with_section.as_bytes()).unwrap_err().reason,
            DecodeReason::Data,
            "format 1 has no embedding key"
        );
        let newer = FORMAT_VERSION + 1;
        let v3 = V2.replace("\"umx\": 2", &alloc::format!("\"umx\": {newer}"));
        assert_eq!(
            decode_graph(v3.as_bytes()).unwrap_err().reason,
            DecodeReason::NewerFormat(newer)
        );
        let null_section = |umx: &str| {
            V1.replace("\"umx\": 1", umx).replace(
                "\"created\": \"2026-11-09T05:50:12Z\",",
                "\"created\": \"2026-11-09T05:50:12Z\",\n  \"embedding\": null,",
            )
        };
        for bad in [
            null_section("\"umx\": 1"),
            null_section("\"umx\": 2"),
            V2.replace("nomic-embed-text-v1.5@256+p1", "Nomic/x@256"),
            V2.replace("\"secured\"", "\"public\""),
        ] {
            assert_eq!(
                decode_graph(bad.as_bytes()).unwrap_err().reason,
                DecodeReason::Data
            );
        }
    }

    const V4: &str = r#"{
  "umx": 4,
  "id": "00000000000000000000000007",
  "alias": "demo",
  "title": "Demo",
  "created": "2026-11-09T05:50:12Z",
  "ontology": {
    "types": [
      {
        "name": "doc",
        "fields": [
          {
            "name": "file",
            "kind": "string",
            "role": "data"
          },
          {
            "name": "summary",
            "kind": "string"
          }
        ]
      }
    ],
    "statuses": [],
    "relations": []
  }
}
"#;

    #[test]
    fn field_roles_and_text_form_two_need_their_format_and_an_older_build_refuses_them() {
        use crate::model::{FORMAT_EXTERNAL_ENDS, FORMAT_FIELD_ROLES};
        let meta = decode_graph(V4.as_bytes()).unwrap();
        assert_eq!(
            String::from_utf8(encode_graph(&meta)).unwrap(),
            V4,
            "byte for byte"
        );
        let file = meta.ontology.field_decl("doc", "file").unwrap();
        assert_eq!(file.role, Some(crate::ontology::Role::Data));
        assert_eq!(
            meta.ontology.field_decl("doc", "summary").unwrap().role,
            None
        );
        assert_eq!(
            decode_graph_with_version(V4.as_bytes(), FORMAT_FIELD_ROLES - 1)
                .unwrap_err()
                .reason,
            DecodeReason::NewerFormat(FORMAT_FIELD_ROLES)
        );
        // Under a lower number the role is a hand edit an older umx would ignore: refused.
        let low = V4.replace(
            "\"umx\": 4",
            &alloc::format!("\"umx\": {FORMAT_EXTERNAL_ENDS}"),
        );
        assert_eq!(
            decode_graph(low.as_bytes()).unwrap_err().reason,
            DecodeReason::Data
        );
        // Text form 2 needs it too, so an older umx never writes v1 lines into a v2 graph.
        let mut v2 = decode_graph(V2.as_bytes()).unwrap();
        v2.embedding.as_mut().unwrap().text_form = 2;
        let text = String::from_utf8(encode_graph(&v2)).unwrap();
        assert!(text.contains("\"umx\": 4"), "{text}");
        assert!(
            decode_graph_with_version(text.as_bytes(), FORMAT_FIELD_ROLES - 1).is_err(),
            "an older build refuses it"
        );
        // A graph without either keeps its number.
        for old in [V1, V2] {
            let meta = decode_graph(old.as_bytes()).unwrap();
            assert_eq!(String::from_utf8(encode_graph(&meta)).unwrap(), old);
        }
    }

    #[test]
    fn graph_json_writes_d53_pairs_so_an_old_reader_sees_secured_and_refuses_pairs_never_written() {
        use crate::model::{EmbedderRule, Policy, Visibility};
        let legacy = decode_graph(V2.as_bytes()).unwrap();
        assert_eq!(
            legacy.file_policy(),
            Policy::SECURED,
            "policy secured keeps today's meaning: named + local"
        );
        let mut meta = legacy.clone();
        let named_trusted = Policy {
            visibility: Visibility::Named,
            embedder: EmbedderRule::Trusted,
        };
        meta.embedding.as_mut().unwrap().policy = named_trusted;
        let text = String::from_utf8(encode_graph(&meta)).unwrap();
        assert!(
            text.contains("\"policy\": \"secured\",\n    \"visibility\": \"named\",\n    \"embedder\": \"trusted\""),
            "{text}"
        );
        assert_eq!(decode_graph(text.as_bytes()).unwrap(), meta);
        // An older umx reads only `policy`, and the extra keys are kept: it sees secured.
        #[derive(serde::Deserialize)]
        struct Old {
            embedding: OldSection,
        }
        #[derive(serde::Deserialize)]
        struct OldSection {
            policy: String,
        }
        let old: Old = serde_json::from_str(&text).unwrap();
        assert_eq!(old.embedding.policy, "secured");
        // Legacy pairs keep their exact bytes.
        assert_eq!(String::from_utf8(encode_graph(&legacy)).unwrap(), V2);
        // Every pair the writer never writes is refused at load, like a bad word.
        let section = |body: &str| V2.replace("\"policy\": \"secured\",", body);
        // A pair beside a policy that does not match it (an older umx rewrote policy to open, or a hand edit) is
        // refused with a reason that names both keys and the fix (F1).
        for bad in [
            "\"policy\": \"secured\", \"visibility\": \"open\", \"embedder\": \"any\",",
            "\"policy\": \"secured\", \"visibility\": \"named\", \"embedder\": \"local\",",
            "\"policy\": \"secured\", \"embedder\": \"local\",",
            "\"policy\": \"open\", \"visibility\": \"named\", \"embedder\": \"trusted\",",
            "\"policy\": \"open\", \"embedder\": \"trusted\",",
            "\"visibility\": \"named\", \"embedder\": \"trusted\",",
        ] {
            let reason = decode_graph(section(bad).as_bytes()).unwrap_err().reason;
            assert_eq!(reason, DecodeReason::PolicyPair, "{bad}");
            let said = reason.to_string();
            assert!(
                said.contains("visibility and embedder")
                    && said.contains("remove both keys or set policy to secured")
                    && !said.contains(',')
                    && !said.contains(';'),
                "{said}"
            );
        }
        // A word no umx writes is a plain data error.
        for bad in [
            "\"policy\": \"secured\", \"embedder\": \"hosted\",",
            "\"policy\": \"custom\",",
        ] {
            assert_eq!(
                decode_graph(section(bad).as_bytes()).unwrap_err().reason,
                DecodeReason::Data,
                "{bad}"
            );
        }
        // One key alone is read beside what policy means for the other.
        let one =
            decode_graph(section("\"policy\": \"secured\", \"embedder\": \"trusted\",").as_bytes())
                .unwrap();
        assert_eq!(one.file_policy(), named_trusted);
    }

    #[test]
    fn external_ends_round_trip_in_pinned_order_and_require_format_three() {
        let mut edge = Edge {
            id: Id(9),
            kind: "blocks".into(),
            from_graph: Some(Id(8)),
            from: Id(1),
            to_graph: Some(Id(6)),
            to: Id(2),
            created: "now".into(),
            created_by: "t".into(),
            authored_by_session: None,
            edited_by_session: None,
            evidence: Vec::new(),
            fields: BTreeMap::from([("unknown".into(), json!({"a": [1, null]}))]),
        };
        let bytes = encode_edges_for_graph(Id(7), [&edge]).unwrap();
        assert_eq!(
            core::str::from_utf8(&bytes).unwrap(),
            "{\"id\":\"00000000000000000000000009\",\"type\":\"blocks\",\"from_graph\":\"00000000000000000000000008\",\"from\":\"00000000000000000000000001\",\"to_graph\":\"00000000000000000000000006\",\"to\":\"00000000000000000000000002\",\"created\":\"now\",\"created_by\":\"t\",\"unknown\":{\"a\":[1,null]}}\n"
        );
        for (from, to) in [
            (Some(Id(8)), None),
            (None, Some(Id(6))),
            (Some(Id(8)), Some(Id(6))),
            (None, None),
        ] {
            edge.from_graph = from;
            edge.to_graph = to;
            let bytes = encode_edges_for_graph(Id(7), [&edge]).unwrap();
            assert_eq!(decode_edges(&bytes).unwrap().records, [edge.clone()]);
            assert_eq!(encode_edges(&decode_edges(&bytes).unwrap().records), bytes);
            for text in [V1, V2] {
                let meta = decode_graph(text.as_bytes()).unwrap();
                let graph = encode_graph_with_edges(&meta, [&edge]).unwrap();
                if edge.has_external_end() {
                    let raised = decode_graph(&graph).unwrap();
                    assert_eq!(raised.umx, 3);
                    assert_eq!(
                        decode_graph_with_version(&graph, crate::model::FORMAT_EXTERNAL_ENDS - 1)
                            .unwrap_err()
                            .reason,
                        DecodeReason::NewerFormat(3)
                    );
                    assert_eq!(
                        decode_graph(&encode_graph_with_edges(&raised, []).unwrap())
                            .unwrap()
                            .umx,
                        3
                    );
                } else {
                    assert_eq!(graph, text.as_bytes());
                }
            }
        }
        for (from, to, end) in [(Some(Id(7)), None, "from"), (None, Some(Id(7)), "to")] {
            edge.from_graph = from;
            edge.to_graph = to;
            assert_eq!(
                encode_edges_for_graph(Id(7), [&edge]).unwrap_err().reason,
                DecodeReason::OwnGraphEnd(end)
            );
            assert!(
                encode_graph_with_edges(&decode_graph(V1.as_bytes()).unwrap(), [&edge]).is_err()
            );
        }
        for key in ["from_graph", "to_graph"] {
            let mut value = serde_json::to_value(&edge).unwrap();
            value[key] = serde_json::Value::Null;
            assert_eq!(
                decode_edges(&serde_json::to_vec(&value).unwrap())
                    .unwrap_err()
                    .reason,
                DecodeReason::Data
            );
        }
    }

    #[test]
    fn every_existing_edge_fixture_keeps_its_bytes_and_has_no_graph_keys() {
        for bytes in [
            include_bytes!("../../umx-cypher/fixtures/design/edges.jsonl").as_slice(),
            include_bytes!("../../umx/tests/fixtures/query-graph/edges.jsonl").as_slice(),
        ] {
            let decoded = decode_edges(bytes).unwrap();
            assert!(decoded.records.iter().all(|edge| !edge.has_external_end()));
            assert_eq!(encode_edges(&decoded.records), bytes);
        }
    }
}
