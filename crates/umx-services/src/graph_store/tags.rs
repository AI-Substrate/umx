//! Std tags in the services (ws008 t2b, req-117): the checks every write and filter shares, so node add, node edit,
//! a batch line and `ontology add tag` refuse and warn alike. The tag rules themselves are umx-core's
//! ([`umx_core::tags`]); the index is umx-graph's.

use super::problems;
use alloc::string::String;
use alloc::vec::Vec;
use umx_core::tags::{self, Pattern, TagRefused, TagsRefused};
use umx_core::value::SetValue;
use umx_core::{Command, Ontology, Problem, ValidName, Value};

/// Whether the graph uses std, so `tags` is the std tag list.
pub(crate) fn is_std(o: &Ontology) -> bool {
    o.std_version().is_some()
}

/// `c` with a tag after it, from checked parts (`*` as a facet's wildcard); None if a part is not a name.
pub(crate) fn with_tag(c: Command, facet: Option<&str>, value: &str) -> Option<Command> {
    let facet = facet.map(ValidName::alias);
    match (facet, value) {
        (Some(Some(f)), tags::ANY_VALUE) => Some(c.name_with(&f, ':', "*")),
        (Some(Some(f)), v) => Some(c.names(&[f, ValidName::alias(v)?], ":")),
        (Some(None), _) => None,
        (None, v) => Some(c.name(&ValidName::alias(v)?)),
    }
}

fn docs() -> Command {
    Command::new("docs get").word("ontology")
}

/// Whether tag text looks like a secret, as the write scan reads a tag ([`super::secrets`]): as stored, and upper
/// cased, since a tag is stored lowercased (security B1).
pub(crate) fn secret_shaped(text: &str) -> bool {
    umx_core::secrets::field_suspect(tags::TAGS, text)
        .or_else(|| umx_core::secrets::field_suspect(tags::TAGS, &text.to_ascii_uppercase()))
        .is_some()
}

/// How a flag's values are read: tags to write, or patterns (`<facet>:*` too) to match or remove.
#[derive(Clone, Copy)]
pub(crate) enum Reads {
    Tags,
    Patterns,
}

/// The command a refused tag's next repeats (junglefowl F2): `base` (graph flags included), then each tag flag with
/// every value given, each part in stored form and the refused one replaced by its snake_case form, commas kept.
pub(crate) struct TagRetry<'a> {
    pub base: Command,
    /// Each flag (empty for a positional), its values as given, and how they are read.
    pub flags: Vec<(&'static str, &'a [String], Reads)>,
}

impl TagRetry<'_> {
    /// One part as the retry gives it: in stored form, or its fix; None when it has none or looks like a secret, so
    /// derived text never reaches a next (security N1).
    fn part(text: &str, reads: Reads) -> Option<String> {
        let read = match reads {
            Reads::Tags => tags::normalise_tag(text),
            Reads::Patterns => Pattern::parse(text).map(|p| p.text()),
        };
        let out = match read {
            Ok(t) => t,
            Err(r) => match r.fixed? {
                (Some(f), v) => alloc::format!("{f}:{v}"),
                (None, v) => v,
            },
        };
        (!secret_shaped(text) && !secret_shaped(&out)).then_some(out)
    }

    /// `base` and the tag flags again with fixed tags, else the tags docs (also when it would be too long to print as
    /// written).
    pub(crate) fn fix(&self) -> Command {
        let mut c = self.base.clone();
        for (flag, values, reads) in &self.flags {
            for v in values.iter() {
                let parts: Option<Vec<String>> = v
                    .split(',')
                    .filter(|p| !p.trim().is_empty())
                    .map(|p| Self::part(p, *reads))
                    .collect();
                let flagged = if flag.is_empty() {
                    c.clone()
                } else {
                    c.clone().word(flag)
                };
                let Some(next) = parts.and_then(|p| flagged.tags(&p)) else {
                    return docs();
                };
                c = next;
            }
        }
        // A next longer than a checked command keeps would be cleaned, so it could not run as printed (F6).
        if c.as_str().len() > Command::MAX {
            return docs();
        }
        c
    }

    /// `tag_invalid` for `r`: [`Self::fix`] when `r` has a fix, else the tags docs.
    pub(crate) fn refused(&self, r: &TagRefused) -> Problem {
        let fix = match r.fixed {
            Some(_) => self.fix(),
            None => docs(),
        };
        problems::tag_invalid(r.reason, fix)
    }
}

/// The refusal for a tags value: a bad tag's next is `retry`'s fix, if any, else the tags docs (as for `--set tags`
/// and batch lines, which have no command to repeat).
pub(crate) fn refused(r: &TagsRefused, retry: Option<&TagRetry<'_>>) -> Problem {
    match r {
        TagsRefused::Tag(t) => retry.map_or_else(
            || problems::tag_invalid(t.reason, docs()),
            |retry| retry.refused(t),
        ),
        TagsRefused::NotAString => problems::tag_invalid("a tag must be a string", docs()),
        TagsRefused::TooMany(n) => problems::tag_limit(*n, docs()),
    }
}

fn empty() -> Problem {
    problems::tag_invalid("a tag is empty", docs())
}

/// `--tag` values (each may be a comma list) in stored form: sorted, repeats dropped, at most 64. A value with no tag
/// in it (`--tag ""`) is refused.
pub(crate) fn given(values: &[String], retry: &TagRetry<'_>) -> Result<Vec<String>, Problem> {
    let mut out = Vec::new();
    for v in values {
        let got = tags::normalise_tags(&Value::String(v.clone()))
            .map_err(|r| refused(&r, Some(retry)))?;
        if got.is_empty() {
            return Err(empty());
        }
        out.extend(got);
    }
    tags::finish(out).map_err(|r| refused(&r, None))
}

/// `--untag` values as patterns (a tag or `<facet>:*`).
pub(crate) fn patterns(values: &[String], retry: &TagRetry<'_>) -> Result<Vec<Pattern>, Problem> {
    let mut out = Vec::new();
    for v in values {
        let before = out.len();
        for part in v.split(',').filter(|p| !p.trim().is_empty()) {
            out.push(Pattern::parse(part).map_err(|r| retry.refused(&r))?);
        }
        if out.len() == before {
            return Err(empty());
        }
    }
    Ok(out)
}

/// A `--set tags=...` value in a std graph, in stored form.
pub(crate) fn set_value(value: &SetValue) -> Result<Vec<String>, Problem> {
    let given = match value {
        SetValue::Raw(s) | SetValue::Str(s) => Value::String(s.clone()),
        SetValue::Json(v) => v.clone(),
    };
    tags::normalise_tags(&given).map_err(|r| refused(&r, None))
}

/// The value a node stores for `tags`: none when empty.
pub(crate) fn stored(tags: Vec<String>) -> Option<Value> {
    (!tags.is_empty()).then(|| Value::Array(tags.into_iter().map(Value::String).collect()))
}

/// `tag_off_vocabulary` for each of `tags` its graph's declarations leave out (see [`tags::off_vocabulary`]).
pub(crate) fn off_vocabulary(graph: &ValidName, o: &Ontology, tags: &[String]) -> Vec<Problem> {
    tags::off_vocabulary(tags, &o.tags)
        .into_iter()
        .filter_map(|t| {
            let (facet, value) = tags::split(t);
            let facet_name = ValidName::alias(facet?)?;
            let fix = with_tag(Command::new("ontology add tag"), facet, value)?
                .word("--graph")
                .name(graph);
            Some(problems::tag_off_vocabulary(graph, &facet_name, fix))
        })
        .collect()
}

/// `--tag` and `--no-tag` values as a filter: each `--tag` a group of comma-separated patterns, every `--no-tag`
/// pattern in one list.
pub(crate) fn filter(
    all: &[String],
    none: &[String],
    retry: &TagRetry<'_>,
) -> Result<umx_graph::TagFilter, Problem> {
    let group = |text: &str| -> Result<Vec<Pattern>, Problem> {
        let mut out: Vec<Pattern> = Vec::new();
        for part in text.split(',').filter(|p| !p.trim().is_empty()) {
            let p = Pattern::parse(part).map_err(|r| retry.refused(&r))?;
            if !out.contains(&p) {
                out.push(p);
            }
        }
        if out.is_empty() {
            return Err(empty());
        }
        Ok(out)
    };
    let mut f = umx_graph::TagFilter::default();
    for a in all {
        f.all.push(group(a)?);
    }
    for n in none {
        f.none.extend(group(n)?);
    }
    Ok(f)
}

/// The tags `n`'s value holds after `--tag add` and `--untag remove` (patterns), kept with any entry umx cannot use;
/// None when nothing is left. Also the tags this edit added, for `tag_off_vocabulary`.
pub(crate) fn edited(
    current: Option<&Value>,
    add: &[String],
    remove: &[Pattern],
) -> Result<(Option<Value>, Vec<String>), Problem> {
    let have = current.map(tags::stored_tags).unwrap_or_default();
    let mut next: Vec<String> = have.iter().chain(add).cloned().collect();
    next.retain(|t| !remove.iter().any(|p| p.matches(t)));
    let next = tags::finish(next).map_err(|r| refused(&r, None))?;
    let added: Vec<String> = next.iter().filter(|t| !have.contains(t)).cloned().collect();
    // Entries the index does not hold stay as they are (junglefowl F3): ones umx cannot read, and tags past the
    // first 64 of a hand-edited value.
    let kept: Vec<Value> = match current {
        Some(Value::Array(items)) => items
            .iter()
            .filter(|i| match i.as_str().map(tags::normalise_tag) {
                Some(Ok(t)) => !have.contains(&t) && !remove.iter().any(|p| p.matches(&t)),
                _ => true,
            })
            .cloned()
            .collect(),
        _ => Vec::new(),
    };
    let all: Vec<Value> = next.into_iter().map(Value::String).chain(kept).collect();
    Ok((
        (!all.is_empty()).then(|| tags::canonical(&Value::Array(all))),
        added,
    ))
}

/// Whether a read may cover several graphs (ws008 t2b): `--graph a,b`, `--scope`, `--all`, or no graph named (nor a
/// default) while `loaded` graphs are loaded, more than one. A graph in it without std is skipped with
/// `tag_filter_not_std`; only a read of a single graph is refused with `tag_needs_std`.
pub(crate) fn several(ctx: &crate::Ctx, loaded: usize) -> bool {
    ctx.scope.is_some()
        || ctx.graph.as_deref().is_some_and(|g| g.contains(','))
        || (ctx.graph.is_none() && ctx.default_graph.is_none() && loaded > 1)
}

/// `cmd` with the graph a tag next repeats: a single named graph as named, and a read of several as `first`, its
/// first std graph (t2b-p1 ruling), else none.
pub(crate) fn read_graph(
    ctx: &crate::Ctx,
    cmd: Command,
    first: Option<&str>,
    loaded: usize,
) -> Command {
    if !several(ctx, loaded) {
        return ctx.same_graph(cmd);
    }
    match first.and_then(ValidName::alias) {
        Some(g) => cmd.word("--graph").name(&g),
        None => cmd,
    }
}

/// A read's `--tag` and `--no-tag` as a filter, None when it has neither; a refused tag's next is `base` and the tag
/// flags again, fixed ([`TagRetry`]).
pub(crate) fn read_filter(
    tags: &[String],
    no_tags: &[String],
    base: Command,
) -> Result<Option<umx_graph::TagFilter>, Problem> {
    if tags.is_empty() && no_tags.is_empty() {
        return Ok(None);
    }
    let retry = TagRetry {
        base,
        flags: alloc::vec![
            ("--tag", tags, Reads::Patterns),
            ("--no-tag", no_tags, Reads::Patterns),
        ],
    };
    filter(tags, no_tags, &retry).map(Some)
}

/// [`read_filter`], where a refused tag's next is the tags docs when there is no `base` that could run as printed.
pub(crate) fn read_filter_or_docs(
    tags: &[String],
    no_tags: &[String],
    base: Option<Command>,
) -> Result<Option<umx_graph::TagFilter>, Problem> {
    let Some(base) = base else {
        return read_filter(tags, no_tags, docs()).map_err(|mut p| {
            for (key, slot) in &mut p.slots {
                if *key == "fix" {
                    *slot = umx_core::problem::Slot::Command(docs());
                }
            }
            p
        });
    };
    read_filter(tags, no_tags, base)
}

/// A tag filter applied to rows a read found another way (walk's listed nodes, neighbours' far ends; ws008 t2b). A row
/// whose node the read cannot show (its graph not shown or not loaded, the node missing) is never tested: it is left
/// out and only counted (d-20), so neither `--tag` nor `--no-tag` tells anything about it. A row in a graph without
/// std is left out and its graph counted.
pub(crate) struct RowTags {
    filter: umx_graph::TagFilter,
    not_std: alloc::collections::BTreeSet<umx_core::Id>,
    untested: usize,
}

impl RowTags {
    pub(crate) fn new(filter: umx_graph::TagFilter) -> RowTags {
        RowTags {
            filter,
            not_std: Default::default(),
            untested: 0,
        }
    }

    /// Counts graph `id` as skipped for having no std, whether or not a row lands in it.
    pub(crate) fn skip(&mut self, id: umx_core::Id) {
        self.not_std.insert(id);
    }

    /// Whether the row of `node` stays: `graph` is the loaded graph the read shows it in, with that graph's id, or
    /// None when the read cannot show it.
    pub(crate) fn keeps(
        &mut self,
        graph: Option<(&umx_graph::Graph, umx_core::Id)>,
        node: umx_core::Id,
    ) -> bool {
        let Some((g, id)) = graph else {
            self.untested += 1;
            return false;
        };
        if !g.std_tags() {
            self.not_std.insert(id);
            return false;
        }
        match g.node(node) {
            Some(n) => g.tags_pass(n, &self.filter),
            None => {
                self.untested += 1;
                false
            }
        }
    }

    /// `tag_filter_not_std` and `tag_filter_untested`, each carrying a count alone, when any.
    pub(crate) fn notes(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        if !self.not_std.is_empty() {
            out.push(problems::tag_filter_not_std(self.not_std.len()));
        }
        if self.untested > 0 {
            out.push(problems::tag_filter_untested(self.untested));
        }
        out
    }
}
