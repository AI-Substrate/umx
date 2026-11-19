//! graph set: change a graph's settings (PRD 001 req-71). The settings themselves are umx-core's table.

use super::{Ctx, Done, GraphStore, Outcome, alias_name, persist, problems};
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use umx_core::codec::GraphFile;
use umx_core::settings::{self, SETTINGS};
use umx_core::ui::Touched;
use umx_core::{Command, ValidName};

#[derive(Clone, Debug, PartialEq)]
pub struct GraphSet {
    pub alias: String,
    /// The settings changed, in the order given.
    pub changed: Vec<&'static str>,
    pub title: String,
}

/// `graph new`'s next commands: how to change the new graph's vocabulary and settings, each runnable as
/// printed. Built from the alias and umx's own words only. The type is one std v1 does not have, so on a std graph
/// (the default) the add declares it rather than finding it there already (ws008 T2b).
pub(crate) fn change_commands(graph: &ValidName) -> Vec<Command> {
    let mut out: Vec<Command> = [
        Command::new("ontology add type").word("experiment"),
        Command::new("ontology add relation").word("mitigates"),
        Command::new("ontology add status").word("parked"),
    ]
    .into_iter()
    .map(|c| c.word("--graph").name(graph))
    .collect();
    for s in SETTINGS.iter().filter(|s| s.suggest) {
        out.push(
            Command::new("graph set")
                .name(graph)
                .word(s.flag)
                .word(s.example),
        );
    }
    out
}

impl GraphStore {
    /// Applies `--<name> <value>` settings to a loaded graph and writes graph.json. Every name is checked before
    /// anything changes; a refused value or a failed write leaves the graph as it was.
    pub fn graph_set(
        &mut self,
        _ctx: &Ctx,
        alias: &str,
        given: &[(String, String)],
    ) -> Outcome<GraphSet> {
        if !self.graphs.contains_key(alias) {
            return Err(self.not_loaded(alias));
        }
        self.files_unchanged(alias)?;
        let graph = alias_name(alias);
        if given.is_empty() {
            return Err(problems::usage_invalid(
                "name a setting to change (for example --title <text>)",
                "graph",
            ));
        }
        let mut rows = Vec::new();
        for (name, value) in given {
            let setting =
                settings::find(name).ok_or_else(|| problems::setting_unknown(name, &graph))?;
            rows.push((setting, value));
        }
        let now = self.ports.clock.now_ms();
        let files = self.ports.files.clone();
        let before = self.graphs[alias].meta.clone();
        let mut meta = before.clone();
        let mut warnings = Vec::new();
        for (setting, value) in &rows {
            warnings.extend((setting.apply)(&mut meta, &graph, value)?);
        }
        // Loosening either setting in graph.json: the stricter pair must be on this machine's record first (review R1).
        if before.file_policy().exceeds(meta.file_policy()) {
            warnings.extend(self.record_before_lowering(alias, before.file_policy())?);
        }
        let l = self.graphs.get_mut(alias).expect("checked above");
        l.last_used.store(now, Ordering::Relaxed);
        l.meta = meta;
        // A write_not_durable warning means the change is on disk; it goes back with the result.
        match persist(
            files.as_ref(),
            alias,
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
        let (changed, title) = (
            rows.iter().map(|(s, _)| s.name).collect(),
            l.meta.title.clone(),
        );
        // `--policy` raises at once; a lower one stays the file's word until a person confirms it.
        warnings.extend(self.settle_policy(alias));
        Ok(Done {
            value: GraphSet {
                alias: graph.as_str().into(),
                changed,
                title,
            },
            notes: warnings,
        })
    }
}
