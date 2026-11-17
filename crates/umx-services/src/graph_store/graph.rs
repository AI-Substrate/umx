//! graph new | load | list.

use super::ignore::ignore_hides_temp_files;
use super::{
    Ctx, Done, GraphStore, Loaded, Outcome, Page, PolicyView, alias_name, off_vocabulary, page,
    problems,
};
use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::codec::{self, DecodeReason, GraphFile};
use umx_core::ontology::TextFields;
use umx_core::ports::{Change, FileRef, FilesError, GraphDir, GraphStamps, Written};
use umx_core::{Command, GraphEmbedding, GraphMeta, Id, Ontology, Problem, ValidName};
use umx_graph::{EdgesLoad, Graph, NodesLoad};

pub(super) type OwnEnds = Vec<(Id, &'static str, usize)>;

#[derive(Clone, Debug, PartialEq)]
pub struct GraphInfo {
    pub id: Id,
    pub alias: String,
    pub title: String,
    pub path: String,
    pub nodes: usize,
    pub edges: usize,
    /// When a command last used the graph (ISO 8601 UTC).
    pub last_used: String,
    /// Seconds until idle unload, or None when idle unload is off.
    pub unloads_in_s: Option<u64>,
    /// Lines in the files that are not in the graph: earlier lines of repeated ids (req-67) and dangling edges
    /// (req-76). Nodes plus edges plus kept lines is every record line on disk.
    pub kept_lines: usize,
    /// graph.json's embedding section: the graph's model key and policy (D32), if it has one.
    pub embedding: Option<GraphEmbedding>,
    /// The policy umx acts on, and when a person last lowered it (ws004 s06 A2).
    pub policy: PolicyView,
    /// Nodes whose vectors in the graph's own model are missing or stale (req-75); None when the graph has no
    /// model yet and there is no embedder.
    pub vectors_pending: Option<usize>,
    /// Nodes whose vectors wait only because the secret hold keeps a text of theirs from the hosted embedder.
    pub vectors_held: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GraphNew {
    pub graph: GraphInfo,
    pub types: usize,
    pub statuses: usize,
    pub relations: usize,
    /// Commands that change the new graph's vocabulary and settings (req-71).
    pub next: Vec<Command>,
    /// The repo Graphfile line this create added (ws002 s17), with its undo; None when it added none.
    pub graphfile: Option<super::graphfile::GraphfileAdded>,
}

/// One finding of `umx check`: what (`duplicate_id`, `unknown_key`), in which file, about what (an id, or a key
/// path, which is graph text), and on which lines (1-based; empty when lines do not apply).
/// `umx check`'s answer (F113): the rows, and with a list or scope each checked graph's own count, in scope order
/// (`page.graphs` names each row's graph).
#[derive(Clone, Debug, PartialEq)]
pub struct Checked {
    pub page: Page<CheckRow>,
    pub graphs: Vec<(String, usize)>,
    /// Each checked graph's spine line (B4), in scope order: data, never a warning.
    pub spines: Vec<(String, super::SpineLine)>,
    /// `--lint` (F139): what the lint found in the checked graphs; None without `--lint`.
    pub lint: Option<super::lint::Lint>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CheckRow {
    pub kind: &'static str,
    pub file: FileRef,
    pub subject: String,
    pub lines: Vec<usize>,
}

/// At most this many load-rule warnings (`unsorted_file`, `duplicate_id`, `dangling_edge`, `unknown_key`) per load;
/// one more counts the rest.
pub const MAX_LOAD_WARNINGS: usize = 20;

/// Keys under graph.json's `embedding` and `ontology` this build does not know, as paths from the top, each once,
/// sorted.
fn unknown_keys(meta: &GraphMeta) -> BTreeSet<Vec<&str>> {
    let mut out = BTreeSet::new();
    for k in meta.embedding.iter().flat_map(|e| e.extra.keys()) {
        out.insert(alloc::vec!["embedding", k.as_str()]);
    }
    let o = &meta.ontology;
    for k in o.extra.keys() {
        out.insert(alloc::vec!["ontology", k.as_str()]);
    }
    for t in &o.types {
        for k in t.extra.keys() {
            out.insert(alloc::vec!["ontology", "types", k.as_str()]);
        }
        for f in &t.fields {
            for k in f.extra.keys() {
                out.insert(alloc::vec!["ontology", "types", "fields", k.as_str()]);
            }
        }
    }
    out
}

/// F96: one `type_column_unknown` per default column a type does not declare (a hand edit, or a field removed
/// later); node list skips them. A type whose name is not a valid name is left to the other load rules.
fn type_column_rules(alias: &ValidName, meta: &GraphMeta) -> Vec<Problem> {
    let mut out = Vec::new();
    for t in &meta.ontology.types {
        let Some(kind) = ValidName::new(&t.name) else {
            continue;
        };
        for c in t.columns.iter().filter(|c| !t.column_known(c)) {
            out.push(problems::type_column_unknown(alias, &kind, c));
        }
    }
    out
}

fn decode_meta(dir: &GraphDir, bytes: &[u8]) -> Result<GraphMeta, Problem> {
    codec::decode_graph(bytes).map_err(|e| match e.reason {
        DecodeReason::NewerFormat(found) => problems::graph_format_newer(dir, found),
        _ => problems::graph_invalid(&e),
    })
}

/// nodes.jsonl, then edges.jsonl, each loaded in full before the next is read: the first bad line wins. A reversed
/// pair of a relation in `symmetric` is one edge (D49; `Graph::load_edges_symmetric`).
pub(super) fn load_files(
    read: impl Fn(GraphFile) -> Result<Option<Vec<u8>>, Problem>,
    text_fields: TextFields,
    symmetric: &[&str],
    owner: Id,
) -> Result<(Graph, NodesLoad, EdgesLoad, OwnEnds), Problem> {
    let node_bytes = read(GraphFile::Nodes)?.unwrap_or_default();
    // What the image holds: the file, and the last newline if it has none.
    let size =
        node_bytes.len() as u64 + u64::from(!node_bytes.is_empty() && !node_bytes.ends_with(b"\n"));
    let (mut graph, nodes) = Graph::load_nodes_indexing_for_graph(node_bytes, text_fields, owner)
        .map_err(|e| problems::decode_failed(&e, size))?;
    let edge_bytes = read(GraphFile::Edges)?.unwrap_or_default();
    // B3 (s20): the edges load as read, and the columns and kept lines say whether any end names this graph. Only
    // then, or when the load fails, is the file scanned: the old order (normalize, then load) runs from the mark,
    // so its warnings and its first error are the ones reported, never the first pass's.
    let mark = graph.edges_mark();
    let first = graph.load_edges(&edge_bytes);
    let (edge_bytes, mut edges, own_ends) = match first {
        Ok(edges) if external_ends(&graph, owner).is_some() => (edge_bytes, edges, Vec::new()),
        _ => {
            graph.reset_edges(mark);
            let (edge_bytes, own_ends) = normalize_own_ends(edge_bytes, owner)?;
            let edges = graph
                .load_edges(&edge_bytes)
                .map_err(|e| problems::decode_failed(&e, edge_bytes.len() as u64))?;
            (edge_bytes, edges, own_ends)
        }
    };
    let reversed = super::external::reversed_pairs(&graph, owner, symmetric);
    if !reversed.is_empty() {
        let lines = umx_graph::edge_lines(&edge_bytes, &reversed)
            .map_err(|e| problems::decode_failed(&e, edge_bytes.len() as u64))?;
        edges.reversed.extend(graph.keep_reversed(&reversed, lines));
    }
    Ok((graph, nodes, edges, own_ends))
}

/// Whether any edge end is in another graph, from the end columns and the kept lines, never a scan of edges.jsonl
/// (B3, s20). `None` when an end names `owner`, or a kept line that may hold an end does not decode: only the old
/// scan of the file then gives the exact warning or error.
pub(super) fn external_ends(graph: &Graph, owner: Id) -> Option<bool> {
    let mut any = false;
    for g in graph.external_end_graphs() {
        if g == owner {
            return None;
        }
        any = true;
    }
    for line in graph.kept_edge_lines().filter(|l| has_graph_key(l)) {
        for edge in codec::decode_edges(line).ok()?.records {
            if edge.from_graph == Some(owner) || edge.to_graph == Some(owner) {
                return None;
            }
            any |= edge.has_external_end();
        }
    }
    Some(any)
}

pub(super) fn has_graph_key(line: &[u8]) -> bool {
    line.contains(&b'\\') || line.windows(7).any(|w| w == b"_graph\"")
}

/// Rewrites edge ends that name the owning graph's own id as local ends. Walk and path rely on it: a loaded graph's
/// edges never name its own id (`traversal::differential::a_graphs_own_edges_never_name_its_own_id` pins it).
pub(super) fn normalize_own_ends(bytes: Vec<u8>, owner: Id) -> Result<(Vec<u8>, OwnEnds), Problem> {
    let mut rewritten: Option<Vec<u8>> = None;
    let mut warnings = Vec::new();
    let mut at = 0;
    for (line_no, line) in bytes.split_inclusive(|byte| *byte == b'\n').enumerate() {
        // Escaped keys must use the decoder too; a raw-key-only check would miss from_\\u0067raph.
        let candidate = has_graph_key(line);
        let mut changed = false;
        if candidate && !line.iter().all(u8::is_ascii_whitespace) {
            let mut decoded = codec::decode_edges(line).map_err(|e| {
                problems::graph_invalid(&codec::DecodeError {
                    line: line_no + 1,
                    ..e
                })
            })?;
            if let Some(edge) = decoded.records.first_mut() {
                for (end, graph) in [("from", &mut edge.from_graph), ("to", &mut edge.to_graph)] {
                    if *graph == Some(owner) {
                        *graph = None;
                        changed = true;
                        warnings.push((edge.id, end, line_no + 1));
                    }
                }
                if changed {
                    let out = rewritten.get_or_insert_with(|| bytes[..at].to_vec());
                    out.extend(codec::encode_edges([&*edge]));
                }
            }
        }
        if !changed && let Some(out) = &mut rewritten {
            out.extend_from_slice(line);
        }
        at += line.len();
    }
    Ok((rewritten.unwrap_or(bytes), warnings))
}

#[derive(Clone, Debug, PartialEq)]
pub struct GraphLoaded {
    pub graph: GraphInfo,
    pub off_vocabulary: usize,
    /// The folder was already loaded and this load replaced it (it keeps its place in load order).
    pub replaced: bool,
    /// ws006 S23: this machine knew the graph at another folder that no longer holds it (a `git mv` by hand), so the
    /// load is a move: the registry row now names this folder. With the `root:repo` pointers into the folder it
    /// left, listed once (a load writes no graph.json, so `umx check` cannot name them later; `graph move` does).
    pub moved_from: Option<GraphDir>,
    pub stale: Vec<(Id, String)>,
    pub stale_total: usize,
}

fn info(l: &Loaded, alias: &str, now_ms: u64, idle_ms: u64) -> GraphInfo {
    let used = l.last_used_ms();
    GraphInfo {
        id: l.meta.id,
        alias: alias.to_string(),
        title: l.meta.title.clone(),
        path: l.dir.as_str().to_string(),
        nodes: l.read_scope().node_count(),
        edges: l.read_scope().edges().count(),
        last_used: umx_core::time::iso_from_ms(used),
        kept_lines: l.graph.kept_line_count(),
        embedding: l.meta.embedding.clone(),
        policy: l.policy.clone(),
        vectors_pending: l.pending_count(),
        vectors_held: l.held_count(),
        unloads_in_s: (idle_ms > 0).then(|| {
            used.saturating_add(idle_ms)
                .saturating_sub(now_ms)
                .div_ceil(1000)
        }),
    }
}

fn dir(path: &str) -> Result<GraphDir, Problem> {
    GraphDir::parse(path).map_err(|_| {
        problems::usage_invalid("the path must be absolute with no . or .. parts", "graph")
    })
}

pub(super) fn files_problem(
    dir: &GraphDir,
    roots: &[GraphDir],
    file: GraphFile,
    e: FilesError,
) -> Problem {
    match e {
        FilesError::OutsideRoots(_) => problems::path_outside_roots(dir, roots),
        FilesError::RecoveryConflict(files) => problems::recovery_conflict(dir, &files, None),
        FilesError::TooLarge { file, size, cap } => problems::graph_too_large(&file, size, cap),
        e => problems::write_failed(file, &e),
    }
}

/// The `.gitignore` line that keeps umx's temp and marker files out of git (workshop 008 section 4.1).
pub const IGNORE_LINE: &str = ".umx-tmp-*";

/// Whether some line is exactly umx's own (`.umx-tmp-*` or `/.umx-tmp-*`, only trailing whitespace ignored, as git
/// does: a line like `  .umx-tmp-*` is another pattern, review R54-2).
fn has_umx_line(gitignore: &[u8]) -> bool {
    gitignore.split(|b| *b == b'\n').any(|line| {
        let line = line.trim_ascii_end();
        line == IGNORE_LINE.as_bytes() || line.strip_prefix(b"/") == Some(IGNORE_LINE.as_bytes())
    })
}

/// `.gitignore` with umx's line appended, or None if umx's own line is there and git hides every temp and marker
/// name (architect: a wider line someone wrote is not read as umx's; a redundant line is harmless). Appended last,
/// the line also wins over an earlier `!` line. The existing bytes are kept exactly; a newline goes first if the
/// file does not end with one.
fn with_ignore_line(existing: Option<&[u8]>) -> Option<Vec<u8>> {
    let old = existing.unwrap_or_default();
    if has_umx_line(old) && ignore_hides_temp_files(existing) {
        return None;
    }
    let mut out = old.to_vec();
    if !out.is_empty() && !out.ends_with(b"\n") {
        out.push(b'\n');
    }
    out.extend_from_slice(IGNORE_LINE.as_bytes());
    out.push(b'\n');
    Some(out)
}

/// `<alias>_2`, for the alias_in_use next.
pub(crate) fn second(alias: &ValidName) -> ValidName {
    ValidName::alias(&format!("{}_2", alias.as_str())).unwrap_or_else(|| alias.clone())
}

impl GraphStore {
    /// Creates a graph's files with the starter ontology (unless `empty`) and loads it. `embedding` is the graph's
    /// model key and policy (D32), written only when given.
    pub fn graph_new(
        &mut self,
        _ctx: &Ctx,
        path: &str,
        alias: &str,
        title: Option<&str>,
        empty: bool,
        embedding: Option<GraphEmbedding>,
    ) -> Outcome<GraphNew> {
        let dir = self.identity(path)?;
        let fix = |good: &ValidName| {
            Command::new("graph new")
                .path(&dir)
                .word("--alias")
                .name(good)
        };
        let Some(alias) = ValidName::alias(alias) else {
            return Err(problems::alias_invalid(alias, &fix));
        };
        if self
            .graphs
            .get(alias.as_str())
            .is_some_and(|l| l.dir != dir)
        {
            return Err(problems::alias_in_use(&alias, fix(&second(&alias))));
        }
        let files = &self.ports.files;
        let present = files
            .list(&dir)
            .map_err(|e| files_problem(&dir, &self.roots, GraphFile::Graph, e))?;
        // A folder is a graph when it has graph.json; files left by a failed create do not block a retry.
        if present.contains(&GraphFile::Graph) {
            return Err(problems::graph_exists(&dir));
        }
        // Record files without graph.json are refused, never adopted or truncated: a new graph starts empty, so
        // its first write would erase them. Empty ones (left by a failed create) are fine.
        for file in [GraphFile::Nodes, GraphFile::Edges] {
            if present.contains(&file) {
                let bytes = files
                    .read(&dir, file)
                    .map_err(|e| files_problem(&dir, &self.roots, file, e))?
                    .unwrap_or_default();
                if !bytes.iter().all(u8::is_ascii_whitespace) {
                    return Err(problems::graph_orphan_records(&dir, file));
                }
            }
        }
        let mut meta = GraphMeta {
            spines: Vec::new(),
            former_aliases: Vec::new(),
            former_paths: Vec::new(),
            umx: 1,
            id: self.ports.ids.next_id(),
            alias: alias.as_str().to_string(),
            title: title.unwrap_or(alias.as_str()).to_string(),
            created: self.now(),
            embedding,
            ontology: if empty {
                Ontology::default()
            } else {
                Ontology::starter()
            },
        };
        meta.umx = meta.needed_format();
        // One set, all or nothing: the record files this call creates (existing empty ones are never
        // overwritten), graph.json, and the .gitignore line for umx's temp and marker files.
        let ignore = files
            .read(&dir, GraphFile::GitIgnore)
            .map_err(|e| files_problem(&dir, &self.roots, GraphFile::GitIgnore, e))?;
        let ignore = with_ignore_line(ignore.as_deref());
        let graph_bytes =
            codec::encode_graph_with_edges(&meta, []).map_err(|e| problems::graph_invalid(&e))?;
        let mut changes: Vec<Change<'_>> = [GraphFile::Nodes, GraphFile::Edges]
            .into_iter()
            .filter(|f| !present.contains(f))
            .map(|file| Change {
                file: file.into(),
                bytes: Some(&[][..]),
            })
            .collect();
        changes.push(Change {
            file: GraphFile::Graph.into(),
            bytes: Some(&graph_bytes),
        });
        if let Some(bytes) = &ignore {
            changes.push(Change {
                file: GraphFile::GitIgnore.into(),
                bytes: Some(bytes),
            });
        }
        let which: Vec<GraphFile> = changes
            .iter()
            .filter_map(|c| match &c.file {
                FileRef::Graph(f) => Some(*f),
                FileRef::Shard(_) | FileRef::SimilarMeta(_) | FileRef::Receipts => None,
            })
            .collect();
        let mut warnings = Vec::new();
        // A new graph has nothing loaded to compare with: no check, and the stamps it returns are the load's.
        let stamps = match files.write_set_checked(&dir, &changes, None) {
            Ok(committed) => {
                if let Written::NotSynced(e) = &committed.written {
                    warnings.push(problems::write_not_durable(super::named(&which, e), e));
                }
                committed.stamps
            }
            Err(e) => {
                let file = super::named(&which, &e);
                return Err(files_problem(&dir, &self.roots, file, e));
            }
        };
        let o = &meta.ontology;
        let (types, statuses, relations) = (o.types.len(), o.statuses.len(), o.relations.len());
        let now_ms = self.ports.clock.now_ms();
        let loaded = Loaded::new(dir, meta, Graph::new(), now_ms, stamps);
        if let Some(old) = self.graphs.get(alias.as_str()) {
            self.incoming.remove_source(old.instance);
        }
        self.resets.insert(alias.as_str().to_string());
        self.graphs.insert(alias.as_str().to_string(), loaded);
        warnings.extend(self.settle_policy(alias.as_str()));
        warnings.extend(self.remember(alias.as_str(), false));
        let graph = info(
            &self.graphs[alias.as_str()],
            alias.as_str(),
            now_ms,
            self.idle_ms,
        );
        let next = super::settings::change_commands(&alias);
        Ok(Done {
            value: GraphNew {
                graph,
                types,
                statuses,
                relations,
                next,
                graphfile: None,
            },
            notes: warnings,
        })
    }

    /// Loads a graph's files into memory. If that physical folder is already loaded (by any spelling of its
    /// path), the new load replaces it, so there is only ever one working copy. `as_alias` overrides the alias in
    /// graph.json for this daemon session.
    pub fn graph_load(
        &mut self,
        ctx: &Ctx,
        path: &str,
        as_alias: Option<&str>,
    ) -> Outcome<GraphLoaded> {
        let mut done = self.graph_load_expecting(ctx, path, as_alias, None)?;
        let in_memory = done.value.replaced;
        let alias = done.value.graph.alias.clone();
        if !in_memory && let Some((from, stale, total)) = self.heal_moved(&alias) {
            done.value.moved_from = Some(from);
            done.value.stale = stale;
            done.value.stale_total = total;
        }
        done.notes.extend(self.remember(&alias, in_memory));
        Ok(done)
    }

    /// [`Self::graph_load`] without touching the registry. With `expect`, a folder whose graph.json holds another
    /// graph id is refused before anything is loaded, as `graph_id_changed`.
    pub(crate) fn graph_load_expecting(
        &mut self,
        _ctx: &Ctx,
        path: &str,
        as_alias: Option<&str>,
        expect: Option<Id>,
    ) -> Outcome<GraphLoaded> {
        let dir = self.identity(path)?;
        let (files, roots) = (&self.ports.files, &self.roots);
        // Each content file's stamp from the handle its bytes came from (F144): a later write refuses when one moved.
        let stamps = core::cell::Cell::new(GraphStamps::ABSENT);
        let read = |file| {
            let (bytes, stamp) = files
                .read_stamped(&dir, file)
                .map_err(|e| files_problem(&dir, roots, file, e))?;
            let mut now = stamps.get();
            now.set(file, stamp);
            stamps.set(now);
            Ok(bytes)
        };
        let meta_bytes = read(GraphFile::Graph)?.ok_or_else(|| problems::graph_not_found(&dir))?;
        let meta = decode_meta(&dir, &meta_bytes)?;
        if expect.is_some_and(|id| id != meta.id) {
            return Err(problems::graph_id_changed(
                as_alias.unwrap_or(&meta.alias),
                &dir,
            ));
        }
        // Nodes then edges, each checked in full before the next file is read: the first bad line wins. The word
        // index is built in the same pass over the nodes, over the fields the ontology declares as text (req-68).
        let (graph, nodes, edges, own_ends) = load_files(
            read,
            meta.ontology.text_fields(),
            &meta.ontology.symmetric_kinds(),
            meta.id,
        )?;
        let fix = |good: &ValidName| {
            Command::new("graph load")
                .path(&dir)
                .word("--as")
                .name(good)
        };
        // ws006 S23: a name graph.json lists as a former alias loads under the live one, so a registry row a rename
        // did not reach (a crash after graph.json) heals here, and `--as <former>` is the graph's live alias.
        let given = match as_alias {
            Some(a) if meta.former_aliases.iter().any(|f| f == a) => &meta.alias,
            Some(a) => a,
            None => &meta.alias,
        };
        let Some(alias) = ValidName::alias(given) else {
            return Err(problems::alias_invalid(given, &fix));
        };
        if self
            .graphs
            .get(alias.as_str())
            .is_some_and(|l| l.dir != dir)
        {
            return Err(problems::alias_in_use(&alias, fix(&second(&alias))));
        }
        // Load rules (req-67, req-76, E-4): repeated ids, dangling edges and unknown graph.json keys load, are kept
        // and are reported.
        let mut rules: Vec<Problem> = [
            (GraphFile::Nodes, nodes.unsorted),
            (GraphFile::Edges, edges.unsorted),
        ]
        .into_iter()
        .filter_map(|(file, line)| Some(problems::unsorted_file(&alias, file, line?)))
        .collect();
        rules.extend(
            nodes
                .repeats
                .iter()
                .chain(&edges.repeats)
                .map(|r| problems::duplicate_id(&alias, r)),
        );
        rules.extend(
            nodes
                .invalid_provenance
                .iter()
                .chain(&edges.invalid_provenance)
                .map(|issue| problems::provenance_key_invalid(&alias, issue)),
        );
        rules.extend(
            nodes
                .duplicate_prov_keys
                .iter()
                .map(|d| problems::duplicate_prov_key(d.node, d.in_use)),
        );
        rules.extend(
            graph
                .nodes()
                .filter(|n| {
                    !graph.is_provenance(n.ix())
                        && umx_core::provenance::user_node_warning(n.kind()).is_some()
                })
                .map(|n| problems::reserved_type(&alias_name(n.kind()))),
        );
        rules.extend(
            edges
                .dangling
                .iter()
                .map(|d| problems::dangling_edge(&alias, d.edge, d.end, d.node)),
        );
        rules.extend(
            edges
                .reversed
                .iter()
                .map(|r| problems::symmetric_duplicate(&alias, r.edge, r.in_use)),
        );
        rules.extend(
            graph
                .duplicate_locators()
                .map(|d| problems::duplicate_locator(&alias, d.node, d.in_use)),
        );
        rules.extend(
            graph
                .unnormalised_locators()
                .map(|id| problems::locator_not_normalised(&alias, id)),
        );
        rules.extend(
            graph
                .kept_locator_fields()
                .into_iter()
                .map(|id| problems::locator_field_kept(&alias, id)),
        );
        rules.extend(
            own_ends
                .iter()
                .map(|(edge, end, _)| problems::own_graph_end(*edge, end)),
        );
        // req-124: an older edge's own `evidence` field that is not evidence is kept, never read as evidence.
        rules.extend(
            graph
                .edges_with_invalid_evidence()
                .map(problems::evidence_field_kept),
        );
        rules.extend(
            unknown_keys(&meta)
                .iter()
                .map(|path| problems::unknown_key(&alias, GraphFile::Graph, path)),
        );
        // ws008 E1: stored keys a hand edit left out of stored form, or that cannot be used at all.
        for (node, f) in graph.entity_key_findings() {
            if f.not_canonical {
                rules.push(problems::entity_key_not_canonical(&alias, node));
            }
            if f.unusable {
                rules.push(problems::entity_key_unusable(&alias, node));
            }
        }
        // ws008 E1: a key two nodes of one type hold; the lower id is the one a key lookup finds.
        let mut keyed: Vec<(Id, Id)> = Vec::new();
        for d in graph.duplicate_entity_keys() {
            if !keyed.contains(&(d.node, d.in_use)) {
                keyed.push((d.node, d.in_use));
                let kind = graph
                    .node(d.node)
                    .map(|n| n.kind().to_string())
                    .unwrap_or_default();
                rules.push(problems::duplicate_key(&alias, &kind, &d));
            }
        }
        if graph.nodes().any(|node| node.kind() == "graph") {
            rules.push(problems::reserved_type(&alias_name("graph")));
        }
        if let Some(reserved) = &nodes.graph_id_node {
            rules.insert(
                0,
                problems::node_id_is_graph_id(reserved.id, reserved.lines[0]),
            );
        }
        rules.extend(type_column_rules(&alias, &meta));
        let hidden = rules.len().saturating_sub(MAX_LOAD_WARNINGS);
        rules.truncate(MAX_LOAD_WARNINGS);
        if hidden > 0 {
            rules.push(problems::load_warnings_not_shown(&alias, hidden));
        }
        let mut warnings = rules;
        // A reload of a folder already loaded replaces that load and keeps its place in load order (d-21).
        let order = self.graphs.values().find(|l| l.dir == dir).map(|l| l.order);
        let replaced: Vec<u64> = self
            .graphs
            .values()
            .filter(|l| l.dir == dir)
            .map(|l| l.instance)
            .collect();
        for instance in replaced {
            self.incoming.remove_source(instance);
        }
        let resets = &mut self.resets;
        self.graphs.retain(|other, l| {
            let keep = l.dir != dir;
            if !keep {
                resets.insert(other.clone());
            }
            keep
        });
        self.resets.insert(alias.as_str().to_string());
        let now_ms = self.ports.clock.now_ms();
        let mut loaded = Loaded::new(dir, meta, graph, now_ms, stamps.get());
        loaded.edges_normalized = !own_ends.is_empty();
        if let Some(order) = order {
            loaded.order = order;
        }
        self.incoming.add_graph(loaded.instance, &loaded.graph);
        self.graphs.insert(alias.as_str().to_string(), loaded);
        warnings.extend(self.settle_policy(alias.as_str()));
        let loaded = &self.graphs[alias.as_str()];
        let result = GraphLoaded {
            graph: info(loaded, alias.as_str(), now_ms, self.idle_ms),
            off_vocabulary: off_vocabulary(loaded),
            replaced: order.is_some(),
            moved_from: None,
            stale: Vec::new(),
            stale_total: 0,
        };
        Ok(Done {
            value: result,
            notes: warnings,
        })
    }

    /// `umx check`: reads the selected graph's files from disk (the truth, req-01), not the working copy, and lists
    /// repeated ids (req-67) and unknown graph.json keys. Lines are as the files hold them now.
    ///
    /// F113: with `--graph a,b` or `--scope`, every graph in scope is checked, and the whole list or scope is the
    /// named set (d-20, d-21), so links between them resolve whatever their visibility. Each checked graph whose
    /// links reach graphs the request did not name gets one `link_targets_unnamed`, a count only: it is the same
    /// for an unloaded, a hidden and an open target, and never names one (security ruling a).
    pub fn check(&self, ctx: &Ctx, lint: Option<&super::lint::LintRequest>) -> Outcome<Checked> {
        let single = ctx.scope.is_none() && !ctx.graph.as_deref().is_some_and(|g| g.contains(','));
        if single {
            let alias = self.select(ctx, &[])?;
            let Done {
                value: (rows, unnamed, read, meta),
                notes: mut warnings,
            } = self.check_one(ctx, &alias)?;
            if unnamed > 0 {
                warnings.push(problems::link_targets_unnamed(&alias_name(&alias), unnamed));
            }
            warnings.extend(super::entity::check_key_types(&[(alias.as_str(), &read)]));
            warnings.extend(self.reserved_spines(&alias));
            let lint = lint.map(|req| {
                let (found, hints) = self.lint(&[(alias.as_str(), &read, meta.as_ref())], req);
                warnings.extend(hints);
                found
            });
            let total = rows.len();
            return Ok(Done {
                value: Checked {
                    page: page(&alias, rows, total, 0),
                    graphs: Vec::new(),
                    spines: alloc::vec![(alias.clone(), self.spine_of(&alias))],
                    lint,
                },
                notes: warnings,
            });
        }
        let scope = self.graphs_in_scope(ctx, None, "graph")?;
        // The named set is the whole request, in one order whatever order it was given in (graph-addressing-expert 3).
        // A graph's primary copy comes first, so when two copies are named its far ends resolve against the primary
        // (graph-addressing-expert N2).
        let mut named_list = scope.aliases.clone();
        named_list.sort_by_key(|a| {
            (
                self.primary_of(self.graphs[a].meta.id) != Some(a.as_str()),
                a.clone(),
            )
        });
        let named = Ctx {
            graph: Some(named_list.join(",")),
            scope: None,
            default_graph: None,
            ..ctx.clone()
        };
        let (mut rows, mut owners, mut graphs, mut spines, mut warnings) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut read: Vec<(&str, Graph, Option<GraphMeta>)> = Vec::new();
        for alias in &scope.aliases {
            let Done {
                value: (found, unnamed, decoded, meta),
                notes: more,
            } = self.check_one(&named, alias)?;
            read.push((alias.as_str(), decoded, meta));
            graphs.push((alias.clone(), found.len()));
            spines.push((alias.clone(), self.spine_of(alias)));
            owners.extend(core::iter::repeat_n(alias.clone(), found.len()));
            rows.extend(found);
            warnings.extend(more);
            warnings.extend(self.reserved_spines(alias));
            if unnamed > 0 {
                warnings.push(problems::link_targets_unnamed(&alias_name(alias), unnamed));
            }
        }
        let checked_graphs: Vec<(&str, &Graph)> = read.iter().map(|(a, g, _)| (*a, g)).collect();
        warnings.extend(super::entity::check_key_types(&checked_graphs));
        let lint = lint.map(|req| {
            let linted: Vec<_> = read.iter().map(|(a, g, m)| (*a, g, m.as_ref())).collect();
            let (found, hints) = self.lint(&linted, req);
            warnings.extend(hints);
            found
        });
        // As node list does: a saved scope's member and copy warnings, and secured graphs left out
        // (graph-addressing-expert N3).
        warnings.extend(self.scope_warnings(&scope)?);
        let total = rows.len();
        let mut checked = page(&scope.aliases[0], rows, total, 0);
        checked.graphs = Some(crate::PageGraphs {
            rows: owners,
            copies: false,
        });
        Ok(Done {
            value: Checked {
                page: checked,
                graphs,
                spines,
                lint,
            },
            notes: warnings,
        })
    }

    /// The loaded graph's spine line for `umx check` (B4): data, never a warning, so check's status and exit code
    /// are what they were.
    fn spine_of(&self, alias: &str) -> super::SpineLine {
        let l = self.get(alias);
        super::spine::spine_line(&l.graph, &l.meta.ontology, &alias_name(alias))
    }

    /// D71 step 12 (pm-001): a declared spine under a built-in's name, stored before umx reserved them. It still
    /// loads, and `--along` with that name reads the declared one.
    fn reserved_spines(&self, alias: &str) -> Vec<Problem> {
        let graph = alias_name(alias);
        self.get(alias)
            .meta
            .spines
            .iter()
            .filter(|s| super::reserved_spine_name(&s.name))
            .map(|s| problems::spine_name_reserved_stored(&graph, &s.name))
            .collect()
    }

    /// One graph's check (see [`Self::check`]), with the number of its external ends whose graph `ctx` does not name.
    /// Also gives the graph and its graph.json as decoded from the files just read (R6: what the G5 scan reads; R377-1:
    /// what the lint reads), an empty graph and None when they could not be read.
    #[allow(clippy::type_complexity)]
    fn check_one(
        &self,
        ctx: &Ctx,
        alias: &str,
    ) -> Outcome<(Vec<CheckRow>, usize, Graph, Option<GraphMeta>)> {
        let dir = self.get(alias).dir.clone();
        // A cut-off write that cannot be finished: list the files that changed since, and nothing else, since
        // the folder cannot be read until it is resolved.
        if let Err(FilesError::RecoveryConflict(files)) = self.ports.files.list(&dir) {
            let rows: Vec<CheckRow> = files
                .into_iter()
                .map(|file| CheckRow {
                    kind: "recovery_conflict",
                    subject: file.path(),
                    file,
                    lines: Vec::new(),
                })
                .collect();
            return Ok(Done::ok((rows, 0, Graph::new(), None)));
        }
        let read = |file| {
            self.ports
                .files
                .read(&dir, file)
                .map_err(|e| files_problem(&dir, &self.roots, file, e))
        };
        let meta_bytes = read(GraphFile::Graph)?.ok_or_else(|| problems::graph_not_found(&dir))?;
        let meta = decode_meta(&dir, &meta_bytes)?;
        let mut rows: Vec<CheckRow> = unknown_keys(&meta)
            .into_iter()
            .map(|path| CheckRow {
                kind: "unknown_key",
                file: GraphFile::Graph.into(),
                subject: path.join("."),
                lines: Vec::new(),
            })
            .collect();
        let (graph, nodes, edges, own_ends) = load_files(
            read,
            TextFields::default(),
            &meta.ontology.symmetric_kinds(),
            meta.id,
        )?;
        rows.extend(
            nodes
                .invalid_provenance
                .iter()
                .chain(&edges.invalid_provenance)
                .map(|issue| CheckRow {
                    kind: "provenance_key_invalid",
                    file: issue.file.into(),
                    subject: format!("{} {}", issue.record, issue.key),
                    lines: alloc::vec![issue.line],
                }),
        );
        rows.extend(nodes.duplicate_prov_keys.iter().map(|d| CheckRow {
            kind: "duplicate_prov_key",
            file: GraphFile::Nodes.into(),
            subject: d.node.to_text(),
            lines: Vec::new(),
        }));
        // F75: an edge's own updated or updated_by, which umx now reserves for its edit stamp; edge edit refuses it.
        let reserved: Vec<CheckRow> = graph
            .edges()
            .map(|e| e.to_edge())
            .filter(|e| !super::edge::stamp_or_none(&e.fields))
            .map(|e| CheckRow {
                kind: "edge_reserved_field_in_use",
                file: GraphFile::Edges.into(),
                subject: e.id.to_text(),
                lines: Vec::new(),
            })
            .collect();
        rows.extend(reserved);
        rows.extend(own_ends.into_iter().map(|(edge, _, line)| CheckRow {
            kind: "own_graph_end",
            file: GraphFile::Edges.into(),
            subject: edge.to_text(),
            lines: alloc::vec![line],
        }));
        rows.extend(graph.edges_with_invalid_evidence().map(|edge| CheckRow {
            kind: "evidence_field_kept",
            file: GraphFile::Edges.into(),
            subject: edge.to_text(),
            lines: Vec::new(),
        }));
        if let Some(reserved) = &nodes.graph_id_node {
            rows.push(CheckRow {
                kind: "node_id_is_graph_id",
                file: GraphFile::Nodes.into(),
                subject: reserved.id.to_text(),
                lines: reserved.lines.clone(),
            });
        }
        let mut edge_warnings = Vec::new();
        let mut unnamed = 0;
        self.ports.clock.now_ms();
        for edge in graph.edges() {
            for (end, target, node) in [
                ("from", edge.from_graph(), edge.from()),
                ("to", edge.to_graph(), edge.to()),
            ] {
                if let Some(target) = target {
                    if !self.target_named(ctx, target) {
                        unnamed += 1;
                    }
                    let resolved = self.read_end(ctx, &[], target, node);
                    if let Some(state) = resolved.state {
                        if state == "edge_broken" {
                            edge_warnings.push(problems::edge_broken(edge.id(), end, target, node));
                        }
                        rows.push(CheckRow {
                            kind: state,
                            file: GraphFile::Edges.into(),
                            subject: format!("{} {end} umx:{target}/{node}", edge.id()),
                            lines: Vec::new(),
                        });
                    }
                }
            }
        }
        rows.extend(
            [
                (GraphFile::Nodes, nodes.unsorted),
                (GraphFile::Edges, edges.unsorted),
            ]
            .into_iter()
            .filter_map(|(file, line)| {
                Some(CheckRow {
                    kind: "unsorted_file",
                    file: file.into(),
                    subject: file.file_name().to_string(),
                    lines: alloc::vec![line?],
                })
            }),
        );
        rows.extend(
            nodes
                .repeats
                .into_iter()
                .chain(edges.repeats)
                .map(|r| CheckRow {
                    kind: "duplicate_id",
                    file: r.file.into(),
                    subject: r.id.to_text(),
                    lines: r.lines,
                }),
        );
        rows.extend(edges.dangling.into_iter().map(|d| CheckRow {
            kind: "dangling_edge",
            file: GraphFile::Edges.into(),
            subject: d.edge.to_text(),
            lines: alloc::vec![d.line],
        }));
        rows.extend(edges.reversed.into_iter().map(|r| CheckRow {
            kind: "symmetric_duplicate",
            file: GraphFile::Edges.into(),
            subject: r.edge.to_text(),
            lines: alloc::vec![r.line],
        }));
        rows.extend(graph.duplicate_locators().map(|d| CheckRow {
            kind: "duplicate_locator",
            file: GraphFile::Nodes.into(),
            subject: d.node.to_text(),
            lines: Vec::new(),
        }));
        rows.extend(graph.unnormalised_locators().map(|id| CheckRow {
            kind: "locator_not_normalised",
            file: GraphFile::Nodes.into(),
            subject: id.to_text(),
            lines: Vec::new(),
        }));
        rows.extend(graph.kept_locator_fields().into_iter().map(|id| CheckRow {
            kind: "locator_field_kept",
            file: GraphFile::Nodes.into(),
            subject: id.to_text(),
            lines: Vec::new(),
        }));
        // F211: a date field's stored value that is not a date (it loads as written and compares as text).
        let graph_name = alias_name(alias);
        for (id, edge, field) in super::time_check::stored_not_dates(&graph, &meta.ontology) {
            rows.push(CheckRow {
                kind: "date_not_a_date",
                file: if edge {
                    GraphFile::Edges
                } else {
                    GraphFile::Nodes
                }
                .into(),
                subject: format!("{id} {field}"),
                lines: Vec::new(),
            });
            if let Some(field) = ValidName::new(&field) {
                edge_warnings.push(problems::date_not_a_date(&graph_name, id, edge, &field));
            }
        }
        // The folder's .gitignore must hide umx's temp and marker files, or a crash leaves them for git to see.
        let ignore = read(GraphFile::GitIgnore)?;
        let mut warnings = edge_warnings;
        if !ignore_hides_temp_files(ignore.as_deref()) {
            rows.push(CheckRow {
                kind: "gitignore_line_missing",
                file: GraphFile::GitIgnore.into(),
                subject: IGNORE_LINE.to_string(),
                lines: Vec::new(),
            });
            warnings.push(problems::gitignore_line_missing(&dir));
        }
        // ws006 S23: root:repo pointers into a folder the graph moved from (graph.json's former_paths). An entry that
        // names where the graph is now (a move cut short before its rename) is not one (architect).
        if !meta.former_paths.is_empty()
            && let Ok(Some(top)) = self.repo_top(&dir)
            && let Some(here) = super::graphfile::relative(&top, &dir)
        {
            let olds: Vec<&str> = meta
                .former_paths
                .iter()
                .map(String::as_str)
                .filter(|p| *p != here)
                .collect();
            let stale = super::rename::stale_pointers(&graph, &olds);
            for old in &olds {
                let count = stale.iter().filter(|(_, _, o)| o == old).count();
                if count > 0 {
                    warnings.push(problems::evidence_root_stale(
                        &alias_name(alias),
                        count,
                        old,
                        &here,
                    ));
                }
            }
            rows.extend(stale.into_iter().map(|(id, place, _)| CheckRow {
                kind: "evidence_root_stale",
                file: if place.starts_with("edge:") {
                    GraphFile::Edges.into()
                } else {
                    GraphFile::Nodes.into()
                },
                subject: format!("{} {place}", id.to_text()),
                lines: Vec::new(),
            }));
        }
        Ok(Done {
            value: (rows, unnamed, graph, Some(meta)),
            notes: warnings,
        })
    }

    /// The folder's physical identity, confined to the roots (`umx doctor`). Read only: it resolves the path and
    /// never reads, lists or recovers the graph's files.
    pub fn folder(&self, path: &str) -> Result<GraphDir, Problem> {
        self.identity(path)
    }

    /// The folder's physical identity, so every spelling of one folder names one loaded graph.
    pub(super) fn identity(&self, path: &str) -> Result<GraphDir, Problem> {
        let given = dir(path)?;
        self.ports
            .files
            .resolve(&given)
            .map_err(|e| files_problem(&given, &self.roots, GraphFile::Graph, e))
    }

    /// Loaded graphs in alias order.
    pub fn graph_list(&self) -> Vec<GraphInfo> {
        let now_ms = self.ports.clock.now_ms();
        self.graphs
            .iter()
            .map(|(alias, l)| info(l, alias, now_ms, self.idle_ms))
            .collect()
    }

    pub fn graph_list_in(&self, ctx: &Ctx) -> Vec<GraphInfo> {
        self.graph_list()
            .into_iter()
            .map(|mut info| {
                if self.external_visible(ctx, &[], &info.alias) {
                    let loaded = self.get(&info.alias);
                    let include = self.provenance_in_read(ctx, &[], info.id);
                    let read_ctx = ctx.clone().with_provenance(include);
                    info.nodes = read_ctx.read_scope(&loaded.graph).node_count();
                    info.edges = self
                        .read_census(&read_ctx, &info.alias)
                        .relations
                        .iter()
                        .map(|tally| tally.count)
                        .sum::<usize>()
                        + if include {
                            loaded.graph.provenance_edges().count()
                        } else {
                            0
                        };
                }
                info
            })
            .collect()
    }
}
