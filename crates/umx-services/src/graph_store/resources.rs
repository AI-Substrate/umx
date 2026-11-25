//! Resource nodes through the verbs (ws006 S3b, req-77; workshop 010 section 3): `node add --locator` (one resource
//! node per locator in a graph; another loaded graph's copy is named), `node edit --locator` and `--unset locator`,
//! and `resource show`. A locator is checked as evidence is: its text by umx-core, a `root:` source against its root
//! (security S1), and its source by evidence's secret scan (security). Nothing is read.
//!
//! Two spellings of one file meet (graph-addressing-expert, N3): a graph-folder path and its `root:repo/` form
//! name the same file when the graph folder is in git, so lookups try each spelling while storing what was typed.

use super::problems;
use super::sources::SourceCheck;
use super::{GraphStore, Loaded, alias_name, resolve_node_in};
use crate::{Ctx, Done, Outcome};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::config::REPO_ROOT;
use umx_core::locator::SourceError;
use umx_core::ports::GraphDir;
use umx_core::{Command, Id, Locator, LocatorKind, Node, Problem, Value};
use umx_graph::Graph;

/// What a locator names, so two spellings of one source compare equal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Named {
    kind: LocatorKind,
    place: Place,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Place {
    /// A URL, or `root:<name>/<path>` (a machine's root by name): its text names it.
    Text(String),
    /// A file in a git repo: the repo top and the path inside it.
    Repo { top: String, path: String },
    /// A path in a graph folder that is not in git.
    Dir { dir: String, path: String },
}

/// The folder `dir` is in its repo, as a path inside `top` ("" for the top itself); None when it is not inside.
fn prefix<'a>(dir: &'a GraphDir, top: &GraphDir) -> Option<&'a str> {
    if !dir.is_within(top) {
        return None;
    }
    let rest = &dir.as_str()[top.as_str().len()..];
    Some(rest.trim_start_matches('/'))
}

fn joined(prefix: &str, path: &str) -> String {
    if prefix.is_empty() {
        path.to_string()
    } else {
        alloc::format!("{prefix}/{path}")
    }
}

/// What `l` names for the graph in `dir` (`top`: the git top level holding it, if any).
pub(crate) fn named(l: &Locator, dir: &GraphDir, top: Option<&GraphDir>) -> Named {
    // The normal form, so a hand-edited `./docs//a.md` names what `docs/a.md` names (review R255-2).
    let source = l.key_source();
    let in_repo = |path: &str| {
        top.and_then(|t| prefix(dir, t).map(|p| (t, p)))
            .map(|(t, p)| Place::Repo {
                top: t.as_str().into(),
                path: joined(p, path),
            })
    };
    let place = match l.kind {
        LocatorKind::Url => Place::Text(source.clone()),
        _ => match source.strip_prefix("root:") {
            Some(rest) => match rest.split_once('/') {
                // With no repo top the source names nothing outside this graph: a place local to it
                // (graph-addressing-expert).
                Some((REPO_ROOT, path)) => top.map_or_else(
                    || Place::Dir {
                        dir: dir.as_str().into(),
                        path: source.clone(),
                    },
                    |t| Place::Repo {
                        top: t.as_str().into(),
                        path: path.into(),
                    },
                ),
                _ => Place::Text(source.clone()),
            },
            None => in_repo(&source).unwrap_or(Place::Dir {
                dir: dir.as_str().into(),
                path: source.clone(),
            }),
        },
    };
    Named {
        kind: l.kind,
        place,
    }
}

fn spelled(kind: LocatorKind, source: String) -> Locator {
    Locator {
        kind,
        format: None,
        source,
        hash: None,
        length: None,
        seq_max: None,
        hash_state: None,
        hash_accepted: None,
        extra: Default::default(),
    }
}

/// Every spelling of `n` a node in the graph in `dir` could store: its index keys for that thing.
pub(crate) fn spellings(n: &Named, dir: &GraphDir, top: Option<&GraphDir>) -> Vec<Locator> {
    let mut out = Vec::new();
    match &n.place {
        Place::Text(text) => out.push(spelled(n.kind, text.clone())),
        Place::Repo { top: t, path } => {
            if top.is_some_and(|top| top.as_str() == t) {
                out.push(spelled(n.kind, alloc::format!("root:{REPO_ROOT}/{path}")));
                let inside = top.and_then(|top| prefix(dir, top)).and_then(|p| {
                    if p.is_empty() {
                        Some(path.as_str())
                    } else {
                        path.strip_prefix(p)?.strip_prefix('/')
                    }
                });
                if let Some(rel) = inside {
                    out.push(spelled(n.kind, rel.into()));
                }
            }
        }
        Place::Dir { dir: d, path } => {
            if dir.as_str() == d {
                out.push(spelled(n.kind, path.clone()));
            }
        }
    }
    out
}

/// The lowest id among the resource nodes in `g` holding any of `spellings`.
pub(crate) fn holder(g: &Graph, spellings: &[Locator]) -> Option<Id> {
    spellings
        .iter()
        .filter_map(|l| g.by_locator(l).map(|n| n.id()))
        .min()
}

/// A title for a new resource node from its source: the last path part (a folder's, a file's name), else a URL's
/// host. umx reads nothing, so a document's first heading waits for S4.
pub(crate) fn default_title(l: &Locator) -> String {
    let path = match l.kind {
        LocatorKind::Url => {
            let rest = l
                .source
                .split_once("://")
                .map_or(l.source.as_str(), |(_, r)| r);
            let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
            let path = path.split_once('?').map_or(path, |(p, _)| p);
            match path.rsplit('/').find(|p| !p.is_empty()) {
                Some(last) => return last.to_string(),
                None => return host.to_string(),
            }
        }
        _ => l.source.as_str(),
    };
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// An evidence-shaped `{"source": ...}`, so a locator's source meets evidence's checks and secret scan.
pub(crate) fn pointer(source: &str) -> Value {
    Value::Object(
        [("source".to_string(), Value::String(source.into()))]
            .into_iter()
            .collect(),
    )
}

/// `--locator` text as a checked locator for the graph in `dir`: parsed and normalised by umx-core, and a `root:`
/// source checked against its root as evidence's are. `verb` (`node add` or `node edit`) shapes the fix an absolute
/// path gets: the `root:` form to use instead.
pub(crate) fn checked_locator(
    text: &str,
    dir: &GraphDir,
    sources: &SourceCheck,
    verb: &'static str,
) -> Result<Locator, Problem> {
    let locator = Locator::parse(text).map_err(|e| match e {
        SourceError::Invalid(reason) => problems::locator_invalid(reason),
        SourceError::OutsideRoots => {
            problems::locator_invalid("the path leaves the graph folder or its root (.. or ~)")
        }
        SourceError::Absolute => {
            let in_repo = text.starts_with('/')
                && sources
                    .roots
                    .as_deref()
                    .and_then(|r| r.repo_top(dir))
                    .and_then(|top| GraphDir::parse(text).ok().map(|p| p.is_within(&top)))
                    .unwrap_or(false);
            let form = if in_repo {
                "root:repo/<path>"
            } else {
                "root:<name>/<path>"
            };
            let fix = Command::new(verb);
            let fix = if verb == "node add" {
                fix.word("--type").word("<type>")
            } else {
                fix.word("<node>")
            };
            problems::locator_absolute(fix.word("--locator").word(form))
        }
    })?;
    if locator.source.starts_with("root:") {
        sources.check(dir, &[pointer(&locator.source)])?;
    }
    let mut locator = locator;
    sources.hash_locator(dir, &mut locator);
    Ok(locator)
}

/// How many edges touch `node` (local or to another graph, each once), and how many evidence pointers in `g` cite what `old` names: what a re-pointed or
/// cleared locator leaves to check (`locator_changed`).
pub(crate) fn citations(
    g: &Graph,
    node: Id,
    old: &Named,
    dir: &GraphDir,
    top: Option<&GraphDir>,
) -> (usize, usize) {
    // Each edge once (review Q1): a self-loop is in both lists, and a link to or from another graph is in the
    // external lists.
    let edges = g.node(node).map_or(0, |n| {
        let ix = n.ix();
        let mut seen = alloc::collections::BTreeSet::new();
        seen.extend(g.edges_out(ix).map(|s| s.edge.index()));
        seen.extend(g.edges_in(ix).map(|s| s.edge.index()));
        seen.extend(g.edges_out_external(ix).map(|(e, ..)| e.index()));
        seen.extend(g.edges_in_external(ix).map(|(e, ..)| e.index()));
        seen.len()
    });
    let evidence = g
        .nodes()
        .map(|n| {
            n.to_node()
                .evidence
                .iter()
                .filter_map(|p| p.get("source").and_then(Value::as_str))
                .filter_map(|s| Locator::parse(s).ok())
                .filter(|l| named(l, dir, top).place == old.place)
                .count()
        })
        .sum();
    (edges, evidence)
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResourceShown {
    pub alias: String,
    pub node: Node,
}

impl GraphStore {
    /// The git top level holding `dir`, if the machine's source roots allow looking (the Graphfile's `repo_top`
    /// asks the config files port instead).
    pub(crate) fn source_repo_top(&self, dir: &GraphDir) -> Option<GraphDir> {
        self.source_roots.as_deref().and_then(|r| r.repo_top(dir))
    }

    /// `resource_elsewhere` for each other graph holding what `l` names (the graph in `dir`, alias `own`). Only
    /// graphs a read that names none would show are asked (d-20, security and graph-addressing-expert): a
    /// secured graph is skipped silently, with no alias, id or count. Each other graph id is asked once, through its
    /// primary copy, and copies of `own`'s graph are not other graphs (d-21). A write names one graph, so no other
    /// graph is ever named explicitly.
    pub(crate) fn resource_elsewhere(
        &self,
        own: &str,
        l: &Locator,
        dir: &GraphDir,
    ) -> Vec<Problem> {
        let n = named(l, dir, self.source_repo_top(dir).as_ref());
        let own_id = self.graphs[own].meta.id;
        let mut ids: Vec<Id> = self.graphs.values().map(|g| g.meta.id).collect();
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter()
            .filter(|id| *id != own_id)
            .filter_map(|id| self.primary_of(id))
            .filter(|alias| self.shown_unnamed(alias))
            .filter_map(|alias| {
                let other: &Loaded = &self.graphs[alias];
                let top = self.source_repo_top(&other.dir);
                holder(&other.graph, &spellings(&n, &other.dir, top.as_ref()))
                    .map(|id| problems::resource_elsewhere(&alias_name(alias), id))
            })
            .collect()
    }

    /// `umx resource show <node>`: a resource node and its locator; a plain node is `not_a_resource`.
    pub fn resource_show(&self, ctx: &Ctx, node: &str) -> Outcome<ResourceShown> {
        let alias = self.select(ctx, &[Some(node)])?;
        let l = self.get(&alias);
        // A read like `node get`: a node the caller's scope hides (a provenance record) is not found (F185).
        let id = resolve_node_in(l, ctx, &alias, node)?;
        let node = l.graph.node(id).expect("resolved").to_node();
        if node.locator.is_none() {
            return Err(problems::not_a_resource(id));
        }
        Ok(Done {
            value: ResourceShown { alias, node },
            notes: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(p: &str) -> GraphDir {
        GraphDir::parse(p).unwrap()
    }

    /// graph-addressing-expert N3: a graph-folder path and its root:repo/ form are one thing, in this graph and
    /// in another graph of the same repo; a different repo's root:repo/ is not.
    #[test]
    fn a_graph_folder_path_and_its_repo_form_name_one_file() {
        let top = dir("/work");
        let g = dir("/work/graphs/a");
        let rel = Locator::parse("docs/brief.md").unwrap();
        let repo = Locator::parse("root:repo/graphs/a/docs/brief.md").unwrap();
        assert_eq!(named(&rel, &g, Some(&top)), named(&repo, &g, Some(&top)));
        let keys = |s: &[Locator]| s.iter().map(|l| l.source.clone()).collect::<Vec<_>>();
        assert_eq!(
            keys(&spellings(&named(&rel, &g, Some(&top)), &g, Some(&top))),
            ["root:repo/graphs/a/docs/brief.md", "docs/brief.md"]
        );
        let other = dir("/work/graphs/b");
        assert_eq!(
            keys(&spellings(&named(&rel, &g, Some(&top)), &other, Some(&top))),
            ["root:repo/graphs/a/docs/brief.md"]
        );
        assert_eq!(
            keys(&spellings(
                &named(&rel, &g, Some(&top)),
                &dir("/elsewhere/g"),
                Some(&dir("/elsewhere"))
            )),
            Vec::<String>::new()
        );
        assert_eq!(
            keys(&spellings(&named(&rel, &g, None), &g, None)),
            ["docs/brief.md"],
            "not in git: the graph folder names it"
        );
        let folder = Locator::parse("docs/").unwrap();
        assert_ne!(named(&folder, &g, Some(&top)), named(&rel, &g, Some(&top)));
    }

    #[test]
    fn a_default_title_is_the_last_part_or_the_host() {
        for (text, title) in [
            ("docs/brief.md", "brief.md"),
            ("transcripts/", "transcripts"),
            ("root:repo/specs/a.md", "a.md"),
            ("https://example.com/a/b.html?x=1", "b.html"),
            ("https://example.com/", "example.com"),
        ] {
            assert_eq!(
                default_title(&Locator::parse(text).unwrap()),
                title,
                "{text}"
            );
        }
    }
}
