use super::GraphStore;
use super::search::Searched;
use crate::{Ctx, Done, Outcome, Page, PageGraphs, problems};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::ports::{GraphDir, LocalStateError, StateWritten};
use umx_core::scopes::{Error, Member, Saved, Scopes};
use umx_core::{Id, Problem, ValidName};

/// One graph selection, resolved by the same d-20/d-21 helper for every scoped read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    Graphs(Vec<String>),
    Saved(String),
    /// Runtime request selection only; saved scopes always contain explicit graph members.
    All {
        include_named: bool,
    },
}

pub(crate) struct SavedRead {
    name: String,
    members: Vec<Member>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScopeMemberInfo {
    pub id: Id,
    pub alias: String,
    pub folder: String,
    pub loaded: bool,
    /// Unknown when not loaded, rather than a false promise that it is open.
    pub secured: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScopeInfo {
    pub name: String,
    pub members: Vec<ScopeMemberInfo>,
}

fn checked_name(name: &str) -> Result<(), Problem> {
    if ValidName::alias(name).is_some() {
        Ok(())
    } else {
        Err(problems::usage_invalid(
            "a scope name is snake_case (one name and not a list)",
            "scope",
        ))
    }
}

impl GraphStore {
    pub(super) fn read_scopes(&self) -> Result<Scopes, Problem> {
        let file = self.ports.scopes.path();
        let bytes = self.ports.scopes.read().map_err(|e| match e {
            LocalStateError::Refused {
                reason: umx_core::ports::config_files::REFUSED_TOO_LARGE,
                ..
            } => problems::scope_limit(&file),
            e => problems::scope_store_invalid(&file, &e.to_string()),
        })?;
        bytes.map_or_else(
            || Ok(Scopes::default()),
            |bytes| Scopes::decode(&bytes).map_err(|e| self.scope_codec_error(e)),
        )
    }

    fn scope_codec_error(&self, e: Error) -> Problem {
        let file = self.ports.scopes.path();
        match e {
            Error::Invalid => problems::scope_store_invalid(&file, "invalid format"),
            Error::Limit => problems::scope_limit(&file),
            Error::Newer(found) => problems::scope_format_newer(&file, found),
        }
    }

    pub(super) fn write_scopes(&self, scopes: &Scopes) -> Result<Vec<Problem>, Problem> {
        let bytes = scopes.encode().map_err(|e| self.scope_codec_error(e))?;
        let file = self.ports.scopes.path();
        match self
            .ports
            .scopes
            .write(&bytes)
            .map_err(|e| problems::scope_store_not_saved(&file, &e.to_string()))?
        {
            StateWritten::Durable => Ok(Vec::new()),
            StateWritten::NotSynced(why) => {
                Ok(alloc::vec![problems::local_state_not_durable(&file, &why)])
            }
        }
    }

    fn scope_info(&self, name: &str, saved: &Saved) -> ScopeInfo {
        ScopeInfo {
            name: name.to_string(),
            members: saved
                .members
                .iter()
                .map(|m| {
                    let copies = self.copies_of(m.id);
                    let secured =
                        (!copies.is_empty()).then(|| copies.iter().any(|a| !self.shown_unnamed(a)));
                    ScopeMemberInfo {
                        id: m.id,
                        alias: m.alias.clone(),
                        folder: m.folder.clone(),
                        loaded: !copies.is_empty(),
                        secured,
                    }
                })
                .collect(),
        }
    }

    pub fn scope_list(&self) -> Outcome<Vec<ScopeInfo>> {
        let scopes = self.read_scopes()?;
        Ok(Done::ok(
            scopes
                .scopes
                .iter()
                .map(|(n, s)| self.scope_info(n, s))
                .collect(),
        ))
    }

    pub fn scope_show(&self, name: &str) -> Outcome<ScopeInfo> {
        checked_name(name)?;
        let scopes = self.read_scopes()?;
        let saved = scopes
            .scopes
            .get(name)
            .ok_or_else(|| problems::scope_unknown(name))?;
        Ok(Done::ok(self.scope_info(name, saved)))
    }

    pub fn scope_save(&mut self, ctx: &Ctx, name: &str, replace: bool) -> Outcome<ScopeInfo> {
        checked_name(name)?;
        if ctx.graph.is_none() || ctx.scope.is_some() {
            return Err(problems::usage_invalid(
                "scope save needs --graph naming two or more graphs (not --scope)",
                "scope",
            ));
        }
        let mut scopes = self.read_scopes()?;
        if scopes.scopes.contains_key(name) && !replace {
            return Err(problems::scope_exists(name));
        }
        let selected = self.graphs_in_scope(ctx, None, "scope")?;
        let mut saved = scopes.scopes.remove(name).unwrap_or_default();
        let mut members = Vec::new();
        let mut warnings = Vec::new();
        for alias in &selected.aliases {
            let l = &self.graphs[alias];
            let copies = self.copies_of(l.meta.id);
            let primary = self.primary_of(l.meta.id).ok_or_else(|| {
                problems::copy_ambiguous(&copies.iter().map(|a| a.to_string()).collect::<Vec<_>>())
            })?;
            if alias != primary {
                warnings.push(problems::copy_dropped(alias));
            }
            if members.iter().any(|m: &Member| m.id == l.meta.id) {
                continue;
            }
            if !self.shown_unnamed(alias) || !self.shown_unnamed(primary) {
                warnings.push(problems::scope_includes_secured(alias));
            }
            let extra = saved
                .members
                .iter()
                .find(|m| m.id == l.meta.id)
                .map(|m| m.extra.clone())
                .unwrap_or_default();
            members.push(Member {
                id: l.meta.id,
                alias: primary.to_string(),
                folder: self.graphs[primary].dir.as_str().to_string(),
                extra,
            });
        }
        saved.members = members;
        let info = self.scope_info(name, &saved);
        scopes.scopes.insert(name.to_string(), saved);
        warnings.extend(self.write_scopes(&scopes)?);
        Ok(Done {
            value: info,
            notes: warnings,
        })
    }

    pub fn scope_delete(&mut self, name: &str) -> Outcome<String> {
        checked_name(name)?;
        let mut scopes = self.read_scopes()?;
        if scopes.scopes.remove(name).is_none() {
            return Err(problems::scope_unknown(name));
        }
        let warnings = self.write_scopes(&scopes)?;
        Ok(Done {
            value: name.to_string(),
            notes: warnings,
        })
    }

    /// F99: the registry rows to load so saved scope `name` can be read, decided for the whole scope at once: for
    /// each member with no copy in memory, the row of the same graph id remembered as loaded (it unloaded when idle,
    /// or did not reload at start), its own folder first. Empty: every member is in memory now. A missing member
    /// with no such row (unloaded on purpose: F72 never loads it on demand), or a registry that cannot be read,
    /// refuses the whole plan with `scope_graph_not_loaded` for that member, so nothing loads (review R213-B1). A
    /// saved scope names its members, so secured members load like any graph a command names (d-20).
    pub fn scope_load_plan(&self, name: &str) -> Result<Vec<umx_core::registry::Row>, Problem> {
        checked_name(name)?;
        let scopes = self.read_scopes()?;
        let saved = scopes
            .scopes
            .get(name)
            .ok_or_else(|| problems::scope_unknown(name))?;
        let registry = self.read_registry().ok();
        let mut plan: Vec<umx_core::registry::Row> = Vec::new();
        for member in &saved.members {
            if !self.copies_of(member.id).is_empty() {
                continue;
            }
            let remembered = |r: &&umx_core::registry::Row| {
                r.loaded && r.id == member.id && !self.graphs.contains_key(&r.alias)
            };
            let row = registry.as_ref().and_then(|registry| {
                registry
                    .rows
                    .iter()
                    .filter(remembered)
                    .find(|r| r.folder == member.folder)
                    .or_else(|| registry.rows.iter().find(remembered))
            });
            let Some(row) = row else {
                let folder = GraphDir::parse(&member.folder).expect("validated scope folder");
                return Err(problems::scope_graph_not_loaded(&member.alias, &folder));
            };
            if !plan.iter().any(|p| p.folder == row.folder) {
                plan.push(row.clone());
            }
        }
        Ok(plan)
    }

    pub(super) fn saved_scope(&self, name: &str) -> Result<Searched, Problem> {
        checked_name(name)?;
        let scopes = self.read_scopes()?;
        let saved = scopes
            .scopes
            .get(name)
            .ok_or_else(|| problems::scope_unknown(name))?;
        let mut aliases = Vec::new();
        for member in &saved.members {
            let copies = self.copies_of(member.id);
            if copies.is_empty() {
                let folder = GraphDir::parse(&member.folder).expect("validated scope folder");
                return Err(problems::scope_graph_not_loaded(&member.alias, &folder));
            }
            let primary = self.primary_of(member.id).ok_or_else(|| {
                problems::copy_ambiguous(&copies.iter().map(|a| a.to_string()).collect::<Vec<_>>())
            })?;
            aliases.push(primary.to_string());
        }
        Ok(Searched {
            aliases,
            named: true,
            skipped: 0,
            copies_skipped: Vec::new(),
            twice: Vec::new(),
            saved: Some(SavedRead {
                name: name.to_string(),
                members: saved.members.clone(),
            }),
        })
    }

    pub(crate) fn saved_warnings(
        &self,
        saved: &SavedRead,
        aliases: &[String],
    ) -> Result<Vec<Problem>, Problem> {
        let mut warnings = Vec::new();
        let mut others = Vec::new();
        let mut hidden = 0;
        for member in &saved.members {
            let alias = aliases
                .iter()
                .find(|a| self.graphs[*a].meta.id == member.id)
                .ok_or_else(|| {
                    let folder = GraphDir::parse(&member.folder).expect("validated scope folder");
                    problems::scope_graph_not_loaded(&member.alias, &folder)
                })?;
            let l = &self.graphs[alias];
            if l.dir.as_str() != member.folder {
                let folder = GraphDir::parse(&member.folder).expect("validated scope folder");
                warnings.push(problems::scope_member_moved(
                    &saved.name,
                    &member.alias,
                    &folder,
                    &l.dir,
                ));
            }
            if !self.shown_unnamed(alias) {
                warnings.push(problems::scope_includes_secured(alias));
            }
            for other in self
                .copies_of(member.id)
                .into_iter()
                .filter(|a| *a != alias.as_str())
            {
                if self.shown_unnamed(other) {
                    others.push(other.to_string());
                } else {
                    hidden += 1;
                }
            }
        }
        if !others.is_empty() {
            warnings.push(problems::copies_skipped(&others));
        }
        if hidden > 0 {
            warnings.push(problems::secured_graphs_skipped(hidden));
        }
        Ok(warnings)
    }

    /// Lists keep their old single-graph default; only an explicit list or saved scope enters this path, or
    /// `all_unnamed` (a key lookup naming no graph, ws008 E2b F9), which covers every loaded open graph.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn scoped_page<T>(
        &self,
        ctx: &Ctx,
        refs: &[Option<&str>],
        noun: &'static str,
        limit: usize,
        offset: usize,
        all_unnamed: bool,
        mut read: impl FnMut(&Ctx, usize, usize) -> Outcome<Page<T>>,
    ) -> Outcome<Page<T>> {
        if !all_unnamed
            && ctx.scope.is_none()
            && !ctx.graph.as_deref().is_some_and(|g| g.contains(','))
        {
            let mut done = read(ctx, limit, offset)?;
            if let Some(note) = self.copies_note(ctx, refs, &done.value.alias) {
                done.notes.push(note);
            }
            done.notes.extend(self.stale_note(&done.value.alias));
            return Ok(done);
        }
        let scope = self.graphs_in_scope(ctx, None, noun)?;
        let mut aliases = scope.aliases.clone();
        match self.addressed(ctx, refs)? {
            Some(alias) if aliases.contains(&alias) => aliases.retain(|a| *a == alias),
            Some(alias) => return Err(problems::graph_mismatch(&alias, &aliases.join(","))),
            None if refs.iter().any(Option::is_some) && aliases.len() != 1 => {
                return Err(problems::usage_invalid(
                    "node filters need addresses when several graphs are in scope",
                    noun,
                ));
            }
            None => {}
        }
        let mut rows = Vec::new();
        let mut graphs = Vec::new();
        let mut total = 0usize;
        let mut remaining = offset;
        let mut warnings = Vec::new();
        for alias in aliases {
            let single = Ctx {
                graph: Some(alias.clone()),
                scope: None,
                default_graph: None,
                ..ctx.clone()
            };
            let done = read(&single, limit.saturating_sub(rows.len()), remaining)?;
            let count = done
                .value
                .total
                .expect("node and edge list count their matches");
            remaining = remaining.saturating_sub(count);
            total = total.saturating_add(count);
            warnings.extend(done.notes);
            warnings.extend(self.stale_note(&alias));
            graphs.extend(core::iter::repeat_n(alias, done.value.rows.len()));
            rows.extend(done.value.rows);
        }
        warnings.extend(self.scope_warnings(&scope)?);
        Ok(Done {
            value: Page {
                alias: scope.aliases.join(","),
                truncated: offset.saturating_add(rows.len()) < total,
                rows,
                total: Some(total),
                graphs: Some(PageGraphs {
                    rows: graphs,
                    copies: !scope.twice.is_empty(),
                }),
                along: None,
            },
            notes: warnings,
        })
    }
}
