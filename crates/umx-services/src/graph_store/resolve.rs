//! `umx resolve` (req-15 in part, ws006 S0; workshop 002 sections 7.5 and 7.9) and the check of `umx:` evidence
//! sources. Read-only over the loaded graphs: nothing here loads a graph. Resolve's graph_not_loaded error names
//! the alias as its subject, so the daemon's on-demand load (F72) may load a remembered graph and retry; evidence
//! checks only warn, and a warning never starts that load.

use super::{
    Ctx, Done, GraphStore, Outcome, alias_name, problems, resolve_in, resolve_read_node,
    split_address,
};
use alloc::borrow::Cow;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use umx_core::address::{Fragment as Parsed, GRAPH_FRAGMENT_REASON, parse_fragment};
use umx_core::{Command, Edge, Id, IdText, Node, Problem, ValidName, Value};
use umx_graph::Graph;

/// An address in its parts. [`split`] is the one place addresses are split by hand until ws006 S2's
/// `umx_core::address::parse_address` replaces it.
pub(crate) struct Parts<'a> {
    /// The address with `#...` and `?repo=...` taken off: `umx:<alias>/<id>` or a bare id.
    pub body: &'a str,
    pub alias: Option<&'a str>,
    pub id: &'a str,
    pub fragment: Option<&'a str>,
    /// `?repo=` and all after it, as written.
    pub repo_part: Option<&'a str>,
}

impl Parts<'_> {
    pub fn repo(&self) -> Option<&str> {
        self.repo_part.map(|r| &r["?repo=".len()..])
    }
}

/// The address with its scheme folded to `umx:` (`UMX:` and `Umx:` are the same scheme); alias and id are left as
/// written (F3).
pub(crate) fn fold_scheme(text: &str) -> Cow<'_, str> {
    match text.get(..4) {
        Some(scheme) if scheme != "umx:" && scheme.eq_ignore_ascii_case("umx:") => {
            Cow::Owned(format!("umx:{}", &text[4..]))
        }
        _ => Cow::Borrowed(text),
    }
}

/// `?repo=` is split off first, then `#<fragment>`, then `umx:<alias>/` (workshop 002 section 7.9). The scheme must
/// already be folded ([`fold_scheme`]).
pub(crate) fn split(text: &str) -> Parts<'_> {
    let (rest, repo_part) = match text.find("?repo=") {
        Some(i) => (&text[..i], Some(&text[i..])),
        None => (text, None),
    };
    let (body, fragment) = match rest.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (rest, None),
    };
    let (alias, id) = split_address(body);
    Parts {
        body,
        alias,
        id,
        fragment,
        repo_part,
    }
}

/// What follows `#`, parsed by the address grammar (ws006 S2): a field name, or selectors in canonical form.
pub enum Fragment {
    Field(ValidName),
    Selector(String),
}

/// One grammar for every fragment (graph-addressing-expert F1): what S2 refuses, resolve refuses, with the
/// grammar's reason.
fn fragment(text: &str) -> Result<Fragment, Problem> {
    match parse_fragment(text) {
        Ok(Parsed::Selectors(s)) => Ok(Fragment::Selector(s.canonical())),
        Ok(Parsed::Field(name)) => ValidName::alias(&name)
            .map(Fragment::Field)
            .ok_or_else(|| problems::usage_invalid("a field name is snake_case", "explore")),
        Err(e) => Err(problems::usage_invalid(e.reason, "explore")),
    }
}

/// How an address resolved (ADDR-4 in workshop 002). Every status but `Ok` comes with the problem it is refused with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveStatus {
    Ok,
    /// An id prefix matching several nodes (candidates in the problem).
    Ambiguous,
    GraphNotLoaded,
    NodeNotFound,
    /// No graph named, and copies of one graph with none primary (d-21).
    CopyAmbiguous,
    /// `#<field>` names a field the node or edge does not have.
    FieldNotFound,
    /// The address named a former alias (ws006 S23, req-56): it resolved in the graph that had it, now under its
    /// live alias (`Resolved::redirected_from` holds the name given).
    Redirected,
}

impl ResolveStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ResolveStatus::Ok => "ok",
            ResolveStatus::Ambiguous => "ambiguous",
            ResolveStatus::GraphNotLoaded => "graph_not_loaded",
            ResolveStatus::NodeNotFound => "node_not_found",
            ResolveStatus::CopyAmbiguous => "copy_ambiguous",
            ResolveStatus::FieldNotFound => "field_not_found",
            ResolveStatus::Redirected => "redirected",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Node(Node),
    Edge(Edge),
}

impl Target {
    pub fn id(&self) -> Id {
        match self {
            Target::Node(n) => n.id,
            Target::Edge(e) => e.id,
        }
    }
}

/// What `umx resolve` found.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub status: ResolveStatus,
    /// The alias the address resolved in, when a graph was chosen.
    pub alias: Option<String>,
    pub graph_id: Option<Id>,
    pub target: Option<Target>,
    /// `#<field>` and its value.
    pub field: Option<(String, Value)>,
    /// A canonical `#<selector>`; not read yet (ws006 S4 reads passages).
    pub selector: Option<String>,
    pub repo: Option<String>,
    /// Why it did not resolve, for every status but `Ok` and `Redirected`.
    pub problem: Option<Problem>,
    /// For `Redirected`: the former alias the address gave.
    pub redirected_from: Option<String>,
}

impl Resolved {
    fn failed(
        status: ResolveStatus,
        alias: Option<String>,
        repo: Option<&str>,
        p: Problem,
    ) -> Resolved {
        Resolved {
            status,
            alias,
            graph_id: None,
            target: None,
            field: None,
            selector: None,
            repo: repo.map(ToString::to_string),
            problem: Some(p),
            redirected_from: None,
        }
    }
}

/// A full id given as text, if it is one.
pub(crate) fn full_id(text: &str) -> Option<Id> {
    IdText::new(text).and_then(|t| t.as_str().parse::<Id>().ok())
}

impl GraphStore {
    /// What `text` (an address, or an id or prefix in the graph a command selects, workshop 007 rule 6) names in
    /// the loaded graphs. A usage error is the only refusal; every other outcome is a [`ResolveStatus`].
    pub fn resolve(&self, ctx: &Ctx, text: &str) -> Outcome<Resolved> {
        let text = fold_scheme(text.trim());
        let parts = split(&text);
        if parts.body.is_empty() {
            return Err(problems::usage_invalid(
                "give an address umx:<graph>/<id> or an id",
                "explore",
            ));
        }
        if parts.alias.is_some() && parts.id.is_empty() && parts.fragment.is_some() {
            return Err(problems::usage_invalid(GRAPH_FRAGMENT_REASON, "explore"));
        }
        // `umx:<alias>@<copy>/<id>` is planned (req-69, usage 016). Until then a copy loads under its own alias, and
        // the `@` form would otherwise fall through to a graph_not_loaded that cannot name the alias.
        if parts.alias.is_some_and(|a| a.contains('@')) {
            return Err(problems::usage_invalid(
                "a copy loads under its own alias (write umx:<copy alias>/<id>) and the copy form is planned (req-69)",
                "explore",
            ));
        }
        let repo = parts.repo();
        let refs = [Some(parts.body)];
        let alias = match self.select(ctx, &refs) {
            Ok(a) => a,
            Err(p) if p.code == "graph_not_loaded" => {
                let named = parts
                    .alias
                    .or(ctx.graph.as_deref())
                    .or(ctx.default_graph.as_deref())
                    .unwrap_or_default();
                let p = match repo {
                    Some(r) => problems::graph_not_loaded_hint(named, r),
                    None => p,
                };
                let status = ResolveStatus::GraphNotLoaded;
                return Ok(Done::ok(Resolved::failed(
                    status,
                    Some(named.into()),
                    repo,
                    p,
                )));
            }
            Err(p) if p.code == "copy_ambiguous" => {
                let status = ResolveStatus::CopyAmbiguous;
                return Ok(Done::ok(Resolved::failed(status, None, repo, p)));
            }
            Err(p) => return Err(p),
        };
        let l = self.get(&alias);
        let warnings: Vec<Problem> = self
            .copies_note(ctx, &refs, &alias)
            .into_iter()
            .chain(self.stale_note(&alias))
            .collect();
        let fail = |status, p| {
            Ok(Done::ok(Resolved::failed(
                status,
                Some(alias.clone()),
                repo,
                p,
            )))
        };
        let target = match resolve_read_node(l, ctx, &alias, parts.body) {
            Ok(id) if id == l.meta.id => Target::Node(self.virtual_node(&alias)),
            Ok(id) => Target::Node(l.graph.node(id).expect("resolved").to_node()),
            Err(p) if p.code == "id_ambiguous" => return fail(ResolveStatus::Ambiguous, p),
            Err(p) if p.code == "node_not_found" => {
                match full_id(parts.id).and_then(|id| self.edge_read_row(ctx, &refs, &alias, id)) {
                    Some(row) => Target::Edge(row.edge.into_stored().expect("stored address")),
                    None => return fail(ResolveStatus::NodeNotFound, p),
                }
            }
            Err(p) => return Err(p),
        };
        if target.id() == l.meta.id && parts.fragment.is_some() {
            return Err(problems::usage_invalid(GRAPH_FRAGMENT_REASON, "explore"));
        }
        let fragment = parts.fragment.map(fragment).transpose()?;
        let (mut field, mut selector) = (None, None);
        match fragment {
            Some(Fragment::Field(name)) => {
                let value = match &target {
                    Target::Node(n) => n.field(name.as_str()),
                    Target::Edge(e) => l.graph.edge(e.id).and_then(|r| r.field(name.as_str())),
                };
                match value {
                    Some(v) => field = Some((name.as_str().to_string(), v)),
                    None => {
                        let graph = alias_name(&alias);
                        let id = target.id();
                        let fix = match &target {
                            Target::Node(_) => Command::new("node get").address(&graph, id),
                            Target::Edge(_) => {
                                Command::new("edge list").word("--graph").name(&graph)
                            }
                        };
                        let p = problems::field_not_found(&graph, id, &name, fix);
                        return fail(ResolveStatus::FieldNotFound, p);
                    }
                }
            }
            Some(Fragment::Selector(s)) => selector = Some(s),
            None => {}
        }
        let redirected_from = parts
            .alias
            .filter(|a| *a != alias && a.parse::<Id>().is_err())
            .map(ToString::to_string);
        Ok(Done {
            value: Resolved {
                status: if redirected_from.is_some() {
                    ResolveStatus::Redirected
                } else {
                    ResolveStatus::Ok
                },
                redirected_from,
                graph_id: Some(l.meta.id),
                alias: Some(alias),
                target: Some(target),
                field,
                selector,
                repo: repo.map(ToString::to_string),
                problem: None,
            },
            notes: warnings,
        })
    }
}

/// What an evidence lookup finds for a graph name (None: no graph it may check against, `evidence_unchecked`).
pub(crate) type Found<'g> = Option<Evidence<'g>>;

/// A graph name an evidence lookup knows.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Evidence<'g> {
    /// The graph's id, the graph and its live alias (ws006 S23: the name may be a former alias).
    Graph(Id, &'g Graph, &'g str),
    /// A former alias of two or more graphs the lookup may show: umx never picks one (review R16).
    Ambiguous,
}

/// Checks each `umx:` source among new evidence `pointers` against the loaded graphs (`lookup` gives one by alias)
/// and returns the warnings; missing targets only warn (evidence is provenance, so ingest order must not matter).
/// Graph fragments are refused before parsing by [`check_evidence_fragment`]. A source in a
/// loaded graph that names neither a single stored node nor an edge gets `dangling_evidence`;
/// one in a graph that is not loaded gets
/// `evidence_unchecked`. A short prefix that finds one node is stored as the full id, alias kept as written, so the
/// pointer cannot become ambiguous as the graph grows.
/// It must never load a graph: a load would be a side effect of writing to another graph, and an oracle into
/// secured graphs (graph-addressing-expert, ws006 S0). This holds by construction: `lookup` hands out shared
/// borrows of graphs already loaded (the written graph and open graphs only, so a secured graph reads exactly as an
/// unloaded one: security B1), so nothing here can reach the store's load path, and the only results are
/// warnings, which never start the daemon's on-demand load (F72 retries on a graph_not_loaded error only). An
/// unloaded graph's sources are kept as `evidence_unchecked`. The CLI test
/// `umx_evidence_is_kept_and_checked_against_loaded_graphs_only` checks the graph list is unchanged after the write.
pub(crate) fn check_evidence<'g>(
    pointers: &mut [Value],
    lookup: &dyn Fn(&str) -> Found<'g>,
) -> Vec<Problem> {
    let mut warnings = Vec::new();
    for p in pointers.iter_mut() {
        let Some(source) = p.get("source").and_then(Value::as_str) else {
            continue;
        };
        if !source
            .get(..4)
            .is_some_and(|s| s.eq_ignore_ascii_case("umx:"))
        {
            continue;
        }
        // Stored with the scheme folded, so every later reader sees `umx:`.
        let source = fold_scheme(source).into_owned();
        if p["source"].as_str() != Some(source.as_str()) {
            p["source"] = Value::String(source.clone());
        }
        let parts = split(&source);
        let Some(alias) = parts.alias else {
            warnings.push(problems::dangling_evidence(
                "",
                "it is not an address umx:<graph>/<id>",
                Vec::new(),
            ));
            continue;
        };
        let (graph_id, graph, live) = match lookup(alias) {
            Some(Evidence::Graph(id, graph, live)) => (id, graph, live),
            Some(Evidence::Ambiguous) => {
                warnings.push(problems::dangling_evidence(
                    alias,
                    "two or more loaded graphs once had this alias (name one by its live alias)",
                    Vec::new(),
                ));
                continue;
            }
            None => {
                warnings.push(problems::evidence_unchecked(alias));
                continue;
            }
        };
        // ws006 S23 (graph-addressing-expert): a new pointer through a former alias is stored under the live alias,
        // as a short prefix is stored as the full id; pointers already stored are never rewritten.
        if live != alias {
            warnings.push(problems::alias_redirected(alias, live, None));
            let fragment = &source[source.find('/').unwrap_or(source.len())..];
            p["source"] = Value::String(format!("umx:{live}{fragment}"));
        }
        let alias = live;
        if full_id(parts.id) == Some(graph_id) {
            continue;
        }
        match resolve_in(graph, alias, parts.id) {
            Ok(id) => {
                if parts.id != id.to_text() {
                    let full = format!("umx:{alias}/{id}{}", parts.repo_part.unwrap_or_default());
                    p["source"] = Value::String(full);
                }
            }
            Err(e) if e.code == "id_ambiguous" => warnings.push(problems::dangling_evidence(
                alias,
                "the id prefix matches several nodes",
                e.candidates,
            )),
            Err(e) => {
                if full_id(parts.id)
                    .and_then(|id| graph.edge(id))
                    .filter(|edge| super::ReadScope::new(graph).stored_edge(*edge))
                    .is_none()
                {
                    warnings.push(problems::dangling_evidence(
                        alias,
                        "no node has that id",
                        e.candidates,
                    ));
                }
            }
        }
    }
    warnings
}

pub(crate) fn check_evidence_fragment<'g>(
    text: &str,
    lookup: &dyn Fn(&str) -> Found<'g>,
) -> Result<(), Problem> {
    let text = fold_scheme(text);
    let parts = split(&text);
    if parts.fragment.is_some()
        && let Some(alias) = parts.alias
        && let Some(Evidence::Graph(graph_id, _, _)) = lookup(alias)
        && full_id(parts.id) == Some(graph_id)
    {
        return Err(problems::evidence_pointer_invalid(GRAPH_FRAGMENT_REASON));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_then_fragment_then_alias_split_off() {
        let p = split("umx:a/01JC#seq=1-2?repo=https://x/y.git");
        assert_eq!(
            (p.body, p.alias, p.id, p.fragment, p.repo()),
            (
                "umx:a/01JC",
                Some("a"),
                "01JC",
                Some("seq=1-2"),
                Some("https://x/y.git")
            )
        );
        let p = split("01JC");
        assert_eq!(
            (p.alias, p.id, p.fragment, p.repo_part),
            (None, "01JC", None, None)
        );
    }

    #[test]
    fn only_the_scheme_is_folded() {
        for (given, folded) in [
            ("UMX:a/01JC", "umx:a/01JC"),
            ("Umx:Ab/01jc#L1", "umx:Ab/01jc#L1"),
            ("umx:a/01JC", "umx:a/01JC"),
            ("UMXa/01JC", "UMXa/01JC"),
            ("01jc", "01jc"),
        ] {
            assert_eq!(fold_scheme(given), folded);
        }
        assert!(matches!(fold_scheme("umx:a/01JC"), Cow::Borrowed(_)));
        let folded = fold_scheme("UMX:a/01JC");
        let p = split(&folded);
        assert_eq!((p.alias, p.id), (Some("a"), "01JC"));
    }

    #[test]
    fn a_fragment_is_parsed_by_the_address_grammar() {
        for (s, canonical) in [
            ("seq=1-3", "seq=1-3"),
            ("L10", "L10"),
            ("L10-L24", "L10-L24"),
            ("L7-L7", "L7"),
            ("quote=a b", "quote=a%20b"),
        ] {
            assert!(
                matches!(fragment(s), Ok(Fragment::Selector(c)) if c == canonical),
                "{s}"
            );
        }
        for f in ["problem", "l10", "title"] {
            assert!(matches!(fragment(f), Ok(Fragment::Field(_))), "{f}");
        }
        // What S2 refuses, resolve refuses: a backwards range, an unknown key, a field that is not snake_case.
        for bad in ["", "seq=3-1", "t=1-2", "Problem", "Lx"] {
            let p = fragment(bad).err().unwrap_or_else(|| panic!("{bad}"));
            assert_eq!(p.code, "usage_invalid", "{bad}");
        }
    }
}
