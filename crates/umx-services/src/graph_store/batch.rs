//! `umx batch` (req-88, D48; ws003 s07): node adds, edge adds, node edits, edge deletes and edge edits (F75), and
//! evidence adds and removes (req-126), as one change to one graph. Each line
//! goes through the single verb's own checks (shared functions, never copies), in line order, against the graph as
//! the earlier lines left it, so a near-duplicate check at line N sees lines 1 to N-1. The changes go into one
//! umx-graph batch; then every graph file the batch touched is written in one `write_set`. Any line error, a failed
//! write or a dry run rolls the graph back to exactly its state before the batch: all or nothing. Refs name nodes
//! added earlier in the batch, or nodes an earlier applied batch bound (`--refs-from`, ws003 s24); they are never
//! stored. An imported ref resolves like an explicit id in this graph, at the line that uses it.
//!
//! A batch may carry an idempotency key (ws003 s09). A real batch that succeeds with one records its receipt (the
//! ids its refs bound and its counts) in the graph folder's `receipts.jsonl`, in the same write set as its files. The
//! same key with the same input then returns that receipt and writes nothing; with another input it is refused. The
//! file is read only by keyed batches ([`umx_core::receipts`] holds its format and bounds).

use super::edge::{
    EdgeEditRequest, EdgeEvidence, append_to_existing, check_changeable, check_deletable,
    check_relation, edge_evidence, new_edge, plan_edge, plan_edge_edit, repeat_warning,
    resolve_edge, sentence_text,
};
use super::evidence_edit::{Removal, check_reason, pointer_to_match, remove_pointer};
use super::external::{StoredEnd, find_edge_where, ordered_ends, set_ends, whole_node_edge};
use super::node::{
    NodeDraft, draft_node, enrich, key_holder, near_duplicates, other_type_holders, part_of_warning,
};
use super::node_more::{
    NodeNoteRequest, NodeStatusRequest, add_evidence, edit_node, evidence_pointers, note_node,
    status_node,
};
use super::resolve::Evidence;
use super::secrets::{edge_write_secrets, node_write_secrets, title_suspect};
use super::sources::SourceCheck;
use super::{
    Ctx, Done, GraphStore, Loaded, NodeAddRequest, NodeEditRequest, ReceiptsWrite, alias_name,
    check_evidence, persist_parts, persist_receipts, problems, resolve_in_with,
};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::batch::{
    MAX_BATCH_CANDIDATES, MAX_BATCH_ERRORS, MAX_BATCH_EVIDENCE, MAX_BATCH_OPS, MAX_BATCH_REPLY,
    MAX_BATCH_WARNINGS, MAX_ECHO_SENTENCE_CHARS, MAX_ECHO_SENTENCES, MAX_IMPORTED_REFS, check_ref,
};
use umx_core::codec::{GraphFile, Repeat};
use umx_core::entity::ENTITY_KEY;
use umx_core::ontology::PART_OF;
use umx_core::ports::{FileRef, FileStamp, FilesError, GraphDir, IdGenerator};
use umx_core::receipts::{self, Lookup, OpCount, Receipt, Receipts};
use umx_core::secret_allows::SecretAllows;
use umx_core::standard::RAISED_IN;
use umx_core::value::SetValue;
use umx_core::{Command, Edge, Id, Ontology, Problem, Slot, ValidName};
use umx_graph::{Batch, EdgeRef, Graph};

/// One op of a batch, with its single verb's fields. A node is named by a real reference (id, prefix or address) or
/// by a ref, never both: `parent` / `parent_ref`, `from` / `from_ref`, `to` / `to_ref`, `node` / `node_ref`.
#[derive(Clone, Debug)]
pub enum BatchOp {
    /// `req.parent` is the real parent; `ref_name` names the new node for later lines.
    NodeAdd {
        id: Option<String>,
        created: Option<String>,
        ref_name: Option<String>,
        req: NodeAddRequest,
        parent_ref: Option<String>,
    },
    EdgeAdd {
        id: Option<String>,
        from: Option<String>,
        from_ref: Option<String>,
        relation: String,
        to: Option<String>,
        to_ref: Option<String>,
        sets: Vec<(String, SetValue)>,
        /// Evidence for the edge (req-124), appended when it exists.
        evidence: EdgeEvidence,
    },
    NodeEdit {
        node: Option<String>,
        node_ref: Option<String>,
        req: NodeEditRequest,
    },
    NodeNote {
        node: Option<String>,
        node_ref: Option<String>,
        req: NodeNoteRequest,
    },
    NodeStatus {
        node: Option<String>,
        node_ref: Option<String>,
        req: NodeStatusRequest,
    },
    /// An edge is named by its id or address only: refs name nodes.
    EdgeDelete {
        edge: String,
    },
    EdgeEdit {
        edge: String,
        req: EdgeEditRequest,
    },
    /// `umx node evidence <node> add` as a line (req-126).
    EvidenceAdd {
        node: Option<String>,
        node_ref: Option<String>,
        pointer: String,
        evidence_kind: Option<String>,
    },
    /// `umx node evidence <node> remove` as a line: `reason` is the line's own, else the batch's; with `skip_missing`
    /// a pointer that matches nothing warns instead of failing the batch.
    EvidenceRemove {
        node: Option<String>,
        node_ref: Option<String>,
        pointer: String,
        reason: Option<String>,
        at: Option<usize>,
        skip_missing: bool,
    },
}

impl BatchOp {
    pub fn label(&self) -> &'static str {
        match self {
            BatchOp::NodeAdd { .. } => "node add",
            BatchOp::EdgeAdd { .. } => "edge add",
            BatchOp::NodeEdit { .. } => "node edit",
            BatchOp::NodeNote { .. } => "node note",
            BatchOp::NodeStatus { .. } => "node status",
            BatchOp::EdgeDelete { .. } => "edge delete",
            BatchOp::EdgeEdit { .. } => "edge edit",
            BatchOp::EvidenceAdd { .. } => "evidence add",
            BatchOp::EvidenceRemove { .. } => "evidence remove",
        }
    }

    /// The real node references the op names, for choosing the graph (workshop 007 rule 6).
    fn named(&self) -> [Option<&str>; 2] {
        match self {
            BatchOp::NodeAdd { req, .. } => [req.parent.as_deref(), None],
            BatchOp::EdgeAdd { from, to, .. } => [from.as_deref(), to.as_deref()],
            BatchOp::NodeEdit { node, .. }
            | BatchOp::NodeNote { node, .. }
            | BatchOp::NodeStatus { node, .. }
            | BatchOp::EvidenceAdd { node, .. }
            | BatchOp::EvidenceRemove { node, .. } => [node.as_deref(), None],
            BatchOp::EdgeDelete { edge } | BatchOp::EdgeEdit { edge, .. } => {
                [Some(edge.as_str()), None]
            }
        }
    }

    /// The plain node fields the op gives, each with its `_ref` twin and its text (plan 205: a ref in a plain field).
    fn plain_fields(&self) -> [(&'static str, &'static str, Option<&str>); 2] {
        match self {
            BatchOp::NodeAdd { req, .. } => [
                ("parent", "parent_ref", req.parent.as_deref()),
                ("", "", None),
            ],
            BatchOp::EdgeAdd { from, to, .. } => [
                ("from", "from_ref", from.as_deref()),
                ("to", "to_ref", to.as_deref()),
            ],
            BatchOp::NodeEdit { node, .. }
            | BatchOp::NodeNote { node, .. }
            | BatchOp::NodeStatus { node, .. }
            | BatchOp::EvidenceAdd { node, .. }
            | BatchOp::EvidenceRemove { node, .. } => {
                [("node", "node_ref", node.as_deref()), ("", "", None)]
            }
            BatchOp::EdgeDelete { .. } | BatchOp::EdgeEdit { .. } => {
                [("", "", None), ("", "", None)]
            }
        }
    }

    /// Each valid field name the op's `sets` give more than once, once, in order (plan 205): only the last value is
    /// kept. An invalid name fails the line anyway.
    fn duplicate_sets(&self) -> Vec<ValidName> {
        let sets = match self {
            BatchOp::NodeAdd { req, .. } => &req.sets,
            BatchOp::NodeEdit { req, .. } => &req.sets,
            BatchOp::EdgeAdd { sets, .. } => sets,
            BatchOp::EdgeEdit { req, .. } => &req.sets,
            _ => return Vec::new(),
        };
        let (mut seen, mut said) = (BTreeSet::new(), BTreeSet::new());
        let mut twice: Vec<ValidName> = Vec::new();
        for (name, _) in sets {
            if !seen.insert(name.as_str())
                && said.insert(name.as_str())
                && let Some(valid) = ValidName::new(name)
            {
                twice.push(valid);
            }
        }
        twice
    }
}

/// One line of the batch file: its line number (blank and comment lines count) and its op.
#[derive(Clone, Debug)]
pub struct BatchLine {
    pub line: u32,
    pub op: BatchOp,
}

#[derive(Clone, Debug, Default)]
pub struct BatchRequest {
    pub lines: Vec<BatchLine>,
    /// Check and plan every line exactly as a real run would, then roll back: nothing written, no id drawn.
    pub dry_run: bool,
    /// `--reason`: every `evidence remove` line's reason unless the line gives its own (req-126).
    pub reason: Option<String>,
    pub echo_sentences: bool,
    /// Refs an earlier applied batch bound (`--refs-from`): ref name to full id. Checked here, never trusted.
    pub refs: BTreeMap<String, String>,
    /// The graph id the receipts name, when any names one: the batch's graph must have it.
    pub refs_graph: Option<String>,
    /// `--raised-in <anchor>` (ws008 TM4): every node the batch adds gets a raised_in edge to it, unless its line
    /// names its own `raised_in`. Resolved once, before any line runs; an anchor that does not resolve refuses the
    /// batch.
    pub raised_in: Option<String>,
    /// `--idempotency-key` (ws003 s09).
    pub idempotency: Option<Idempotency>,
}

/// A batch's idempotency key as sent, and the digest of its canonical input (umx-protocol builds the bytes and the
/// daemon hashes them, since only they see the wire). The services check the key again and digest it themselves.
#[derive(Clone, Debug)]
pub struct Idempotency {
    pub key: String,
    pub input: String,
}

/// A keyed batch's own reply fields.
#[derive(Clone, Debug, PartialEq)]
pub struct Keyed {
    /// The key as the caller sent it.
    pub key: String,
    /// The reply is the receipt a first run recorded: nothing was checked, planned or written.
    pub replayed: bool,
    /// When the receipt was recorded (epoch ms); None for a dry run that does not replay (nothing is recorded).
    pub recorded_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BatchSentence {
    pub line: u32,
    pub op: &'static str,
    pub id: Option<Id>,
    pub sentence: String,
    /// F234: an end's whole title (before the echo cuts it) is one umx's secret check flags.
    pub sentence_flagged: bool,
}

/// What one line did. `id` is the new or changed node, or the edge; None in a dry run (no id is drawn).
#[derive(Clone, Debug, PartialEq)]
pub struct BatchRow {
    pub line: u32,
    pub op: &'static str,
    pub ref_name: Option<String>,
    pub id: Option<Id>,
    /// A node add's parent edge.
    pub parent_edge: Option<Id>,
    /// A node add's raised_in edge (ws008 TM4; None in a dry run, as every drawn id is).
    pub raised_in_edge: Option<Id>,
    /// The anchor a node add's raised_in edge goes to, as an address (shown in a dry run too).
    pub raised_in: Option<String>,
    /// A node edit's changed fields, in the order given; an edge edit's `type` and `direction`.
    pub changed: Vec<String>,
    /// An edge add whose edge already existed: nothing was added.
    pub existed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BatchDone {
    pub alias: String,
    pub dry_run: bool,
    /// The graph's id, for a later batch's `--refs-from` check (ws003 s24).
    pub graph_id: Id,
    /// Each ref this batch defined, in the order defined, with its new node's id (None in a dry run). Imported refs
    /// are never listed, so a chain of receipts does not grow and a stale import is not passed on.
    pub refs: Vec<(String, Option<Id>)>,
    pub rows: Vec<BatchRow>,
    pub sentences: Option<Vec<BatchSentence>>,
    pub sentences_omitted: usize,
    /// Nodes added or changed, for the embed queue (after the write, never inside the batch).
    pub touched: Vec<Id>,
    /// Every warning the batch counted, shown or not: what `batch_warnings_truncated` totals (plan 205: warnings
    /// found after the lines ran join it through [`add_late_warnings`]).
    pub warned: usize,
    /// Each op's lines, and how many found what they would add already there (ws003 s09): what a receipt keeps.
    pub counts: BTreeMap<String, OpCount>,
    /// Set when the batch carried an idempotency key.
    pub keyed: Option<Keyed>,
}

/// Each op's lines and existing finds, from a run's rows.
fn op_counts(rows: &[BatchRow]) -> BTreeMap<String, OpCount> {
    let mut out: BTreeMap<String, OpCount> = BTreeMap::new();
    for r in rows {
        let c = out.entry(r.op.to_string()).or_default();
        c.lines += 1;
        c.existed += u32::from(r.existed);
    }
    out
}

/// What a keyed batch found before any line ran (ws003 s09).
enum KeyedStart {
    Replay(Done<BatchDone>),
    Run(KeyedRun),
}

/// A keyed batch that runs: its receipts as read and bounded, the read's stamp, and whether its key was dropped.
struct KeyedRun {
    key: String,
    digest: String,
    input: String,
    receipts: Receipts,
    read: FileStamp,
    /// When the key's dropped receipt was recorded: the reply says the key expired (prime ruling).
    expired: Option<u64>,
}

/// A batch's result. A failed batch gives every problem to show: a refusal of the whole batch is one problem; failed
/// lines are `batch_failed` first, then each line's problem (at most [`MAX_BATCH_ERRORS`]), then
/// `batch_errors_truncated` if there were more.
pub type BatchOutcome = Result<Done<BatchDone>, Vec<Problem>>;

/// Adds warnings found after a batch's lines ran (its write's, the vector recheck's) under the same cap as the
/// lines' own (plan 205): shown while fewer than [`MAX_BATCH_WARNINGS`] are, always counted in
/// `batch_warnings_truncated`'s total, before the hints.
pub(crate) fn add_late_warnings(done: &mut Done<BatchDone>, late: Vec<Problem>) {
    if late.is_empty() {
        return;
    }
    let notes = &mut done.notes;
    notes.retain(|p| p.code != "batch_warnings_truncated");
    let mut shown = notes
        .iter()
        .position(|p| problems::is_hint(p.code))
        .unwrap_or(notes.len());
    for p in late {
        if problems::is_hint(p.code) {
            notes.push(p);
            continue;
        }
        done.value.warned += 1;
        if shown < MAX_BATCH_WARNINGS {
            notes.insert(shown, p);
            shown += 1;
        }
    }
    if done.value.warned > MAX_BATCH_WARNINGS {
        notes.insert(
            shown,
            problems::batch_warnings_truncated(done.value.warned, MAX_BATCH_WARNINGS),
        );
    }
}

/// What a ref names: a node an earlier line added, or a line that failed.
enum Named {
    Node(Id),
    Failed,
}

/// Where new ids come from: the id generator, or, for a dry run, numbers above every id the graph holds.
enum Ids<'a> {
    Real(&'a dyn IdGenerator, Option<Id>),
    DryRun(u128),
}

fn draw_id(
    ids: &mut Ids<'_>,
    reserved: &BTreeSet<Id>,
    minted: &mut BTreeMap<Id, u32>,
    graph: &Graph,
    owner: Id,
    line: u32,
) -> Id {
    loop {
        let id = match ids {
            Ids::Real(generator, first) => first.take().unwrap_or_else(|| generator.next_id()),
            Ids::DryRun(value) => {
                *value += 1;
                Id(*value)
            }
        };
        if id != owner
            && !reserved.contains(&id)
            && !minted.contains_key(&id)
            && !graph.holds_id(id)
        {
            minted.insert(id, line);
            return id;
        }
    }
}

struct EchoGraph<'g> {
    id: Id,
    alias: &'g str,
    title: &'g str,
    graph: &'g Graph,
}

impl EchoGraph<'_> {
    /// The end as the sentence echoes it, and whether the title it echoes is flagged (F234: the whole title, so a cut
    /// one is flagged too, as a read flags the title it holds). An address is never flagged.
    fn endpoint(&self, node: Id, cap: usize, allows: &SecretAllows) -> (String, bool) {
        let title = if node == self.id {
            Some(if self.title.is_empty() {
                self.alias
            } else {
                self.title
            })
        } else {
            self.graph.node(node).map(|node| node.title())
        };
        match title {
            Some(title) => (
                echo_text(title, cap),
                title_suspect(|| allows, self.id, node, title),
            ),
            None => (format!("umx:{}/{node}", self.id), false),
        }
    }
}

fn echo_text(text: &str, cap: usize) -> String {
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(cap).collect();
    if chars.next().is_some() {
        let cut = out
            .char_indices()
            .nth(cap - 3)
            .map_or(out.len(), |(at, _)| at);
        out.truncate(cut);
        out.push_str("...");
    }
    out
}

struct Run<'a, 'g> {
    ctx: &'a Ctx,
    alias: &'a str,
    graph: ValidName,
    o: &'a Ontology,
    dir: &'a GraphDir,
    sources: SourceCheck,
    allows: SecretAllows,
    /// The batch's `--reason`, for `evidence remove` lines without their own.
    reason: Option<String>,
    owner: Id,
    owner_title: &'a str,
    local_aliases: Vec<String>,
    prepared: BTreeMap<String, Result<(StoredEnd, Vec<Problem>), Problem>>,
    /// Each prepared external end's loaded copy and its node's type (F180).
    end_kinds: BTreeMap<String, (String, String)>,
    /// The prepared external ends whose graph is open (d-20): only their types are named in a warning (ws008 T2b).
    open_ends: BTreeSet<String>,
    /// TM4: each line's own far raised_in anchor's type, from an open graph only.
    anchor_kinds: BTreeMap<String, String>,
    b: Batch<'g>,
    /// The other loaded open graphs, for checking `umx:` evidence sources (the batch's own graph is `b`'s).
    others: BTreeMap<&'g str, Evidence<'g>>,
    /// The batch's own graph's former aliases (ws006 S23): `umx:<former>/` evidence checks against `b`.
    own_former: Vec<String>,
    echo_graphs: BTreeMap<Id, EchoGraph<'g>>,
    echo_sentences: bool,
    sentences: Vec<BatchSentence>,
    sentences_omitted: usize,
    ids: Ids<'a>,
    now: String,
    refs: BTreeMap<String, Named>,
    ref_order: Vec<String>,
    /// Refs `--refs-from` imported: checked names and full ids, resolved at each use.
    imported: BTreeMap<String, Id>,
    /// Edges the batch added or edited, by their (from, relation, to) now: the graph's adjacency does not hold them
    /// until commit. An edge deleted or edited later leaves under its old key.
    added_edges: BTreeMap<(StoredEnd, String, StoredEnd), Id>,
    /// Edges with an end in another graph as they were before a line deleted or edited them (F122): their incoming
    /// entries come out after the batch commits, and never before.
    external_old: Vec<Edge>,
    /// A line changed an edge with an end in another graph (edit, delete or evidence append), so graph.json joins the
    /// write set (F122; graph-storage-expert R210-M1). Set by [`Run::changing_edge`] only.
    graph_json: bool,
    /// Ids drawn by this batch and the line that drew each, so a dry run or a failed batch never shows one as if it
    /// were a node of the graph.
    minted: BTreeMap<Id, u32>,
    reserved: BTreeSet<Id>,
    parent_edges: BTreeMap<Id, Id>,
    /// TM4: the batch's `--raised-in` anchor, and each preplanned node's raised_in edge id.
    anchor: Option<super::raised_in::Anchor>,
    raised_edges: BTreeMap<Id, Id>,
    /// Whether this write has said raised_in is off its vocabulary (R341-1: once per write, the batch's anchor or a
    /// line's own, so lines never fill the warning cap with it).
    vocabulary_said: bool,
    /// Off after the first failed line: the batch will not be written, so its near-duplicate warnings would never be
    /// shown, and the title scan is most of a batch's cost.
    near_checks: bool,
    nodes: bool,
    edges: bool,
    touched: Vec<Id>,
    /// Each touched node with the first line that touched it (ws002 X1, review R3: a batch's segment hint names it).
    touched_at: BTreeMap<Id, u32>,
    session: Option<Id>,
}

type LineResult = Result<(BatchRow, Vec<Problem>), Problem>;

/// A node add line giving both an explicit id and on_key (ws003 s08): umx-protocol's words, checked again here.
const ON_KEY_WITH_ID: &str = "give id or on_key, not both";

/// An on_key node add line with no entity key to look up (ws003 s08).
const ON_KEY_NO_KEY: &str = "on_key needs an entity_key in sets";

/// `umx batch --file <file> --dry-run`.
fn dry_run_command() -> Slot {
    Slot::Command(problems::batch_dry_run_command())
}

fn row(line: u32, op: &'static str) -> BatchRow {
    BatchRow {
        line,
        op,
        ref_name: None,
        id: None,
        parent_edge: None,
        raised_in_edge: None,
        raised_in: None,
        changed: Vec::new(),
        existed: false,
    }
}

/// An upper bound on the JSON bytes the daemon's reply gives for this result: every row field at its longest, each
/// problem's message and next with every slot at the 300 characters a fill holds, and its candidates. Rows carry no
/// graph text and candidates no titles, so the bound follows from the request, not the graph (review R103-1).
fn reply_bound(
    alias: &str,
    rows: &[BatchRow],
    refs: usize,
    problems: &[&Problem],
    sentences: &[BatchSentence],
) -> u64 {
    let row: usize = rows
        .iter()
        .map(|r| {
            320 + 2 * alias.len()
                + r.ref_name.as_ref().map_or(0, String::len)
                + r.changed.iter().map(|c| c.len() + 3).sum::<usize>()
                + r.raised_in.as_ref().map_or(0, |a| 96 + a.len())
        })
        .sum();
    let issue: usize = problems.iter().map(|p| issue_bound(p)).sum();
    // F234: a flagged sentence also carries `,"secret_suspected":["field:sentence"]` (38 bytes).
    let echo: usize = sentences
        .iter()
        .map(|s| 128 + 6 * s.sentence.len() + if s.sentence_flagged { 40 } else { 0 })
        .sum();
    (1024 + row + refs * (MAX_REF_ROW) + issue + echo) as u64
}

/// One issue's share of [`reply_bound`].
fn issue_bound(p: &Problem) -> usize {
    1024 + 700 * p.slots.len()
        + p.subject.as_ref().map_or(0, |s| s.len())
        + p.candidates.iter().map(|c| 64 + c.id.len()).sum::<usize>()
}

/// A ref's entry in the reply's ref map: the ref (at most 32), its id and the JSON around them.
const MAX_REF_ROW: usize = 80;

/// A problem as a batch reports it: candidates carry ids and scores but no titles (graph text the batch may have
/// made), at most [`MAX_BATCH_CANDIDATES`] of them, so the reply never grows with titles (review R103-1).
fn bounded(mut p: Problem) -> Problem {
    p.candidates.truncate(MAX_BATCH_CANDIDATES);
    for c in &mut p.candidates {
        c.title = None;
    }
    p
}

/// The words for a pair given both ways, or neither way when one is needed.
fn pair_reason(field: &'static str, both: bool) -> &'static str {
    match (field, both) {
        ("parent", _) => "give parent or parent_ref, not both",
        ("from", true) => "give from or from_ref, not both",
        ("from", false) => "give from or from_ref",
        ("to", true) => "give to or to_ref, not both",
        ("to", false) => "give to or to_ref",
        ("node", true) => "give node or node_ref, not both",
        _ => "give node or node_ref",
    }
}

/// `text` as an Id when it is a full id spelled canonically (upper case, no Crockford aliases).
fn canonical_id(text: &str) -> Option<Id> {
    text.parse::<Id>().ok().filter(|id| id.to_text() == text)
}

/// The refs `--refs-from` imports, checked (ws003 s24 rule 4): at most [`MAX_IMPORTED_REFS`], every name a ref and
/// every value a full canonical id. The CLI reads them from applied receipts; a raw client is checked the same.
fn imported_refs(refs: &BTreeMap<String, String>) -> Result<BTreeMap<String, Id>, Problem> {
    if refs.len() > MAX_IMPORTED_REFS {
        return Err(problems::batch_too_large(
            refs.len() as u64,
            "imported refs",
            MAX_IMPORTED_REFS as u64,
        ));
    }
    refs.iter()
        .map(|(name, id)| match (check_ref(name), canonical_id(id)) {
            (Ok(()), Some(id)) => Ok((name.clone(), id)),
            _ => Err(problems::usage_invalid(
                "--refs-from gives a ref that is not a ref, or an id that is not a full umx id",
                "batch",
            )),
        })
        .collect()
}

impl Run<'_, '_> {
    /// Records a node the batch added or changed, with the first line that touched it.
    fn touch(&mut self, id: Id, line: u32) {
        self.touched.push(id);
        self.touched_at.entry(id).or_insert(line);
    }

    fn session(&mut self, line: u32, warnings: &mut Vec<Problem>) -> Result<Id, Problem> {
        if let Some(session) = self.session {
            return Ok(session);
        }
        let (session, notes) = {
            let Self {
                ctx,
                b,
                ids,
                now,
                minted,
                reserved,
                owner,
                ..
            } = self;
            let mut next = |graph: &Graph| draw_id(ids, reserved, minted, graph, *owner, line);
            super::provenance::upsert_session(b, &ctx.actor, &mut next, now)?
        };
        for file in self.b.changed_files() {
            match file {
                GraphFile::Nodes => self.nodes = true,
                GraphFile::Edges => self.edges = true,
                _ => unreachable!("a graph batch changes records only"),
            }
        }
        warnings.extend(notes);
        self.session = Some(session);
        Ok(session)
    }

    fn capture_sentence(&mut self) -> bool {
        if !self.echo_sentences {
            return false;
        }
        if self.sentences.len() == MAX_ECHO_SENTENCES {
            self.sentences_omitted += 1;
            return false;
        }
        true
    }

    fn echo_endpoint(&self, end: StoredEnd, cap: usize) -> (String, bool) {
        let graph = end.graph.unwrap_or(self.owner);
        if graph == self.owner {
            EchoGraph {
                id: self.owner,
                alias: self.alias,
                title: self.owner_title,
                graph: self.b.graph(),
            }
            .endpoint(end.node, cap, &self.allows)
        } else {
            self.echo_graphs.get(&graph).map_or_else(
                || (format!("umx:{graph}/{}", end.node), false),
                |graph| graph.endpoint(end.node, cap, &self.allows),
            )
        }
    }

    fn record_sentence(&mut self, line: u32, op: &'static str, edge: &Edge) {
        let relation = echo_text(&edge.kind, MAX_ECHO_SENTENCE_CHARS / 4);
        let cap = (MAX_ECHO_SENTENCE_CHARS - relation.chars().count() - 2) / 2;
        // An edit or a delete never reads the far end (security, F122): its address as stored, whether or not that
        // node exists or its graph is loaded. An add echoes what it resolved.
        let stored = op != "edge add";
        let end = |this: &Self, graph: Option<Id>, node: Id| match graph {
            Some(graph) if stored && graph != this.owner => (format!("umx:{graph}/{node}"), false),
            _ => this.echo_endpoint(StoredEnd { graph, node }, cap),
        };
        let (from, from_flagged) = end(self, edge.from_graph, edge.from);
        let (to, to_flagged) = end(self, edge.to_graph, edge.to);
        self.sentences.push(BatchSentence {
            line,
            op,
            id: Some(edge.id),
            sentence: sentence_text(&from, &relation, &to),
            sentence_flagged: from_flagged || to_flagged,
        });
    }

    fn next_id(&mut self, line: u32) -> Id {
        draw_id(
            &mut self.ids,
            &self.reserved,
            &mut self.minted,
            self.b.graph(),
            self.owner,
            line,
        )
    }

    /// Whether `text` is a ref this batch has bound so far (an earlier line's, or `--refs-from`'s): plan 205's hint
    /// when a plain field names one.
    fn knows_ref(&self, text: &str) -> bool {
        self.refs.contains_key(text) || self.imported.contains_key(text)
    }

    /// A node named by a ref: defined by an earlier line that succeeded, or imported by `--refs-from` and resolved
    /// here exactly as an explicit id would be, through the write resolver's scope (no provenance node, no
    /// placeholder). A missing, deleted or provenance id is the ref's error, never re-minted or guessed.
    fn use_ref(&self, line: u32, name: &str) -> Result<Id, Problem> {
        check_ref(name).map_err(|e| problems::batch_ref_invalid(line, e))?;
        match self.refs.get(name) {
            Some(Named::Node(id)) => Ok(*id),
            Some(Named::Failed) => Err(problems::batch_ref_unknown(
                line,
                name,
                "was defined by a line that failed",
            )),
            None => match self.imported.get(name) {
                Some(id) => resolve_in_with(self.b.graph(), self.alias, &id.to_text(), |id| {
                    !self.b.is_placeholder(id)
                })
                .map_err(|_| {
                    problems::batch_ref_unknown(
                        line,
                        name,
                        "a --refs-from receipt imports, but its id names no node of this graph",
                    )
                }),
                None => Err(problems::batch_ref_unknown(
                    line,
                    name,
                    "no earlier line defines",
                )),
            },
        }
    }

    /// One of a pair: a real reference resolved in the graph as it is so far, or a ref. Exactly one is given, or
    /// neither when `needed` is false.
    fn pair(
        &self,
        line: u32,
        real: Option<&str>,
        by_ref: Option<&str>,
        field: &'static str,
        needed: bool,
    ) -> Result<Option<Id>, Problem> {
        match (real, by_ref) {
            (Some(_), Some(_)) => Err(problems::batch_line_invalid(line, pair_reason(field, true))),
            (None, None) if needed => Err(problems::batch_line_invalid(
                line,
                pair_reason(field, false),
            )),
            (None, None) => Ok(None),
            (Some(text), None) => {
                if let Some(selector) = super::split_address(text).0
                    && selector.parse::<Id>().ok() != Some(self.owner)
                    && !self.local_aliases.iter().any(|alias| alias == selector)
                {
                    return Err(problems::graph_mismatch(selector, self.alias));
                }
                resolve_in_with(self.b.graph(), self.alias, text, |id| {
                    !self.b.is_placeholder(id)
                })
                .map(Some)
            }
            (None, Some(name)) => self.use_ref(line, name).map(Some),
        }
    }

    /// The graph alias and type of an edge end as this batch sees it: a local node (one an earlier line added
    /// included), or the node an external end's address names in a loaded copy (as `entity link` reads it, R250-4).
    fn end_kind(&self, text: Option<&str>, end: StoredEnd) -> Option<(String, String)> {
        match end.graph {
            None => self
                .b
                .graph()
                .node(end.node)
                .map(|n| (self.alias.to_string(), n.kind().to_string())),
            Some(_) => self.end_kinds.get(text?).cloned(),
        }
    }

    fn pair_end(
        &self,
        line: u32,
        real: Option<&str>,
        by_ref: Option<&str>,
        field: &'static str,
    ) -> Result<(StoredEnd, Vec<Problem>), Problem> {
        if by_ref.is_none()
            && let Some(text) = real
            && let Some(result) = self.prepared.get(text)
        {
            return result.clone();
        }
        self.pair(line, real, by_ref, field, true)
            .map(|node| (StoredEnd::local(node.expect("needed end")), Vec::new()))
    }

    fn node_add(
        &mut self,
        line: u32,
        ref_name: Option<String>,
        id: Option<String>,
        created: Option<String>,
        req: NodeAddRequest,
        parent_ref: Option<String>,
    ) -> LineResult {
        if let Some(name) = &ref_name {
            check_ref(name).map_err(|e| problems::batch_ref_invalid(line, e))?;
            if self.refs.contains_key(name) {
                return Err(problems::batch_ref_repeated(line, name));
            }
            if self.imported.contains_key(name) {
                return Err(problems::batch_ref_conflict(
                    line,
                    name,
                    "a line of this batch defines it and --refs-from imports it",
                ));
            }
        }
        let added = self.add_node(line, id, created, req, parent_ref);
        if let Some(name) = &ref_name {
            let named = match &added {
                Ok((r, _)) => Named::Node(r.id.expect("an added node has an id")),
                Err(_) => Named::Failed,
            };
            self.refs.insert(name.clone(), named);
            self.ref_order.push(name.clone());
        }
        added.map(|(mut r, w)| {
            r.ref_name = ref_name;
            (r, w)
        })
    }

    /// A line's own anchor (TM4's `raised_in`, TM6's status `in`), resolved as an edge add's `to` is: a far one was
    /// prepared before the run. Its type, when it can be read (d-20), is checked against raised_in's.
    fn line_anchor(
        &self,
        line: u32,
        text: &str,
        warnings: &mut Vec<Problem>,
    ) -> Result<super::raised_in::Anchor, Problem> {
        let (end, end_warnings) = self.pair_end(line, Some(text), None, "anchor")?;
        warnings.extend(end_warnings);
        let kind = match end.graph {
            None => self.b.graph().node(end.node).map(|n| n.kind().to_string()),
            Some(_) => self.anchor_kinds.get(text).cloned(),
        };
        warnings.extend(kind.and_then(|k| super::raised_in::unexpected(&self.graph, self.o, &k)));
        let address = match end.graph {
            None => crate::address(self.alias, end.node),
            Some(g) => alloc::format!("umx:{}/{}", g.to_text(), end.node.to_text()),
        };
        Ok(super::raised_in::Anchor { end, address })
    }

    fn add_node(
        &mut self,
        line: u32,
        explicit: Option<String>,
        created: Option<String>,
        req: NodeAddRequest,
        parent_ref: Option<String>,
    ) -> LineResult {
        // plan 205: an entry's own kind or the line's evidence_kind, never both (the CLI checks it too).
        if req.evidence_kind.is_some() && req.evidence.iter().any(|e| e.kind.is_some()) {
            return Err(problems::batch_line_invalid(
                line,
                "give a kind on each evidence entry or evidence_kind but not both",
            ));
        }
        let on_key = req.on_key_resolve;
        let status_given = req.status.is_some();
        let mut warnings = Vec::new();
        let real_parent = req.parent.clone();
        // TM4: the line's own raised_in, resolved as an edge add's `to` is; else the batch's anchor.
        let anchor = match req.raised_in.as_deref() {
            Some(text) => {
                let anchor = self.line_anchor(line, text, &mut warnings)?;
                if !core::mem::replace(&mut self.vocabulary_said, true) {
                    warnings.extend(super::raised_in::vocabulary(self.o, &self.graph));
                }
                Some(anchor)
            }
            None => self.anchor.clone(),
        };
        let draft = draft_node(
            self.ctx,
            &self.graph,
            self.o,
            self.b.graph(),
            self.dir,
            &self.sources,
            req,
            &mut || {
                self.pair(
                    line,
                    real_parent.as_deref(),
                    parent_ref.as_deref(),
                    "parent",
                    false,
                )
            },
            &|a| {
                if a == self.alias || self.own_former.iter().any(|f| f == a) {
                    Some(Evidence::Graph(self.owner, self.b.graph(), self.alias))
                } else {
                    self.others.get(a).copied()
                }
            },
            &mut warnings,
        )?;
        if on_key {
            if !draft.fields.contains_key(ENTITY_KEY) {
                return Err(problems::batch_line_invalid(line, ON_KEY_NO_KEY));
            }
            // ws003 s08: the graph as the earlier lines left it, so a later line finds an earlier line's add.
            let g = self.b.graph();
            let scope = super::ReadScope::new(g);
            match key_holder(g, &draft.kind, &draft.fields, |id| {
                scope.contains(id) && !self.b.is_placeholder(id)
            }) {
                Err((a, b)) => {
                    return Err(problems::on_key_ambiguous(&self.graph, &draft.kind, a, b));
                }
                Ok(Some(id)) => {
                    return self.resolve_line(line, id, draft, status_given, anchor, warnings);
                }
                Ok(None) => {}
            }
        }
        let near = if self.near_checks {
            near_duplicates(self.b.graph(), &draft.title, &draft.kind)
        } else {
            None
        };
        let session = self.session(line, &mut warnings)?;
        let is_explicit = explicit.is_some();
        let id = explicit
            .map(|s| s.parse().expect("explicit ids passed preflight"))
            .unwrap_or_else(|| self.next_id(line));
        let parent = draft.parent;
        let mut node = draft.into_node(id, &self.now, &self.ctx.by);
        warnings.extend(super::ontology::value_warnings(
            &self.graph,
            self.o,
            &node,
            node.fields.keys().map(String::as_str),
        ));
        if let Some(created) = created {
            node.created =
                umx_core::time::normalise_date(&created).expect("created passed preflight");
        }
        self.check_node_secrets(line, &node)?;
        warnings.extend(super::node::title_key_warnings(None, &node));
        node.record_author(session);
        super::provenance::stamp_evidence(&mut node.evidence, session);
        if let Some(near) = near {
            // The best match is the node an earlier line adds: name that line (ws004 s30 field report). Its `umx node
            // get` becomes the dry-run command where the id will not exist, as every command naming a drawn id does.
            warnings.push(match self.minted.get(&near.first) {
                Some(&earlier) => near.line_warning(&self.graph, id, earlier),
                None => near.warning(&self.graph, id),
            });
        }
        let edge = parent.map(|p| self.part_of_edge(line, id, p, session));
        if edge.is_some() {
            warnings.extend(part_of_warning(self.o, &self.graph));
        }
        let raised = anchor.map(|a| (self.raised_in_edge(line, id, &a, session), a));
        if is_explicit {
            self.b
                .fill(id, node)
                .map_err(|e| problems::image_full(&e))?;
        } else {
            self.b
                .insert_node(node)
                .map_err(|e| problems::image_full(&e))?;
        }
        if on_key {
            let g = self.b.graph();
            let scope = super::ReadScope::new(g);
            let added = g.node(id).expect("the node just added").to_node();
            warnings.extend(other_type_holders(&self.graph, g, &added, |n| {
                scope.contains(n)
            }));
        }
        let mut r = row(line, "node add");
        self.place_edges(&mut r, edge, raised);
        self.nodes = true;
        self.touch(id, line);
        r.id = Some(id);
        Ok((r, warnings))
    }

    /// A node add's part_of edge from `node` to `parent`: its preplanned id, else one drawn now (an on_key line is
    /// never preplanned, ws003 s08).
    fn part_of_edge(&mut self, line: u32, node: Id, parent: Id, session: Id) -> Edge {
        let edge_id = match self.parent_edges.remove(&node) {
            Some(id) => id,
            None => self.next_id(line),
        };
        let mut edge = new_edge(
            edge_id,
            node,
            PART_OF,
            parent,
            &self.now,
            &self.ctx.by,
            BTreeMap::new(),
        );
        edge.record_author(session);
        edge
    }

    /// A node add's raised_in edge from `node` to anchor `a` (TM4): its preplanned id, else one drawn now.
    fn raised_in_edge(
        &mut self,
        line: u32,
        node: Id,
        a: &super::raised_in::Anchor,
        session: Id,
    ) -> Edge {
        let edge_id = match self.raised_edges.remove(&node) {
            Some(id) => id,
            None => self.next_id(line),
        };
        let mut edge = super::raised_in::edge(edge_id, node, a, &self.now, &self.ctx.by);
        edge.record_author(session);
        edge
    }

    /// Puts a node add's new edges in the batch and names them on its row.
    fn place_edges(
        &mut self,
        r: &mut BatchRow,
        edge: Option<Edge>,
        raised: Option<(Edge, super::raised_in::Anchor)>,
    ) {
        if let Some((e, a)) = raised {
            r.raised_in_edge = Some(e.id);
            r.raised_in = Some(a.address);
            self.added_edges.insert(
                (StoredEnd::local(e.from), RAISED_IN.to_string(), a.end),
                e.id,
            );
            self.b.insert_edge(e);
            self.edges = true;
        }
        if let Some(e) = edge {
            r.parent_edge = Some(e.id);
            self.added_edges.insert(
                (
                    StoredEnd::local(e.from),
                    PART_OF.to_string(),
                    StoredEnd::local(e.to),
                ),
                e.id,
            );
            self.b.insert_edge(e);
            self.edges = true;
        }
    }

    /// The stored far ends of the live `kind` edges out of node `id` as the batch holds them so far: the graph's own
    /// (through its adjacency, each checked against the batch) and the ones earlier lines added or edited.
    fn ends_out(&self, id: Id, kind: &str) -> BTreeSet<StoredEnd> {
        let mut ends = super::node::ends_out(self.b.graph(), id, kind, |e| self.b.edge_record(e));
        let low = StoredEnd {
            graph: None,
            node: Id(0),
        };
        let high = StoredEnd {
            graph: Some(Id(u128::MAX)),
            node: Id(u128::MAX),
        };
        let from = StoredEnd::local(id);
        ends.extend(
            self.added_edges
                .range((from, kind.to_string(), low)..=(from, kind.to_string(), high))
                .filter(|(_, e)| self.b.edge_record(**e).is_some())
                .map(|((_, _, to), _)| *to),
        );
        ends
    }

    /// ws003 s08: an on_key node add whose keys node `id` holds. The node takes what it lacks ([`enrich`]: evidence,
    /// absent fields and keys, an empty note) and the part_of and raised_in edges it lacks, compared by stored ends; a
    /// held value is never overwritten and the `on_key_field_held` hint names it. The row says `existed`; its ref
    /// binds to `id`. Nothing is written when nothing would change.
    fn resolve_line(
        &mut self,
        line: u32,
        id: Id,
        draft: NodeDraft,
        status_given: bool,
        anchor: Option<super::raised_in::Anchor>,
        mut warnings: Vec<Problem>,
    ) -> LineResult {
        let before = self.b.graph().node(id).expect("a key holder").to_node();
        let parent = draft.parent;
        let (mut n, changed, mut held) =
            enrich(&before, draft, status_given, super::tags::is_std(self.o))?;
        warnings.extend(super::ontology::value_warnings(
            &self.graph,
            self.o,
            &n,
            changed.iter().map(String::as_str),
        ));
        let parent = parent.filter(|p| {
            let ends = self.ends_out(id, PART_OF);
            if !ends.is_empty() && !ends.contains(&StoredEnd::local(*p)) {
                held.push(problems::Held::Parent);
            }
            ends.is_empty()
        });
        let anchor = anchor.filter(|a| !self.ends_out(id, RAISED_IN).contains(&a.end));
        if !changed.is_empty() {
            n.updated = self.now.clone();
            self.replace_node(line, n, true, before.evidence.len(), &mut warnings)?;
        }
        let mut r = row(line, "node add");
        if parent.is_some() || anchor.is_some() {
            let session = self.session(line, &mut warnings)?;
            let edge = parent.map(|p| self.part_of_edge(line, id, p, session));
            if edge.is_some() {
                warnings.extend(part_of_warning(self.o, &self.graph));
            }
            let raised = anchor.map(|a| (self.raised_in_edge(line, id, &a, session), a));
            self.place_edges(&mut r, edge, raised);
        }
        if !held.is_empty() {
            warnings.push(problems::on_key_field_held(&self.graph, id, &held));
        }
        r.existed = true;
        r.id = Some(id);
        r.changed = changed;
        Ok((r, warnings))
    }

    #[allow(clippy::too_many_arguments)]
    fn edge_add(
        &mut self,
        line: u32,
        explicit: Option<String>,
        from: Option<String>,
        from_ref: Option<String>,
        relation: String,
        to: Option<String>,
        to_ref: Option<String>,
        sets: Vec<(String, SetValue)>,
        given: EdgeEvidence,
    ) -> LineResult {
        let rel = check_relation(self.ctx, &relation)?;
        let from_text = from.clone();
        let (from, mut warnings) =
            self.pair_end(line, from.as_deref(), from_ref.as_deref(), "from")?;
        let (to_end, to_warnings) = self.pair_end(line, to.as_deref(), to_ref.as_deref(), "to")?;
        let end_kinds = (
            self.end_kind(from_text.as_deref(), from),
            self.end_kind(to.as_deref(), to_end),
        );
        // A far end's type is named in a warning only from an open graph (d-20).
        let shown = |text: Option<&str>, end: StoredEnd, kind: &Option<(String, String)>| {
            (end.graph.is_none() || text.is_some_and(|t| self.open_ends.contains(t)))
                .then(|| kind.as_ref().map(|k| k.1.clone()))
                .flatten()
        };
        let end_types = (
            shown(from_text.as_deref(), from, &end_kinds.0),
            shown(to.as_deref(), to_end, &end_kinds.1),
        );
        // F180 (ws008 E2b): a same_as line between two types warns as `umx entity link` does.
        if relation == umx_core::ontology::SAME_AS
            && let (Some(a), Some(b)) = &end_kinds
            && a.1 != b.1
        {
            warnings.push(problems::entity_type_mismatch(
                (&alias_name(&a.0), from.node, &a.1),
                (&alias_name(&b.0), to_end.node, &b.1),
                "are linked as one entity",
            ));
        }
        let to = to_end;
        warnings.extend(to_warnings);
        let pointers = edge_evidence(
            self.dir,
            &given,
            &self.sources,
            &|a| {
                if a == self.alias || self.own_former.iter().any(|f| f == a) {
                    Some(Evidence::Graph(self.owner, self.b.graph(), self.alias))
                } else {
                    self.others.get(a).copied()
                }
            },
            &mut warnings,
        )?;
        let symmetric = self.o.is_symmetric(&relation);
        let (from, to) = ordered_ends(self.owner, symmetric, from, to);
        // Compare with the fields as earlier lines left them, including pending out-of-order edges.
        let found = |a: StoredEnd, b: StoredEnd| {
            whole_node_edge(
                self.find_edge(a, &relation, b),
                self.b
                    .graph()
                    .edges()
                    .filter(|edge| self.b.edge(edge.id()).is_some()),
            )
            .map(|edge| edge.to_edge())
            .or_else(|| {
                self.added_edges
                    .get(&(a, relation.clone(), b))
                    .and_then(|id| self.b.edge_record(*id))
                    .filter(|edge| {
                        !edge.fields.contains_key("from_selector")
                            && !edge.fields.contains_key("to_selector")
                    })
            })
        };
        let existing = found(from, to).or_else(|| symmetric.then(|| found(to, from)).flatten());
        let key = (from, relation, to);
        let planned = plan_edge(
            self.ctx,
            &self.graph,
            self.o,
            existing.as_ref(),
            &rel,
            &sets,
            &mut warnings,
        )?;
        let mut r = row(line, "edge add");
        match planned {
            None => {
                r.existed = true;
                r.id = existing.as_ref().map(|edge| edge.id);
                let id = existing.expect("plan_edge found it").id;
                // As the batch holds it so far: an edge an earlier line added (placed or not) or edited (F75).
                let mut edge = self
                    .b
                    .edge_record(id)
                    .expect("plan_edge found it in the batch");
                let start = edge.evidence.len();
                if append_to_existing(&self.graph, &mut edge, pointers, &mut warnings)? > 0 {
                    if let Some(hit) = edge_write_secrets(&BTreeMap::new(), &edge.evidence[start..])
                        .into_iter()
                        .next()
                    {
                        return Err(hit.refusal(line));
                    }
                    let session = self.session(line, &mut warnings)?;
                    edge.record_edit(session);
                    super::provenance::stamp_evidence(&mut edge.evidence[start..], session);
                    self.changing_edge(&edge);
                    self.b.insert_edge(edge);
                    self.edges = true;
                }
                if self.capture_sentence() {
                    let edge = self.live_edge(&id.to_text())?;
                    self.record_sentence(line, "edge add", &edge);
                }
            }
            Some(fields) => {
                if let Some(hit) = edge_write_secrets(&fields, &pointers).into_iter().next() {
                    return Err(hit.refusal(line));
                }
                warnings.extend(super::edge::endpoint_warning(
                    &self.graph,
                    self.o,
                    &rel,
                    end_types.0.as_deref(),
                    end_types.1.as_deref(),
                ));
                let session = self.session(line, &mut warnings)?;
                let id = explicit
                    .map(|s| s.parse().expect("explicit ids passed preflight"))
                    .unwrap_or_else(|| self.next_id(line));
                let mut edge = set_ends(
                    new_edge(
                        id,
                        from.node,
                        &key.1,
                        to.node,
                        &self.now,
                        &self.ctx.by,
                        fields,
                    ),
                    from,
                    to,
                );
                edge.evidence = pointers;
                edge.record_author(session);
                super::provenance::stamp_evidence(&mut edge.evidence, session);
                if self.capture_sentence() {
                    self.record_sentence(line, "edge add", &edge);
                }
                self.b.insert_edge(edge);
                self.added_edges.insert(key.clone(), id);
                self.edges = true;
                r.id = Some(id);
            }
        }
        Ok((r, warnings))
    }

    fn node_edit(
        &mut self,
        line: u32,
        node: Option<String>,
        node_ref: Option<String>,
        req: NodeEditRequest,
    ) -> LineResult {
        let id = self
            .pair(line, node.as_deref(), node_ref.as_deref(), "node", true)?
            .expect("a needed node");
        let mut n = self.b.graph().node(id).expect("a resolved node").to_node();
        let mut warnings = Vec::new();
        let changed = edit_node(
            &self.graph,
            self.o,
            self.b.graph(),
            &mut n,
            req,
            &mut warnings,
        )?;
        if !changed.is_empty() {
            n.updated = self.now.clone();
            let evidence_start = n.evidence.len();
            self.replace_node(line, n, true, evidence_start, &mut warnings)?;
        }
        let mut r = row(line, "node edit");
        r.id = Some(id);
        r.changed = changed;
        Ok((r, warnings))
    }

    fn check_node_secrets(&self, line: u32, node: &umx_core::Node) -> Result<(), Problem> {
        let before = self.b.graph().node(node.id).map(|old| old.to_node());
        match node_write_secrets(before.as_ref(), node, self.o, self.owner, &self.allows)
            .into_iter()
            .next()
        {
            Some(hit) => Err(hit.refusal(line)),
            None => Ok(()),
        }
    }

    fn replace_node(
        &mut self,
        line: u32,
        mut node: umx_core::Node,
        touched: bool,
        evidence_start: usize,
        warnings: &mut Vec<Problem>,
    ) -> Result<(), Problem> {
        super::node_more::canonical_keys(&mut node, super::tags::is_std(self.o));
        self.check_node_secrets(line, &node)?;
        let before = self.b.graph().node(node.id).map(|old| old.to_node());
        warnings.extend(super::node::title_key_warnings(before.as_ref(), &node));
        let session = self.session(line, warnings)?;
        if !umx_core::provenance::is_provenance(&node) {
            node.record_edit(session);
        }
        super::provenance::stamp_evidence(&mut node.evidence[evidence_start..], session);
        let moves = before.as_ref().map_or(0, |old| old.status_history.len());
        super::provenance::stamp_status_evidence(&mut node.status_history[moves..], session);
        // Only a node with a trail can have gained an entry, so only then is the stored node read.
        if node
            .fields
            .contains_key(umx_core::evidence::EVIDENCE_REMOVED)
        {
            let from = self
                .b
                .graph()
                .node(node.id)
                .map_or(0, |old| super::evidence_edit::trail_len(&old.to_node()));
            super::evidence_edit::stamp_removals(&mut node, from, session);
        }
        let id = node.id;
        self.b
            .insert_node(node)
            .map_err(|e| problems::image_full(&e))?;
        self.nodes = true;
        if touched {
            self.touch(id, line);
        }
        if let Some(lines) = self.b.graph().repeat_lines(GraphFile::Nodes, id) {
            warnings.push(problems::duplicate_id(
                &self.graph,
                &Repeat {
                    file: GraphFile::Nodes,
                    id,
                    lines,
                },
            ));
        }
        Ok(())
    }

    fn node_note(
        &mut self,
        line: u32,
        node: Option<String>,
        node_ref: Option<String>,
        req: NodeNoteRequest,
    ) -> LineResult {
        if req.evidence.len() > MAX_BATCH_EVIDENCE {
            return Err(problems::batch_line_invalid(
                line,
                "a node note line holds more than 32 evidence pointers",
            ));
        }
        let id = self
            .pair(line, node.as_deref(), node_ref.as_deref(), "node", true)?
            .expect("a needed node");
        let mut candidate = self.b.graph().node(id).expect("resolved").to_node();
        let evidence_start = candidate.evidence.len();
        let (added, mut warnings) = note_node(
            self.dir,
            &self.sources,
            &mut candidate,
            req,
            &self.now,
            &|alias| {
                if alias == self.alias || self.own_former.iter().any(|f| f == alias) {
                    Some(Evidence::Graph(self.owner, self.b.graph(), self.alias))
                } else {
                    self.others.get(alias).copied()
                }
            },
        )?;
        self.replace_node(line, candidate, true, evidence_start, &mut warnings)?;
        let mut result = row(line, "node note");
        result.id = Some(id);
        result.changed.push("note".into());
        if added > 0 {
            result.changed.push("evidence".into());
        }
        Ok((result, warnings))
    }

    fn node_status(
        &mut self,
        line: u32,
        node: Option<String>,
        node_ref: Option<String>,
        req: NodeStatusRequest,
    ) -> LineResult {
        let id = self
            .pair(line, node.as_deref(), node_ref.as_deref(), "node", true)?
            .expect("a needed node");
        let mut candidate = self.b.graph().node(id).expect("resolved").to_node();
        let evidence_start = candidate.evidence.len();
        let changed = candidate.status != req.status;
        let mut warnings = Vec::new();
        // TM6: the line's `in` and `evidence`, resolved as the verb resolves them (a batch's --raised-in is not one).
        let place = match req.anchor.as_deref() {
            Some(text) => {
                let anchor = self.line_anchor(line, text, &mut warnings)?;
                Some(super::node_more::place_address(self.owner, anchor.end))
            }
            None => None,
        };
        let (evidence, evidence_warnings) = super::node_more::status_evidence(
            self.dir,
            req.evidence.as_deref(),
            req.evidence_kind.as_deref(),
            &self.sources,
            &|a| {
                if a == self.alias || self.own_former.iter().any(|f| f == a) {
                    Some(Evidence::Graph(self.owner, self.b.graph(), self.alias))
                } else {
                    self.others.get(a).copied()
                }
            },
        )?;
        warnings.extend(evidence_warnings);
        status_node(
            self.ctx,
            &self.graph,
            self.o,
            &mut candidate,
            req,
            (place, evidence),
            &self.now,
            &mut warnings,
        )?;
        self.replace_node(line, candidate, false, evidence_start, &mut warnings)?;
        let mut result = row(line, "node status");
        result.id = Some(id);
        if changed {
            result.changed.push("status".into());
        }
        result.changed.push("status_history".into());
        Ok((result, warnings))
    }

    /// The edge (from, kind, to) as the batch holds it so far: a deleted or edited edge is not found under its old
    /// key. Local ends go through the batch's own lookup; an external end scans the graph's edges the batch has not
    /// removed (R287-1: an edge an earlier line deleted is no twin). An edit rewrites its edge in place, so it is
    /// found at its new key only; edges the batch added out of order are the caller's (`added_edges`).
    fn find_edge(&self, from: StoredEnd, kind: &str, to: StoredEnd) -> Option<EdgeRef<'_>> {
        if from.graph.is_none() && to.graph.is_none() {
            self.b.find_edge(from.node, kind, to.node)
        } else {
            find_edge_where(self.b.graph(), from, kind, to, |id| {
                self.b.edge(id).is_some()
            })
        }
    }

    /// The batch's edge `text` names, as earlier lines left it.
    fn live_edge(&self, text: &str) -> Result<umx_core::Edge, Problem> {
        let scope = super::ReadScope::new(self.b.graph());
        resolve_edge(self.alias, text, |id| {
            self.b.edge_record(id).filter(|e| scope.stored_record(e))
        })
    }

    /// Takes edge `e` out of the overlay, if it is there under its current key (graph-qualified ends, F122).
    fn forget(&mut self, e: &umx_core::Edge) {
        let key = (
            StoredEnd {
                graph: e.from_graph,
                node: e.from,
            },
            e.kind.clone(),
            StoredEnd {
                graph: e.to_graph,
                node: e.to,
            },
        );
        if self.added_edges.get(&key) == Some(&e.id) {
            self.added_edges.remove(&key);
        }
    }

    /// Notes that a line changes `before` (the edge as it was): one with an end in another graph puts graph.json in
    /// the write set (F122), whatever the change.
    fn changing_edge(&mut self, before: &Edge) {
        self.graph_json |= before.has_external_end();
    }

    fn edge_delete(&mut self, line: u32, edge: String) -> LineResult {
        let current = self.live_edge(&edge)?;
        check_changeable(&current)?;
        check_deletable(self.b.graph(), &self.graph, &current)?;
        if self.capture_sentence() {
            self.record_sentence(line, "edge delete", &current);
        }
        self.forget(&current);
        self.changing_edge(&current);
        if current.has_external_end() {
            self.external_old.push(current.clone());
        }
        self.b.remove_edge(current.id);
        self.edges = true;
        let mut r = row(line, "edge delete");
        r.id = Some(current.id);
        Ok((r, Vec::new()))
    }

    fn edge_edit(&mut self, line: u32, edge: String, req: EdgeEditRequest) -> LineResult {
        let current = self.live_edge(&edge)?;
        let mut warnings = Vec::new();
        let (mut edited, changed) = plan_edge_edit(
            self.ctx,
            &self.graph,
            self.owner,
            self.o,
            &current,
            &req,
            (self.now.as_str(), self.ctx.actor_display()),
            &|a, k, b| {
                self.added_edges
                    .get(&(a, k.to_string(), b))
                    .copied()
                    .or_else(|| self.find_edge(a, k, b).map(|e| e.id()))
            },
            &mut warnings,
        )?;
        if !changed.is_empty()
            && let Some(hit) = edge_write_secrets(&edited.fields, &[]).into_iter().next()
        {
            return Err(hit.refusal(line));
        }
        if self.capture_sentence() {
            self.record_sentence(line, "edge edit", &edited);
        }
        if !changed.is_empty() {
            let session = self.session(line, &mut warnings)?;
            edited.record_edit(session);
            self.forget(&current);
            self.changing_edge(&current);
            if current.has_external_end() {
                self.external_old.push(current.clone());
            }
            self.added_edges.insert(
                (
                    StoredEnd {
                        graph: edited.from_graph,
                        node: edited.from,
                    },
                    edited.kind.clone(),
                    StoredEnd {
                        graph: edited.to_graph,
                        node: edited.to,
                    },
                ),
                edited.id,
            );
            self.b.insert_edge(edited);
            self.edges = true;
            warnings.extend(repeat_warning(self.b.graph(), &self.graph, current.id));
        }
        let mut r = row(line, "edge edit");
        r.id = Some(current.id);
        r.changed = changed;
        Ok((r, warnings))
    }

    fn apply(&mut self, line: u32, op: BatchOp) -> LineResult {
        match op {
            BatchOp::NodeAdd {
                id,
                created,
                ref_name,
                req,
                parent_ref,
            } => self.node_add(line, ref_name, id, created, req, parent_ref),
            BatchOp::EdgeAdd {
                id,
                from,
                from_ref,
                relation,
                to,
                to_ref,
                sets,
                evidence,
            } => self.edge_add(
                line, id, from, from_ref, relation, to, to_ref, sets, evidence,
            ),
            BatchOp::NodeEdit {
                node,
                node_ref,
                req,
            } => self.node_edit(line, node, node_ref, req),
            BatchOp::NodeNote {
                node,
                node_ref,
                req,
            } => self.node_note(line, node, node_ref, req),
            BatchOp::NodeStatus {
                node,
                node_ref,
                req,
            } => self.node_status(line, node, node_ref, req),
            BatchOp::EdgeDelete { edge } => self.edge_delete(line, edge),
            BatchOp::EdgeEdit { edge, req } => self.edge_edit(line, edge, req),
            BatchOp::EvidenceAdd {
                node,
                node_ref,
                pointer,
                evidence_kind,
            } => self.evidence_add(line, node, node_ref, &pointer, evidence_kind),
            BatchOp::EvidenceRemove {
                node,
                node_ref,
                pointer,
                reason,
                at,
                skip_missing,
            } => self.evidence_remove(line, node, node_ref, &pointer, reason, at, skip_missing),
        }
    }

    /// `evidence add`: the node verb's parser and checks (`root:` sources, `umx:` evidence against the loaded graphs,
    /// this batch's graph as the earlier lines left it). A pointer the node already holds changes nothing.
    fn evidence_add(
        &mut self,
        line: u32,
        node: Option<String>,
        node_ref: Option<String>,
        pointer: &str,
        evidence_kind: Option<String>,
    ) -> LineResult {
        let id = self
            .pair(line, node.as_deref(), node_ref.as_deref(), "node", true)?
            .expect("a needed node");
        let lookup = |a: &str| {
            if a == self.alias || self.own_former.iter().any(|f| f == a) {
                Some(Evidence::Graph(self.owner, self.b.graph(), self.alias))
            } else {
                self.others.get(a).copied()
            }
        };
        let mut pointers = evidence_pointers(
            self.dir,
            &[pointer.to_string()],
            evidence_kind.as_deref(),
            &self.sources,
            &lookup,
        )?;
        let mut warnings = check_evidence(&mut pointers, &lookup);
        warnings.extend(self.sources.warnings(&pointers));
        let mut n = self.b.graph().node(id).expect("a resolved node").to_node();
        let mut r = row(line, "evidence add");
        r.id = Some(id);
        let evidence_start = n.evidence.len();
        if add_evidence(&mut n.evidence, pointers) == 1 {
            n.updated = self.now.clone();
            self.replace_node(line, n, false, evidence_start, &mut warnings)?;
            r.changed = alloc::vec!["evidence".to_string()];
        }
        Ok((r, warnings))
    }

    /// `evidence remove`: the node verb's matching and trail. already_removed always warns; a pointer that matches
    /// nothing fails the line unless the line says `"missing": "skip"`.
    #[allow(clippy::too_many_arguments)]
    fn evidence_remove(
        &mut self,
        line: u32,
        node: Option<String>,
        node_ref: Option<String>,
        pointer: &str,
        reason: Option<String>,
        at: Option<usize>,
        skip_missing: bool,
    ) -> LineResult {
        let id = self
            .pair(line, node.as_deref(), node_ref.as_deref(), "node", true)?
            .expect("a needed node");
        let reason = check_reason(
            reason.as_deref().or(self.reason.as_deref()),
            "batch",
            super::evidence_edit::EVIDENCE_REASON_MISSING,
        )?;
        let given = pointer_to_match(self.dir, pointer)?;
        let mut n = self.b.graph().node(id).expect("a resolved node").to_node();
        let mut r = row(line, "evidence remove");
        r.id = Some(id);
        let mut warnings = Vec::new();
        match remove_pointer(
            &self.graph,
            &mut n,
            &given,
            at,
            &reason,
            &self.now,
            &self.ctx.by,
        ) {
            Ok(Removal::Removed(_)) => {
                n.updated = self.now.clone();
                let evidence_start = n.evidence.len();
                self.replace_node(line, n, false, evidence_start, &mut warnings)?;
                r.changed = alloc::vec![
                    "evidence".to_string(),
                    umx_core::evidence::EVIDENCE_REMOVED.to_string()
                ];
            }
            Ok(Removal::AlreadyRemoved) => {
                warnings.push(problems::already_removed(&self.graph, id))
            }
            Err(p) if skip_missing && p.code == "evidence_not_found" => {
                warnings.push(problems::evidence_missing_skipped(&self.graph, id));
            }
            Err(p) => return Err(p),
        }
        Ok((r, warnings))
    }

    /// A line's error as a batch shows it: every command in it becomes the batch's dry run. The fix is in that line of
    /// the file, never a single verb run against the graph, and a command would carry the line's values (review
    /// R103-3: `use_status_verb` suggested `umx node status <node> <the line's status>`). Kept: the fix to a line that
    /// writes a denial (ws008 E2c, R6), `umx entity deny` or `undeny` with placeholders only, since no batch line
    /// can write one.
    fn retarget(p: Problem) -> Problem {
        let mut p = p;
        let denial_fix = |c: &Command| {
            ["umx entity deny ", "umx entity undeny "]
                .iter()
                .any(|v| c.as_str().starts_with(v))
        };
        // plan 205: edge_exists_differs keeps its in-place `edge edit` of the stored edge, and on_key_ambiguous (ws003
        // s08) its key removal on a node of the graph (a drawn id still hides).
        if p.code == "edge_exists_differs" || p.code == "on_key_ambiguous" {
            return p;
        }
        for (_, slot) in &mut p.slots {
            if matches!(slot, Slot::Command(c) if !denial_fix(c)) {
                *slot = dry_run_command();
            }
        }
        p
    }

    /// The problem as shown when the ids this batch drew will not exist (a dry run, or a failed batch): such an id
    /// becomes `<id>` in a slot, a command naming one becomes the dry-run command, and a candidate holding one is
    /// named by its line.
    fn hide_drawn(&self, mut p: Problem) -> Problem {
        let drawn = |id: &Id| self.minted.contains_key(id);
        let names_drawn = |text: &str| {
            text.split([' ', '/'])
                .filter_map(|w| w.parse::<Id>().ok())
                .any(|id| drawn(&id))
        };
        for (_, slot) in &mut p.slots {
            match slot {
                Slot::Id(id) if drawn(id) => *slot = Slot::Word("<id>"),
                Slot::Command(c) if names_drawn(c.as_str()) => *slot = dry_run_command(),
                _ => {}
            }
        }
        if p.subject.as_deref().is_some_and(names_drawn) {
            p.subject = None;
        }
        for c in &mut p.candidates {
            if let Some(line) = c.id.parse::<Id>().ok().and_then(|id| self.minted.get(&id)) {
                c.id = alloc::format!("line:{line}");
            }
        }
        p
    }
}

impl GraphStore {
    /// Reads the graph's receipts for a keyed batch and decides: a replay, a refusal, or a run (ws003 s09). Runs
    /// after the graph is chosen (so a caller who may not write it cannot read its receipts) and before any line,
    /// anchor or id: a replay says what the batch did then, not what it would do now.
    fn keyed_start(
        &self,
        alias: &str,
        graph_id: Id,
        given: Idempotency,
        (lines, dry_run, now): (&[BatchLine], bool, u64),
    ) -> Result<KeyedStart, Problem> {
        let loaded = &self.graphs[alias];
        let graph = alias_name(alias);
        let (bytes, read) = match self
            .ports
            .files
            .read_ref_stamped(&loaded.dir, &FileRef::Receipts)
        {
            Ok(read) => read,
            Err(FilesError::TooLarge { .. }) => {
                return Err(problems::receipts_unreadable(
                    &graph,
                    "it is over the 16 MiB umx reads",
                ));
            }
            Err(FilesError::RecoveryConflict(conflicts)) => {
                return Err(problems::recovery_conflict(
                    &loaded.dir,
                    &conflicts,
                    Some(&graph),
                ));
            }
            Err(e) => {
                return Err(problems::receipts_unreadable(
                    &graph,
                    &format!("it could not be read ({e})"),
                ));
            }
        };
        let mut kept = match bytes {
            None => Receipts::new(graph_id),
            Some(bytes) => Receipts::decode(&bytes, graph_id)
                .map_err(|e| problems::receipts_unreadable(&graph, &e.to_string()))?,
        };
        kept.evict(now);
        let digest = receipts::key_digest(&given.key);
        let expired = match kept.find(&digest) {
            Lookup::None => None,
            Lookup::Dropped { at, .. } => Some(at),
            Lookup::Live { at, input, .. } if input != given.input => {
                return Err(problems::idempotency_key_reused(&given.key, &graph, at));
            }
            Lookup::Live { at, receipt, .. } => {
                // graph-addressing: never hand back an id the graph does not hold.
                let missing = receipt
                    .refs
                    .iter()
                    .filter(|id| !loaded.graph.holds_id(**id))
                    .count();
                if missing > 0 {
                    return Err(problems::idempotency_receipt_stale(
                        &given.key, &graph, missing,
                    ));
                }
                // The same input, so the same lines: the ref names are the replay's own, in definition order.
                let names: Vec<String> = lines
                    .iter()
                    .filter_map(|l| match &l.op {
                        BatchOp::NodeAdd {
                            ref_name: Some(name),
                            ..
                        } => Some(name.clone()),
                        _ => None,
                    })
                    .collect();
                if names.len() != receipt.refs.len() {
                    return Err(problems::receipts_unreadable(
                        &graph,
                        "a receipt does not match its batch's refs",
                    ));
                }
                let refs = names
                    .into_iter()
                    .zip(receipt.refs.iter().map(|id| Some(*id)))
                    .collect();
                return Ok(KeyedStart::Replay(Done {
                    value: BatchDone {
                        alias: alias.to_string(),
                        dry_run,
                        graph_id,
                        refs,
                        rows: Vec::new(),
                        sentences: None,
                        sentences_omitted: 0,
                        touched: Vec::new(),
                        warned: 0,
                        counts: receipt.counts.clone(),
                        keyed: Some(Keyed {
                            key: given.key.clone(),
                            replayed: true,
                            recorded_at: Some(at),
                        }),
                    },
                    notes: alloc::vec![problems::batch_replayed(&given.key, at)],
                }));
            }
        };
        Ok(KeyedStart::Run(KeyedRun {
            key: given.key,
            digest,
            input: given.input,
            receipts: kept,
            read,
            expired,
        }))
    }

    fn batch_ids(
        &self,
        alias: &str,
        lines: &[BatchLine],
        now: u64,
    ) -> Result<BTreeSet<Id>, Vec<Problem>> {
        let loaded = &self.graphs[alias];
        let graph = &loaded.graph;
        let (mut ids, mut errors, mut failed) = (BTreeSet::new(), Vec::new(), 0);
        for BatchLine { line, op } in lines {
            if let BatchOp::NodeAdd {
                id: Some(_), req, ..
            } = op
                && req.on_key_resolve
            {
                failed += 1;
                if errors.len() < MAX_BATCH_ERRORS {
                    errors.push(problems::batch_line_invalid(*line, ON_KEY_WITH_ID));
                }
                continue;
            }
            let (id, created) = match op {
                BatchOp::NodeAdd { id, created, .. } => (id, created.as_deref()),
                BatchOp::EdgeAdd { id, .. } => (id, None),
                BatchOp::NodeEdit { .. }
                | BatchOp::NodeNote { .. }
                | BatchOp::NodeStatus { .. }
                | BatchOp::EdgeDelete { .. }
                | BatchOp::EdgeEdit { .. }
                | BatchOp::EvidenceAdd { .. }
                | BatchOp::EvidenceRemove { .. } => {
                    continue;
                }
            };
            let problem = match id.as_deref().map(str::parse::<Id>) {
                Some(Err(_)) => Some(problems::id_invalid()),
                Some(Ok(id)) if matches!(op, BatchOp::NodeAdd { .. }) && id == loaded.meta.id => {
                    Some(problems::id_taken(
                        "the graph id (reserved for its virtual node)",
                    ))
                }
                Some(Ok(id)) if graph.holds_id(id) || !ids.insert(id) => Some(problems::id_taken(
                    "already held by this graph or another line of this batch",
                )),
                Some(Ok(id)) if (id.0 >> 80) > u128::from(now.saturating_add(60_000)) => {
                    Some(problems::id_in_future())
                }
                _ => None,
            }
            .or_else(|| {
                created
                    .filter(|s| umx_core::time::normalise_date(s).is_none_or(|s| !s.contains('T')))
                    .map(|_| problems::batch_line_invalid(*line, "created must be a timestamp"))
            });
            if let Some(p) = problem {
                failed += 1;
                if errors.len() < MAX_BATCH_ERRORS {
                    errors.push(p.at_line(*line));
                }
            }
        }
        if failed == 0 {
            return Ok(ids);
        }
        let mut all = alloc::vec![problems::batch_failed(failed)];
        all.extend(errors);
        if failed > MAX_BATCH_ERRORS {
            all.push(problems::batch_errors_truncated(failed, MAX_BATCH_ERRORS));
        }
        Err(all)
    }

    /// `umx batch`: applies every line of `req` to one graph as one change, or none of them (see the module docs).
    pub fn batch(&mut self, ctx: &Ctx, mut req: BatchRequest) -> BatchOutcome {
        if req.lines.is_empty() {
            return Err(alloc::vec![problems::usage_invalid(
                "the batch holds no ops",
                "batch"
            )]);
        }
        if req.lines.len() > MAX_BATCH_OPS {
            return Err(alloc::vec![problems::batch_too_large(
                req.lines.len() as u64,
                "ops",
                MAX_BATCH_OPS as u64,
            )]);
        }
        if let Some(given) = &req.idempotency {
            if !receipts::check_key(&given.key) {
                return Err(alloc::vec![problems::usage_invalid(
                    receipts::KEY_RULE,
                    "batch"
                )]);
            }
            if !receipts::is_digest(&given.input) {
                return Err(alloc::vec![problems::internal("batch_input_digest")]);
            }
        }
        let imported = imported_refs(&req.refs).map_err(|p| alloc::vec![p])?;
        let named: Vec<Option<&str>> = req.lines.iter().flat_map(|l| l.op.named()).collect();
        let alias = self
            .select_author(ctx, &named)
            .map_err(|p| alloc::vec![p])?;
        let graph_id = self.graphs[&alias].meta.id;
        let now_ms = self.ports.clock.now_ms();
        let mut keyed = match req.idempotency.take() {
            None => None,
            Some(given) => match self
                .keyed_start(&alias, graph_id, given, (&req.lines, req.dry_run, now_ms))
                .map_err(|p| alloc::vec![p])?
            {
                KeyedStart::Replay(done) => return Ok(done),
                KeyedStart::Run(run) => Some(run),
            },
        };
        // TM4: the batch's anchor, resolved once right after the graph is chosen, before any line is checked (R341-2:
        // its refusal is edge add's own, never a line's preflight error); one that does not resolve refuses the batch
        // as edge add would refuse that end. Its warnings are the batch's, once.
        let mut anchor_warnings = Vec::new();
        let anchor = match req.raised_in.as_deref() {
            Some(text) => {
                let (anchor, warnings) = self
                    .raised_in_anchor(&alias, text)
                    .map_err(|p| alloc::vec![p])?;
                anchor_warnings.extend(warnings);
                anchor_warnings.extend(super::raised_in::vocabulary(
                    self.graphs[&alias].ontology(),
                    &alias_name(&alias),
                ));
                Some(anchor)
            }
            None => None,
        };
        // Copies of one graph share its id, so any copy matches; an alias is never compared (it is a hint).
        match req.refs_graph.as_deref().map(canonical_id) {
            None => {}
            Some(Some(id)) if id == graph_id => {}
            Some(Some(_)) => return Err(alloc::vec![problems::batch_refs_other_graph()]),
            Some(None) => {
                return Err(alloc::vec![problems::usage_invalid(
                    "the --refs-from graph id is not a full umx id",
                    "batch"
                )]);
            }
        }
        let reserved = self.batch_ids(&alias, &req.lines, now_ms)?;
        let mut other_warnings = BTreeMap::new();
        for BatchLine { line, op } in &req.lines {
            if let BatchOp::NodeAdd { id: Some(id), .. } = op {
                let id = id.parse::<Id>().expect("explicit ids passed preflight");
                let warnings: Vec<Problem> = self
                    .graphs
                    .iter()
                    .filter(|(other, l)| {
                        l.meta.id != graph_id
                            && self.external_visible(ctx, &named, other)
                            && l.graph.node(id).is_some()
                    })
                    .map(|(other, _)| problems::id_in_other_graph(id, &alias_name(other)))
                    .collect();
                other_warnings.insert(*line, warnings);
            }
        }
        let mut prepared = BTreeMap::new();
        let mut end_kinds = BTreeMap::new();
        let mut open_ends = BTreeSet::new();
        // A line's own anchor (a node add's raised_in, a status move's in, TM6) in another graph is prepared as an edge
        // end is; its type is kept only from an open
        // graph (d-20).
        let mut anchor_kinds = BTreeMap::new();
        for line in &req.lines {
            let text = match &line.op {
                BatchOp::NodeAdd { req: add, .. } => add.raised_in.as_ref(),
                BatchOp::NodeStatus { req: status, .. } => status.anchor.as_ref(),
                _ => None,
            };
            if let Some(text) = text
                && !self.local_end(&alias, text)
            {
                let end = self.write_end(&alias, text);
                if let Ok((end, _)) = &end
                    && let Some(copy) = self
                        .end_alias(&alias, text)
                        .filter(|c| self.shown_unnamed(c))
                    && let Some(n) = self.graphs.get(&copy).and_then(|l| l.graph.node(end.node))
                {
                    anchor_kinds.insert(text.clone(), n.kind().to_string());
                }
                prepared.insert(text.clone(), end);
            }
        }
        for line in &req.lines {
            if let BatchOp::EdgeAdd { from, to, .. } = &line.op {
                for text in from.iter().chain(to) {
                    if !self.local_end(&alias, text) {
                        let end = self.write_end(&alias, text);
                        if let Ok((end, _)) = &end
                            && let Some(copy) = self.end_alias(&alias, text)
                            && let Some(n) =
                                self.graphs.get(&copy).and_then(|l| l.graph.node(end.node))
                        {
                            if self.shown_unnamed(&copy) {
                                open_ends.insert(text.clone());
                            }
                            end_kinds.insert(text.clone(), (copy, n.kind().to_string()));
                        }
                        prepared.insert(text.clone(), end);
                    }
                }
            }
        }
        let owner = self.graphs[&alias].meta.id;
        let mut local_aliases: Vec<String> = self
            .copies_of(owner)
            .into_iter()
            .map(ToString::to_string)
            .collect();
        // ws006 S23 (review R11): a former alias that names this graph is local too, as its live alias is.
        let formers: Vec<String> = local_aliases
            .iter()
            .flat_map(|a| self.graphs[a].meta.former_aliases.iter().cloned())
            .filter(|f| {
                self.former_owner(f, &|_| true)
                    .ok()
                    .flatten()
                    .is_some_and(|live| self.graphs[&live].meta.id == owner)
            })
            .collect();
        local_aliases.extend(formers);
        let now = self.now();
        let files = self.ports.files.clone();
        let id_port = self.ports.ids.clone();
        let sources = self.source_check(&alias);
        let allows = self.secret_allows();
        // Evidence is checked against open graphs only (B1): a secured graph reads as not loaded.
        let open = self.open_aliases();
        // An endpoint address is not an opt-in to echo a named graph's title or alias.
        let echo_aliases: BTreeSet<String> = if req.echo_sentences {
            self.graphs
                .iter()
                .filter(|(alias, loaded)| {
                    self.shown_unnamed(alias)
                        && self.primary_of(loaded.meta.id) == Some(alias.as_str())
                })
                .map(|(alias, _)| alias.clone())
                .collect()
        } else {
            BTreeSet::new()
        };
        // ws006 S23: which former aliases redirect to the batch's graph or an open one (graph-addressing-expert).
        let shown = |a: &str| a == alias || open.contains(a);
        // None: two shown graphs had it (review R16), which evidence reports rather than reading as unchecked.
        let mut redirect: BTreeMap<String, Option<String>> = BTreeMap::new();
        for (a, l) in self.graphs.iter().filter(|(a, _)| shown(a)) {
            for former in &l.meta.former_aliases {
                match self.former_owner(former, &shown) {
                    Ok(Some(live)) if live == *a => {
                        redirect.insert(former.clone(), Some(live));
                    }
                    Err(_) => {
                        redirect.insert(former.clone(), None);
                    }
                    _ => {}
                }
            }
        }
        let own_former: Vec<String> = redirect
            .iter()
            .filter(|(_, live)| live.as_deref() == Some(alias.as_str()))
            .map(|(former, _)| former.clone())
            .collect();
        let mut target = None;
        let mut others = BTreeMap::new();
        let mut echo_graphs = BTreeMap::new();
        for (a, l) in self.graphs.iter_mut() {
            if *a == alias {
                target = Some(l);
            } else {
                let l: &Loaded = l;
                if open.contains(a.as_str()) {
                    others.insert(a.as_str(), Evidence::Graph(l.meta.id, &l.graph, a.as_str()));
                    for former in &l.meta.former_aliases {
                        match redirect.get(former) {
                            Some(Some(live)) if live == a => {
                                others.insert(
                                    former.as_str(),
                                    Evidence::Graph(l.meta.id, &l.graph, a.as_str()),
                                );
                            }
                            Some(None) => {
                                others.insert(former.as_str(), Evidence::Ambiguous);
                            }
                            _ => {}
                        }
                    }
                }
                if echo_aliases.contains(a.as_str()) {
                    echo_graphs.insert(
                        l.meta.id,
                        EchoGraph {
                            id: l.meta.id,
                            alias: a,
                            title: &l.meta.title,
                            graph: &l.graph,
                        },
                    );
                }
            }
        }
        let Loaded {
            dir,
            meta,
            ontology,
            graph: g,
            edges_normalized,
            instance,
            feed,
            stamps,
            ..
        } = target.expect("selected graph is loaded");
        let floor = g
            .last_id()
            .into_iter()
            .chain(reserved.iter().copied())
            .max();
        let first = (!req.dry_run
            && floor.is_some()
            && req
                .lines
                .iter()
                .any(|l| matches!(l.op, BatchOp::NodeAdd { id: None, .. })))
        .then(|| id_port.next_id());
        let preplanned = first.is_some_and(|id| floor.is_some_and(|floor| id <= floor));
        let ids = if req.dry_run {
            // Content draws at most three ids per line (a node, its parent edge and its raised_in edge, TM4); first
            // attribution adds at most five nodes and four edges.
            let base = g
                .last_id()
                .into_iter()
                .chain(reserved.iter().copied())
                .chain([owner])
                .map(|id| id.0)
                .max()
                .unwrap_or(0);
            let needed = 3 * req.lines.len() as u128 + super::provenance::MAX_NEW_IDS;
            if base > u128::MAX - needed {
                return Err(alloc::vec![problems::internal("dry_run_ids")]);
            }
            Ids::DryRun(base)
        } else {
            Ids::Real(id_port.as_ref(), first)
        };
        let (
            (refs, counts),
            rows,
            sentences,
            sentences_omitted,
            warnings,
            warned,
            mut late,
            touched,
            (added_edges, external_old),
            written_umx,
            provenance_changed,
        ) = {
            let mut run = Run {
                ctx,
                alias: &alias,
                graph: alias_name(&alias),
                o: ontology,
                dir,
                owner,
                owner_title: &meta.title,
                local_aliases,
                sources,
                allows,
                reason: req.reason.clone(),
                prepared,
                end_kinds,
                open_ends,
                b: g.batch(),
                others,
                own_former,
                echo_graphs,
                echo_sentences: req.echo_sentences,
                sentences: Vec::new(),
                sentences_omitted: 0,
                ids,
                now,
                refs: BTreeMap::new(),
                ref_order: Vec::new(),
                imported,
                added_edges: BTreeMap::new(),
                external_old: Vec::new(),
                graph_json: false,
                minted: BTreeMap::new(),
                reserved,
                parent_edges: BTreeMap::new(),
                vocabulary_said: anchor.is_some(),
                anchor,
                anchor_kinds,
                raised_edges: BTreeMap::new(),
                near_checks: true,
                nodes: false,
                edges: false,
                touched: Vec::new(),
                touched_at: BTreeMap::new(),
                session: None,
            };
            // Only a mixed batch below an imported/future maximum needs minted nodes in the bulk merge.
            // Keep the generator's identities: floor + 1 would collide across independent graph copies.
            if preplanned {
                for BatchLine { line, op } in &mut req.lines {
                    match op {
                        // ws003 s08: an on_key line may resolve, so it holds no placeholder; if it creates, its
                        // node and edges draw their ids at its line (a middle insert).
                        BatchOp::NodeAdd { req, .. } if req.on_key_resolve => {}
                        BatchOp::NodeAdd {
                            id,
                            req,
                            parent_ref,
                            ..
                        } => {
                            let node = id
                                .as_deref()
                                .map(|s| s.parse().expect("checked id"))
                                .unwrap_or_else(|| run.next_id(*line));
                            if id.is_none() {
                                *id = Some(node.to_text());
                            }
                            if req.parent.is_some() || parent_ref.is_some() {
                                let edge = run.next_id(*line);
                                run.parent_edges.insert(node, edge);
                            }
                            if req.raised_in.is_some() || run.anchor.is_some() {
                                let edge = run.next_id(*line);
                                run.raised_edges.insert(node, edge);
                            }
                        }
                        BatchOp::EdgeAdd { id, .. } if id.is_none() => {
                            *id = Some(run.next_id(*line).to_text());
                        }
                        _ => {}
                    }
                }
            }
            let explicit_nodes = req
                .lines
                .iter()
                .filter_map(|l| match &l.op {
                    BatchOp::NodeAdd { id: Some(id), .. } => {
                        Some(id.parse::<Id>().expect("explicit ids passed preflight"))
                    }
                    _ => None,
                })
                .collect();
            run.b
                .insert_placeholders(explicit_nodes, &run.now)
                .map_err(|e| alloc::vec![problems::image_full(&e)])?;
            let (mut rows, mut warnings, mut errors) = (Vec::new(), Vec::new(), Vec::new());
            // Hints (ws002 X1) are listed after the warnings, capped alike, and never counted in `warned`.
            let mut hints = Vec::new();
            // ws003 s09 (prime ruling): a key whose receipt was dropped runs as new, and the reply says so.
            if let Some(k) = &keyed
                && let Some(at) = k.expired
            {
                hints.push(problems::idempotency_key_expired(&k.key, at));
            }
            let (mut failed, mut warned) = (0usize, 0usize);
            for p in anchor_warnings {
                warned += 1;
                if warnings.len() < MAX_BATCH_WARNINGS {
                    warnings.push(bounded(p));
                }
            }
            // plan 205: hints that ride a failed batch (`batch_use_ref_field`), listed after its errors.
            let mut failure_hints = Vec::new();
            for BatchLine { line, op } in req.lines {
                let duplicates = op.duplicate_sets();
                let ref_fields: Vec<(&'static str, &'static str)> = op
                    .plain_fields()
                    .into_iter()
                    .filter_map(|(field, ref_field, text)| {
                        text.filter(|t| run.knows_ref(t))
                            .map(|_| (field, ref_field))
                    })
                    .collect();
                match run.apply(line, op) {
                    Ok((r, mut w)) => {
                        rows.push(r);
                        w.extend(other_warnings.remove(&line).unwrap_or_default());
                        w.extend(
                            duplicates
                                .iter()
                                .map(|f| problems::duplicate_field(line, f)),
                        );
                        for p in w {
                            if problems::is_hint(p.code) {
                                // Every hint is kept: the daemon picks the first by catalogue order and counts
                                // the rest (review R1), and the reply bound counts only those it shows.
                                hints.push(bounded(p).at_line(line));
                                continue;
                            }
                            warned += 1;
                            if warnings.len() < MAX_BATCH_WARNINGS {
                                warnings.push(bounded(p).at_line(line));
                            }
                        }
                    }
                    Err(p) => {
                        failed += 1;
                        run.near_checks = false;
                        if errors.len() < MAX_BATCH_ERRORS {
                            if p.code == "node_not_found" {
                                failure_hints.extend(
                                    ref_fields
                                        .into_iter()
                                        .map(|(f, rf)| problems::batch_use_ref_field(line, f, rf)),
                                );
                            }
                            errors.push(bounded(Run::retarget(p)).at_line(line));
                        }
                    }
                }
            }
            if failed > 0 {
                let mut all = alloc::vec![problems::batch_failed(failed)];
                all.extend(errors.into_iter().map(|p| run.hide_drawn(p)));
                if failed > MAX_BATCH_ERRORS {
                    all.push(problems::batch_errors_truncated(failed, MAX_BATCH_ERRORS));
                }
                all.extend(failure_hints);
                run.b.rollback();
                return Err(all);
            }
            if run.b.unfilled() != 0 {
                run.b.rollback();
                return Err(alloc::vec![problems::internal("batch_unfilled")]);
            }
            // ws008 E1: each key a touched node now shares with another of its type, once per pair. Counted and
            // capped with the line warnings, before the reply bound, so a dry run reports them too.
            let mut pairs: Vec<(Id, Id)> = Vec::new();
            let g = run.b.graph();
            for id in &run.touched {
                let Some(n) = g.node(*id) else { continue };
                if n.field(umx_core::entity::ENTITY_KEY).is_none() {
                    continue;
                }
                for d in g.duplicate_entity_keys_of(*id) {
                    if !pairs.contains(&(d.node, d.in_use)) {
                        pairs.push((d.node, d.in_use));
                        warned += 1;
                        if warnings.len() < MAX_BATCH_WARNINGS {
                            warnings.push(problems::duplicate_key(&run.graph, n.kind(), &d));
                        }
                    }
                }
            }
            // ws002 X1: each touched std segment left with no part_of edge out of it, judged on the batch's result
            // (review R2, R5): the image's edge rows less those the batch removed, plus the edges the batch added but
            // has not placed yet (an out-of-order id); only an edge whose source is in this graph parents a node here.
            let part_of = |kind: &str| kind == umx_core::ontology::PART_OF;
            let mut parented: alloc::collections::BTreeSet<Id> = g
                .edges()
                .filter(|e| {
                    part_of(e.kind()) && e.from_graph().is_none() && run.b.edge(e.id()).is_some()
                })
                .map(|e| e.from())
                .collect();
            for id in run.added_edges.values() {
                if let Some(e) = run.b.edge_record(*id)
                    && part_of(&e.kind)
                    && e.from_graph.is_none()
                {
                    parented.insert(e.from);
                }
            }
            let mut by_line: Vec<(Id, u32)> =
                run.touched_at.iter().map(|(i, l)| (*i, *l)).collect();
            by_line.sort_by_key(|(_, l)| *l);
            for (id, line) in &by_line {
                let Some(n) = g.node(*id) else { continue };
                if let Some(hint) =
                    super::node::segment_hint(&run.graph, &n.to_node(), parented.contains(id))
                {
                    hints.push(hint.at_line(*line));
                }
            }
            if warned > MAX_BATCH_WARNINGS {
                warnings.push(problems::batch_warnings_truncated(
                    warned,
                    MAX_BATCH_WARNINGS,
                ));
            }
            // The reply must fit what the client reads: refuse before anything is written, dry run or not, so a dry run
            // that passes still means the real run passes. Of the hints, the reply shows at most HINTS_SHOWN: the
            // largest that many are counted.
            let mut shown_hints: Vec<&Problem> = hints.iter().collect();
            shown_hints.sort_by_key(|p| core::cmp::Reverse(issue_bound(p)));
            shown_hints.truncate(problems::HINTS_SHOWN);
            let bound = reply_bound(
                &alias,
                &rows,
                run.ref_order.len(),
                &warnings.iter().chain(shown_hints).collect::<Vec<_>>(),
                &run.sentences,
            );
            warnings.extend(hints);
            if bound > MAX_BATCH_REPLY {
                run.b.rollback();
                return Err(alloc::vec![problems::batch_too_large(
                    bound,
                    "reply bytes",
                    MAX_BATCH_REPLY
                )]);
            }
            let refs: Vec<(String, Option<Id>)> = run
                .ref_order
                .iter()
                .map(|name| match run.refs.get(name) {
                    Some(Named::Node(id)) => (
                        name.clone(),
                        (!req.dry_run || !run.minted.contains_key(id)).then_some(*id),
                    ),
                    _ => (name.clone(), None),
                })
                .collect();
            if req.dry_run {
                let warnings = warnings.into_iter().map(|p| run.hide_drawn(p)).collect();
                for r in &mut rows {
                    if r.id.is_some_and(|id| run.minted.contains_key(&id)) {
                        r.id = None;
                    }
                    r.parent_edge = None;
                    r.raised_in_edge = None;
                }
                for sentence in &mut run.sentences {
                    if sentence.id.is_some_and(|id| run.minted.contains_key(&id)) {
                        sentence.id = None;
                    }
                }
                let sentences = req
                    .echo_sentences
                    .then(|| core::mem::take(&mut run.sentences));
                let sentences_omitted = run.sentences_omitted;
                run.b.rollback();
                let counts = op_counts(&rows);
                return Ok(Done {
                    value: BatchDone {
                        alias,
                        dry_run: true,
                        graph_id,
                        refs,
                        rows,
                        sentences,
                        sentences_omitted,
                        touched: Vec::new(),
                        warned,
                        counts,
                        keyed: keyed.map(|k| Keyed {
                            key: k.key,
                            replayed: false,
                            recorded_at: None,
                        }),
                    },
                    notes: warnings,
                });
            }
            // graph.json joins the set when a line changed an edge with an end in another graph (graph-storage-expert,
            // F122).
            let which: Vec<GraphFile> = [
                (run.nodes, GraphFile::Nodes),
                (run.edges, GraphFile::Edges),
                (run.graph_json, GraphFile::Graph),
            ]
            .into_iter()
            .filter_map(|(on, f)| on.then_some(f))
            .collect();
            let mut written_umx = None;
            // Warnings from the write itself, added under the cap with the vector recheck's (plan 205).
            let mut late = Vec::new();
            let counts = op_counts(&rows);
            // ws003 s09: the receipt, in the order the batch defined its refs. A real run that got here bound every
            // ref it defined (a failed line fails the batch).
            let receipt_bytes = match keyed.as_mut() {
                None => None,
                Some(k) => {
                    let Some(ids) = refs.iter().map(|(_, id)| *id).collect::<Option<Vec<Id>>>()
                    else {
                        run.b.rollback();
                        return Err(alloc::vec![problems::internal("batch_receipt_refs")]);
                    };
                    let receipt = Receipt {
                        refs: ids,
                        counts: counts.clone(),
                    };
                    k.receipts
                        .record(k.digest.clone(), now_ms, k.input.clone(), receipt);
                    k.receipts.evict(now_ms);
                    Some(k.receipts.encode())
                }
            };
            let receipts_write = match (&receipt_bytes, &keyed) {
                (Some(bytes), Some(k)) => Some(ReceiptsWrite {
                    bytes,
                    read: k.read,
                }),
                _ => None,
            };
            if which.is_empty()
                && let Some(w) = &receipts_write
            {
                // graph-storage Q4: a keyed batch that changed nothing still records its receipt, alone.
                match persist_receipts(files.as_ref(), &alias, dir, stamps, w) {
                    Ok(warnings) => late.extend(warnings),
                    Err(p) => {
                        run.b.rollback();
                        return Err(alloc::vec![p]);
                    }
                }
            }
            if !which.is_empty() {
                // prepare places out-of-order edges and drops the deleted edges' rows, so the file is encoded
                // without them.
                if run.b.prepare().is_err() {
                    run.b.rollback();
                    return Err(alloc::vec![problems::internal("batch_edge_ends")]);
                }
                if !run.b.ready_to_persist() {
                    run.b.rollback();
                    return Err(alloc::vec![problems::internal("batch_unprepared")]);
                }
                let touched = run.b.touched();
                match persist_parts(
                    files.as_ref(),
                    &alias,
                    dir,
                    meta,
                    run.b.graph(),
                    super::Commit {
                        which: &which,
                        rewrite_edges: *edges_normalized,
                        feed,
                        touched,
                        stamps,
                        receipts: receipts_write,
                    },
                ) {
                    Ok(written) => {
                        written_umx = Some(written.umx);
                        late.extend(written.warnings);
                    }
                    Err(p) => {
                        let p = run.hide_drawn(p);
                        run.b.rollback();
                        return Err(alloc::vec![p]);
                    }
                }
            }
            let touched = core::mem::take(&mut run.touched);
            let sentences = req
                .echo_sentences
                .then(|| core::mem::take(&mut run.sentences));
            let sentences_omitted = run.sentences_omitted;
            let added_edges: Vec<Id> = run.added_edges.values().copied().collect();
            let external_old = core::mem::take(&mut run.external_old);
            let applied = run.b.commit();
            (
                (refs, counts),
                rows,
                sentences,
                sentences_omitted,
                warnings,
                warned,
                late,
                touched,
                (added_edges, external_old),
                written_umx,
                applied.provenance_changed,
            )
        };
        if let Some(umx) = written_umx {
            meta.umx = umx;
            *edges_normalized = false;
        }
        // After the commit only (F122): edited and deleted external edges' old entries out, then every added or
        // edited edge's entries in, so the index equals one rebuilt from the graphs.
        for old in &external_old {
            self.incoming.remove_edge(*instance, old);
        }
        for id in added_edges {
            self.incoming
                .add_edge(*instance, g.edge(id).expect("committed edge"));
        }
        late.extend(self.provenance_changed(&alias, &provenance_changed));
        let mut done = Done {
            value: BatchDone {
                alias,
                dry_run: false,
                graph_id,
                refs,
                rows,
                sentences,
                sentences_omitted,
                touched,
                warned,
                counts,
                keyed: keyed.map(|k| Keyed {
                    key: k.key,
                    replayed: false,
                    recorded_at: Some(now_ms),
                }),
            },
            notes: warnings,
        };
        add_late_warnings(&mut done, late);
        Ok(done)
    }
}
