//! graph unload, and idle unload (req-03) without a polling loop (req-06): the daemon asks for the earliest
//! deadline, sleeps until then (or until the next request), and calls `unload_idle`.

use super::{Ctx, Done, GraphStore, Outcome};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Clone, Debug, PartialEq)]
pub struct GraphUnloaded {
    pub alias: String,
}

impl GraphStore {
    /// Sets the idle time after which a graph unloads itself; 0 turns idle unload off.
    pub fn set_idle_unload_ms(&mut self, ms: u64) {
        self.idle_ms = ms;
    }

    pub fn idle_unload_ms(&self) -> u64 {
        self.idle_ms
    }

    /// Unloads a graph from memory. Every write already reached its files. The registry stops listing it as loaded,
    /// so it does not come back when the daemon starts (F72). An alias the registry remembers as loaded but that is
    /// not in memory (it did not reload, or it unloaded when idle) is forgotten the same way. Owed similar_to files
    /// are written first (ws004 s25a); what that found comes back as warnings (R180-4): a write that failed leaves
    /// `_meta.tsv` removed, so the next load rebuilds the lists.
    pub fn graph_unload(&mut self, _ctx: &Ctx, alias: &str) -> Outcome<GraphUnloaded> {
        let flushed = self.flush_similar_of(alias);
        let noted = self.note_unload(alias);
        let left = self.before_unload(alias);
        let removed = self.graphs.remove(alias);
        if let Some(loaded) = &removed {
            self.incoming.remove_source(loaded.instance);
            self.resets.insert(alias.to_string());
        }
        let mut warnings = flushed;
        warnings.extend(noted);
        warnings.extend(self.after_unload(alias, left));
        let folder = match &removed {
            Some(l) => l.dir.as_str().to_string(),
            None => self
                .remembered_folder(alias)
                .ok_or_else(|| self.not_loaded(alias))?,
        };
        warnings.extend(self.forget(alias, &folder).0);
        Ok(Done {
            value: GraphUnloaded {
                alias: alias.to_string(),
            },
            notes: warnings,
        })
    }

    /// Clock ms at which the next graph falls idle, or None when idle unload is off or nothing is loaded. Each
    /// command that uses a graph pushes its deadline back, so the daemon re-asks after every request.
    pub fn next_idle_unload_ms(&self) -> Option<u64> {
        if self.idle_ms == 0 {
            return None;
        }
        self.graphs
            .values()
            .map(|l| l.last_used_ms().saturating_add(self.idle_ms))
            .min()
    }

    /// How long the daemon may sleep before the next graph falls idle (0 when one is already due), or None when
    /// nothing will fall idle.
    pub fn idle_wait_ms(&self) -> Option<u64> {
        self.next_idle_unload_ms()
            .map(|at| at.saturating_sub(self.ports.clock.now_ms()))
    }

    /// Unloads every graph whose idle time has passed and returns their aliases in order.
    pub fn unload_idle(&mut self) -> Vec<String> {
        if self.idle_ms == 0 {
            return Vec::new();
        }
        let now = self.ports.clock.now_ms();
        let idle = self.idle_ms;
        let due: Vec<String> = self
            .graphs
            .iter()
            .filter(|(_, l)| l.last_used_ms().saturating_add(idle) <= now)
            .map(|(alias, _)| alias.clone())
            .collect();
        for alias in &due {
            let problems = self.flush_similar_of(alias);
            self.note_unload_problems(problems);
            let noted = self.note_unload(alias);
            self.note_unload_problems(noted);
            let left = self.before_unload(alias);
            if let Some(loaded) = self.graphs.remove(alias) {
                self.incoming.remove_source(loaded.instance);
                self.resets.insert(alias.clone());
            }
            // Logged by the daemon with the idle unload's other problems.
            let marked = self.after_unload(alias, left);
            self.note_unload_problems(marked);
        }
        due
    }
}
