//! `umx graph rename` and `umx graph move` (ws006 S23, req-56; plan 209, graph-addressing-expert and architect), and
//! the one lookup that turns a name into a loaded graph: a live alias as it is, a former alias to the graph that
//! had it (graph.json's `former_aliases`), recorded for the `alias_redirected` hint.

use super::graphfile::{Updated, relative, scan_cmd};
use super::{Done, GraphStore, Outcome, alias_name, persist, problems, second};
use crate::Ctx;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::codec::GraphFile;
use umx_core::model::MAX_FORMER;
use umx_core::ontology::{DERIVED_RELATIONS, PROVENANCE_RELATIONS};
use umx_core::ports::{FilesError, GraphDir, RELOCATE_PARENT_MISSING, RELOCATE_TARGET_EXISTS};
use umx_core::ui::Touched;
use umx_core::{Command, Id, Problem, ValidName};
use umx_graph::Graph;

/// The most stale pointers a move reply lists; `umx check` lists them all, paged.
pub const STALE_SHOWN: usize = 100;

/// What `umx graph move` did.
#[derive(Clone, Debug, PartialEq)]
pub struct Moved {
    pub graph: String,
    pub graph_id: Id,
    pub from: GraphDir,
    pub to: GraphDir,
    /// graph.json's `former_paths` now (repo-relative), oldest first.
    pub former_paths: Vec<String>,
    /// This graph's `root:repo` pointers into the folder it left: (node or edge id, place), at most
    /// [`STALE_SHOWN`]. Other graphs citing the folder are not checked (graph-addressing-expert (i)).
    pub stale: Vec<(Id, String)>,
    pub stale_total: usize,
    /// As [`Renamed::graphfile`].
    pub graphfile: &'static str,
}

/// The `root:repo` pointers of `g` into any of `olds` (repo-relative folders): (node or edge id, place, the folder
/// it points into), places counted from 1 (F217). A node's evidence and its status moves' evidence; a stored edge's
/// evidence. A pointer is compared in the locator normal form (`./` and repeated `/` dropped, R241-2), never as
/// written, and is listed once, under the longest of `olds` it is in (review R22, R23).
pub(crate) fn stale_pointers<'o>(g: &Graph, olds: &[&'o str]) -> Vec<(Id, String, &'o str)> {
    let into = |p: &umx_core::Value| -> Option<&'o str> {
        let source = p.get("source")?.as_str()?;
        let (root, rest) = umx_core::evidence::root_source(source)?;
        if root != umx_core::config::REPO_ROOT {
            return None;
        }
        let path = rest.split_once('#').map_or(rest, |(path, _)| path);
        let path = umx_core::locator::repo_path(path).ok()?;
        olds.iter()
            .copied()
            .filter(|old| {
                path.strip_prefix(*old)
                    .is_some_and(|tail| tail.starts_with('/'))
            })
            .max_by_key(|old| old.len())
    };
    let mut out = Vec::new();
    for n in g.nodes().filter(|n| !g.is_provenance(n.ix())) {
        let node = n.to_node();
        for (i, p) in node.evidence.iter().enumerate() {
            if let Some(old) = into(p) {
                out.push((node.id, alloc::format!("evidence:{}", i + 1), old));
            }
        }
        for (i, change) in node.status_history.iter().enumerate() {
            if let Some(old) = change.get("evidence").and_then(into) {
                out.push((node.id, alloc::format!("status_evidence:{}", i + 1), old));
            }
        }
    }
    for e in g.edges() {
        if PROVENANCE_RELATIONS.contains(&e.kind()) || DERIVED_RELATIONS.contains(&e.kind()) {
            continue;
        }
        let edge = e.to_edge();
        for (i, p) in edge.evidence.iter().enumerate() {
            if let Some(old) = into(p) {
                out.push((
                    edge.id,
                    alloc::format!("edge:{}/evidence:{}", edge.id.to_text(), i + 1),
                    old,
                ));
            }
        }
    }
    out
}

/// A hand move found at load: the folder the graph left, its stale pointers (at most [`STALE_SHOWN`]) and their
/// count.
pub(crate) type Healed = (GraphDir, Vec<(Id, String)>, usize);

/// `path`'s parent folder, if it has one.
fn parent(path: &GraphDir) -> Option<GraphDir> {
    let (p, _) = path.as_str().rsplit_once('/')?;
    GraphDir::parse(if p.is_empty() { "/" } else { p }).ok()
}

/// What `umx graph rename` did.
#[derive(Clone, Debug, PartialEq)]
pub struct Renamed {
    /// The live alias now.
    pub graph: String,
    pub graph_id: Id,
    /// The alias it had.
    pub from: String,
    /// graph.json's `former_aliases` now, oldest first.
    pub former_aliases: Vec<String>,
    /// The oldest former aliases dropped to keep [`MAX_FORMER`], oldest first.
    pub dropped: Vec<String>,
    /// The Graphfile at the folder's repo top: `updated`, `unchanged` (no entry for it, or none at all) or
    /// `not_updated` (a warning says why and what to do).
    pub graphfile: &'static str,
}

impl GraphStore {
    /// The live alias of the loaded graph whose graph.json lists `name` as a former alias, among the graphs `shown`
    /// lets through. None when a loaded graph holds `name` as its live alias, when an unloaded row of this
    /// machine's registry does (that row keeps its meaning: graph-addressing-expert), or when none holds it.
    /// Two graphs (different ids) holding it: `former_alias_ambiguous`, never a pick. Copies of one id are one
    /// graph: its primary copy (d-21), else the first loaded.
    pub(crate) fn former_owner(
        &self,
        name: &str,
        shown: &dyn Fn(&str) -> bool,
    ) -> Result<Option<String>, Problem> {
        if self.graphs.contains_key(name) {
            return Ok(None);
        }
        let mut ids: Vec<Id> = self
            .graphs
            .iter()
            .filter(|(a, l)| shown(a) && l.meta.former_aliases.iter().any(|f| f == name))
            .map(|(_, l)| l.meta.id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        if ids.is_empty()
            || self
                .read_registry()
                .is_ok_and(|r| r.rows.iter().any(|row| row.alias == name))
        {
            return Ok(None);
        }
        let live = |id: Id| -> String {
            self.primary_of(id)
                .or_else(|| self.copies_of(id).first().copied())
                .expect("a loaded graph has a copy")
                .to_string()
        };
        match ids.as_slice() {
            [id] => Ok(Some(live(*id))),
            _ => Err(problems::former_alias_ambiguous(
                name,
                ids.len(),
                ids.iter()
                    .map(|id| live(*id))
                    .filter(|a| self.shown_unnamed(a))
                    .collect(),
            )),
        }
    }

    /// What a name a request gave means: a loaded alias as it is, else the live alias of the graph that had it as a
    /// former alias (recorded on `ctx` for the hint), else the name unchanged, which the caller reports as not
    /// loaded. Redirect first, then policy: the caller applies every rule to the live alias exactly as if it had
    /// been named (graph-addressing-expert).
    pub(crate) fn canonical(&self, ctx: &Ctx, name: &str) -> Result<String, Problem> {
        match self.former_owner(name, &|_| true)? {
            Some(live) => {
                ctx.redirect(name, &live);
                Ok(live)
            }
            None => Ok(name.to_string()),
        }
    }

    /// `selector` as an equality or policy check compares it: a former alias of a loaded graph as that graph's live
    /// alias (recorded on `ctx` for the hint, and looked up once per request), anything else as it is. Naming a
    /// graph by a former alias names it exactly as its live alias would, never more (graph-addressing-expert, review
    /// R10-R13). An ambiguous former alias stays as given, so the step that resolves it reports
    /// `former_alias_ambiguous`.
    pub(crate) fn live_in<'a>(&'a self, ctx: &Ctx, selector: &'a str) -> &'a str {
        if self.graphs.contains_key(selector) || selector.parse::<Id>().is_ok() {
            return selector;
        }
        let known = ctx
            .redirected
            .borrow()
            .iter()
            .find(|(former, _)| former == selector)
            .map(|(_, live)| live.clone());
        let live = known.or_else(|| self.former_owner(selector, &|_| true).ok().flatten());
        match live.and_then(|live| self.graphs.get_key_value(live.as_str())) {
            Some((live, _)) => {
                ctx.redirect(selector, live);
                live.as_str()
            }
            None => selector,
        }
    }

    /// The one graph named by the addresses among `refs`, if any, as [`Self::live_in`] reads it: two different
    /// graphs are cross-graph, a former and a live alias of one graph are one (review R12).
    pub(crate) fn addressed(
        &self,
        ctx: &Ctx,
        refs: &[Option<&str>],
    ) -> Result<Option<String>, Problem> {
        let mut found: Option<&str> = None;
        for selector in refs
            .iter()
            .flatten()
            .filter_map(|r| super::split_address(r).0)
        {
            let selector = self.live_in(ctx, selector);
            match found {
                Some(f) if f != selector => return Err(problems::cross_graph_not_yet()),
                _ => found = Some(selector),
            }
        }
        Ok(found.map(ToString::to_string))
    }

    /// Records, for the hint, each `umx:<former>/...` among `texts` that names an open graph by a former alias. A
    /// secured graph reached so is not recorded: its live alias is never shown to a request that did not name it.
    pub(crate) fn note_redirects(&self, ctx: &Ctx, texts: &[&str]) {
        for text in texts {
            if let (Some(selector), _) = super::split_address(text)
                && let Ok(Some(live)) = self.former_owner(selector, &|a| self.shown_unnamed(a))
            {
                ctx.redirect(selector, &live);
            }
        }
    }

    /// `umx graph rename <old> <new>` (ws006 S23, req-56; design note plan 209). In one write graph.json gets the
    /// new alias and keeps the old one in `former_aliases` (format 7), so `umx:<old>/<id>` keeps resolving
    /// everywhere; renaming back to a former alias takes it off the list (graph-addressing-expert (c)). Then this
    /// machine's memory, registry row and saved-scope hints follow, and the Graphfile entry and sets, if any (a
    /// Graphfile that cannot be updated is a warning, never a refusal: pm-001). All under the write lock: a reader
    /// sees the graph before or after, never half (architect).
    pub fn graph_rename(&mut self, ctx: &Ctx, old: &str, new: &str) -> Outcome<Renamed> {
        let old = self.canonical(ctx, old)?;
        if !self.graphs.contains_key(&old) {
            return Err(self.not_loaded(&old));
        }
        let old_name = alias_name(&old);
        let fix = |good: &ValidName| Command::new("graph rename").name(&old_name).name(good);
        let Some(name) = ValidName::alias(new) else {
            return Err(problems::alias_invalid(new, &fix));
        };
        if new == old {
            return Err(problems::usage_invalid(
                "the graph already has that alias",
                "graph",
            ));
        }
        let (id, dir) = (self.graphs[&old].meta.id, self.graphs[&old].dir.clone());
        let taken = self.graphs.contains_key(new)
            || self.read_registry().is_ok_and(|r| {
                r.rows
                    .iter()
                    .any(|row| row.alias == new && row.folder != dir.as_str())
            })
            || self
                .graphs
                .values()
                .any(|l| l.meta.id != id && l.meta.former_aliases.iter().any(|f| f == new));
        if taken {
            return Err(problems::alias_in_use(&name, fix(&second(&name))));
        }
        self.files_unchanged(&old)?;
        let files = self.ports.files.clone();
        let before = self.graphs[&old].meta.clone();
        let mut meta = before.clone();
        // A graph loaded `--as` another alias has two old names: graph.json's own, which pointers elsewhere use, and
        // this machine's (review R7). Both redirect; graph.json's is the older.
        let olds: Vec<String> = [before.alias.clone(), old.clone()]
            .into_iter()
            .filter(|a| a != new)
            .fold(Vec::new(), |mut v, a| {
                if !v.contains(&a) {
                    v.push(a);
                }
                v
            });
        meta.former_aliases
            .retain(|f| f != new && !olds.contains(f));
        meta.former_aliases.extend(olds);
        let mut dropped = Vec::new();
        while meta.former_aliases.len() > MAX_FORMER {
            dropped.push(meta.former_aliases.remove(0));
        }
        meta.alias = new.to_string();
        let l = self.graphs.get_mut(&old).expect("checked above");
        l.meta = meta;
        let mut warnings = match persist(
            files.as_ref(),
            &old,
            l,
            &[GraphFile::Graph],
            Touched::default(),
        ) {
            Ok(w) => w,
            Err(p) => {
                l.meta = before;
                return Err(p);
            }
        };
        let loaded = self.graphs.remove(&old).expect("checked above");
        let former_aliases = loaded.meta.former_aliases.clone();
        self.graphs.insert(new.to_string(), loaded);
        self.resets.insert(old.clone());
        self.resets.insert(new.to_string());
        warnings.extend(self.remember(new, true));
        warnings.extend(self.rename_scope_hints(id, &dir, new));
        let (graphfile, more) = self.rename_in_graphfile(&dir, id, &old, new);
        warnings.extend(more);
        Ok(Done {
            value: Renamed {
                graph: new.to_string(),
                graph_id: id,
                from: old,
                former_aliases,
                dropped,
                graphfile,
            },
            notes: warnings,
        })
    }

    /// Saved scopes stay keyed by graph id; their alias hint for this folder follows the rename, so a scope read
    /// does not warn `scope_member_moved`. A scope file umx cannot read or write is left as it is.
    fn rename_scope_hints(&self, id: Id, dir: &GraphDir, new: &str) -> Vec<Problem> {
        let Ok(mut scopes) = self.read_scopes() else {
            return Vec::new();
        };
        let mut changed = false;
        for m in scopes
            .scopes
            .values_mut()
            .flat_map(|s| s.members.iter_mut())
        {
            if m.id == id && m.folder == dir.as_str() && m.alias != new {
                m.alias = new.to_string();
                changed = true;
            }
        }
        if !changed {
            return Vec::new();
        }
        self.write_scopes(&scopes)
            .unwrap_or_else(|p| alloc::vec![p])
    }

    /// The Graphfile at `dir`'s repo top, if it lists this graph: its entry and set members renamed.
    fn rename_in_graphfile(
        &self,
        dir: &GraphDir,
        id: Id,
        old: &str,
        new: &str,
    ) -> (&'static str, Vec<Problem>) {
        let Ok(Some(top)) = self.repo_top(dir) else {
            return ("unchanged", Vec::new());
        };
        let retry = scan_cmd(&top);
        let mut clash = false;
        let updated = self.update_graphfile(&top, false, &retry, |gf| {
            clash = false;
            gf.rename(id, new).or_else(|_| {
                clash = true;
                Ok(false)
            })
        });
        match updated {
            Ok(Updated::Written) => ("updated", Vec::new()),
            Ok(Updated::Unchanged) if !clash => ("unchanged", Vec::new()),
            Ok(Updated::Unchanged) => (
                "not_updated",
                alloc::vec![problems::graphfile_not_updated(
                    &self.graphfile_path(&top),
                    old,
                    new,
                    retry
                )],
            ),
            Ok(Updated::Left(p)) | Err(p) => ("not_updated", alloc::vec![p]),
        }
    }

    /// `umx graph move <alias> <path>` (ws006 S23; design note plan 209, architect's order). Inside one git repo (or
    /// none): graph.json at the old place first records the repo-relative folder it is leaving in `former_paths`
    /// (format 7), then the folder moves in one rename (`GraphFiles::relocate`, never a copy), then the graph loads
    /// from the new place under its alias, and the registry row, saved-scope hints and the Graphfile entry follow.
    /// The reply lists this graph's `root:repo` pointers into the folder it left; `umx check` keeps naming them.
    pub fn graph_move(&mut self, ctx: &Ctx, alias: &str, to: &str) -> Outcome<Moved> {
        let alias = self.canonical(ctx, alias)?;
        if !self.graphs.contains_key(&alias) {
            return Err(self.not_loaded(&alias));
        }
        let graph = alias_name(&alias);
        let to = self.identity(to)?;
        let (from, id) = (self.graphs[&alias].dir.clone(), self.graphs[&alias].meta.id);
        if to == from {
            return Err(problems::usage_invalid(
                "the graph is already in that folder",
                "graph",
            ));
        }
        if to.is_within(&from) {
            return Err(problems::usage_invalid(
                "a graph cannot move inside its own folder",
                "graph",
            ));
        }
        let top = self.repo_top(&from)?;
        if let Some(top) = &top {
            let to_top = match parent(&to) {
                Some(p) => self.repo_top(&p).ok().flatten(),
                None => None,
            };
            if to_top.as_ref() != Some(top) {
                return Err(problems::graph_move_cross_repo(&graph, &to));
            }
        }
        self.files_unchanged(&alias)?;
        let rel = |dir: &GraphDir| top.as_ref().and_then(|t| relative(t, dir));
        let (old_rel, new_rel) = (rel(&from), rel(&to));
        let refused =
            |why: &'static str, fix: Command| problems::graph_move_refused(&graph, why, fix);
        let again = || Command::new("graph move").name(&graph).word("<path>");
        let files = self.ports.files.clone();
        let before = self.graphs[&alias].meta.clone();
        let mut warnings = Vec::new();
        // graph.json at the old place first (architect): a crash before the rename leaves an entry naming its own
        // path, which check ignores; never a moved graph that forgot where it was. A folder graph.json could not
        // record is refused before anything is written (review R1).
        if let Some(old_rel) = &old_rel {
            let mut meta = before.clone();
            meta.former_paths
                .retain(|p| p != old_rel && Some(p) != new_rel.as_ref());
            meta.former_paths.push(old_rel.clone());
            if meta.former_paths.len() > MAX_FORMER {
                meta.former_paths.remove(0);
            }
            if !meta.former_ok() {
                return Err(refused(
                    "graph.json cannot record the folder it leaves (its name is not a repo path)",
                    Command::new("graph load").path(&from),
                ));
            }
            let l = self.graphs.get_mut(&alias).expect("checked above");
            l.meta = meta;
            match persist(
                files.as_ref(),
                &alias,
                l,
                &[GraphFile::Graph],
                Touched::default(),
            ) {
                Ok(w) => warnings.extend(w),
                Err(p) => {
                    l.meta = before;
                    return Err(p);
                }
            }
        }
        // The graph stays loaded through the move (review R2-R4): its vectors, similar_to lists, queued work, copy
        // order, primary mark and registry row are this load's, and only its folder changes.
        let shards = self.secured_shards(&alias);
        if let Err(e) = files.relocate(&from, &to) {
            // Nothing moved: graph.json gets back what it held, so no former path is lost (review R5).
            if old_rel.is_some() {
                let l = self.graphs.get_mut(&alias).expect("checked above");
                l.meta = before;
                let _ = persist(
                    files.as_ref(),
                    &alias,
                    l,
                    &[GraphFile::Graph],
                    Touched::default(),
                );
            }
            return Err(match e {
                FilesError::Invalid(RELOCATE_TARGET_EXISTS) => {
                    refused("the new folder exists", again())
                }
                FilesError::Invalid(RELOCATE_PARENT_MISSING) => {
                    refused("the new folder's parent does not exist", again())
                }
                FilesError::CrossDevice => refused(
                    "it is on another file system",
                    Command::new("graph load").path(&to),
                ),
                e => super::graph::files_problem(&to, &self.roots, GraphFile::Graph, e),
            });
        }
        self.graphs.get_mut(&alias).expect("checked above").dir = to.clone();
        if let (Some(old), Some(new)) = (shards, self.secured_shards(&alias)) {
            // A secured graph's shards live in a cache keyed by folder: they follow it. If they cannot, the cache at
            // the new key is only incomplete, and a later load embeds what it lacks.
            let _ = self.ports.vector_cache.relocate(&old, &new);
        }
        self.resets.insert(alias.clone());
        self.move_registry_row(&from, &to);
        warnings.extend(self.move_scope_hints(id, &from, &to));
        let mut graphfile = "unchanged";
        if let (Some(top), Some(new_rel)) = (&top, &new_rel) {
            let retry = scan_cmd(top).word("--add");
            match self.update_graphfile(
                top,
                false,
                &retry,
                |gf| Ok(gf.move_to(id, new_rel.clone())),
            ) {
                Ok(Updated::Written) => graphfile = "updated",
                Ok(Updated::Unchanged) => {}
                Ok(Updated::Left(p)) | Err(p) => {
                    graphfile = "not_updated";
                    warnings.push(p);
                }
            }
        }
        let l = &self.graphs[&alias];
        let mut stale: Vec<(Id, String)> = old_rel
            .as_deref()
            .map(|old| {
                stale_pointers(&l.graph, &[old])
                    .into_iter()
                    .map(|(id, place, _)| (id, place))
                    .collect()
            })
            .unwrap_or_default();
        let stale_total = stale.len();
        if let (Some(old), Some(new)) = (&old_rel, &new_rel)
            && stale_total > 0
        {
            warnings.push(problems::evidence_root_stale(&graph, stale_total, old, new));
        }
        stale.truncate(STALE_SHOWN);
        Ok(Done {
            value: Moved {
                graph: alias.clone(),
                graph_id: id,
                from,
                to,
                former_paths: l.meta.former_paths.clone(),
                stale,
                stale_total,
                graphfile,
            },
            notes: warnings,
        })
    }

    /// The cache folder of a loaded graph's shards when they are not in its own folder (a graph that is not open).
    fn secured_shards(&self, alias: &str) -> Option<GraphDir> {
        let policy = self.effective_policy(alias)?;
        if policy == umx_core::Policy::OPEN {
            return None;
        }
        self.shard_home(&self.graphs[alias], policy)
            .ok()
            .map(|(_, dir)| dir)
    }

    /// The registry row for `from` now names `to`, so the moved graph keeps its place and leaves no stale row. A row
    /// already at `to` is of a graph that is no longer there (the folder did not exist, or now holds this graph), so
    /// it goes: one folder, one row (review R21).
    fn move_registry_row(&self, from: &GraphDir, to: &GraphDir) {
        if let Ok(mut registry) = self.read_registry() {
            let rows = registry.rows.len();
            registry.rows.retain(|r| r.folder != to.as_str());
            let mut changed = registry.rows.len() != rows;
            for row in registry
                .rows
                .iter_mut()
                .filter(|r| r.folder == from.as_str())
            {
                row.folder = to.as_str().to_string();
                changed = true;
            }
            if changed {
                let _ = self.try_write_registry(&registry);
            }
        }
    }

    /// Saved scopes' folder hint for this graph follows the move.
    fn move_scope_hints(&self, id: Id, from: &GraphDir, to: &GraphDir) -> Vec<Problem> {
        let Ok(mut scopes) = self.read_scopes() else {
            return Vec::new();
        };
        let mut changed = false;
        for m in scopes
            .scopes
            .values_mut()
            .flat_map(|s| s.members.iter_mut())
        {
            if m.id == id && m.folder == from.as_str() {
                m.folder = to.as_str().to_string();
                changed = true;
            }
        }
        if !changed {
            return Vec::new();
        }
        self.write_scopes(&scopes)
            .unwrap_or_else(|p| alloc::vec![p])
    }

    /// After a fresh load of `alias`: when this machine's registry knows its graph id only at other folders, none of
    /// which still holds that id, the graph was moved by hand (graph-addressing-expert (g)); the most recently loaded
    /// of those rows now names the new folder, and this graph's `root:repo` pointers into the old one come back once.
    /// Any other folder that still holds the id makes this a copy (d-21), never a move; so does one umx cannot look
    /// at (review R20). Only a row for this id at this folder means the move was already healed: a row there for
    /// another graph is not (review R21).
    pub(crate) fn heal_moved(&self, alias: &str) -> Option<Healed> {
        let l = &self.graphs[alias];
        let (id, dir) = (l.meta.id, l.dir.clone());
        let registry = self.read_registry().ok()?;
        let rows: Vec<&umx_core::registry::Row> =
            registry.rows.iter().filter(|r| r.id == id).collect();
        if rows.iter().any(|r| r.folder == dir.as_str()) {
            return None;
        }
        let holds = |folder: &GraphDir| match self.ports.files.peek_graph(folder) {
            Ok(None) => false,
            Ok(Some(found)) => {
                found.pending
                    || found
                        .graph_json
                        .as_deref()
                        .and_then(|b| umx_core::codec::decode_graph(b).ok())
                        .is_none_or(|m| m.id == id)
            }
            Err(_) => true,
        };
        let mut olds = Vec::new();
        for row in &rows {
            let old = GraphDir::parse(&row.folder).ok()?;
            if holds(&old) {
                return None;
            }
            olds.push(old);
        }
        let old = olds.pop()?;
        self.move_registry_row(&old, &dir);
        let top = self.repo_top(&dir).ok().flatten()?;
        let stale: Vec<(Id, String)> = relative(&top, &old)
            .map(|rel| {
                stale_pointers(&l.graph, &[rel.as_str()])
                    .into_iter()
                    .map(|(id, place, _)| (id, place))
                    .collect()
            })
            .unwrap_or_default();
        let total = stale.len();
        Some((old, stale.into_iter().take(STALE_SHOWN).collect(), total))
    }
}
