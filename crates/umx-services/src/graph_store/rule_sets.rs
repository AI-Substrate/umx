//! Graph-local rule-set copies. Source files are read by the CLI, never by the daemon.

use super::rules::{name_notes, parse_patterns};
use super::{Commit, GraphStore, alias_name, persist_parts};
use crate::{Ctx, Done, Outcome, problems};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::ports::{
    Change, EntryKind, FileRef, FileStamp, FilesError, GraphDir, GraphFiles, RuleSetFile, Written,
};
use umx_core::rules::{MAX_RULE_SET_BYTES, MAX_RULE_SETS, RuleSet};
use umx_core::ui::Touched;
use umx_core::{GraphMeta, Problem, ValidName};

#[derive(Clone, Debug, PartialEq)]
pub struct RuleSetRow {
    pub name: String,
    pub rules: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuleSetsListed {
    pub graph: String,
    pub rows: Vec<RuleSetRow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuleSetChanged {
    pub graph: String,
    pub name: String,
    pub rules: usize,
    pub unchanged: bool,
    pub format: u32,
}

pub(super) struct SetWrite<'a> {
    pub(super) file: FileRef,
    pub(super) bytes: &'a [u8],
    pub(super) stamp: FileStamp,
}

#[derive(Clone, Debug, Default)]
pub(super) struct SetReads {
    pub(super) files: Vec<FileRef>,
    pub(super) stamps: Vec<(FileRef, FileStamp)>,
    failed: Option<Problem>,
}

fn io_problem(dir: &GraphDir, graph: &ValidName, error: FilesError) -> Problem {
    match error {
        FilesError::Changed(files) => problems::graph_files_changed(graph, dir, &files),
        FilesError::RecoveryConflict(files) => {
            problems::recovery_conflict(dir, &files, Some(graph))
        }
        error => problems::files_error(&error, |error| {
            problems::write_failed(umx_core::codec::GraphFile::Graph, error)
        }),
    }
}

fn exclusion(error: &FilesError) -> Option<&'static str> {
    match error {
        FilesError::TooLarge { .. } => Some("a set file holds at most 64 KiB"),
        FilesError::OutsideRoots(_) => Some("the path is outside the roots or follows a link"),
        FilesError::Io(_) => Some("an I/O failure prevents reading the set"),
        _ => None,
    }
}

pub(super) fn read_sets(
    files: &dyn GraphFiles,
    dir: &GraphDir,
    meta: &mut GraphMeta,
    graph: &ValidName,
) -> Outcome<SetReads> {
    meta.rule_sets.clear();
    let mut seen = SetReads::default();
    let mut notes = Vec::new();
    let listing = match files.list_rule_sets(dir) {
        Ok(listing) => listing,
        Err(error) => match exclusion(&error) {
            Some(reason) => {
                notes.push(problems::rule_set_skipped("rules", reason, graph));
                return Ok(Done { value: seen, notes });
            }
            None => return Err(io_problem(dir, graph, error)),
        },
    };
    if listing.limited {
        notes.push(problems::rule_set_skipped(
            "rules",
            "the rules folder exceeds 1000 directory entries",
            graph,
        ));
        return Ok(Done { value: seen, notes });
    }
    let mut candidates = Vec::new();
    for (name, kind) in listing.entries {
        let Some(set) = RuleSetFile::from_file_name(&name) else {
            notes.push(problems::rule_set_skipped(
                &name,
                "the file stem must be snake_case and never std, with a .json suffix",
                graph,
            ));
            continue;
        };
        if kind != EntryKind::File {
            notes.push(problems::rule_set_skipped(
                &name,
                "a set must be a regular file, never a folder or a link",
                graph,
            ));
            continue;
        }
        candidates.push(set);
    }
    if candidates.len() > MAX_RULE_SETS {
        let omitted = candidates[MAX_RULE_SETS..]
            .iter()
            .map(|set| set.name().to_string())
            .collect::<Vec<_>>();
        notes.push(problems::rule_sets_cut(&omitted, graph));
    }
    let ontology = meta.ontology.effective();
    for file in candidates.into_iter().take(MAX_RULE_SETS) {
        let name = file.name().to_string();
        let filename = file.file_name();
        let file = FileRef::RuleSet(file);
        seen.files.push(file.clone());
        let (bytes, stamp) = match files.read_ref_stamped(dir, &file) {
            Ok(read) => read,
            Err(error) => {
                let Some(reason) = exclusion(&error) else {
                    return Err(io_problem(dir, graph, error));
                };
                notes.push(problems::rule_set_skipped(&filename, reason, graph));
                match files.stamp_ref(dir, &file) {
                    Ok(stamp) => seen.stamps.push((file, stamp)),
                    Err(error) if exclusion(&error).is_some() => {}
                    Err(error) => return Err(io_problem(dir, graph, error)),
                }
                continue;
            }
        };
        seen.stamps.push((file, stamp));
        let Some(bytes) = bytes else {
            notes.push(problems::rule_set_skipped(
                &filename,
                "the file disappeared while it was being listed",
                graph,
            ));
            continue;
        };
        let set = match RuleSet::from_bytes(&bytes) {
            Ok(set) => set,
            Err(reason) => {
                notes.push(problems::rule_set_skipped(&filename, reason, graph));
                continue;
            }
        };
        if set.rules.iter().any(|rule| parse_patterns(rule).is_err()) {
            notes.push(problems::rule_set_skipped(
                &filename,
                "a rule regex has invalid or unsupported syntax",
                graph,
            ));
            continue;
        }
        let set = set.qualified(&ValidName::alias(&name).expect("checked set stem"));
        notes.extend(
            set.rules
                .iter()
                .flat_map(|rule| name_notes(&ontology, rule, graph)),
        );
        meta.rule_sets.insert(name, set);
    }
    Ok(Done { value: seen, notes })
}

pub(super) fn moved_sets(
    files: &dyn GraphFiles,
    dir: &GraphDir,
    before: &SetReads,
    graph: &ValidName,
) -> Result<Vec<FileRef>, Problem> {
    if let Some(problem) = &before.failed {
        return Err(problem.clone());
    }
    let listing = match files.list_rule_sets(dir) {
        Ok(listing) => listing,
        Err(error) if exclusion(&error).is_some() => return Ok(before.files.clone()),
        Err(error) => return Err(io_problem(dir, graph, error)),
    };
    let now: Vec<_> = listing
        .entries
        .iter()
        .filter(|(_, kind)| *kind == EntryKind::File && !listing.limited)
        .filter_map(|(name, _)| RuleSetFile::from_file_name(name).map(FileRef::RuleSet))
        .take(MAX_RULE_SETS)
        .collect();
    let mut moved: Vec<_> = before
        .files
        .iter()
        .chain(&now)
        .filter(|file| !before.files.contains(file) || !now.contains(file))
        .cloned()
        .collect();
    for file in now.iter().filter(|file| before.files.contains(file)) {
        let stamp = files
            .stamp_ref(dir, file)
            .map_err(|error| io_problem(dir, graph, error))?;
        if !before
            .stamps
            .iter()
            .any(|(old, was)| old == file && *was == stamp)
        {
            moved.push(file.clone());
        }
    }
    moved.sort_by_key(FileRef::path);
    moved.dedup();
    Ok(moved)
}

fn refresh(
    files: &dyn GraphFiles,
    dir: &GraphDir,
    meta: &mut GraphMeta,
    snapshot: &mut SetReads,
    graph: &ValidName,
) -> Result<Vec<Problem>, Problem> {
    let mut fresh_meta = meta.clone();
    match read_sets(files, dir, &mut fresh_meta, graph) {
        Ok(read) => {
            *meta = fresh_meta;
            *snapshot = read.value;
            Ok(read.notes)
        }
        Err(problem) => {
            snapshot.stamps.clear();
            snapshot.failed = Some(problem.clone());
            Err(problems::rule_sets_refresh_failed(graph, dir, &problem))
        }
    }
}

impl GraphStore {
    pub fn rule_set_list(&self, ctx: &Ctx) -> Outcome<RuleSetsListed> {
        let alias = self.select(ctx, &[])?;
        let loaded = &self.graphs[&alias];
        let mut meta = loaded.meta.clone();
        let mut read = read_sets(
            self.ports.files.as_ref(),
            &loaded.dir,
            &mut meta,
            &alias_name(&alias),
        )?;
        read.notes.extend(self.stale_note(&alias));
        Ok(Done {
            value: RuleSetsListed {
                graph: alias,
                rows: meta
                    .rule_sets
                    .iter()
                    .map(|(name, set)| RuleSetRow {
                        name: name.clone(),
                        rules: set.rules.len(),
                    })
                    .collect(),
            },
            notes: read.notes,
        })
    }

    pub fn rule_set_add(&mut self, ctx: &Ctx, name: &str, bytes: &[u8]) -> Outcome<RuleSetChanged> {
        let alias = self.select_write(ctx, &[])?;
        let graph = alias_name(&alias);
        let file = RuleSetFile::new(name).ok_or_else(|| {
            problems::rule_invalid("a set name is a snake_case file stem and never std")
        })?;
        if bytes.len() as u64 > MAX_RULE_SET_BYTES {
            return Err(problems::rule_invalid("a set file holds at most 64 KiB"));
        }
        let set = RuleSet::from_bytes(bytes).map_err(problems::rule_invalid)?;
        for rule in &set.rules {
            super::rules::RuleMatcher::new(rule)?;
            if let Some(target) = rule.to_node {
                super::resolve_node_in(self.get(&alias), ctx, &alias, &target.to_text())?;
            }
        }
        let name = ValidName::alias(name).expect("checked set stem");
        let files = self.ports.files.clone();
        let dir = self.graphs[&alias].dir.clone();
        let listing = files
            .list_rule_sets(&dir)
            .map_err(|error| io_problem(&dir, &graph, error))?;
        let named: Vec<_> = listing
            .entries
            .iter()
            .filter(|(_, kind)| *kind == EntryKind::File)
            .filter_map(|(file, _)| RuleSetFile::from_file_name(file))
            .map(|file| file.name().to_string())
            .collect();
        if listing
            .entries
            .iter()
            .any(|(entry, kind)| *entry == file.file_name() && *kind != EntryKind::File)
        {
            return Err(problems::rule_set_name_taken(&name, &graph));
        }
        let file = FileRef::RuleSet(file);
        let (old, stamp) = match files.read_ref_stamped(&dir, &file) {
            Ok(read) => read,
            Err(FilesError::TooLarge { .. }) => {
                return Err(problems::rule_set_name_taken(&name, &graph));
            }
            Err(error) => return Err(io_problem(&dir, &graph, error)),
        };
        if old.as_deref().is_some_and(|old| old != bytes) {
            return Err(problems::rule_set_name_taken(&name, &graph));
        }
        let unchanged = old.as_deref() == Some(bytes);
        if !unchanged
            && (listing.limited
                || named.iter().filter(|found| *found != name.as_str()).count() >= MAX_RULE_SETS)
        {
            return Err(problems::rule_sets_full(&graph, &named, listing.limited));
        }
        let count = set.rules.len();
        let before = self.graphs[&alias].meta.clone();
        let mut meta = before.clone();
        let mut read = read_sets(files.as_ref(), &dir, &mut meta, &graph)?;
        if unchanged {
            let loaded = self.graphs.get_mut(&alias).expect("selected");
            loaded.meta = meta;
            loaded.rule_sets_read = read.value;
            return Ok(Done {
                value: RuleSetChanged {
                    graph: alias,
                    name: name.as_str().into(),
                    rules: count,
                    unchanged,
                    format: loaded.meta.umx,
                },
                notes: read.notes,
            });
        }
        meta.rule_sets
            .insert(name.as_str().into(), set.qualified(&name));
        let compiled = super::rules::compile_rules(
            super::rules::std_rules(self.central(&meta), &meta),
            super::rules::MAX_GRAPH_COMPILED_BYTES,
        )?;
        for rule in &meta.rule_sets[name.as_str()].rules {
            if let Some(reason) = compiled.skipped_reason(&rule.name) {
                return Err(problems::rule_regex_invalid(reason, 1));
            }
        }
        let compiled_notes = compiled.notes(&graph);
        let ontology = meta.ontology.effective();
        read.notes.extend(
            meta.rule_sets[name.as_str()]
                .rules
                .iter()
                .flat_map(|rule| name_notes(&ontology, rule, &graph)),
        );
        let loaded = self.graphs.get_mut(&alias).expect("selected");
        loaded.meta = meta;
        // Metadata stamps serialize concurrent adds even at format 10, protecting the 32-file cap.
        let written = persist_parts(
            files.as_ref(),
            &alias,
            &dir,
            &loaded.meta,
            &loaded.graph,
            Commit {
                which: &[umx_core::codec::GraphFile::Graph],
                rewrite_edges: loaded.edges_normalized,
                feed: &mut loaded.feed,
                touched: Touched::default(),
                stamps: &mut loaded.stamps,
                receipts: None,
                rule_set: Some(SetWrite {
                    file: file.clone(),
                    bytes,
                    stamp,
                }),
                rule_set_stamps: &loaded.rule_sets_read.stamps,
            },
        )
        .inspect_err(|_| loaded.meta = before)?;
        loaded.meta.umx = written.umx;
        loaded.edges_normalized = false;
        loaded.rule_sets_read.files.push(file);
        match refresh(
            files.as_ref(),
            &dir,
            &mut loaded.meta,
            &mut loaded.rule_sets_read,
            &graph,
        ) {
            Ok(notes) => read.notes = notes,
            Err(problem) => read.notes.push(problem),
        }
        read.notes.extend(compiled_notes);
        read.notes.extend(written.warnings);
        Ok(Done {
            value: RuleSetChanged {
                graph: alias,
                name: name.as_str().into(),
                rules: count,
                unchanged,
                format: loaded.meta.umx,
            },
            notes: read.notes,
        })
    }

    pub fn rule_set_remove(&mut self, ctx: &Ctx, name: &str) -> Outcome<RuleSetChanged> {
        let alias = self.select(ctx, &[])?;
        let graph = alias_name(&alias);
        let file = RuleSetFile::new(name).ok_or_else(|| {
            problems::rule_invalid("a set name is a snake_case file stem and never std")
        })?;
        let files = self.ports.files.clone();
        let loaded = self.graphs.get_mut(&alias).expect("selected");
        let listing = files
            .list_rule_sets(&loaded.dir)
            .map_err(|error| io_problem(&loaded.dir, &graph, error))?;
        let present = listing
            .entries
            .iter()
            .find(|(found, _)| *found == file.file_name());
        if present.is_some_and(|(_, kind)| *kind == EntryKind::Folder) {
            return Err(problems::rule_invalid(
                "a set copy is a file, never a folder",
            ));
        }
        let file = FileRef::RuleSet(file);
        let stamp = files
            .stamp_ref(&loaded.dir, &file)
            .map_err(|error| io_problem(&loaded.dir, &graph, error))?;
        let unchanged = stamp == FileStamp::ABSENT;
        let mut notes = Vec::new();
        if !unchanged {
            let committed = files
                .write_set_checked_also(
                    &loaded.dir,
                    &[Change {
                        file: file.clone(),
                        bytes: None,
                    }],
                    Some(&loaded.stamps),
                    &[(file, stamp)],
                )
                .map_err(|error| io_problem(&loaded.dir, &graph, error))?;
            loaded.stamps = committed.stamps;
            loaded.feed.commit(&loaded.graph, Touched::default());
            loaded.meta.rule_sets.remove(name);
            if let Written::NotSynced(error) = committed.written {
                notes.push(problems::rule_set_remove_not_durable(name, &error));
            }
        }
        match refresh(
            files.as_ref(),
            &loaded.dir,
            &mut loaded.meta,
            &mut loaded.rule_sets_read,
            &graph,
        ) {
            Ok(fresh) => notes.extend(fresh),
            Err(problem) => notes.push(problem),
        }
        Ok(Done {
            value: RuleSetChanged {
                graph: alias,
                name: name.into(),
                rules: 0,
                unchanged,
                format: loaded.meta.umx,
            },
            notes,
        })
    }
}
