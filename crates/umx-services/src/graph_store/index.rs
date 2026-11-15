//! This machine's graph index (ws002 s16, req-116): F72's registry rows outlive an unload, so `graph list --all`
//! shows every graph loaded here, `graph load <alias>` finds an unloaded one by alias and `graph forget` drops one.
//! And F80 (d-21 across a restart): a graph id loaded as two or more copies whose primary unloaded stays without a
//! primary after the daemon restarts.
//!
//! Security (plan 098): a row's folder is checked against the roots, on its canonical path, before anything reads
//! it; one outside them is never listed, read or walked. The policy shown for a graph that is not loaded is the
//! stricter of the row's hint, this machine's policy record and graph.json as it is now (read capped, strictly
//! parsed); one that cannot be read counts as secured. Only an open graph shows its counts, model, last load and
//! repo.

use super::{Ctx, Done, GraphInfo, GraphStore, Outcome};
use crate::problems;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::codec::{self, GraphFile};
use umx_core::ports::{FilesError, GraphDir};
use umx_core::registry::{Registry, Row};
use umx_core::{Command, Id, Policy, Problem, ValidName};

/// Whether a row's folder holds its graph, as `graph list --all` shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderState {
    Present,
    /// No graph.json there (renamed, moved or deleted).
    Missing,
    /// Outside the graph roots: never read (security, plan 098).
    OutsideRoots,
}

impl FolderState {
    pub fn as_str(self) -> &'static str {
        match self {
            FolderState::Present => "present",
            FolderState::Missing => "missing",
            FolderState::OutsideRoots => "outside_roots",
        }
    }
}

/// One row of `graph list --all`.
#[derive(Clone, Debug, PartialEq)]
pub struct IndexRow {
    pub alias: String,
    pub id: Id,
    pub path: String,
    /// `loaded`; `idle` (remembered as loaded: it comes back at daemon start or when a command names it);
    /// `not_reloaded` (it failed to come back at daemon start); `unloaded` (unloaded on purpose).
    pub state: &'static str,
    pub folder: FolderState,
    pub policy: Policy,
    /// The rest are None for a graph that is not loaded and not open.
    pub last_loaded: Option<String>,
    pub nodes: Option<u64>,
    pub edges: Option<u64>,
    pub model: Option<String>,
    /// The folder holding `.git` above the graph's folder, within the roots.
    pub repo: Option<String>,
    /// `umx graph load <folder> --as <alias>`.
    pub reload: Command,
    /// A loaded graph: everything `graph list` shows.
    pub loaded: Option<GraphInfo>,
}

/// What `graph forget` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphForgotten {
    pub alias: String,
    /// The index rows removed (one per folder the alias named).
    pub rows: usize,
}

/// The index rows for an alias, for `graph load <alias>`.
pub(crate) enum Known {
    None,
    One(Row),
    /// Rows for two or more folders (graph-addressing-expert, plan 098: never pick one).
    Many(Vec<Row>),
}

fn reload_command(dir: &GraphDir, alias: &str) -> Command {
    let c = Command::new("graph load").path(dir).word("--as");
    match ValidName::alias(alias) {
        Some(name) => c.name(&name),
        None => c.word("<alias>"),
    }
}

impl GraphStore {
    /// `graph list --all`: every loaded graph, then every graph this machine's index knows that is not loaded, in
    /// alias order. `repo` finds a folder's repository (the config files port). A registry that cannot be used lists
    /// the loaded graphs and says why.
    pub fn graph_index(&self, repo: &dyn Fn(&GraphDir) -> Option<GraphDir>) -> Done<Vec<IndexRow>> {
        let mut warnings = Vec::new();
        let registry = self.read_registry().unwrap_or_else(|why| {
            warnings.push(problems::registry_unreadable(&self.registry_file(), &why));
            Registry::default()
        });
        let mut rows = Vec::new();
        for info in self.graph_list() {
            let l = &self.graphs[info.alias.as_str()];
            let remembered = registry.rows.iter().find(|r| r.folder == l.dir.as_str());
            rows.push(IndexRow {
                alias: info.alias.clone(),
                id: info.id,
                path: info.path.clone(),
                state: "loaded",
                folder: self.folder_state(l.dir.as_str()).0,
                policy: info.policy.effective,
                last_loaded: remembered.and_then(|r| r.last_loaded.clone()),
                nodes: Some(info.nodes as u64),
                edges: Some(info.edges as u64),
                model: info
                    .embedding
                    .as_ref()
                    .and_then(|e| e.key.as_ref())
                    .map(ToString::to_string),
                repo: repo(&l.dir).map(|r| r.as_str().to_string()),
                reload: reload_command(&l.dir, &info.alias),
                loaded: Some(info),
            });
        }
        // Read once: a record that cannot be used counts every graph as secured.
        let record = self.read_state().ok().map(|(state, _)| state);
        let mut others: Vec<IndexRow> = registry
            .rows
            .iter()
            .filter(|r| !self.graphs.values().any(|l| l.dir.as_str() == r.folder))
            .map(|r| self.index_row(r, record.as_ref(), repo))
            .collect();
        others.sort_by(|a, b| (&a.alias, &a.path).cmp(&(&b.alias, &b.path)));
        rows.extend(others);
        Done {
            value: rows,
            notes: warnings,
        }
    }

    /// The folder's state, and its canonical identity when it is inside the roots. Resolving is the roots check:
    /// nothing is listed or read outside them.
    fn folder_state(&self, folder: &str) -> (FolderState, Option<GraphDir>) {
        let Ok(given) = GraphDir::parse(folder) else {
            return (FolderState::Missing, None);
        };
        match self.ports.files.resolve(&given) {
            Err(FilesError::OutsideRoots(_)) => (FolderState::OutsideRoots, None),
            Err(_) => (FolderState::Missing, None),
            Ok(dir) => match self.ports.files.list(&dir) {
                Ok(files) if files.contains(&GraphFile::Graph) => (FolderState::Present, Some(dir)),
                _ => (FolderState::Missing, Some(dir)),
            },
        }
    }

    fn index_row(
        &self,
        row: &Row,
        record: Option<&umx_core::local_state::PolicyState>,
        repo: &dyn Fn(&GraphDir) -> Option<GraphDir>,
    ) -> IndexRow {
        let (folder, dir) = self.folder_state(&row.folder);
        let file = match (folder, &dir) {
            (FolderState::Present, Some(d)) => self
                .ports
                .files
                .read(d, GraphFile::Graph)
                .ok()
                .flatten()
                .and_then(|bytes| codec::decode_graph(&bytes).ok())
                .map(|meta| meta.file_policy()),
            _ => None,
        };
        let recorded = match record {
            Some(state) => state.recorded(row.id, &row.folder).unwrap_or(Policy::OPEN),
            None => Policy::SECURED,
        };
        let policy = file
            .unwrap_or(Policy::SECURED)
            .join(row.policy_hint().unwrap_or(Policy::OPEN))
            .join(recorded);
        let open = policy == Policy::OPEN;
        let state = if self
            .reload
            .not_reloaded
            .iter()
            .any(|n| n.folder == row.folder)
        {
            "not_reloaded"
        } else if row.loaded {
            "idle"
        } else {
            "unloaded"
        };
        let shown = GraphDir::parse(&row.folder).ok();
        IndexRow {
            alias: row.alias.clone(),
            id: row.id,
            path: row.folder.clone(),
            state,
            folder,
            policy,
            last_loaded: row.last_loaded.clone().filter(|_| open),
            nodes: row.nodes.filter(|_| open),
            edges: row.edges.filter(|_| open),
            model: row.model.clone().filter(|_| open),
            repo: dir
                .filter(|_| open)
                .and_then(|d| repo(&d))
                .map(|r| r.as_str().to_string()),
            reload: match &shown {
                Some(d) => reload_command(d, &row.alias),
                None => Command::new("graph list").word("--all"),
            },
            loaded: None,
        }
    }

    /// `graph forget <alias>` (req-116): removes the alias's rows from this machine's index, so it is not listed and
    /// never comes back at daemon start or on demand. A loaded alias is refused: unload it first.
    pub fn graph_forget(&mut self, _ctx: &Ctx, alias: &str) -> Outcome<GraphForgotten> {
        if let Some(name) = ValidName::alias(alias)
            && self.graphs.contains_key(alias)
        {
            return Err(problems::graph_forget_loaded(&name));
        }
        let mut registry = self.read_registry().map_err(|why| {
            problems::graph_not_known(alias, &alloc::format!("the index cannot be read ({why})"))
        })?;
        let rows = registry.forget(alias);
        if rows == 0 {
            return Err(problems::graph_not_known(alias, "no row has that alias"));
        }
        // Forgetting changes nothing but the index, so a write that did not replace it fails the verb, and the
        // report changes only once it stands (review R193-B1). A failed folder sync after the replace is a warning.
        let warnings = self
            .try_write_registry(&registry)
            .map_err(|why| problems::graph_forget_not_saved(alias, &self.registry_file(), &why))?;
        self.reload.not_reloaded.retain(|n| n.alias != alias);
        Ok(Done {
            value: GraphForgotten {
                alias: alias.to_string(),
                rows,
            },
            notes: warnings,
        })
    }

    /// The index rows for `alias`: one folder, several, or none (or a registry that cannot be used).
    pub(crate) fn known(&self, alias: &str) -> Known {
        let Ok(registry) = self.read_registry() else {
            return Known::None;
        };
        let mut rows: Vec<Row> = registry
            .rows
            .into_iter()
            .filter(|r| r.alias == alias)
            .collect();
        match rows.len() {
            0 => Known::None,
            1 => Known::One(rows.remove(0)),
            _ => Known::Many(rows),
        }
    }

    /// alias_ambiguous for `rows`, each with the command that loads it.
    pub(crate) fn ambiguous(&self, alias: &ValidName, rows: &[Row]) -> Problem {
        let rows: Vec<(GraphDir, Command)> = rows
            .iter()
            .filter_map(|r| GraphDir::parse(&r.folder).ok())
            .map(|d| {
                let load = reload_command(&d, alias.as_str());
                (d, load)
            })
            .collect();
        problems::alias_ambiguous(alias, &rows)
    }

    /// F80: writes whether graph id `id` is without a primary copy, so a restart keeps it. Never fails the verb: a
    /// registry that cannot be used is named in a warning.
    pub(crate) fn save_no_primary(&self, id: Id, alias: &str) -> Vec<Problem> {
        let none = self.demoted.contains(&id);
        let mut registry = match self.read_registry() {
            Ok(r) => r,
            Err(why) => {
                return alloc::vec![problems::registry_unreadable(&self.registry_file(), &why)];
            }
        };
        match registry.set_no_primary(id, none) {
            Ok(false) => Vec::new(),
            Ok(true) => self.update_registry(&[alias], &registry),
            Err(_) => alloc::vec![problems::registry_update_not_saved(
                alias,
                &self.registry_file(),
                &alloc::format!(
                    "it already marks {} graphs without a primary copy",
                    umx_core::registry::MAX_ROWS
                ),
            )],
        }
    }

    /// F80, after the start-up reload: each graph id the registry lists as without a primary stays so while two or
    /// more of its copies came back. One copy back is primary again and is reported (`primary_restored`); none back
    /// drops the mark. Returns the aliases restored.
    pub(crate) fn restore_no_primary(&mut self) -> Vec<String> {
        let mut registry = match self.read_registry() {
            Ok(r) => r,
            Err(why) => {
                // Read at the start of the reload; now refused or broken: the marks are not settled, and that is said
                // (F103). Copies whose primary was unloaded come back with the earliest loaded as primary.
                let why = alloc::format!(
                    "it could not be read again to settle the no-primary marks ({why})"
                );
                let unreadable = problems::registry_unreadable(&self.registry_file(), &why);
                self.reload.warnings.push(unreadable);
                return Vec::new();
            }
        };
        let mut restored = Vec::new();
        let mut changed = false;
        for id in registry.no_primary.clone() {
            let copies = self.copies_of(id);
            match copies.len() {
                0 => {}
                1 => restored.push(copies[0].to_string()),
                _ => {
                    self.demoted.insert(id);
                    continue;
                }
            }
            // Clearing never hits the limit.
            changed |= registry.set_no_primary(id, false).unwrap_or(false);
        }
        if changed {
            // Never fatal at start, but reported (review R193-B1): a mark left in place is cleared at the next start.
            let named: Vec<&str> = if restored.is_empty() {
                alloc::vec![""]
            } else {
                restored.iter().map(String::as_str).collect()
            };
            let warnings = self.update_registry(&named, &registry);
            self.reload.warnings.extend(warnings);
        }
        for alias in &restored {
            if let Some(r) = self.reload.reloaded.iter_mut().find(|r| &r.alias == alias) {
                r.warnings.push("primary_restored");
            }
        }
        self.reload.primary_restored = restored.clone();
        restored
    }
}
