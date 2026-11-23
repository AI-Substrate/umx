//! A graph's embedding policy (ws004 s06 A2, D83). graph.json states one (`open` when the section or key is missing);
//! this machine's record (the LocalState port) holds the last one umx recorded here, by graph id and by folder. umx
//! acts on the stricter of the two, so an edit or a pull can raise a graph's policy at once but never lowers it: a
//! umx call does, in one step by any caller (`graph set` lowering it, or `graph confirm-policy` for a file lowered
//! outside umx), and the reply warns `policy_lowered` with what loosened and who may now get the graph's text.
//! Every downgrade is recorded with its time. A record that cannot be trusted or read counts every graph as secured,
//! and is never rewritten. A secured policy this daemon could not record yet is held in memory (and retried on every
//! write), and umx refuses to lower graph.json's policy until the higher one is recorded.

use super::resolve::Evidence;
use super::{Ctx, Done, GraphStore, Outcome, alias_name, problems};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::local_state::PolicyState;
use umx_core::ports::{FileRef, LocalStateError, StateWritten};
use umx_core::vectors::ShardKind;
use umx_core::{Policy, Problem};

/// What a loaded graph's policy is, as of its last load, `graph set` or `graph confirm-policy`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyView {
    /// What graph.json says.
    pub file: Policy,
    /// What umx acts on: the stricter of the file and this machine's record; secured when the record is unusable.
    pub effective: Policy,
    /// When a umx call last lowered it on this machine, shown while the graph is open.
    pub downgraded_at: Option<String>,
}

impl PolicyView {
    /// Before the record is read: the file's word only.
    pub(crate) fn file_only(file: Policy) -> PolicyView {
        PolicyView {
            file,
            effective: file,
            downgraded_at: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PolicyConfirmed {
    pub alias: String,
    /// The policy umx acted on before.
    pub was: Policy,
    pub policy: Policy,
    /// Whether this machine's record changed.
    pub changed: bool,
}

/// One request's read of this machine's record ([`GraphStore::shown_unnamed_in`]), shared by every clone of its
/// [`Ctx`]: the store's write count when read, and the record or why it cannot be used.
#[derive(Clone, Debug, Default)]
pub struct PolicyMemo(alloc::rc::Rc<core::cell::RefCell<Option<MemoRead>>>);

type MemoRead = (u64, Result<PolicyState, String>);

impl GraphStore {
    /// This machine's record, with the secured policies this daemon could not write yet merged in (they count as
    /// recorded). Err carries why the record cannot be used (umx's words, never the file's contents). The bool says
    /// whether anything was merged, so the next write carries it.
    pub(super) fn read_state(&self) -> Result<(PolicyState, bool), String> {
        let mut state = match self.ports.state.read() {
            Ok(None) => PolicyState::default(),
            Ok(Some(bytes)) => PolicyState::decode(&bytes).map_err(|e| e.reason())?,
            Err(LocalStateError::Refused { reason, .. }) => return Err(reason.to_string()),
            Err(LocalStateError::Io(m)) => return Err(m),
        };
        let now = self.now();
        let mut merged = false;
        for (id, folder, policy) in &self.unsaved {
            merged |= state.raise(*id, folder, *policy, &now);
        }
        Ok((state, merged))
    }

    /// Writes the record. Ok(None): written and synced, and nothing is held in memory any more; Ok(Some(why)): in
    /// place but the folder sync failed; Err: not written, and what was held stays held.
    fn write_state(&mut self, state: &PolicyState) -> Result<Option<String>, String> {
        self.state_writes += 1;
        let written = match self.ports.state.write(&state.encode()) {
            Ok(StateWritten::Durable) => None,
            Ok(StateWritten::NotSynced(why)) => Some(why),
            Err(e) => return Err(e.to_string()),
        };
        self.unsaved.clear();
        Ok(written)
    }

    /// The policy umx acts on for a loaded graph, read fresh from this machine's record (secured if it cannot be
    /// used). Call it right before any graph text goes to an embedder. None if the alias is not loaded.
    pub fn effective_policy(&self, alias: &str) -> Option<Policy> {
        let l = self.graphs.get(alias)?;
        let file = l.meta.file_policy();
        Some(match self.read_state() {
            Ok((state, _)) => file.join(state.recorded(l.meta.id, l.dir.as_str()).unwrap_or(file)),
            Err(_) => Policy::SECURED,
        })
    }

    /// [`shown_unnamed`](Self::shown_unnamed) for one request: the record is read once per request (`ctx.policy`)
    /// rather than once per edge or end, so a read over many rows does not reopen the file for each. After a write of
    /// the record by this store, or while secured policies are held unsaved, it is read fresh as before.
    pub(crate) fn shown_unnamed_in(&self, ctx: &Ctx, alias: &str) -> bool {
        let Some(l) = self.graphs.get(alias) else {
            return false;
        };
        if !self.unsaved.is_empty() {
            return self.shown_unnamed(alias);
        }
        let mut memo = ctx.policy.0.borrow_mut();
        if memo
            .as_ref()
            .is_none_or(|(writes, _)| *writes != self.state_writes)
        {
            *memo = Some((self.state_writes, self.read_state().map(|(state, _)| state)));
        }
        let file = l.meta.file_policy();
        let policy = match memo.as_ref().map(|(_, state)| state) {
            Some(Ok(state)) => file.join(state.recorded(l.meta.id, l.dir.as_str()).unwrap_or(file)),
            _ => Policy::SECURED,
        };
        policy.visibility == umx_core::Visibility::Open
    }

    /// Whether reads that do not name the graph may show it (d-20, D53): its effective visibility is open. Not loaded,
    /// or a record that cannot be used, is not shown.
    pub fn shown_unnamed(&self, alias: &str) -> bool {
        self.effective_policy(alias)
            .is_some_and(|p| p.visibility == umx_core::Visibility::Open)
    }

    /// The loaded aliases whose effective visibility is open ([`shown_unnamed`](Self::shown_unnamed)).
    pub(crate) fn open_aliases(&self) -> BTreeSet<String> {
        self.graphs
            .keys()
            .filter(|a| self.shown_unnamed(a))
            .cloned()
            .collect()
    }

    /// The graphs a write to `own` may check `umx:` evidence against ([`check_evidence`](super::check_evidence)):
    /// `own` itself, and every other loaded graph whose effective visibility is open. A secured graph is left out, so
    /// it reads as not loaded (evidence_unchecked): no existence answer, no candidates and no full id from it reach
    /// another graph's evidence (security, B1, d-20).
    ///
    /// Each value carries the graph's live alias. A former alias of one of these graphs (ws006 S23) is a key too,
    /// unless [`GraphStore::former_owner`] keeps it from redirecting, so `umx:<former>/<id>` checks against its
    /// graph and is stored under the live alias; a secured graph's former alias is never a key.
    pub(crate) fn evidence_graphs(&self, own: &str) -> BTreeMap<&str, Evidence<'_>> {
        let shown = |a: &str| a == own || self.shown_unnamed(a);
        let mut out: BTreeMap<&str, Evidence<'_>> = self
            .graphs
            .iter()
            .filter(|(a, _)| shown(a))
            .map(|(a, l)| (a.as_str(), Evidence::Graph(l.meta.id, &l.graph, a.as_str())))
            .collect();
        for (a, l) in self.graphs.iter().filter(|(a, _)| shown(a)) {
            for former in &l.meta.former_aliases {
                match self.former_owner(former, &shown) {
                    Ok(Some(live)) if live == *a => {
                        out.insert(
                            former.as_str(),
                            Evidence::Graph(l.meta.id, &l.graph, a.as_str()),
                        );
                    }
                    // Two shown graphs had it (review R16): known, never unchecked.
                    Err(_) => {
                        out.insert(former.as_str(), Evidence::Ambiguous);
                    }
                    _ => {}
                }
            }
        }
        out
    }

    /// Before `graph set` lowers graph.json's policy from `before`: the record must already hold `before` for this
    /// graph, or be written with it now. Otherwise the change is refused (`policy_not_recorded`) and nothing is
    /// written, so a failed record can never turn into an unrecorded downgrade. Ok carries a durability warning.
    pub(crate) fn record_before_lowering(
        &mut self,
        alias: &str,
        before: Policy,
    ) -> Result<Vec<Problem>, Problem> {
        let l = &self.graphs[alias];
        let (id, folder) = (l.meta.id, l.dir.as_str().to_string());
        let path = self.ports.state.path();
        let (mut state, merged) = self
            .read_state()
            .map_err(|why| problems::policy_not_recorded(&path, &why))?;
        let raised = state
            .recorded(id, &folder)
            .is_none_or(|r| before.exceeds(r))
            && state.raise(id, &folder, before, &self.now());
        if !(raised || merged) {
            return Ok(Vec::new());
        }
        match self.write_state(&state) {
            Ok(None) => Ok(Vec::new()),
            Ok(Some(why)) => Ok(alloc::vec![problems::local_state_not_durable(&path, &why)]),
            Err(why) => Err(problems::policy_not_recorded(&path, &why)),
        }
    }

    /// Works out a loaded graph's policy, records a secured file policy for this copy (a raise needs no call),
    /// and returns the warnings: `policy_downgrade_unconfirmed`, `local_state_unreadable`, `local_state_not_saved`
    /// (held secured in memory until a later write lands) and `local_state_not_durable`.
    pub(crate) fn settle_policy(&mut self, alias: &str) -> Vec<Problem> {
        let l = &self.graphs[alias];
        let (id, folder, file) = (l.meta.id, l.dir.as_str().to_string(), l.meta.file_policy());
        let path = self.ports.state.path();
        let mut warnings = Vec::new();
        let view = match self.read_state() {
            Err(reason) => {
                warnings.push(problems::local_state_unreadable(&path, &reason));
                PolicyView {
                    file,
                    effective: Policy::SECURED,
                    downgraded_at: None,
                }
            }
            Ok((mut state, merged)) => {
                let recorded = state.recorded(id, &folder);
                let raised = file != Policy::OPEN && state.raise(id, &folder, file, &self.now());
                if raised || merged {
                    match self.write_state(&state) {
                        Ok(None) => {}
                        Ok(Some(why)) => {
                            warnings.push(problems::local_state_not_durable(&path, &why));
                        }
                        Err(why) => {
                            if raised {
                                match self.unsaved.iter_mut().find(|u| u.0 == id && u.1 == folder) {
                                    Some(u) => u.2 = u.2.join(file),
                                    None => self.unsaved.push((id, folder.clone(), file)),
                                }
                            }
                            warnings.push(problems::local_state_not_saved(
                                &alias_name(alias),
                                &path,
                                &why,
                            ));
                        }
                    }
                }
                if let Some(r) = recorded.filter(|r| r.exceeds(file)) {
                    warnings.push(problems::policy_downgrade_unconfirmed(
                        &alias_name(alias),
                        r,
                        file,
                    ));
                }
                let effective = file.join(recorded.unwrap_or(file));
                PolicyView {
                    file,
                    effective,
                    downgraded_at: (effective != Policy::SECURED)
                        .then(|| state.last_downgrade(id, &folder))
                        .flatten(),
                }
            }
        };
        // Any graph that is not fully open keeps its shards in this machine's cache, as a secured one does.
        let secured = view.effective != Policy::OPEN;
        self.graphs
            .get_mut(alias)
            .expect("settled graphs are loaded")
            .policy = view;
        if secured {
            warnings.extend(self.secured_shards_remain(alias));
        }
        warnings
    }

    /// A graph that is not fully open whose own folder still holds vector shards written while it was open (git
    /// tracks them if the folder is in a repository; umx does not check):
    /// new vectors go to the local cache, but these stay where they are until a person moves them (security gate e).
    /// Names the files, counted; never their content.
    fn secured_shards_remain(&self, alias: &str) -> Vec<Problem> {
        let dir = &self.graphs[alias].dir;
        let files = &self.ports.files;
        let graph = alias_name(alias);
        // An inspection that fails is said, never read as "none remain" (R91-2).
        let unchecked = |e: umx_core::ports::FilesError| {
            alloc::vec![problems::secured_shards_unchecked(&graph, &e)]
        };
        let spaces = match files.list_vector_spaces(dir) {
            Ok(spaces) => spaces,
            Err(e) => return unchecked(e),
        };
        let mut shards = Vec::new();
        // Vector shards, similar_to shards and their `_meta.tsv`: a neighbour list reveals relationships (ws004 s09).
        for key in spaces {
            for kind in [ShardKind::Vectors, ShardKind::Similar] {
                match files.list_shards(dir, &key, kind) {
                    Ok(found) => shards.extend(
                        found
                            .into_iter()
                            .map(|s| alloc::format!("{}/{}", s.folder(), s.file_name())),
                    ),
                    Err(e) => return unchecked(e),
                }
            }
            let meta = FileRef::SimilarMeta(key);
            match files.read_ref(dir, &meta) {
                Ok(Some(_)) => shards.push(meta.path()),
                Ok(None) => {}
                Err(e) => return unchecked(e),
            }
        }
        if shards.is_empty() {
            return Vec::new();
        }
        alloc::vec![problems::secured_shards_remain(&graph, &shards)]
    }

    /// Makes graph.json's policy (as loaded) this machine's record for the graph: every record of it (by id or folder)
    /// stricter on either setting is lowered to the file's pair, with the time (D83), or the file's policy is raised
    /// into it. Then every loaded graph is settled again, since another copy of it may have changed. The one path that
    /// lowers the record: `graph set` lowering a policy and `graph confirm-policy` both call it. Err carries why the
    /// record could not be read or written; then nothing changed.
    fn record_file_policy(&mut self, alias: &str) -> Result<FileRecorded, String> {
        let l = &self.graphs[alias];
        let (id, folder, file) = (l.meta.id, l.dir.as_str().to_string(), l.meta.file_policy());
        let path = self.ports.state.path();
        let (mut state, merged) = self.read_state()?;
        let recorded = state.recorded(id, &folder);
        let was = file.join(recorded.unwrap_or(file));
        let now = self.now();
        let changed = match recorded.filter(|r| r.exceeds(file)) {
            // Every record stricter on either setting becomes the file's pair; the rest are no stricter on any.
            Some(_) => state.lower(id, &folder, file, &now) > 0,
            None => file != Policy::OPEN && state.raise(id, &folder, file, &now),
        };
        let mut warnings = Vec::new();
        if (changed || merged)
            && let Some(why) = self.write_state(&state)?
        {
            warnings.push(problems::local_state_not_durable(&path, &why));
        }
        let aliases: Vec<String> = self.graphs.keys().cloned().collect();
        for a in &aliases {
            let w = self.settle_policy(a);
            if a == alias {
                warnings.extend(w);
            }
        }
        Ok(FileRecorded {
            was,
            changed,
            warnings,
        })
    }

    /// After `graph set` wrote graph.json's lower policy (`before` was the file's): records it ([`Self::record_file_policy`]).
    /// If the record cannot be written the graph stays at its recorded policy and the reply says so
    /// (`policy_not_lowered`, with `policy_downgrade_unconfirmed`), so a failed record never becomes an unrecorded
    /// downgrade and no new embedder gets the graph's text.
    pub(super) fn lower_after_set(&mut self, alias: &str) -> Vec<Problem> {
        match self.record_file_policy(alias) {
            Ok(done) => done.warnings,
            Err(why) => {
                let path = self.ports.state.path();
                let mut warnings = alloc::vec![problems::policy_not_lowered(
                    &alias_name(alias),
                    &path,
                    &why
                )];
                warnings.extend(self.settle_policy(alias));
                warnings
            }
        }
    }

    /// `graph confirm-policy`: makes graph.json's policy (as loaded) this machine's record. Raising it, keeping it or
    /// lowering it needs nothing more (D83: any caller, one call); the services' reply warns `policy_lowered` for a
    /// lowering. A record that cannot be read or written is `policy_not_recorded`, and nothing changed.
    pub fn confirm_policy(&mut self, _ctx: &Ctx, alias: &str) -> Outcome<PolicyConfirmed> {
        if !self.graphs.contains_key(alias) {
            return Err(self.not_loaded(alias));
        }
        let path = self.ports.state.path();
        let done = self
            .record_file_policy(alias)
            .map_err(|why| problems::policy_not_recorded(&path, &why))?;
        let policy = self.graphs[alias].policy.effective;
        Ok(Done {
            value: PolicyConfirmed {
                alias: alias.to_string(),
                was: done.was,
                policy,
                changed: done.changed,
            },
            notes: done.warnings,
        })
    }
}

/// What [`GraphStore::record_file_policy`] did.
struct FileRecorded {
    /// The policy umx acted on before.
    was: Policy,
    /// Whether this machine's record changed.
    changed: bool,
    warnings: Vec<Problem>,
}
