//! The graph view's agent channel (ws009 S6 and S7, architect W7): the ring of presentation commands and the person's
//! selection, in daemon memory only, never in a graph or repo file. The daemon keeps one [`UiChannel`] behind a lock
//! of its own, beside the services and never inside them, and does only the waiting; the checks, the scope and the
//! node resolution are here.

use crate::graph_store::SelectedRow;
use crate::{Ctx, Done, GraphStore, Outcome, problems};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use umx_core::ports::Clock;
use umx_core::provenance::Actor;
use umx_core::ui::{
    Change, ChangeLog, Fed, MAX_SELECTION, MAX_SUPERVISOR_WAITS, MAX_WAIT_GRAPHS, MAX_WAIT_MS,
    MAX_WAITERS, UiAction, UiCmd, UiEvent, UiRing, VIEW_GONE_MS,
};
use umx_core::{Command, Id, Problem, ValidName};

pub struct UiChannel {
    /// Drawn once when the daemon starts: a view that saw another epoch resets.
    epoch: u64,
    clock: Arc<dyn Clock>,
    ring: UiRing,
    /// What committed writes changed, per graph (ws009 S5a): apart from the ring, so writes never evict a command.
    changes: ChangeLog,
    /// What the person selected, per graph alias (S7).
    selection: BTreeMap<String, Vec<Id>>,
    waiters: usize,
    /// When a wait last began or ended.
    seen_ms: Option<u64>,
    /// `umx ui`'s supervisor's waits (ws009 M1b2): at most [`MAX_SUPERVISOR_WAITS`], apart from the views' and never
    /// an open view; and when one last began or ended.
    supervisors: usize,
    supervisor_seen_ms: Option<u64>,
    /// Set before the daemon stops or restarts: every wait answers at once, empty.
    closing: bool,
}

/// One view's wait, from [`UiChannel::begin_wait`] to [`UiChannel::end_wait`].
#[derive(Clone, Debug)]
pub struct UiWaiter {
    graphs: Vec<String>,
    since: u64,
    reset: bool,
    /// The revision the view last applied per graph it named: None for a graph it gave none for (it does not follow
    /// that graph's changes), and 0 for every one it gave under another epoch (they were another daemon's).
    revs: Vec<Option<u64>>,
    supervisor: bool,
}

/// What a wait answers: the events after the view's `since` for its graphs, or with `reset` every kept event for
/// them (the view rebuilds what agents set from these alone).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiWaited {
    pub epoch: String,
    /// The latest seq, events for other graphs included: the view's next `since`.
    pub seq: u64,
    pub reset: bool,
    pub events: Vec<UiEvent>,
    /// What changed in each graph the view named since the revision it gave, one entry per graph (ws009 S5a).
    pub changes: Vec<Change>,
}

/// What `umx ui open` did (ws009 M1b1): the event it kept, naming only the graphs it accepted, and each graph it
/// refused with why (`secured`, `unknown` or `unloaded`). A refused graph never enters the ring (security S2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiOpened {
    pub event: UiEvent,
    pub refused: Vec<(String, &'static str)>,
}

/// What a view published as the selection in one graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiSelected {
    pub graph: String,
    pub nodes: usize,
}

impl UiChannel {
    /// A channel with no events; `epoch` comes from the id generator (architect W2: random, never the clock).
    pub(crate) fn new(epoch: u64, clock: Arc<dyn Clock>) -> UiChannel {
        UiChannel {
            epoch,
            clock,
            ring: UiRing::default(),
            changes: ChangeLog::default(),
            selection: BTreeMap::new(),
            waiters: 0,
            seen_ms: None,
            supervisors: 0,
            supervisor_seen_ms: None,
            closing: false,
        }
    }

    /// The epoch as the wire carries it: 16 hex digits (a JSON number would lose bits in a browser).
    pub fn epoch(&self) -> String {
        alloc::format!("{:016x}", self.epoch)
    }

    fn now(&self) -> u64 {
        self.clock.now_ms()
    }

    /// Whether a view is waiting now or waited within [`VIEW_GONE_MS`].
    pub fn view_open(&self) -> bool {
        self.waiters > 0
            || self
                .seen_ms
                .is_some_and(|s| self.now().saturating_sub(s) < VIEW_GONE_MS)
    }

    /// Whether `umx ui`'s supervisor waits now or waited within [`VIEW_GONE_MS`] (ws009 M1b2): an open with
    /// `--raise` then opens a tab.
    pub fn supervisor_live(&self) -> bool {
        self.supervisors > 0
            || self
                .supervisor_seen_ms
                .is_some_and(|s| self.now().saturating_sub(s) < VIEW_GONE_MS)
    }

    /// `ui_no_view`, with what brings a view up for `graphs`: `umx ui open --graph .. --raise` while the supervisor
    /// runs, else `umx ui --detach --graph ..` (ws009 M1b2).
    fn no_view(&self, graphs: &[String]) -> Problem {
        let names: Vec<ValidName> = graphs.iter().filter_map(|g| ValidName::alias(g)).collect();
        let fix = if self.supervisor_live() {
            Command::new("ui open")
                .word("--graph")
                .names(&names, ",")
                .word("--raise")
        } else {
            Command::new("ui")
                .word("--detach")
                .word("--graph")
                .names(&names, ",")
        };
        problems::ui_no_view(fix)
    }

    /// The selection lasts only while a view is open (security T4c).
    fn expire(&mut self) {
        if !self.view_open() {
            self.selection.clear();
        }
    }

    /// `umx ui focus|highlight|filter|fit|clear`: checks the command (umx-core's one check), picks the graph and
    /// resolves its nodes as every read does, and keeps it as the next event. Warns `ui_no_view` when no view is open;
    /// the event is kept for one that opens.
    pub fn command(
        &mut self,
        store: &GraphStore,
        ctx: &Ctx,
        command: UiCmd<String>,
        by: Option<Actor>,
    ) -> Outcome<UiEvent> {
        command
            .check()
            .map_err(|what| problems::usage_invalid(what, "ui"))?;
        // An open names graphs, not nodes: its own checks, whoever sent it (security S1).
        if matches!(command.action, UiAction::Open { .. }) {
            // Any route an open takes ends here; one that reaches the generic command path reports its first refusal.
            let done = self
                .open(store, command, by)
                .map_err(|mut all| all.remove(0))?;
            return Ok(Done {
                value: done.value.event,
                notes: done.notes,
            });
        }
        let refs: Vec<&str> = command.nodes().into_iter().map(String::as_str).collect();
        let (graph, ids) = store.ui_target(ctx, &refs)?;
        let mut ids = ids.into_iter();
        let command = command
            .map(|_| ids.next().ok_or(()))
            .map_err(|()| problems::internal("ui_nodes"))?;
        self.expire();
        let at_ms = self.now();
        let seq = self
            .ring
            .push(at_ms, by.clone(), graph.clone(), command.clone());
        let warnings = if self.view_open() {
            Vec::new()
        } else {
            alloc::vec![self.no_view(core::slice::from_ref(&graph))]
        };
        Ok(Done {
            value: UiEvent {
                seq,
                at_ms,
                by,
                graph,
                command,
            },
            notes: warnings,
        })
    }

    /// `umx ui open --graph a,b [--only]` (ws009 M1b1): checks the command, then each graph as the view may open it
    /// (`GraphStore::ui_open_refusal`). A secured graph is always refused (S1), however the person's view is set; so
    /// are unknown graphs and ones unloaded on purpose, each with the command that would open or load it. The event
    /// names only the accepted graphs, under the first of them (S2), and counts the refused ones (`left_out`, never
    /// named); none accepted refuses the whole command with every graph's refusal in order (architect) and
    /// keeps nothing. Warns `ui_no_view` when no view is open, as every command does.
    pub fn open(
        &mut self,
        store: &GraphStore,
        command: UiCmd<String>,
        by: Option<Actor>,
    ) -> Result<Done<UiOpened>, Vec<Problem>> {
        command
            .check()
            .map_err(|what| Vec::from([problems::usage_invalid(what, "ui")]))?;
        let UiAction::Open {
            graphs,
            only,
            raise,
            ..
        } = command.action
        else {
            return Err(Vec::from([problems::internal("ui_open")]));
        };
        let mut accepted = Vec::new();
        let mut refused = Vec::new();
        for alias in graphs {
            match store.ui_open_refusal(&alias) {
                None => accepted.push(alias),
                Some(why) => refused.push((alias, why)),
            }
        }
        let left_out = !accepted.is_empty();
        let mut warnings: Vec<Problem> = refused
            .iter()
            .map(|(alias, (_, why, fix))| {
                problems::ui_open_refused(alias, why, fix.clone(), left_out)
            })
            .collect();
        let Some(first) = accepted.first().cloned() else {
            return Err(warnings);
        };
        let command = UiCmd {
            action: UiAction::Open {
                graphs: accepted.clone(),
                only,
                left_out: refused.len() as u8,
                raise,
            },
            note: command.note,
        };
        self.expire();
        let at_ms = self.now();
        let seq = self
            .ring
            .push(at_ms, by.clone(), first.clone(), command.clone());
        // A raise with no view but the supervisor up opens a tab there: nothing to warn.
        if !self.view_open() && !(raise && self.supervisor_live()) {
            warnings.push(self.no_view(&accepted));
        }
        Ok(Done {
            value: UiOpened {
                event: UiEvent {
                    seq,
                    at_ms,
                    by,
                    graph: first,
                    command,
                },
                refused: refused
                    .into_iter()
                    .map(|(a, (kind, _, _))| (a, kind))
                    .collect(),
            },
            notes: warnings,
        })
    }

    /// A view starts waiting for the events after `since` for `graphs`. Refused past [`MAX_WAITERS`] waits at once
    /// (`ui_waiters_busy`); a named secured graph is included. `supervisor`: `umx ui`'s supervisor's wait, counted
    /// apart (at most [`MAX_SUPERVISOR_WAITS`]) and never an open view (ws009 M1b2). A graph that is not loaded is waited on too, never
    /// refused: one the view follows answers overflow once (its log's revision, or 0 with no log this epoch). Every
    /// `begin_wait` that succeeds needs one [`Self::end_wait`].
    #[allow(clippy::too_many_arguments)]
    pub fn begin_wait(
        &mut self,
        store: &GraphStore,
        epoch: Option<&str>,
        since: u64,
        timeout_ms: u64,
        graphs: &[String],
        revs: &BTreeMap<String, u64>,
        supervisor: bool,
    ) -> Result<UiWaiter, Problem> {
        if timeout_ms > MAX_WAIT_MS {
            return Err(problems::usage_invalid(
                "a wait lasts at most 25000 ms",
                "ui",
            ));
        }
        if graphs.len() > MAX_WAIT_GRAPHS || revs.len() > MAX_WAIT_GRAPHS {
            return Err(problems::usage_invalid(
                "a wait names at most 64 graphs",
                "ui",
            ));
        }
        let graphs = store.ui_graphs(graphs)?;
        if supervisor {
            if self.supervisors >= MAX_SUPERVISOR_WAITS {
                return Err(problems::ui_waiters_busy());
            }
            self.supervisors += 1;
            self.supervisor_seen_ms = Some(self.now());
        } else {
            if self.waiters >= MAX_WAITERS {
                return Err(problems::ui_waiters_busy());
            }
            // A view back after a gap must not revive a selection that expired meanwhile (R203-1): expire, then beat.
            self.expire();
            self.waiters += 1;
            self.seen_ms = Some(self.now());
        }
        let same_epoch = epoch == Some(self.epoch().as_str());
        let reset = !same_epoch || !self.ring.continues(since);
        // A revision for a graph the view did not name is ignored: changes come only for named graphs (T3).
        let revs = graphs
            .iter()
            .map(|g| revs.get(g).map(|r| if same_epoch { *r } else { 0 }))
            .collect();
        Ok(UiWaiter {
            graphs,
            since,
            reset,
            revs,
            supervisor,
        })
    }

    /// The answer for `waiter` if there is one now: the daemon is closing (an empty reply, before anything else,
    /// R203-2), a reset, or events or changes for its graphs. None: keep waiting.
    pub fn poll(&self, waiter: &UiWaiter) -> Option<UiWaited> {
        if self.closing {
            return Some(UiWaited {
                changes: Vec::new(),
                ..self.timed_out(waiter)
            });
        }
        if waiter.reset || !self.ring.continues(waiter.since) {
            return Some(self.answer(0, waiter, true));
        }
        let answer = self.answer(waiter.since, waiter, false);
        (!answer.events.is_empty() || !answer.changes.is_empty()).then_some(answer)
    }

    /// The answer when the wait timed out: no events, the latest seq, and any changes.
    pub fn timed_out(&self, waiter: &UiWaiter) -> UiWaited {
        UiWaited {
            events: Vec::new(),
            ..self.answer(self.ring.seq(), waiter, false)
        }
    }

    fn answer(&self, since: u64, waiter: &UiWaiter, reset: bool) -> UiWaited {
        UiWaited {
            epoch: self.epoch(),
            seq: self.ring.seq(),
            reset,
            events: self.ring.after(since, &waiter.graphs).cloned().collect(),
            changes: waiter
                .graphs
                .iter()
                .zip(&waiter.revs)
                .filter_map(|(g, rev)| self.changes.since(g, (*rev)?))
                .collect(),
        }
    }

    /// What the graph store's commits, loads and unloads changed (ws009 S5a), in order; true when there was any, so
    /// the daemon wakes the waits.
    pub fn record(&mut self, fed: Vec<Fed>) -> bool {
        let any = !fed.is_empty();
        for f in fed {
            self.changes.record(f);
        }
        any
    }

    pub fn end_wait(&mut self, waiter: &UiWaiter) {
        if waiter.supervisor {
            self.supervisors = self.supervisors.saturating_sub(1);
            self.supervisor_seen_ms = Some(self.now());
        } else {
            self.waiters = self.waiters.saturating_sub(1);
            self.seen_ms = Some(self.now());
        }
    }

    pub fn waiters(&self) -> usize {
        self.waiters
    }

    /// Before the daemon stops or restarts (F64): every wait answers at once, empty, and new ones do too.
    pub fn close(&mut self) {
        self.closing = true;
    }

    pub fn closing(&self) -> bool {
        self.closing
    }

    /// The view publishes what the person selected in the graph it names (S7): replaces that graph's selection; no
    /// nodes clears it. The graph must be named (`--graph` or an address), as the view always does.
    pub fn select(
        &mut self,
        store: &GraphStore,
        ctx: &Ctx,
        nodes: &[String],
    ) -> Outcome<UiSelected> {
        let refs: Vec<&str> = nodes.iter().map(String::as_str).collect();
        let opt: Vec<Option<&str>> = refs.iter().map(|r| Some(*r)).collect();
        if !store.names_graph(ctx, &opt)? {
            return Err(problems::usage_invalid(
                "the view names the graph of a selection with --graph",
                "ui",
            ));
        }
        if nodes.len() > MAX_SELECTION {
            return Err(problems::usage_invalid(
                "a selection is at most 1000 nodes",
                "ui",
            ));
        }
        let (graph, ids) = store.ui_target(ctx, &refs)?;
        let mut unique: Vec<Id> = Vec::with_capacity(ids.len());
        for id in ids {
            if !unique.contains(&id) {
                unique.push(id);
            }
        }
        let count = unique.len();
        self.expire();
        if unique.is_empty() {
            self.selection.remove(&graph);
        } else {
            self.selection.insert(graph.clone(), unique);
        }
        Ok(Done {
            value: UiSelected {
                graph,
                nodes: count,
            },
            notes: Vec::new(),
        })
    }

    /// `umx ui selection [--graph <alias>]` (S7, security T4a): with a graph named, its rows; with none, rows only for
    /// graphs whose policy is open, and the nodes selected in any other graph counted (`ui_selection_hidden`), never
    /// named. Empty once no view has waited for [`VIEW_GONE_MS`].
    pub fn selection(&mut self, store: &GraphStore, ctx: &Ctx) -> Outcome<Vec<SelectedRow>> {
        self.expire();
        if ctx.scope.is_some() {
            return Err(problems::usage_invalid(
                "this command takes one graph (use --graph with one name)",
                "ui",
            ));
        }
        if store.names_graph(ctx, &[])? {
            let graph = store.select(ctx, &[])?;
            let ids = self.selection.get(&graph).map_or(&[][..], Vec::as_slice);
            return Ok(Done {
                value: store.selected_rows(&graph, ids),
                notes: Vec::new(),
            });
        }
        let (mut rows, mut hidden) = (Vec::new(), 0);
        // Per alias, unlike a scoped read's one copy per graph (d-21): the person selected in the copy on screen.
        // A graph unloaded since has no rows and counts nothing.
        for (graph, ids) in &self.selection {
            let found = store.selected_rows(graph, ids);
            if store.shown_unnamed(graph) {
                rows.extend(found);
            } else {
                hidden += found.len();
            }
        }
        let warnings = if hidden > 0 {
            alloc::vec![problems::ui_selection_hidden(hidden)]
        } else {
            Vec::new()
        };
        Ok(Done {
            value: rows,
            notes: warnings,
        })
    }
}
