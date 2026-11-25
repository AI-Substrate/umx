//! This machine's graph registry (req-56's first slice, ws002 F72, plan 081): which graph folders are loaded, so the
//! daemon reloads them when it starts. Rows are added on graph new and load, cleared on an explicit unload and kept on
//! idle unload. A reload goes through exactly the explicit load path (security, F72): nothing in the file stands in
//! for a check, and a folder that now holds another graph id is not loaded.

use super::GraphStore;
use crate::problems;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::ports::{GraphDir, LocalStateError, StateWritten};
use umx_core::registry::{Error, MAX_ROWS, Registry, Row};
use umx_core::{Command, Id, Policy, Problem, ValidName};

/// A remembered graph that did not reload when the daemon started.
#[derive(Clone, Debug, PartialEq)]
pub struct NotReloaded {
    pub alias: String,
    pub folder: String,
    /// The load's own code (graph_not_found, path_outside_roots, graph_format_newer, alias_in_use...), or
    /// `graph_id_changed` when the folder now holds another graph.
    pub code: &'static str,
}

/// A remembered graph that reloaded, with the load's warnings (such as policy_downgrade_unconfirmed).
#[derive(Clone, Debug, PartialEq)]
pub struct Reloaded {
    pub alias: String,
    pub folder: String,
    pub warnings: Vec<&'static str>,
}

/// What the start-up reload did, kept for `daemon status`, doctor and graph_not_loaded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReloadReport {
    /// When the reload ran (clock ms): when this daemon started.
    pub started_ms: u64,
    pub reloaded: Vec<Reloaded>,
    pub not_reloaded: Vec<NotReloaded>,
    /// Why the registry could not be used at all (nothing reloaded), if so.
    pub unreadable: Option<String>,
    /// 1-based positions of rows that did not parse or validate, skipped.
    pub skipped_rows: Vec<usize>,
    /// Aliases that came back as the only copy of a graph without a primary, so primary again (F80).
    pub primary_restored: Vec<String>,
    /// What saving the registry after the reload found (index details, no-primary marks): `daemon status` shows
    /// them as warnings; never fatal (review R193-B1, B2).
    pub warnings: Vec<Problem>,
}

fn codec_reason(e: Error) -> String {
    match e {
        Error::Invalid => "it does not parse".to_string(),
        Error::Limit => format!("it is over its limits ({MAX_ROWS} rows)"),
        Error::Newer(v) => format!("it is format {v}, newer than this umx reads"),
    }
}

/// A failed reload's code in words: message slots turn `_` into `?` (status and doctor carry the code itself).
fn failed(code: &str) -> &'static str {
    match code {
        "graph_not_found" => "its folder has no graph.json",
        "path_outside_roots" => "its folder is outside the graph roots",
        "graph_format_newer" => "its files are a newer format than this umx reads",
        "alias_in_use" | "alias_invalid" => "its alias is taken",
        "graph_id_changed" => "its folder now holds another graph",
        "config_invalid" => "the machine config is invalid",
        "recovery_conflict" => "a cut-off write in its folder needs resolving",
        _ => "its load failed (umx doctor says why)",
    }
}

impl GraphStore {
    /// Placeholder aliases come only from registry history, never from the loaded-graph table.
    pub(crate) fn placeholder_aliases(&self) -> (BTreeMap<Id, String>, Vec<Problem>) {
        let registry = match self.read_registry() {
            Ok(registry) => registry,
            Err(why) => {
                return (
                    BTreeMap::new(),
                    alloc::vec![problems::registry_unreadable(&self.registry_file(), &why)],
                );
            }
        };
        let mut owners: BTreeMap<&str, Option<Id>> = BTreeMap::new();
        for row in &registry.rows {
            owners
                .entry(&row.alias)
                .and_modify(|id| {
                    if *id != Some(row.id) {
                        *id = None;
                    }
                })
                .or_insert(Some(row.id));
        }
        let mut aliases = BTreeMap::new();
        for row in &registry.rows {
            if owners[row.alias.as_str()] == Some(row.id) {
                aliases.entry(row.id).or_insert_with(|| row.alias.clone());
            }
        }
        (aliases, Vec::new())
    }

    /// The registry file, for messages and `daemon status`.
    pub fn registry_file(&self) -> GraphDir {
        self.ports.registry.path()
    }

    /// The registry, or why it cannot be used (refused, unreadable, invalid, newer, too large).
    pub(super) fn read_registry(&self) -> Result<Registry, String> {
        let bytes = self.ports.registry.read().map_err(|e| match e {
            LocalStateError::Refused { reason, .. } => reason.to_string(),
            e => e.to_string(),
        })?;
        bytes.map_or_else(
            || Ok(Registry::default()),
            |b| Registry::decode(&b).map_err(codec_reason),
        )
    }

    /// Writes the registry. Err(why): it was not replaced. Ok carries `local_state_not_durable` when it was
    /// replaced but the folder sync failed.
    pub(super) fn try_write_registry(&self, registry: &Registry) -> Result<Vec<Problem>, String> {
        let bytes = registry.encode().map_err(codec_reason)?;
        match self
            .ports
            .registry
            .write(&bytes)
            .map_err(|e| e.to_string())?
        {
            StateWritten::Durable => Ok(Vec::new()),
            StateWritten::NotSynced(why) => Ok(alloc::vec![problems::local_state_not_durable(
                &self.registry_file(),
                &why
            )]),
        }
    }

    /// [`Self::try_write_registry`] after a load: a failure is `registry_not_saved`.
    pub(super) fn write_registry(&self, alias: &str, registry: &Registry) -> Vec<Problem> {
        self.try_write_registry(registry).unwrap_or_else(|why| {
            alloc::vec![problems::registry_not_saved(
                alias,
                &self.registry_file(),
                &why
            )]
        })
    }

    /// [`Self::try_write_registry`] for a change that is not a load (index details, no-primary marks): a failure is
    /// `registry_update_not_saved`, one per alias named.
    pub(super) fn update_registry(&self, aliases: &[&str], registry: &Registry) -> Vec<Problem> {
        self.try_write_registry(registry).unwrap_or_else(|why| {
            let file = self.registry_file();
            aliases
                .iter()
                .map(|a| problems::registry_update_not_saved(a, &file, &why))
                .collect()
        })
    }

    /// After the start-up reload (review R193-B2): each reloaded graph's row gets its index details as it loaded now,
    /// under its settled effective policy (a graph secured while the daemon was down loses its stored counts and
    /// model) and this load's time. Rows keep their place and the no-primary marks stay; a write that fails is
    /// reported in the reload report.
    pub(crate) fn refresh_reloaded(&mut self) {
        let aliases: Vec<String> = self
            .reload
            .reloaded
            .iter()
            .map(|r| r.alias.clone())
            .collect();
        let mut registry = match self.read_registry() {
            Ok(r) => r,
            Err(why) => {
                // reload_rows read it a moment ago: say why the details were not recorded, never skip silently (F103).
                let why = format!(
                    "it could not be read again to record the details of the reloaded graphs ({why})"
                );
                let unreadable = problems::registry_unreadable(&self.registry_file(), &why);
                self.reload.warnings.push(unreadable);
                return;
            }
        };
        let now = self.now();
        let mut changed = Vec::new();
        for alias in &aliases {
            let Some(l) = self.graphs.get(alias.as_str()) else {
                continue;
            };
            let Some(i) = registry
                .rows
                .iter()
                .position(|r| r.folder == l.dir.as_str())
            else {
                continue;
            };
            let mut row = registry.rows[i].clone();
            row.alias = alias.clone();
            self.details(alias, &mut row);
            if row.nodes.is_some() {
                row.last_loaded = Some(now.clone());
            }
            if row != registry.rows[i] {
                registry.rows[i] = row;
                changed.push(alias.as_str());
            }
        }
        if changed.is_empty() {
            return;
        }
        let warnings = self.update_registry(&changed, &registry);
        self.reload.warnings.extend(warnings);
    }

    /// Records `alias` (just loaded or created) as loaded. `in_memory`: its folder was loaded before this load, so
    /// its row keeps its place in load order (d-21). Never fails the verb: a registry that cannot be used is left
    /// untouched and named in a warning.
    pub(crate) fn remember(&mut self, alias: &str, in_memory: bool) -> Vec<Problem> {
        let Some(row) = self.loaded_row(alias) else {
            return Vec::new();
        };
        let mut registry = match self.read_registry() {
            Ok(r) => r,
            Err(why) => {
                return alloc::vec![problems::registry_unreadable(&self.registry_file(), &why)];
            }
        };
        if !registry.loaded(row, in_memory) {
            let why = format!("the registry is full: {MAX_ROWS} loaded graphs");
            return alloc::vec![problems::registry_not_saved(
                alias,
                &self.registry_file(),
                &why
            )];
        }
        self.write_registry(alias, &registry)
    }

    /// The registry row that records loaded graph `alias` as loaded now, with its index details.
    pub(super) fn loaded_row(&self, alias: &str) -> Option<Row> {
        let l = self.graphs.get(alias)?;
        let mut row = Row::new(
            l.meta.id,
            alias.to_string(),
            l.dir.as_str().to_string(),
            true,
        );
        self.details(alias, &mut row);
        if row.nodes.is_some() {
            row.last_loaded = Some(self.now());
        }
        Some(row)
    }

    /// Whether this machine's index has a row for `folder` (loaded now or before): `graph load --graphfile` names
    /// the graphs new to this machine (ws002 s17). An unreadable index counts every folder as new.
    pub(crate) fn known_folder(&self, folder: &GraphDir) -> bool {
        self.read_registry()
            .is_ok_and(|r| r.rows.iter().any(|row| row.folder == folder.as_str()))
    }

    /// The index details of loaded graph `alias` (req-116): counts and model for an open graph only, none for any
    /// other; a display hint of the policy for every graph (security, plan 098).
    fn details(&self, alias: &str, row: &mut Row) {
        let Some(l) = self.graphs.get(alias) else {
            return;
        };
        let policy = self.effective_policy(alias).unwrap_or(Policy::SECURED);
        row.policy = Some(policy.name().to_string());
        let open = policy == Policy::OPEN;
        let counts = open.then(|| l.read_scope().counts());
        row.nodes = counts.map(|(nodes, _)| nodes as u64);
        row.edges = counts.map(|(_, edges)| edges as u64);
        row.model = l
            .meta
            .embedding
            .as_ref()
            .and_then(|e| e.key.as_ref())
            .map(ToString::to_string)
            .filter(|_| open);
        if !open {
            row.last_loaded = None;
        }
    }

    /// Before loaded graph `alias` leaves memory (explicit or idle unload): its row's counts, model and policy hint
    /// become what it holds now, so `graph list --all` shows the graph as it was when it unloaded. Never fails the
    /// verb.
    pub(crate) fn note_unload(&self, alias: &str) -> Vec<Problem> {
        let Some(l) = self.graphs.get(alias) else {
            return Vec::new();
        };
        let mut registry = match self.read_registry() {
            Ok(r) => r,
            Err(why) => {
                let why = format!(
                    "it could not be read to record the details of graph {alias} at unload ({why})"
                );
                return alloc::vec![problems::registry_unreadable(&self.registry_file(), &why)];
            }
        };
        let Some(i) = registry
            .rows
            .iter()
            .position(|r| r.folder == l.dir.as_str())
        else {
            return Vec::new();
        };
        let mut row = registry.rows[i].clone();
        self.details(alias, &mut row);
        if row == registry.rows[i] {
            return Vec::new();
        }
        registry.rows[i] = row;
        self.update_registry(&[alias], &registry)
    }

    /// Clears the loaded flag for `folder` after an explicit unload, so the graph does not come back at the next
    /// start. Returns the warnings, and whether a row was cleared.
    pub(crate) fn forget(&mut self, alias: &str, folder: &str) -> (Vec<Problem>, bool) {
        self.reload.not_reloaded.retain(|n| n.folder != folder);
        let mut registry = match self.read_registry() {
            Ok(r) => r,
            Err(why) => {
                return (
                    alloc::vec![problems::registry_unreadable(&self.registry_file(), &why)],
                    false,
                );
            }
        };
        if !registry.unloaded(folder) {
            return (Vec::new(), false);
        }
        (self.write_registry(alias, &registry), true)
    }

    /// The folder of a remembered, loaded row for `alias` that is not in memory (it did not reload, or it unloaded
    /// when idle), for `graph unload` to forget.
    pub(crate) fn remembered_folder(&self, alias: &str) -> Option<String> {
        if self.graphs.contains_key(alias) {
            return None;
        }
        let registry = self.read_registry().ok()?;
        registry
            .rows
            .iter()
            .find(|r| r.alias == alias && r.loaded)
            .map(|r| r.folder.clone())
    }

    /// The one registry row for `alias` remembered as loaded when it is not in memory (it unloaded when idle, or did
    /// not reload at start): what an on-demand load may load (req-15, F72). Rows for the alias that were unloaded on
    /// purpose are history, not ambiguity (review R156-B2). None when no row or two loaded rows have the alias (only
    /// a hand-edited file has two), or the registry cannot be used.
    pub fn remembered_loaded(&self, alias: &str) -> Option<Row> {
        if self.graphs.contains_key(alias) {
            return None;
        }
        let registry = self.read_registry().ok()?;
        let mut rows = registry
            .rows
            .into_iter()
            .filter(|r| r.alias == alias && r.loaded);
        match (rows.next(), rows.next()) {
            (Some(row), None) => Some(row),
            _ => None,
        }
    }

    /// Starts the start-up reload: resets the report and returns the rows to load, in load order. A registry that
    /// cannot be used returns none (fail-safe: no graph appears that the user did not load) and is reported.
    pub(crate) fn reload_rows(&mut self) -> Vec<Row> {
        self.reload = ReloadReport {
            started_ms: self.ports.clock.now_ms(),
            ..ReloadReport::default()
        };
        match self.read_registry() {
            Err(why) => {
                self.reload.unreadable = Some(why);
                Vec::new()
            }
            Ok(registry) => {
                self.reload.skipped_rows = registry.skipped;
                registry.rows.into_iter().filter(|r| r.loaded).collect()
            }
        }
    }

    /// The alias graph `id` is loaded under from `folder`, if it is in memory from there.
    pub fn loaded_from(&self, folder: &str, id: Id) -> Option<String> {
        self.graphs
            .iter()
            .find(|(_, l)| l.dir.as_str() == folder && l.meta.id == id)
            .map(|(alias, _)| alias.to_string())
    }

    /// Whether the registry still remembers `row` as loaded, under the same alias, folder and graph id: a reload row
    /// that waited is skipped only when the registry says it was unloaded, forgotten or re-aliased since. A registry
    /// that cannot be read now keeps the row: it came from the start-up read, as every row did before the daemon
    /// served first, and the reload report says the later read failed (F103).
    pub(crate) fn still_remembered(&self, row: &Row) -> bool {
        self.read_registry().map_or(true, |registry| {
            registry.rows.iter().any(|r| {
                r.loaded && r.alias == row.alias && r.folder == row.folder && r.id == row.id
            })
        })
    }

    /// Records one remembered row's reload: its load's warning codes, or the code it failed with.
    pub(crate) fn record_reload(
        &mut self,
        row: &Row,
        result: Result<Vec<&'static str>, &'static str>,
    ) {
        let (alias, folder) = (row.alias.clone(), row.folder.clone());
        match result {
            Ok(warnings) => self.reload.reloaded.push(Reloaded {
                alias,
                folder,
                warnings,
            }),
            Err(code) => self.reload.not_reloaded.push(NotReloaded {
                alias,
                folder,
                code,
            }),
        }
    }

    /// What the start-up reload did (empty before it runs).
    pub fn reload_report(&self) -> &ReloadReport {
        &self.reload
    }

    /// graph_not_loaded for `alias`, saying why and how to load it when this machine remembers it (F72).
    pub fn not_loaded(&self, alias: &str) -> Problem {
        let at = umx_core::time::iso_from_ms(self.reload.started_ms);
        let load = |folder: &str| {
            let dir = GraphDir::parse(folder).ok()?;
            let name = ValidName::alias(alias)?;
            Some(
                Command::new("graph load")
                    .path(&dir)
                    .word("--as")
                    .name(&name),
            )
        };
        if let Some(n) = self.reload.not_reloaded.iter().find(|n| n.alias == alias)
            && let Some(fix) = load(&n.folder)
        {
            let why = format!(
                "it did not reload when the daemon started at {at}: {}",
                failed(n.code)
            );
            return problems::graph_not_loaded_because(alias, &why, fix);
        }
        if let Ok(registry) = self.read_registry()
            && let Some(row) = registry.rows.iter().rev().find(|r| r.alias == alias)
            && let Some(fix) = load(&row.folder)
        {
            let why = if row.loaded {
                "it unloaded after being idle"
            } else {
                "it was unloaded"
            };
            return problems::graph_not_loaded_because(alias, why, fix);
        }
        let why = format!("the daemon started at {at} and has not loaded it");
        problems::graph_not_loaded_because(alias, &why, Command::new("graph list"))
    }
}
