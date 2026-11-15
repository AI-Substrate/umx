//! Copies of one graph (d-21, the joint ruling of graph-addressing-expert and the architect): the same graph id
//! loaded under two or more aliases. A read that names no graph takes one copy per graph id, the primary: the
//! earliest loaded. When the primary unloads and one copy is left, that copy is primary; with two or more left none
//! is, until only one remains (no silent promotion), and an unnamed read refuses with `copy_ambiguous`. Load order is
//! the registry's row order, and a graph id without a primary is kept in the registry too (F80, the d-21 amendment of
//! graph-addressing-expert and the architect), so a daemon restart keeps both: a no-primary state clears only when
//! one copy is left, or `umx graph primary <alias>` names the copy to be primary (F98).

use super::{Ctx, Done, GraphStore, Outcome};
use crate::problems;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::{Id, Problem};

/// What `umx graph primary` did (F98). It names only the copy asked for, never another copy (d-20).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrimarySet {
    pub alias: String,
    pub graph: Id,
    /// It was already the primary (or the only copy). The registry is written only if it did not record that (a
    /// repeat after a save that failed).
    pub unchanged: bool,
}

impl GraphStore {
    /// The aliases loaded with graph id `id`, earliest loaded first (a reload keeps its folder's place).
    pub(crate) fn copies_of(&self, id: Id) -> Vec<&str> {
        let mut copies: Vec<(&str, u64)> = self
            .graphs
            .iter()
            .filter(|(_, l)| l.meta.id == id)
            .map(|(a, l)| (a.as_str(), l.order))
            .collect();
        copies.sort_by_key(|(_, order)| *order);
        copies.into_iter().map(|(a, _)| a).collect()
    }

    /// The primary copy of graph id `id`: the earliest loaded, or None while the id has lost its primary.
    pub(crate) fn primary_of(&self, id: Id) -> Option<&str> {
        if self.demoted.contains(&id) {
            return None;
        }
        self.copies_of(id).first().copied()
    }

    /// `umx graph primary <alias>` (F98, d-21): the named loaded copy becomes its graph id's primary. Its id's copies
    /// keep their load order values, re-dealt with this one first, and a no-primary state ends. The registry row moves
    /// ahead of the id's other rows and the no-primary mark goes, in one write, so a restart loads it first and it is
    /// primary again; a write that fails warns and the change still holds here (as F80's marks). A copy unnamed reads do
    /// not show (effective visibility named, as a secured graph) is allowed, the verb naming it, and warned
    /// `primary_is_secured`: unnamed reads now skip the id.
    pub fn graph_primary(&mut self, _ctx: &Ctx, alias: &str) -> Outcome<PrimarySet> {
        let Some(l) = self.graphs.get(alias) else {
            return Err(problems::graph_not_loaded(alias));
        };
        let (id, folder) = (l.meta.id, l.dir.as_str().to_string());
        let mut value = PrimarySet {
            alias: alias.to_string(),
            graph: id,
            unchanged: self.primary_of(id) == Some(alias),
        };
        if value.unchanged {
            // Repeating a choice whose save failed writes it again (review R227, note 2).
            let warnings = self.save_primary(id, alias, &folder);
            return Ok(Done {
                value,
                notes: warnings,
            });
        }
        let copies: Vec<String> = self
            .copies_of(id)
            .into_iter()
            .map(ToString::to_string)
            .collect();
        let orders: Vec<u64> = copies.iter().map(|a| self.graphs[a].order).collect();
        let first_then_rest = core::iter::once(alias)
            .chain(copies.iter().map(String::as_str).filter(|a| *a != alias));
        for (copy, order) in first_then_rest.zip(orders) {
            if let Some(l) = self.graphs.get_mut(copy) {
                l.order = order;
            }
        }
        self.demoted.remove(&id);
        let mut warnings = self.save_primary(id, alias, &folder);
        // Unnamed reads skip a copy by its effective visibility (d-20, D53), so that is what is warned.
        if !self.shown_unnamed(alias) {
            warnings.push(problems::primary_is_secured());
        }
        value.unchanged = false;
        Ok(Done {
            value,
            notes: warnings,
        })
    }

    /// F98: in one registry write, the row for `folder` records this copy as loaded, goes ahead of its id's other
    /// rows, and the id loses its no-primary mark. A row that is missing (a load whose save failed) or no longer
    /// loaded would not bring the copy back at the next start, so it is written first (review R227-1). Nothing is
    /// written when the registry already says all that.
    fn save_primary(&self, id: Id, alias: &str, folder: &str) -> Vec<Problem> {
        let mut registry = match self.read_registry() {
            Ok(r) => r,
            Err(why) => {
                return alloc::vec![problems::registry_unreadable(&self.registry_file(), &why)];
            }
        };
        let recorded = registry
            .rows
            .iter()
            .find(|r| r.folder == folder)
            .is_some_and(|r| r.loaded && r.id == id && r.alias == alias);
        if !recorded {
            let Some(row) = self.loaded_row(alias) else {
                return Vec::new();
            };
            if !registry.loaded(row, true) {
                let why = alloc::format!(
                    "the registry is full: {} loaded graphs",
                    umx_core::registry::MAX_ROWS
                );
                return alloc::vec![problems::registry_update_not_saved(
                    alias,
                    &self.registry_file(),
                    &why
                )];
            }
        }
        let moved = registry.put_first(folder);
        // Clearing never hits the limit.
        let cleared = registry.set_no_primary(id, false).unwrap_or(false);
        if !recorded || moved || cleared {
            self.update_registry(&[alias], &registry)
        } else {
            Vec::new()
        }
    }

    /// Before `alias` leaves memory: whether it is its graph's primary, to pass to [`Self::after_unload`].
    pub(crate) fn before_unload(&self, alias: &str) -> Option<(Id, bool)> {
        let id = self.graphs.get(alias)?.meta.id;
        Some((id, self.primary_of(id) == Some(alias)))
    }

    /// After a copy of `id` (alias `alias`) left: the primary leaving two or more copies leaves none primary; one
    /// copy left (or none) ends that. A change is written to the registry (F80); its warnings are returned.
    pub(crate) fn after_unload(&mut self, alias: &str, left: Option<(Id, bool)>) -> Vec<Problem> {
        let Some((id, was_primary)) = left else {
            return Vec::new();
        };
        let remaining = self.copies_of(id).len();
        let changed = if remaining <= 1 {
            self.demoted.remove(&id)
        } else {
            was_primary && self.demoted.insert(id)
        };
        if changed {
            self.save_no_primary(id, alias)
        } else {
            Vec::new()
        }
    }

    /// One copy per graph id among `aliases` (in their order): the primary, with the other copies returned to
    /// skip. An id whose primary is not among `aliases` (a secured primary, which d-20 already left out) has all
    /// its copies skipped silently, counted in the second number. An id with no primary refuses with copy_ambiguous.
    pub(crate) fn one_copy_each(
        &self,
        aliases: Vec<String>,
    ) -> Result<(Vec<String>, Vec<String>, usize), Problem> {
        let mut kept = Vec::new();
        let mut skipped = Vec::new();
        let mut hidden = 0;
        for alias in &aliases {
            let id = self.graphs[alias].meta.id;
            match self.primary_of(id) {
                Some(p) if p == alias => kept.push(alias.clone()),
                Some(p) if aliases.iter().any(|a| a == p) => skipped.push(alias.clone()),
                // An open copy whose primary is secured: counted with the secured graphs, never named, on purpose
                // (d-20 wins over d-21; graph-addressing-expert F2).
                Some(_) => hidden += 1,
                None => {
                    let copies: Vec<String> = aliases
                        .iter()
                        .filter(|a| self.graphs[a.as_str()].meta.id == id)
                        .cloned()
                        .collect();
                    return Err(problems::copy_ambiguous(&copies));
                }
            }
        }
        Ok((kept, skipped, hidden))
    }

    /// The aliases among `named` that are copies of a graph also named (`--graph design,design_b`).
    pub(crate) fn named_twice(&self, named: &[String]) -> Vec<String> {
        named
            .iter()
            .filter(|a| {
                let id = self.graphs[a.as_str()].meta.id;
                named
                    .iter()
                    .filter(|b| self.graphs[b.as_str()].meta.id == id)
                    .count()
                    > 1
            })
            .map(ToString::to_string)
            .collect()
    }
}
