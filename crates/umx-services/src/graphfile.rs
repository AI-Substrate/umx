//! `graph load --graphfile [--set <name>]` (ws002 s17, D66, req-128): every graph a repo's Graphfile lists, or one
//! of its sets, loaded through the normal load path (the roots, the stricter-of policy, and the id the entry names
//! must be the id the folder holds), each recorded in this machine's index as `graph load` does. One entry that does
//! not load is one warning; the others still load. A secured member loads and stays named-only for reads (D53).

use crate::graph_store::{second, under};
use crate::{Ctx, Done, Outcome, Services, problems};
use alloc::string::String;
use alloc::vec::Vec;
use umx_core::ports::GraphDir;
use umx_core::{Command, ValidName};

/// At most this many graphs in one `graph load --graphfile` (graph-addressing-expert, s17).
pub const MAX_GRAPHFILE_LOAD: usize = 256;

/// One graph `graph load --graphfile` has loaded (or found already loaded from the same folder).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphfileLoadedEntry {
    /// The alias it is loaded under.
    pub alias: String,
    pub path: String,
    /// This machine's index had no row for its folder before.
    pub new_to_machine: bool,
    /// It was loaded from that folder already: nothing was done.
    pub already: bool,
}

/// One Graphfile entry that did not load, and the code that stopped it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphfileNotLoaded {
    pub alias: String,
    pub path: String,
    pub code: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphfileLoad {
    pub repo: GraphDir,
    pub file: GraphDir,
    pub set: Option<String>,
    pub loaded: Vec<GraphfileLoadedEntry>,
    pub not_loaded: Vec<GraphfileNotLoaded>,
}

impl Services {
    /// `graph load --graphfile [--set <name>]` from the repo that holds `folder`.
    pub fn graph_load_graphfile(
        &mut self,
        ctx: &Ctx,
        folder: &str,
        set: Option<&str>,
    ) -> Outcome<GraphfileLoad> {
        let top = self.graph.repo_top_of(folder)?;
        let (file, gf) = self.graph.graphfile_for_use(&top)?;
        let entries: Vec<_> = match set {
            Some(name) => gf
                .set(name)
                .ok_or_else(|| problems::graphfile_unknown(&file, "set", name, &top))?
                .into_iter()
                .cloned()
                .collect(),
            None => gf.entries().to_vec(),
        };
        if entries.len() > MAX_GRAPHFILE_LOAD {
            return Err(problems::graphfile_load_too_many(entries.len(), &top));
        }
        let scan = Command::new("graph scan").path(&top);
        let (mut loaded, mut not_loaded, mut warnings) = (Vec::new(), Vec::new(), Vec::new());
        for e in entries {
            let shown = under(&top, &e.path).unwrap_or_else(|| top.clone());
            let mut skip = |code: &'static str, fix: Command, warnings: &mut Vec<_>| {
                warnings.push(problems::graphfile_entry_skipped(
                    &e.alias, &shown, code, fix,
                ));
                not_loaded.push(GraphfileNotLoaded {
                    alias: e.alias.clone(),
                    path: e.path.clone(),
                    code,
                });
            };
            let dir = match self.graph.entry_dir(&top, &e.path) {
                Ok(dir) => dir,
                Err(state) => {
                    skip(state, scan.clone(), &mut warnings);
                    continue;
                }
            };
            // The same graph from the same folder is not a clash (graph-addressing-expert, s17).
            if let Some((alias, id)) = self.graph.loaded_at(&dir)
                && id == e.id
            {
                loaded.push(GraphfileLoadedEntry {
                    alias,
                    path: e.path.clone(),
                    new_to_machine: false,
                    already: true,
                });
                continue;
            }
            let new_to_machine = !self.graph.known_folder(&dir);
            match self.load(ctx, dir.as_str(), Some(&e.alias), Some(e.id)) {
                Ok(mut done) => {
                    let (alias, replaced) = (done.value.graph.alias.clone(), done.value.replaced);
                    done.notes.extend(self.graph.remember(&alias, replaced));
                    warnings.extend(done.notes);
                    loaded.push(GraphfileLoadedEntry {
                        alias,
                        path: e.path.clone(),
                        new_to_machine,
                        already: false,
                    });
                }
                Err(p) if p.code == "graph_id_changed" => {
                    warnings.push(problems::graphfile_id_mismatch(&e.alias, &dir, &top));
                    not_loaded.push(GraphfileNotLoaded {
                        alias: e.alias.clone(),
                        path: e.path.clone(),
                        code: "graphfile_id_mismatch",
                    });
                }
                Err(p) => {
                    // An alias in use is refused, never replaced (security, s17); its fix loads it under another.
                    let fix = match ValidName::alias(&e.alias) {
                        Some(a) if p.code == "alias_in_use" => Command::new("graph load")
                            .path(&dir)
                            .word("--as")
                            .name(&second(&a)),
                        _ => scan.clone(),
                    };
                    skip(p.code, fix, &mut warnings);
                }
            }
        }
        Ok(Done {
            value: GraphfileLoad {
                repo: top,
                file,
                set: set.map(String::from),
                loaded,
                not_loaded,
            },
            notes: warnings,
        })
    }
}
