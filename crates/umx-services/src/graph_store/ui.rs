//! The graph a graph-view command acts on, the nodes it names, and the rows of a selection (ws009 S6 and S7). The same
//! scope rules as every read: d-20 (a secured graph only when named) and d-21 (one copy of a graph).

use super::{Ctx, GraphStore, alias_name, problems, resolve_node};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use umx_core::problem::Candidate;
use umx_core::{Id, Problem};

/// One selected node as `umx ui selection` shows it. The title is graph text, cut to [`TITLE_CHARS`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedRow {
    pub graph: String,
    pub id: Id,
    pub kind: String,
    pub title: String,
    /// Whether the title was cut.
    pub cut: bool,
    /// `field:title` when umx's secret check flags the title (ws009 plan 179, as F188's node and entity reads): the
    /// stored title is checked, with this machine's allows, so a cut never hides a flag.
    pub secret_suspected: Vec<String>,
}

/// The longest title a selection row carries, in characters: the reply stays small whatever the graph holds.
pub const TITLE_CHARS: usize = 200;

impl GraphStore {
    /// The daemon's graph-view channel; its epoch is drawn from the id generator, never the clock (architect W2).
    pub fn ui_channel(&self) -> crate::ui::UiChannel {
        let id = self.ports.ids.next_id();
        crate::ui::UiChannel::new(id.0 as u64, alloc::sync::Arc::clone(&self.ports.clock))
    }

    /// Whether the request names its graph: `--graph`, `UMX_GRAPH` or an address among `refs`.
    pub(crate) fn names_graph(&self, ctx: &Ctx, refs: &[Option<&str>]) -> Result<bool, Problem> {
        Ok(ctx.graph.is_some()
            || ctx.default_graph.is_some()
            || self.addressed(ctx, refs)?.is_some())
    }

    /// The graph a graph-view command acts on and each node it names, resolved in it. A named graph is used whatever
    /// its policy. With none named only open graphs count (d-20), one copy each (d-21): the one open graph, else
    /// `graph_none_loaded` or `graph_ambiguous` naming only open graphs, so a loaded secured graph answers exactly as
    /// an unloaded one.
    pub(crate) fn ui_target(&self, ctx: &Ctx, refs: &[&str]) -> Result<(String, Vec<Id>), Problem> {
        let opt: Vec<Option<&str>> = refs.iter().map(|r| Some(*r)).collect();
        let alias = if self.names_graph(ctx, &opt)? {
            self.select(ctx, &opt)?
        } else {
            if ctx.scope.is_some() {
                return Err(problems::usage_invalid(
                    "this command takes one graph (use --graph with one name)",
                    "ui",
                ));
            }
            let scope = self.unnamed_scope()?;
            match scope.aliases.as_slice() {
                [] => return Err(problems::graph_none_loaded()),
                [only] => {
                    self.graphs[only]
                        .last_used
                        .store(self.ports.clock.now_ms(), Ordering::Relaxed);
                    only.clone()
                }
                [first, ..] => {
                    return Err(problems::graph_ambiguous(
                        ctx.retry.as_ref(),
                        &alias_name(first),
                        scope
                            .aliases
                            .iter()
                            .map(|a| Candidate {
                                id: a.clone(),
                                title: None,
                                score: None,
                            })
                            .collect(),
                    ));
                }
            }
        };
        let loaded = &self.graphs[&alias];
        let ids = refs
            .iter()
            .map(|r| resolve_node(loaded, &alias, r))
            .collect::<Result<_, _>>()?;
        Ok((alias, ids))
    }

    /// Why `umx ui open` may not open `alias` in the view (ws009 M1b1), or None when it may: a loaded graph whose
    /// policy is open, or one this machine remembers as loaded (idle) whose last known policy was open; the view's own
    /// read loads it on demand. Refused: `secured` (named visibility, or an idle graph whose policy is not known),
    /// `unloaded` (unloaded on purpose) or `unknown`. Each with why and the command that would open or load it.
    pub(crate) fn ui_open_refusal(
        &self,
        alias: &str,
    ) -> Option<(&'static str, &'static str, umx_core::Command)> {
        use umx_core::{Command, ValidName, Visibility};
        let Some(name) = ValidName::alias(alias) else {
            return Some((
                "unknown",
                "a graph name is snake_case",
                Command::new("graph list").word("--all"),
            ));
        };
        let secured = || {
            (
                "secured",
                "it is secured and only the person opens it (by clicking or with umx ui --graph)",
                Command::new("ui").word("--graph").name(&name),
            )
        };
        if let Some(l) = self.graphs.get(alias) {
            return (l.policy.effective.visibility != Visibility::Open).then(secured);
        }
        if let Some(row) = self.remembered_loaded(alias) {
            // The row's policy as last loaded; none known is treated as secured (security S1: never guess open).
            let open = row
                .policy
                .as_deref()
                .is_some_and(|p| p == "open" || p.starts_with("open+"));
            return (!open).then(secured);
        }
        let rows = self.read_registry().map(|r| r.rows).unwrap_or_default();
        match rows.iter().rev().find(|r| r.alias == alias) {
            Some(row) => Some((
                "unloaded",
                "it was unloaded on purpose",
                match umx_core::ports::GraphDir::parse(&row.folder) {
                    Ok(dir) => Command::new("graph load")
                        .path(&dir)
                        .word("--as")
                        .name(&name),
                    Err(_) => Command::new("graph list").word("--all"),
                },
            )),
            None => Some((
                "unknown",
                "no graph with that alias is known here",
                Command::new("graph list").word("--all"),
            )),
        }
    }

    /// The graphs a view's wait names, each once; a named secured graph is included (it is named). One that is not
    /// loaded is kept, never refused (architect): an unload answers its waits with overflow, and the view
    /// may name a graph it has just seen unloaded.
    pub(crate) fn ui_graphs(graphs: &[String]) -> Result<Vec<String>, Problem> {
        let mut out: Vec<String> = Vec::new();
        for alias in graphs {
            if umx_core::ValidName::alias(alias).is_none() {
                return Err(problems::usage_invalid("a graph name is snake_case", "ui"));
            }
            if !out.contains(alias) {
                out.push(alias.clone());
            }
        }
        Ok(out)
    }

    /// The rows for `ids` selected in `alias`, skipping a graph that is gone and nodes no longer in it.
    pub(crate) fn selected_rows(&self, alias: &str, ids: &[Id]) -> Vec<SelectedRow> {
        let Some(loaded) = self.graphs.get(alias) else {
            return Vec::new();
        };
        // One lazy allows snapshot for the whole read, as entity show's (R324-3).
        let allows = core::cell::OnceCell::new();
        ids.iter()
            .filter(|id| loaded.read_scope().contains(**id))
            .filter_map(|id| loaded.graph.node(*id))
            .map(|n| {
                let title = n.title();
                let cut = title.chars().count() > TITLE_CHARS;
                let secret_suspected = self.member_flags(&allows, alias, n.id(), title, &[]);
                SelectedRow {
                    graph: alias.to_string(),
                    id: n.id(),
                    kind: n.kind().to_string(),
                    title: if cut {
                        title.chars().take(TITLE_CHARS).collect()
                    } else {
                        title.to_string()
                    },
                    cut,
                    secret_suspected,
                }
            })
            .collect()
    }
}
