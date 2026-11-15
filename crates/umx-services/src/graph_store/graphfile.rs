//! The Graphfile in services (ws002 s17, D66, req-128): a repo's committed list of its graphs, at its git top. `graph
//! new` adds the new graph (unless `--no-graphfile`, or its policy is not open and `--graphfile` was not given);
//! `graph scan` lists what is on disk against it and, with `--add` or `--drop`, changes it in one write; `graph list
//! --graphfile` and `graph load --graphfile` read it. Every change is one compare-and-swap through the config files
//! port, retried once (security, s17). A file that is not valid is never used and never rewritten; one that is not in
//! umx's own form is rewritten only with `--rewrite`.

use super::{GraphStore, graph::files_problem};
use crate::problems;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use umx_core::codec::{self, GraphFile};
use umx_core::graphfile::{Clash, Entry, Error as GfError, Graphfile};
use umx_core::locator::repo_path;
use umx_core::ports::{ConfigFile, ConfigFiles, ConfigFilesError, FindLimits, GraphDir, Swapped};
use umx_core::{Command, Id, Policy, Problem, SafeText, Slot, ValidName};

/// How `graph new` treats the repo's Graphfile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GraphfileMode {
    /// Add an open graph; leave out one whose policy is not open (security, s17).
    #[default]
    Add,
    /// `--graphfile`: add it whatever its policy.
    Include,
    /// `--no-graphfile`.
    Skip,
}

/// The Graphfile line `graph new` added, and the command that removes it again (security, s17).
#[derive(Clone, Debug, PartialEq)]
pub struct GraphfileAdded {
    pub file: GraphDir,
    pub path: String,
    pub alias: String,
    pub undo: Command,
}

/// What `graph scan` should do besides list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanRequest {
    /// Write the new graphs and the moved paths.
    pub add: bool,
    /// Write even a Graphfile not in umx's own form (comments and order are lost).
    pub rewrite: bool,
    /// Remove this alias's entry.
    pub drop: Option<String>,
    /// How many folders down to look (default 8, at most 32).
    pub depth: Option<usize>,
    /// No folder was named: scan the whole repo from its top, wherever the folder given is inside it (R289-2).
    pub whole_repo: bool,
}

/// One row of `graph scan`. `state` is listed, new, missing, moved, id_mismatch, unreadable, pending,
/// outside_repo, duplicate_id or nested_repo.
#[derive(Clone, Debug, PartialEq)]
pub struct ScanRow {
    /// Relative to the repo top; `.` for the top.
    pub path: String,
    pub alias: Option<String>,
    pub id: Option<Id>,
    pub state: &'static str,
    /// moved: the path the Graphfile lists.
    pub from: Option<String>,
    /// id_mismatch: the id the folder holds.
    pub found_id: Option<Id>,
    /// new: its graph.json policy is not open, so `--add` leaves it out.
    pub secured: bool,
    /// nested_repo: the scan to run for that repository.
    pub next: Option<Command>,
    /// moved: Some(true) when the scan saw everything, so the id is at that one path only; None when the scan was
    /// incomplete (truncated, a folder it could not list, or a graph.json it could not read or that has a pending
    /// write, met by the walk or in an entry's own folder: F177), so it is not known and `--add` leaves the entry's
    /// path as it is (security, s17).
    pub unique: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scanned {
    pub repo: GraphDir,
    pub file: GraphDir,
    pub rows: Vec<ScanRow>,
    /// A walk limit was hit: there may be more graphs.
    pub truncated: bool,
    /// Graph folders found whose graph.json could not be read or whose path a Graphfile cannot hold, and folders
    /// the walk could not list (F177).
    pub unreadable: usize,
    pub added: usize,
    pub moved: usize,
    pub dropped: bool,
    pub written: bool,
}

/// One row of `graph list --graphfile`. `state` is loaded, not_loaded, missing or outside_repo.
#[derive(Clone, Debug, PartialEq)]
pub struct GraphfileRow {
    pub path: String,
    pub alias: String,
    pub id: Id,
    pub state: &'static str,
    /// loaded: the alias it is loaded under here.
    pub loaded_as: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GraphfileListing {
    pub repo: GraphDir,
    pub file: GraphDir,
    pub rows: Vec<GraphfileRow>,
    pub sets: BTreeMap<String, Vec<String>>,
}

/// What one change to the Graphfile came to.
pub(crate) enum Updated {
    Written,
    Unchanged,
    /// Left as it was, with the warning that says why.
    Left(Problem),
}

/// `dir` relative to `top` (`.` for the top itself), or None when it is not inside.
pub(super) fn relative(top: &GraphDir, dir: &GraphDir) -> Option<String> {
    if dir == top {
        return Some(".".into());
    }
    let rest = dir.as_str().strip_prefix(top.as_str())?;
    let rest = if top.as_str() == "/" {
        rest
    } else {
        rest.strip_prefix('/')?
    };
    Some(rest.to_string())
}

/// `rel` (repo-relative, `.` for the top) under `top`.
pub(crate) fn under(top: &GraphDir, rel: &str) -> Option<GraphDir> {
    if rel == "." {
        Some(top.clone())
    } else {
        top.join(rel).ok()
    }
}

pub(super) fn scan_cmd(top: &GraphDir) -> Command {
    Command::new("graph scan").path(top)
}

/// The longest alias is 64 characters (umx_core::name::is_snake_case).
const ALIAS_MAX: usize = 64;

/// `base` with the first `_2`, `_3`... that no entry has, cut short so the alias stays valid (R289-1); None if every
/// one up to the entry cap is taken.
fn free_alias(gf: &Graphfile, base: &str) -> Option<String> {
    let taken = |a: &str| gf.entries().iter().any(|e| e.alias == a);
    if !taken(base) {
        return Some(base.to_string());
    }
    (2..=umx_core::graphfile::MAX_ENTRIES + 2).find_map(|n| {
        let suffix = alloc::format!("_{n}");
        let keep = base.len().min(ALIAS_MAX.saturating_sub(suffix.len()));
        let alias = alloc::format!("{}{suffix}", base.get(..keep)?);
        (ValidName::alias(&alias).is_some() && !taken(&alias)).then_some(alias)
    })
}

/// What a Graphfile entry's own folder holds, looked at directly rather than through the walk, which may not have
/// reached it (R289-3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AtPath {
    /// No graph there: the only case in which an entry may be missing or moved.
    Absent,
    Graph(Id),
    /// A graph is there, or may be, but its graph.json could not be read.
    Unreadable,
    /// A graph is there with a write cut short: only a load recovers it, and a scan never does.
    Pending,
    /// The path leads out of the repo top (a link): never followed, and never moved.
    Outside,
}

/// A codec reason or an OS message, made safe for a slot.
fn said(why: &str) -> Slot {
    Slot::Os(SafeText::lossy(why))
}

fn unusable(file: &GraphDir, e: &ConfigFilesError, top: &GraphDir) -> Problem {
    let why = match e {
        ConfigFilesError::Refused { reason, .. } => Slot::Word(reason),
        ConfigFilesError::OutsideRoots(_) => Slot::Word("it is outside the folders umx may use"),
        ConfigFilesError::Io(m) => said(m),
    };
    problems::graphfile_unusable(file, why, top)
}

impl GraphStore {
    fn config_files(&self) -> Result<Arc<dyn ConfigFiles>, Problem> {
        self.config_files
            .clone()
            .ok_or_else(|| problems::internal("graphfile_unwired"))
    }

    /// Err(internal) when no config files port is wired, before a verb changes anything.
    pub(crate) fn graphfile_wired(&self) -> Result<(), Problem> {
        self.config_files().map(|_| ())
    }

    /// The git top of `dir` inside the roots, or None.
    /// The Graphfile's path at repo top `top`, for messages.
    pub(super) fn graphfile_path(&self, top: &GraphDir) -> GraphDir {
        self.config_files()
            .map(|cf| cf.path(&ConfigFile::Graphfile(top.clone())))
            .unwrap_or_else(|_| top.clone())
    }

    pub(crate) fn repo_top(&self, dir: &GraphDir) -> Result<Option<GraphDir>, Problem> {
        self.config_files()?.repo_root(dir).map_err(|e| match e {
            ConfigFilesError::Refused { file, reason } => {
                umx_core::config::problems::file_refused(&file, reason)
            }
            _ => problems::graphfile_repo_none(dir),
        })
    }

    /// The git top of the folder at `path`, refused when there is none.
    pub(crate) fn repo_top_of(&self, path: &str) -> Result<GraphDir, Problem> {
        let dir = self.folder(path)?;
        self.repo_top(&dir)?
            .ok_or_else(|| problems::graphfile_repo_none(&dir))
    }

    /// The repo's Graphfile, for list and load: it must be there and valid, or nothing from it is used.
    pub(crate) fn graphfile_for_use(
        &self,
        top: &GraphDir,
    ) -> Result<(GraphDir, Graphfile), Problem> {
        let cf = self.config_files()?;
        let file = ConfigFile::Graphfile(top.clone());
        let path = cf.path(&file);
        let bytes = match cf.read(&file) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Err(problems::graphfile_missing(top)),
            Err(ConfigFilesError::Refused { file, reason }) => {
                return Err(umx_core::config::problems::file_refused(&file, reason));
            }
            Err(e) => {
                return Err(problems::graphfile_invalid(&path, &e.to_string(), top));
            }
        };
        match Graphfile::decode(&bytes) {
            Ok(g) => Ok((path, g)),
            Err(GfError::Newer(v)) => Err(problems::graphfile_newer(&path, v)),
            Err(e) => Err(problems::graphfile_invalid(&path, &e.reason(), top)),
        }
    }

    /// One change to the repo's Graphfile: read it, apply `change` (Ok(true) when it changed something), encode,
    /// and swap it in only over the bytes read; when another writer got there first, all of it once more, then a
    /// `graphfile_changed` warning naming `retry`. A file that is not valid, or not in umx's own form without
    /// `rewrite`, is left with its warning; a missing one is created only when there is a change. An Err from
    /// `change` stops it with nothing written.
    pub(crate) fn update_graphfile(
        &self,
        top: &GraphDir,
        rewrite: bool,
        retry: &Command,
        mut change: impl FnMut(&mut Graphfile) -> Result<bool, Problem>,
    ) -> Result<Updated, Problem> {
        let cf = self.config_files()?;
        let file = ConfigFile::Graphfile(top.clone());
        let path = cf.path(&file);
        for _ in 0..2 {
            let before = match cf.read(&file) {
                Ok(bytes) => bytes,
                Err(e) => return Ok(Updated::Left(unusable(&path, &e, top))),
            };
            let mut gf = match &before {
                None => Graphfile::new(),
                Some(bytes) => match Graphfile::decode(bytes) {
                    Ok(g) if rewrite || Graphfile::is_canonical(bytes) => g,
                    Ok(_) => {
                        return Ok(Updated::Left(problems::graphfile_not_canonical(&path, top)));
                    }
                    Err(e) => {
                        return Ok(Updated::Left(problems::graphfile_unusable(
                            &path,
                            said(&e.reason()),
                            top,
                        )));
                    }
                },
            };
            let changed = change(&mut gf)?;
            let bytes = match gf.encode() {
                Ok(bytes) => bytes,
                Err(e) => {
                    return Ok(Updated::Left(problems::graphfile_too_large(
                        &path,
                        &e.reason(),
                    )));
                }
            };
            // Never commit a Graphfile umx's own reader would refuse (R289-1): it would make every entry unusable.
            if let Err(e) = Graphfile::decode(&bytes) {
                return Ok(Updated::Left(problems::graphfile_unusable(
                    &path,
                    said(&alloc::format!(
                        "umx would write a file it cannot read: {}",
                        e.reason()
                    )),
                    top,
                )));
            }
            if before.as_deref() == Some(bytes.as_slice()) || (before.is_none() && !changed) {
                return Ok(Updated::Unchanged);
            }
            match cf.swap(&file, before.as_deref(), &bytes) {
                Ok(Swapped::Done) => return Ok(Updated::Written),
                Ok(Swapped::Changed) => {}
                Err(e) => return Ok(Updated::Left(unusable(&path, &e, top))),
            }
        }
        Ok(Updated::Left(problems::graphfile_changed(
            &path,
            retry.clone(),
        )))
    }

    /// After `graph new` made `alias`: its Graphfile line, or the warning that says why there is none. Never fails
    /// the verb, which has already made the graph.
    pub(crate) fn graphfile_add_new(
        &self,
        alias: &str,
        mode: GraphfileMode,
    ) -> (Option<GraphfileAdded>, Vec<Problem>) {
        let (Some(l), Some(name)) = (self.graphs.get(alias), ValidName::alias(alias)) else {
            return (None, Vec::new());
        };
        if mode == GraphfileMode::Skip {
            return (None, Vec::new());
        }
        let (dir, id) = (l.dir.clone(), l.meta.id);
        let top = match self.repo_top(&dir) {
            Ok(Some(top)) => top,
            // Only an explicit --graphfile is told: the default must not warn on every graph made outside a repo.
            Ok(None) if mode == GraphfileMode::Include => {
                return (None, vec![problems::graphfile_no_repo(&name, &dir)]);
            }
            Ok(None) => return (None, Vec::new()),
            Err(p) => return (None, vec![p]),
        };
        if mode == GraphfileMode::Add && self.effective_policy(alias) != Some(Policy::OPEN) {
            return (None, vec![problems::graphfile_skipped_secured(alias)]);
        }
        let Ok(cf) = self.config_files() else {
            return (None, Vec::new());
        };
        let file = cf.path(&ConfigFile::Graphfile(top.clone()));
        let rel = relative(&top, &dir).filter(|r| repo_path(r).ok().as_deref() == Some(r.as_str()));
        let Some(rel) = rel else {
            let why = Slot::Word("the folder of the graph has a name a Graphfile path cannot hold");
            return (None, vec![problems::graphfile_unusable(&file, why, &top)]);
        };
        let retry = scan_cmd(&top).word("--add");
        let mut clash = None;
        let updated = self.update_graphfile(&top, false, &retry, |gf| {
            clash = None;
            match gf.add(Entry::new(rel.clone(), alias.to_string(), id)) {
                Ok(()) => Ok(true),
                Err(c) => {
                    clash = Some(c);
                    Ok(false)
                }
            }
        });
        match updated {
            Ok(Updated::Written) => (
                Some(GraphfileAdded {
                    file,
                    path: rel,
                    alias: alias.to_string(),
                    undo: scan_cmd(&top).word("--drop").name(&name),
                }),
                Vec::new(),
            ),
            Ok(Updated::Unchanged) => {
                let why = match clash {
                    Some(Clash::Path) => "it already lists another graph at that folder",
                    Some(Clash::Alias) => "it already lists another graph with that alias",
                    Some(Clash::Id) | None => "it already lists that graph",
                };
                (
                    None,
                    vec![problems::graphfile_unusable(&file, Slot::Word(why), &top)],
                )
            }
            Ok(Updated::Left(p)) | Err(p) => (None, vec![p]),
        }
    }

    /// `graph scan`: the graph folders under `folder` (its repo's top when the CLI gives none) against the repo's
    /// Graphfile; reads only each graph.json's id, alias and policy, and loads nothing. With `add`, `drop` or
    /// `rewrite`, one write.
    pub fn graph_scan(
        &self,
        retry: Option<&Command>,
        folder: &str,
        req: &ScanRequest,
    ) -> crate::Outcome<Scanned> {
        let cf = self.config_files()?;
        let given = self.folder(folder)?;
        let top = self
            .repo_top(&given)?
            .ok_or_else(|| problems::graphfile_repo_none(&given))?;
        let dir = if req.whole_repo { top.clone() } else { given };
        let file = cf.path(&ConfigFile::Graphfile(top.clone()));
        let mut warnings = Vec::new();
        // A missing file lists nothing yet; one umx cannot use is warned, and every graph found shows as new.
        let listed = match cf.read(&ConfigFile::Graphfile(top.clone())) {
            Ok(None) => Graphfile::new(),
            Ok(Some(bytes)) => Graphfile::decode(&bytes).unwrap_or_else(|e| {
                warnings.push(problems::graphfile_unusable(&file, said(&e.reason()), &top));
                Graphfile::new()
            }),
            Err(e) => {
                warnings.push(unusable(&file, &e, &top));
                Graphfile::new()
            }
        };
        let usable = warnings.is_empty();
        let limits = req
            .depth
            .map_or_else(FindLimits::default, FindLimits::with_depth);
        let found = self
            .ports
            .files
            .find_graphs(&dir, limits)
            .map_err(|e| files_problem(&dir, &self.roots, GraphFile::Graph, e))?;
        let prefix = relative(&top, &dir).unwrap_or_else(|| ".".into());
        let repo_rel = |p: &str| match (prefix.as_str(), p) {
            (".", p) => p.to_string(),
            (pre, ".") => pre.to_string(),
            (pre, p) => alloc::format!("{pre}/{p}"),
        };
        let mut disk: Vec<(String, Id, String, bool)> = Vec::new();
        // A folder the walk could not list hides what it holds, as an unreadable graph.json does (F177).
        let mut unreadable = found.unlisted;
        let mut pending: Vec<(String, Option<(Id, String)>)> = Vec::new();
        for g in &found.graphs {
            let path = repo_rel(&g.path);
            let meta = g
                .graph_json
                .as_deref()
                .and_then(|b| codec::decode_graph(b).ok());
            if g.pending {
                pending.push((path, meta.map(|m| (m.id, m.alias.clone()))));
                continue;
            }
            match meta {
                Some(meta) if repo_path(&path).ok().as_deref() == Some(path.as_str()) => {
                    let open = meta.file_policy() == Policy::OPEN;
                    disk.push((path, meta.id, meta.alias.clone(), open));
                }
                _ => unreadable += 1,
            }
        }
        let inside =
            |p: &str| prefix == "." || p == prefix || p.starts_with(&alloc::format!("{prefix}/"));
        let at_path = |rel: &str| {
            let d = match self.entry_dir(&top, rel) {
                Ok(d) => d,
                Err("missing") => return AtPath::Absent,
                Err(_) => return AtPath::Outside,
            };
            // Never GraphFiles::read: that recovers a cut-short write, and a scan only looks (graph-storage-expert).
            match self.ports.files.peek_graph(&d) {
                Ok(None) => AtPath::Absent,
                Ok(Some(g)) if g.pending => AtPath::Pending,
                Ok(Some(g)) => g
                    .graph_json
                    .as_deref()
                    .and_then(|b| codec::decode_graph(b).ok())
                    .map_or(AtPath::Unreadable, |m| AtPath::Graph(m.id)),
                Err(_) => AtPath::Unreadable,
            }
        };
        let mut rows = classify(&listed, &disk, inside, at_path);
        // Unique only when nothing was hidden: no limit hit, no folder or graph.json unread, and no pending write,
        // whether the walk met it or only an entry's own folder did (F177).
        let complete = !found.truncated
            && unreadable == 0
            && pending.is_empty()
            && !rows
                .iter()
                .any(|r| matches!(r.state, "pending" | "unreadable"));
        for r in rows.iter_mut().filter(|r| r.state == "moved") {
            r.unique = complete.then_some(true);
        }
        for (path, meta) in pending {
            if !rows.iter().any(|r| r.path == path) {
                rows.push(ScanRow {
                    path,
                    alias: meta.as_ref().map(|m| m.1.clone()),
                    id: meta.map(|m| m.0),
                    state: "pending",
                    from: None,
                    found_id: None,
                    secured: false,
                    next: None,
                    unique: None,
                });
            }
        }
        for nested in &found.nested_repos {
            rows.push(ScanRow {
                path: repo_rel(nested),
                alias: None,
                id: None,
                state: "nested_repo",
                from: None,
                found_id: None,
                secured: false,
                next: dir.join(nested).ok().map(|d| scan_cmd(&d)),
                unique: None,
            });
        }
        rows.sort_by(|a, b| a.path.cmp(&b.path));
        let mut scanned = Scanned {
            repo: top.clone(),
            file: file.clone(),
            rows,
            truncated: found.truncated,
            unreadable,
            added: 0,
            moved: 0,
            dropped: false,
            written: false,
        };
        if usable && (req.add || req.rewrite || req.drop.is_some()) {
            let adds: Vec<&ScanRow> = scanned
                .rows
                .iter()
                .filter(|r| r.state == "new" && req.add)
                .collect();
            for r in adds.iter().filter(|r| r.secured) {
                warnings.push(problems::graphfile_skipped_secured(
                    r.alias.as_deref().unwrap_or_default(),
                ));
            }
            let adds: Vec<Entry> = adds
                .iter()
                .filter(|r| !r.secured)
                .filter_map(|r| {
                    let alias = r.alias.clone().filter(|a| ValidName::alias(a).is_some());
                    Some(Entry::new(
                        r.path.clone(),
                        alias.unwrap_or_else(|| "graph".into()),
                        r.id?,
                    ))
                })
                .collect();
            let moves: Vec<(Id, String)> = scanned
                .rows
                .iter()
                .filter(|r| r.state == "moved" && r.unique == Some(true) && req.add)
                .filter_map(|r| Some((r.id?, r.path.clone())))
                .collect();
            let own = scan_cmd(&top).word("--add");
            let retry = retry.cloned().unwrap_or(own);
            let (mut added, mut moved, mut dropped) = (0, 0, false);
            let mut stored: BTreeMap<String, String> = BTreeMap::new();
            let updated = self.update_graphfile(&top, req.rewrite, &retry, |gf| {
                (added, moved, dropped) = (0, 0, false);
                stored.clear();
                if let Some(alias) = &req.drop {
                    if !gf.remove(alias) {
                        return Err(problems::graphfile_unknown(&file, "graph", alias, &top));
                    }
                    dropped = true;
                }
                for (id, path) in &moves {
                    moved += usize::from(gf.move_to(*id, path.clone()));
                }
                for entry in &adds {
                    let Some(alias) = free_alias(gf, &entry.alias) else {
                        continue;
                    };
                    let entry = Entry {
                        alias,
                        ..entry.clone()
                    };
                    let (path, alias) = (entry.path.clone(), entry.alias.clone());
                    if gf.add(entry).is_ok() {
                        added += 1;
                        stored.insert(path, alias);
                    }
                }
                Ok(dropped || added > 0 || moved > 0)
            })?;
            match updated {
                Updated::Written => {
                    (scanned.added, scanned.moved, scanned.dropped) = (added, moved, dropped);
                    scanned.written = true;
                    // The reply names each new graph by the alias the Graphfile now holds (graph-addressing-expert).
                    for r in scanned.rows.iter_mut().filter(|r| r.state == "new") {
                        if let Some(alias) = stored.get(&r.path) {
                            r.alias = Some(alias.clone());
                        }
                    }
                }
                Updated::Unchanged => {}
                Updated::Left(p) => warnings.push(p),
            }
        }
        Ok(crate::Done {
            value: scanned,
            notes: warnings,
        })
    }

    /// Where a Graphfile entry's folder is: resolved, and still inside the repo top (a tampered Graphfile names only
    /// its own repo's graphs; architect and security, s17). Err: the state to show (outside_repo, missing).
    pub(crate) fn entry_dir(&self, top: &GraphDir, path: &str) -> Result<GraphDir, &'static str> {
        let given = under(top, path).ok_or("missing")?;
        let resolved = self
            .ports
            .files
            .resolve(&given)
            .map_err(|_| "outside_repo")?;
        if resolved.is_within(top) {
            Ok(resolved)
        } else {
            Err("outside_repo")
        }
    }

    /// The alias and id of the graph loaded from `dir`, if one is.
    pub(crate) fn loaded_at(&self, dir: &GraphDir) -> Option<(String, Id)> {
        self.graphs
            .iter()
            .find(|(_, l)| l.dir == *dir)
            .map(|(a, l)| (a.clone(), l.meta.id))
    }

    /// `graph list --graphfile`: every entry of the repo's Graphfile, loaded here or not, or missing.
    pub fn graph_list_graphfile(&self, folder: &str) -> Result<GraphfileListing, Problem> {
        let top = self.repo_top_of(folder)?;
        let (file, gf) = self.graphfile_for_use(&top)?;
        let rows = gf
            .entries()
            .iter()
            .map(|e| {
                let (state, loaded_as) = match self.entry_dir(&top, &e.path) {
                    Err(state) => (state, None),
                    Ok(d) => match self.loaded_at(&d) {
                        Some((alias, _)) => ("loaded", Some(alias)),
                        None if self
                            .ports
                            .files
                            .list(&d)
                            .is_ok_and(|f| f.contains(&GraphFile::Graph)) =>
                        {
                            ("not_loaded", None)
                        }
                        None => ("missing", None),
                    },
                };
                GraphfileRow {
                    path: e.path.clone(),
                    alias: e.alias.clone(),
                    id: e.id,
                    state,
                    loaded_as,
                }
            })
            .collect();
        Ok(GraphfileListing {
            repo: top,
            file,
            rows,
            sets: gf.sets().clone(),
        })
    }
}

/// Each Graphfile entry under the scanned folder against what is on disk there (path, id, alias, open), then what is
/// on disk that no entry accounts for.
fn classify(
    listed: &Graphfile,
    disk: &[(String, Id, String, bool)],
    inside: impl Fn(&str) -> bool,
    at_path: impl Fn(&str) -> AtPath,
) -> Vec<ScanRow> {
    let row = |path: &str, alias: &str, id: Id, state: &'static str| ScanRow {
        path: path.to_string(),
        alias: Some(alias.to_string()),
        id: Some(id),
        state,
        from: None,
        found_id: None,
        secured: false,
        next: None,
        unique: None,
    };
    let mut rows = Vec::new();
    let mut used: BTreeSet<usize> = BTreeSet::new();
    for e in listed.entries().iter().filter(|e| inside(&e.path)) {
        if let Some(i) = disk.iter().position(|d| d.0 == e.path) {
            used.insert(i);
            if disk[i].1 == e.id {
                rows.push(row(&e.path, &e.alias, e.id, "listed"));
            } else {
                rows.push(ScanRow {
                    found_id: Some(disk[i].1),
                    ..row(&e.path, &e.alias, e.id, "id_mismatch")
                });
            }
            continue;
        }
        // The walk did not see it, which proves nothing when it stopped at a cap or could not read a folder: its
        // own folder is looked at, and only an entry with no graph there is missing or moved (R289-3).
        match at_path(&e.path) {
            AtPath::Absent => {}
            AtPath::Graph(id) if id == e.id => {
                rows.push(row(&e.path, &e.alias, e.id, "listed"));
                continue;
            }
            AtPath::Graph(id) => {
                rows.push(ScanRow {
                    found_id: Some(id),
                    ..row(&e.path, &e.alias, e.id, "id_mismatch")
                });
                continue;
            }
            AtPath::Unreadable => {
                rows.push(row(&e.path, &e.alias, e.id, "unreadable"));
                continue;
            }
            AtPath::Pending => {
                rows.push(row(&e.path, &e.alias, e.id, "pending"));
                continue;
            }
            AtPath::Outside => {
                rows.push(row(&e.path, &e.alias, e.id, "outside_repo"));
                continue;
            }
        }
        // Moved only when its id is at exactly one path no entry lists (graph-addressing-expert, s17).
        let elsewhere: Vec<usize> = disk
            .iter()
            .enumerate()
            .filter(|(i, d)| d.1 == e.id && listed.by_path(&d.0).is_none() && !used.contains(i))
            .map(|(i, _)| i)
            .collect();
        if let [i] = elsewhere.as_slice() {
            used.insert(*i);
            rows.push(ScanRow {
                from: Some(e.path.clone()),
                ..row(&disk[*i].0, &e.alias, e.id, "moved")
            });
        } else {
            rows.push(row(&e.path, &e.alias, e.id, "missing"));
        }
    }
    let mut new_ids: BTreeSet<Id> = BTreeSet::new();
    for (i, (path, id, alias, open)) in disk.iter().enumerate() {
        if used.contains(&i) {
            continue;
        }
        let again = listed.by_id(*id).is_some() || !new_ids.insert(*id);
        rows.push(ScanRow {
            secured: !open,
            ..row(path, alias, *id, if again { "duplicate_id" } else { "new" })
        });
    }
    rows
}
