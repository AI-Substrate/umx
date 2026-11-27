//! The graph view's agent channel (ws009 S6, plan 105): the presentation commands an agent sends to the open graph
//! views, the events the views read, their bounds, and the ring that keeps the last [`RING`] events. Pure: no I/O and
//! no clock; the caller passes the time (architect W7).
//!
//! A command changes only what the view shows. It never reads graph text, and an event carries ids, the graph alias,
//! the command and the agent's note, never titles or node text (security W2/W4).

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;

use crate::Id;
use crate::name::is_snake_case;
use crate::provenance::Actor;

/// `focus --hops`: 1 to this many hops around the node.
pub const MAX_HOPS: u32 = 3;
/// The most nodes one highlight or fit names.
pub const MAX_NODES: usize = 100;
/// The most names in each of a filter's show and hide lists.
pub const MAX_FILTER_NAMES: usize = 64;
/// The longest note, in characters.
pub const MAX_NOTE_CHARS: usize = 200;
/// How many events the ring keeps.
pub const RING: usize = 256;
/// The longest a view's wait may last, in ms.
pub const MAX_WAIT_MS: u64 = 25_000;
/// How many views may wait at once (architect W5); the next is refused `ui_waiters_busy`.
pub const MAX_WAITERS: usize = 4;
/// How many waits `umx ui`'s supervisor may hold at once (ws009 M1b2, architect): apart from [`MAX_WAITERS`].
pub const MAX_SUPERVISOR_WAITS: usize = 1;
/// A view that has not waited for this long, in ms, is gone: commands warn `ui_no_view`, and its selection clears.
pub const VIEW_GONE_MS: u64 = 30_000;
/// The most graphs one wait may name.
pub const MAX_WAIT_GRAPHS: usize = 64;
/// The most nodes a view may publish as its selection (S7).
pub const MAX_SELECTION: usize = 1_000;
/// The most graphs `umx ui open` names, and the most a view shows at once (ws009 M1b1, security S5).
pub const MAX_OPEN_GRAPHS: usize = 12;

/// What a command does. `N` is how a node is named: the text an agent gave (`String`), or the id it resolved to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiAction<N = Id> {
    /// Bring `node` into view and show `hops` hops around it. `centre` (PROTOCOL 110, PRD 005): also re-arrange the
    /// view in rings of hops around it (view only). `select` (PROTOCOL 111): also select it, opening its panel.
    Focus {
        node: N,
        hops: u32,
        centre: bool,
        select: bool,
    },
    /// Mark `nodes`: they replace the agent's highlight, or with `add` join it.
    Highlight { nodes: Vec<N>, add: bool },
    /// The agent's whole filter: `relation:`, `type:` and `status:` names to show and to hide. Each command carries
    /// both full lists, so a view can apply it, and undo it, from the event alone.
    Filter {
        show: Vec<String>,
        hide: Vec<String>,
    },
    /// Fit `nodes` in the view, or the whole graph when there are none.
    Fit { nodes: Vec<N> },
    /// Remove what agents set (highlight, filter); never what the person set.
    Clear,
    /// Open `graphs` in the view (ws009 M1b1): added to what is open, or with `only` in place of it (the view asks the
    /// person first). Aliases, 1 to [`MAX_OPEN_GRAPHS`], each once; the daemon keeps only those it accepted (S2).
    /// `left_out`: how many graphs the command named that the daemon refused, counted and never named (set by the
    /// daemon on the event, whatever the sender said). `raise` (ws009 M1b2): bring the view forward as well.
    Open {
        graphs: Vec<String>,
        only: bool,
        left_out: u8,
        raise: bool,
    },
}

/// A command and the agent's optional note, shown beside it as the agent's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiCmd<N = Id> {
    pub action: UiAction<N>,
    pub note: Option<String>,
}

impl<N> UiCmd<N> {
    /// The one check of a command's bounds and note (the daemon runs it on every command it is sent; the CLI only
    /// parses). The reason is a `usage_invalid` `what`.
    pub fn check(&self) -> Result<(), &'static str> {
        if let Some(note) = &self.note {
            check_note(note)?;
        }
        match &self.action {
            UiAction::Focus { hops, .. } if !(1..=MAX_HOPS).contains(hops) => {
                Err("--hops is a whole number from 1 to 3")
            }
            UiAction::Highlight { nodes, .. } if nodes.is_empty() => {
                Err("name one or more nodes to highlight")
            }
            UiAction::Highlight { nodes, .. } | UiAction::Fit { nodes }
                if nodes.len() > MAX_NODES =>
            {
                Err("name at most 100 nodes")
            }
            UiAction::Open { graphs, .. } => {
                if graphs.is_empty() || graphs.len() > MAX_OPEN_GRAPHS {
                    return Err("--graph names 1 to 12 graphs");
                }
                if !graphs.iter().all(|g| crate::ValidName::alias(g).is_some()) {
                    return Err("a graph name is snake_case");
                }
                if graphs
                    .iter()
                    .enumerate()
                    .any(|(i, g)| graphs[..i].contains(g))
                {
                    return Err("--graph names each graph once");
                }
                Ok(())
            }
            UiAction::Filter { show, hide } => {
                if show.len() > MAX_FILTER_NAMES || hide.len() > MAX_FILTER_NAMES {
                    return Err("--show and --hide take at most 64 names each");
                }
                if !show.iter().chain(hide).all(|n| filter_name(n)) {
                    return Err("a filter name is relation:<name>, type:<name> or status:<name>");
                }
                if show.iter().any(|n| hide.contains(n)) {
                    return Err("a name cannot be both shown and hidden");
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// The nodes the command names, in order.
    pub fn nodes(&self) -> Vec<&N> {
        match &self.action {
            UiAction::Focus { node, .. } => alloc::vec![node],
            UiAction::Highlight { nodes, .. } | UiAction::Fit { nodes } => nodes.iter().collect(),
            UiAction::Filter { .. } | UiAction::Clear | UiAction::Open { .. } => Vec::new(),
        }
    }

    /// The same command with each node renamed by `f`, in [`Self::nodes`] order; the first error stops it.
    pub fn map<M, E>(self, mut f: impl FnMut(N) -> Result<M, E>) -> Result<UiCmd<M>, E> {
        let action = match self.action {
            UiAction::Focus {
                node,
                hops,
                centre,
                select,
            } => UiAction::Focus {
                node: f(node)?,
                hops,
                centre,
                select,
            },
            UiAction::Highlight { nodes, add } => UiAction::Highlight {
                nodes: nodes.into_iter().map(&mut f).collect::<Result<_, _>>()?,
                add,
            },
            UiAction::Filter { show, hide } => UiAction::Filter { show, hide },
            UiAction::Fit { nodes } => UiAction::Fit {
                nodes: nodes.into_iter().map(&mut f).collect::<Result<_, _>>()?,
            },
            UiAction::Clear => UiAction::Clear,
            UiAction::Open {
                graphs,
                only,
                left_out,
                raise,
            } => UiAction::Open {
                graphs,
                only,
                left_out,
                raise,
            },
        };
        Ok(UiCmd {
            action,
            note: self.note,
        })
    }
}

/// A note is at most [`MAX_NOTE_CHARS`] characters with no control characters, no bidi or format controls
/// (U+200E/F, U+202A-202E, U+2066-2069), no line or paragraph separators (U+2028/9) and no zero-width characters
/// (U+200B-200D, U+2060, U+FEFF), so it cannot move the terminal, hide text, or reorder or fake the view's "from the
/// agent" label (security T2, N1).
pub fn check_note(note: &str) -> Result<(), &'static str> {
    if note.chars().count() > MAX_NOTE_CHARS {
        return Err("a note is at most 200 characters");
    }
    let refused = |c: char| {
        c.is_control()
            || matches!(
                c,
                '\u{200B}'..='\u{200F}'
                    | '\u{2028}'..='\u{202E}'
                    | '\u{2060}'
                    | '\u{2066}'..='\u{2069}'
                    | '\u{FEFF}'
            )
    };
    if note.chars().any(refused) {
        return Err("a note cannot hold control, invisible or text-direction characters");
    }
    Ok(())
}

/// `relation:<name>`, `type:<name>` or `status:<name>`, the name snake_case.
pub fn filter_name(text: &str) -> bool {
    text.split_once(':').is_some_and(|(kind, name)| {
        matches!(kind, "relation" | "type" | "status") && is_snake_case(name)
    })
}

/// One command as the views read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiEvent {
    /// 1 for the first event since the daemon started, one more for each.
    pub seq: u64,
    pub at_ms: u64,
    /// Who sent it, as recorded (a claim, never verified): the client's resolved actor for UiCommand; absent for a
    /// view's own events.
    pub by: Option<Actor>,
    /// The graph's alias.
    pub graph: String,
    pub command: UiCmd,
}

/// The last [`RING`] events, oldest first, in daemon memory only.
#[derive(Clone, Debug, Default)]
pub struct UiRing {
    events: VecDeque<UiEvent>,
    seq: u64,
}

impl UiRing {
    /// The seq of the latest event; 0 before the first.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Keeps a new event, dropping the oldest past [`RING`]; returns its seq.
    pub fn push(&mut self, at_ms: u64, by: Option<Actor>, graph: String, command: UiCmd) -> u64 {
        self.seq += 1;
        if self.events.len() == RING {
            self.events.pop_front();
        }
        self.events.push_back(UiEvent {
            seq: self.seq,
            at_ms,
            by,
            graph,
            command,
        });
        self.seq
    }

    /// Whether a reader at `since` has missed nothing: every event after it is still kept, and it is not ahead of
    /// the ring. Otherwise the reader must reset.
    pub fn continues(&self, since: u64) -> bool {
        let oldest = self.events.front().map_or(self.seq + 1, |e| e.seq);
        since <= self.seq && since + 1 >= oldest
    }

    /// The kept events after `since` for the graphs in `graphs`; events for other graphs are skipped, except an open
    /// (ws009 M1b1), which every view gets: it names graphs the view may not have open yet. Its graphs are only those
    /// the daemon accepted, never a secured one (security S2).
    pub fn after<'a>(
        &'a self,
        since: u64,
        graphs: &'a [String],
    ) -> impl Iterator<Item = &'a UiEvent> {
        self.events.iter().filter(move |e| {
            e.seq > since
                && (graphs.contains(&e.graph) || matches!(e.command.action, UiAction::Open { .. }))
        })
    }
}

/// The most ids a change lists in each of its six lists (nodes and edges added, edited, removed); past it the view
/// is told to reload the graph instead (overflow).
pub const MAX_CHANGE_IDS: usize = 500;
/// How many changes the change log keeps per graph (architect L5).
pub const CHANGE_LOG: usize = 64;

/// The ids of one kind (nodes or edges) a write added, edited or removed. Ids only: never a title, a note, text or
/// who wrote it (security T2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TouchedIds {
    pub added: BTreeSet<Id>,
    pub edited: BTreeSet<Id>,
    pub removed: BTreeSet<Id>,
}

impl TouchedIds {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.edited.is_empty() && self.removed.is_empty()
    }

    fn over(&self) -> bool {
        [&self.added, &self.edited, &self.removed]
            .iter()
            .any(|s| s.len() > MAX_CHANGE_IDS)
    }

    /// This change followed by `later`, as one: an id added and then edited is added; added and then removed is
    /// gone from both (the reader never had it); removed and then added again is edited.
    pub fn then(&mut self, later: &TouchedIds) {
        for id in &later.added {
            if self.removed.remove(id) {
                self.edited.insert(*id);
            } else {
                self.added.insert(*id);
            }
        }
        for id in &later.edited {
            if !self.added.contains(id) {
                self.edited.insert(*id);
            }
        }
        for id in &later.removed {
            if !self.added.remove(id) {
                self.edited.remove(id);
                self.removed.insert(*id);
            }
        }
    }
}

/// What one committed write changed in one graph: the ids only (ws009 S5a).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Touched {
    pub nodes: TouchedIds,
    pub edges: TouchedIds,
}

impl Touched {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.edges.is_empty()
    }

    /// Whether a list holds more than [`MAX_CHANGE_IDS`] ids.
    pub fn over(&self) -> bool {
        self.nodes.over() || self.edges.over()
    }

    pub fn then(&mut self, later: &Touched) {
        self.nodes.then(&later.nodes);
        self.edges.then(&later.edges);
    }

    pub fn node_added(id: Id) -> Touched {
        let mut t = Touched::default();
        t.nodes.added.insert(id);
        t
    }

    pub fn node_edited(id: Id) -> Touched {
        let mut t = Touched::default();
        t.nodes.edited.insert(id);
        t
    }
}

/// What the graph store hands the change log, in order: a commit (the graph's revision after it, counted from 0 at
/// its load, and what it touched), or a load, reload or unload of the graph (a reset).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fed {
    Wrote {
        graph: String,
        rev: u64,
        touched: Touched,
    },
    Reset {
        graph: String,
    },
}

/// What a view gets for one graph it named: what changed since its revision, merged into one, or overflow (reload
/// the graph and wait from `rev`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    Changed {
        graph: String,
        from_rev: u64,
        to_rev: u64,
        touched: Touched,
    },
    Overflow {
        graph: String,
        rev: u64,
    },
}

#[derive(Clone, Debug, Default)]
struct GraphLog {
    /// The revision views see: one more per commit and one more per load, reload or unload, so it never repeats in
    /// one daemon epoch even though the store counts from 0 at each load.
    rev: u64,
    /// The store's revision of the last commit seen; a commit that does not follow it means some went missing.
    stored: u64,
    /// The last [`CHANGE_LOG`] steps, oldest first, contiguous and ending at `rev`: (from, to, what changed, or None
    /// when it touched more than the cap).
    steps: VecDeque<(u64, u64, Option<Touched>)>,
}

impl GraphLog {
    fn reset(&mut self) {
        self.rev += 1;
        self.stored = 0;
        self.steps.clear();
    }

    /// The oldest revision a view can catch up from.
    fn floor(&self) -> u64 {
        self.steps.front().map_or(self.rev, |s| s.0)
    }
}

/// The views' change log (architect L5, security T1-T3): the last [`CHANGE_LOG`] changes per graph, apart from the
/// command ring, so no burst of writes evicts an agent's command. Revisions are per graph, so a view of one graph
/// learns nothing of writes to another. Pure: daemon memory only.
#[derive(Clone, Debug, Default)]
pub struct ChangeLog {
    graphs: BTreeMap<String, GraphLog>,
}

impl ChangeLog {
    pub fn record(&mut self, fed: Fed) {
        match fed {
            Fed::Reset { graph } => self.graphs.entry(graph).or_default().reset(),
            Fed::Wrote {
                graph,
                rev,
                touched,
            } => {
                let log = self.graphs.entry(graph).or_default();
                if rev != log.stored + 1 {
                    log.reset();
                }
                log.stored = rev;
                let from = log.rev;
                log.rev += 1;
                if log.steps.len() == CHANGE_LOG {
                    log.steps.pop_front();
                }
                let kept = (!touched.over()).then_some(touched);
                log.steps.push_back((from, log.rev, kept));
            }
        }
    }

    /// The graph's revision now (0 before anything was recorded for it).
    pub fn rev(&self, graph: &str) -> u64 {
        self.graphs.get(graph).map_or(0, |l| l.rev)
    }

    /// What a view at `rev` should apply for `graph`: nothing when it is current; the changes since, merged; else
    /// overflow, when it is older than the log or ahead of it, a step was over the cap, or the merge is. A loaded
    /// graph is never at 0 (its load is a step), so a view with nothing yet asks from 0 and is told to load it.
    pub fn since(&self, graph: &str, rev: u64) -> Option<Change> {
        let empty = GraphLog::default();
        let log = self.graphs.get(graph).unwrap_or(&empty);
        let overflow = || {
            Some(Change::Overflow {
                graph: graph.into(),
                rev: log.rev,
            })
        };
        if rev == log.rev {
            return None;
        }
        if rev < log.floor() || rev > log.rev {
            return overflow();
        }
        let mut merged = Touched::default();
        for (from, _, touched) in log.steps.iter().filter(|s| s.0 >= rev) {
            let Some(touched) = touched else {
                return overflow();
            };
            debug_assert!(*from >= rev);
            merged.then(touched);
        }
        if merged.over() {
            return overflow();
        }
        Some(Change::Changed {
            graph: graph.into(),
            from_rev: rev,
            to_rev: log.rev,
            touched: merged,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn cmd(action: UiAction<String>) -> UiCmd<String> {
        UiCmd { action, note: None }
    }

    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| alloc::format!("n{i}")).collect()
    }

    #[test]
    fn an_open_names_1_to_12_graphs_each_once_by_alias() {
        let open = |graphs: Vec<String>| {
            cmd(UiAction::Open {
                graphs,
                only: false,
                left_out: 0,
                raise: false,
            })
        };
        let aliases = |n: usize| (0..n).map(|i| alloc::format!("g{i}")).collect::<Vec<_>>();
        assert!(open(aliases(1)).check().is_ok() && open(aliases(12)).check().is_ok());
        assert_eq!(
            open(aliases(13)).check(),
            Err("--graph names 1 to 12 graphs")
        );
        assert_eq!(
            open(Vec::new()).check(),
            Err("--graph names 1 to 12 graphs")
        );
        assert_eq!(
            open(vec!["alpha".into(), "alpha".into()]).check(),
            Err("--graph names each graph once")
        );
        assert_eq!(
            open(vec!["--all".into()]).check(),
            Err("a graph name is snake_case")
        );
        assert!(open(aliases(2)).nodes().is_empty());
    }

    #[test]
    fn every_view_gets_an_open_whatever_graphs_it_names() {
        let mut ring = UiRing::default();
        let open = UiCmd {
            action: UiAction::Open {
                graphs: vec!["beta".into()],
                only: false,
                left_out: 0,
                raise: false,
            },
            note: None,
        };
        let fit = UiCmd {
            action: UiAction::Fit { nodes: Vec::new() },
            note: None,
        };
        ring.push(1, None, "beta".into(), open);
        ring.push(2, None, "beta".into(), fit);
        let alpha = ["alpha".to_string()];
        let seen: Vec<u64> = ring.after(0, &alpha).map(|e| e.seq).collect();
        assert_eq!(seen, [1]);
    }

    #[test]
    fn every_bound_is_checked_at_its_edge() {
        let focus = |hops| {
            cmd(UiAction::Focus {
                node: "a".into(),
                hops,
                centre: false,
                select: false,
            })
        };
        assert!(focus(1).check().is_ok() && focus(3).check().is_ok());
        assert!(focus(0).check().is_err() && focus(4).check().is_err());
        let highlight = |n| {
            cmd(UiAction::Highlight {
                nodes: names(n),
                add: false,
            })
        };
        assert!(highlight(100).check().is_ok());
        assert_eq!(highlight(101).check(), Err("name at most 100 nodes"));
        assert!(highlight(0).check().is_err());
        let fit = |n| cmd(UiAction::Fit { nodes: names(n) });
        assert!(fit(0).check().is_ok() && fit(100).check().is_ok());
        assert!(fit(101).check().is_err());
        let filter = |show: usize, hide: usize| {
            let list = |n: usize, k: &str| (0..n).map(|i| alloc::format!("type:{k}{i}")).collect();
            cmd(UiAction::Filter {
                show: list(show, "s"),
                hide: list(hide, "h"),
            })
        };
        assert!(filter(64, 64).check().is_ok());
        assert!(filter(65, 0).check().is_err() && filter(0, 65).check().is_err());
        assert!(cmd(UiAction::Clear).check().is_ok());
    }

    #[test]
    fn filter_names_have_a_known_kind_and_a_snake_case_name() {
        for good in ["relation:blocks", "type:problem", "status:in_contention"] {
            assert!(filter_name(good), "{good}");
        }
        for bad in [
            "blocks", "kind:x", "type:", "type:Bad", "type:a b", ":x", "type:x:y",
        ] {
            assert!(!filter_name(bad), "{bad}");
        }
        let both = cmd(UiAction::Filter {
            show: vec!["type:a".into()],
            hide: vec!["type:a".into()],
        });
        assert_eq!(both.check(), Err("a name cannot be both shown and hidden"));
    }

    #[test]
    fn a_note_is_short_plain_text_with_no_direction_marks() {
        assert!(check_note(&"é".repeat(200)).is_ok());
        assert!(check_note(&"a".repeat(201)).is_err());
        for bad in [
            "a\nb",
            "a\u{1b}[31m",
            "\u{7f}",
            "\u{200E}",
            "\u{200F}",
            "\u{202A}",
            "\u{202E}",
            "\u{2066}",
            "\u{2069}",
            "a\u{2028}b",
            "a\u{2029}b",
            "\u{200B}",
            "\u{200C}",
            "\u{200D}",
            "\u{2060}",
            "\u{FEFF}",
        ] {
            assert!(check_note(bad).is_err(), "{bad:?}");
        }
        assert!(check_note("ok: look at these (3)").is_ok());
        assert!(
            check_note("caf\u{e9} \u{2014} \u{201C}quoted\u{201D} \u{2026}").is_ok(),
            "ordinary punctuation passes"
        );
        let noted = UiCmd {
            action: UiAction::<String>::Clear,
            note: Some("x\u{202E}y".into()),
        };
        assert!(noted.check().is_err());
    }

    #[test]
    fn map_renames_every_node_in_order_and_keeps_the_rest() {
        let c = UiCmd {
            action: UiAction::Highlight {
                nodes: vec!["1".to_string(), "2".to_string()],
                add: true,
            },
            note: Some("n".into()),
        };
        let mapped: UiCmd<u32> = c.map(|n| n.parse::<u32>().map_err(|_| ())).unwrap();
        assert_eq!(mapped.nodes(), [&1, &2]);
        assert_eq!(mapped.note.as_deref(), Some("n"));
        let bad = cmd(UiAction::Fit {
            nodes: vec!["1".into(), "x".into()],
        });
        assert!(bad.map(|n| n.parse::<u32>().map_err(|_| ())).is_err());
    }

    #[test]
    fn the_ring_keeps_the_last_256_and_says_when_a_reader_missed_some() {
        let mut ring = UiRing::default();
        assert!(ring.continues(0));
        assert!(!ring.continues(1), "a reader ahead of the ring resets");
        let clear = || UiCmd {
            action: UiAction::Clear,
            note: None,
        };
        for i in 0..300u64 {
            let graph = if i % 2 == 0 { "a" } else { "b" };
            ring.push(i, None, graph.into(), clear());
        }
        assert_eq!(ring.seq(), 300);
        assert_eq!(ring.events.len(), RING);
        assert_eq!(ring.events.front().map(|e| e.seq), Some(45));
        assert!(ring.continues(44) && ring.continues(300));
        assert!(!ring.continues(43), "event 44 is gone");
        assert!(!ring.continues(301));
        let a = ["a".to_string()];
        let seqs: Vec<u64> = ring.after(295, &a).map(|e| e.seq).collect();
        assert_eq!(seqs, [297, 299], "events for graph b are skipped");
        assert_eq!(ring.after(0, &[]).count(), 0);
    }

    fn id(n: u128) -> Id {
        Id(n)
    }

    fn wrote(graph: &str, rev: u64, touched: Touched) -> Fed {
        Fed::Wrote {
            graph: graph.into(),
            rev,
            touched,
        }
    }

    fn changed(c: Option<Change>) -> (u64, u64, Touched) {
        match c {
            Some(Change::Changed {
                from_rev,
                to_rev,
                touched,
                ..
            }) => (from_rev, to_rev, touched),
            other => panic!("not a change: {other:?}"),
        }
    }

    fn overflow(c: Option<Change>) -> u64 {
        match c {
            Some(Change::Overflow { rev, .. }) => rev,
            other => panic!("not an overflow: {other:?}"),
        }
    }

    #[test]
    fn changes_since_a_revision_merge_into_one_per_graph() {
        let mut log = ChangeLog::default();
        assert_eq!(log.since("g", 0), None, "nothing recorded: nothing to do");
        log.record(Fed::Reset { graph: "g".into() });
        assert_eq!(
            overflow(log.since("g", 0)),
            1,
            "a view with nothing loads the graph"
        );
        log.record(wrote("g", 1, Touched::node_added(id(1))));
        log.record(wrote("g", 2, Touched::node_edited(id(1))));
        log.record(wrote("g", 3, Touched::node_edited(id(2))));
        let mut gone = Touched::default();
        gone.nodes.removed.insert(id(3));
        gone.edges.removed.insert(id(9));
        log.record(wrote("g", 4, gone));
        let (from, to, t) = changed(log.since("g", 1));
        assert_eq!((from, to), (1, 5));
        assert_eq!(
            t.nodes.added,
            BTreeSet::from([id(1)]),
            "added then edited is added"
        );
        assert_eq!(t.nodes.edited, BTreeSet::from([id(2)]));
        assert_eq!(t.nodes.removed, BTreeSet::from([id(3)]));
        assert_eq!(t.edges.removed, BTreeSet::from([id(9)]));
        let (from, _, t) = changed(log.since("g", 3));
        assert_eq!(from, 3);
        assert!(t.nodes.added.is_empty() && t.nodes.edited == BTreeSet::from([id(2)]));
        assert_eq!(log.since("g", 5), None);
        assert_eq!(overflow(log.since("g", 6)), 5, "ahead of the log");
        assert_eq!(log.rev("other"), 0, "revisions are per graph");
        assert_eq!(log.since("other", 0), None);
    }

    #[test]
    fn added_then_removed_is_nothing_and_removed_then_added_is_edited() {
        let mut t = Touched::node_added(id(1));
        let mut gone = Touched::default();
        gone.nodes.removed.insert(id(1));
        gone.nodes.removed.insert(id(2));
        t.then(&gone);
        assert!(t.nodes.added.is_empty() && t.nodes.removed == BTreeSet::from([id(2)]));
        t.then(&Touched::node_added(id(2)));
        assert!(t.nodes.removed.is_empty() && t.nodes.edited == BTreeSet::from([id(2)]));
    }

    #[test]
    fn the_log_keeps_64_steps_and_older_readers_reload() {
        let mut log = ChangeLog::default();
        for rev in 1..=70 {
            log.record(wrote("g", rev, Touched::node_edited(id(rev as u128))));
        }
        assert_eq!(log.rev("g"), 70);
        assert_eq!(overflow(log.since("g", 5)), 70, "steps 0..6 are gone");
        let (from, _, t) = changed(log.since("g", 6));
        assert_eq!((from, t.nodes.edited.len()), (6, 64));
    }

    #[test]
    fn more_than_500_ids_of_a_kind_is_an_overflow_one_step_or_merged() {
        let many = |from: u128, n: u128| {
            let mut t = Touched::default();
            t.edges.added = (from..from + n).map(id).collect();
            t
        };
        let mut log = ChangeLog::default();
        log.record(wrote("g", 1, many(0, 500)));
        assert_eq!(changed(log.since("g", 0)).2.edges.added.len(), 500);
        log.record(wrote("g", 2, many(500, 1)));
        assert_eq!(overflow(log.since("g", 0)), 2, "501 merged");
        assert_eq!(changed(log.since("g", 1)).2.edges.added.len(), 1);
        log.record(wrote("g", 3, many(1000, 501)));
        assert_eq!(overflow(log.since("g", 2)), 3, "501 in one step");
    }

    #[test]
    fn a_load_reload_or_unload_and_a_missed_commit_are_overflow() {
        let mut log = ChangeLog::default();
        log.record(wrote("g", 1, Touched::node_added(id(1))));
        log.record(Fed::Reset { graph: "g".into() });
        assert_eq!(log.rev("g"), 2, "a reload is a step of its own");
        assert_eq!(overflow(log.since("g", 0)), 2);
        assert_eq!(overflow(log.since("g", 1)), 2);
        assert_eq!(log.since("g", 2), None);
        // The store counts from 0 again after the load: its first commit is 1.
        log.record(wrote("g", 1, Touched::node_added(id(2))));
        assert_eq!(changed(log.since("g", 2)).1, 3);
        // A commit the log never got (store revision 2) leaves a gap: everyone before it reloads.
        log.record(wrote("g", 3, Touched::node_added(id(3))));
        assert_eq!(log.rev("g"), 5);
        assert_eq!(overflow(log.since("g", 3)), 5);
        assert_eq!(changed(log.since("g", 4)).0, 4);
    }
}
