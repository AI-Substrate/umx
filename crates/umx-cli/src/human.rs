//! Human output (req-63, req-64): banner, aligned tables and colour. JSON output never comes through here.
//! Colour is a few hand-rolled ANSI SGR codes, on only when [`Style::on`]. Every string that may come from a
//! graph goes through [`clean`] before it is printed, so graph text can never drive the terminal.

use umx_protocol::place::{self, Spot};
use umx_protocol::{Envelope, Issue, Value};

/// Whether one output stream gets colour: `color` from the config when set, else whether it is a terminal.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub on: bool,
}

impl Style {
    pub fn new(color: Option<bool>, terminal: bool) -> Style {
        Style {
            on: color.unwrap_or(terminal),
        }
    }

    fn paint(self, sgr: &str, s: &str) -> String {
        if self.on && !s.is_empty() {
            format!("\x1b[{sgr}m{s}\x1b[0m")
        } else {
            s.to_owned()
        }
    }
}

const BOLD: &str = "1";
const DIM: &str = "2";
const RED: &str = "1;31";
const GREEN: &str = "32";
const YELLOW: &str = "1;33";
const CYAN: &str = "36";
const HEADING: &str = "1;36";

/// The umx wordmark, one colour per row (cyan to violet).
const BANNER: [(&str, &str); 4] = [
    ("38;5;51", "██    ██  ███▄  ▄███  ▀██▄  ▄██▀"),
    ("38;5;81", "██    ██  ██ ▀██▀ ██    ▀████▀  "),
    ("38;5;111", "██    ██  ██      ██    ▄████▄  "),
    ("38;5;141", " ▀████▀   ██      ██  ▄██▀  ▀██▄"),
];

/// The banner, a tagline and a blank line.
pub fn banner(style: Style) -> String {
    let mut s = String::from("\n");
    for (sgr, row) in BANNER {
        s.push_str(&format!("  {}\n", style.paint(sgr, row.trim_end())));
    }
    s.push_str(&format!(
        "  {}\n\n",
        style.paint(DIM, "concept graphs for agents and people")
    ));
    s
}

/// Commands whose human output starts with the banner (on stdout; daemon start puts it on stderr).
const BANNER_ON: [&str; 2] = ["version", "agents-start-here"];

/// What `--human` prints in place of a value umx's secret check flags (F224). JSON output keeps the value.
pub const HIDDEN: &str = "[secret suspected, hidden]";

/// The keys a flag that cannot be placed exactly leaves shown: umx's own, holding no graph text, and reserved on both
/// nodes and edges, so no custom field can take the name (a test in crates/umx checks it). Every other value of that
/// record is hidden: an edge's `field <n>` (the number counts stored fields a reply may not show) or a flag this
/// version does not know fails closed (F224).
pub const PLAIN_KEYS: &[&str] = &[
    "id",
    "address",
    "graph",
    "type",
    "created",
    "created_by",
    "updated",
    "evidence_kinds",
    "secret_suspected",
];

/// `data` with every value its records' `secret_suspected` names replaced by [`HIDDEN`]. The records are where umx
/// puts the flags it computes: the data itself (entity show's keys, edge show, an edge write's sentence), `node`,
/// `parent_edge` and `raised_in_edge` (a node add's edges, F234), and each entry of `rows`, `members`, `proposed` and
/// `sentences` (a batch's echoed sentences, F234). Nothing below a record is read as flags: a node's custom JSON may
/// hold that key too.
fn masked(data: &Value) -> Value {
    let mut data = data.clone();
    mask_record(&mut data);
    if let Some(map) = data.as_object_mut() {
        for key in ["node", "parent_edge", "raised_in_edge"] {
            if let Some(record) = map.get_mut(key) {
                mask_record(record);
            }
        }
        for list in ["rows", "members", "proposed", "sentences"] {
            if let Some(Value::Array(items)) = map.get_mut(list) {
                items.iter_mut().for_each(mask_record);
            }
        }
    }
    data
}

/// `graph guide`'s data with each type's, status's and relation's `example` masked as a record (F234): a node's
/// flagged title, or an edge's sentence holding one.
fn masked_guide(data: &Value) -> Value {
    let mut data = data.clone();
    for key in ["types", "statuses", "relations"] {
        if let Some(Value::Array(entries)) = data.get_mut(key) {
            for entry in entries {
                if let Some(example) = entry.get_mut("example") {
                    mask_record(example);
                }
            }
        }
    }
    data
}

/// One record's flagged values hidden, each at the value path its place names; a place that does not name one
/// exactly hides every value but [`PLAIN_KEYS`].
fn mask_record(record: &mut Value) {
    let flags: Vec<String> = match record.get("secret_suspected") {
        Some(Value::Array(flags)) => flags.iter().map(text).collect(),
        _ => return,
    };
    for flag in flags {
        let exact = place::parse(&flag).is_some_and(|p| {
            p.edge
                .is_none_or(|id| record.get("id").and_then(Value::as_str) == Some(id))
                && mask_spot(record, p.spot)
        });
        if !exact && let Some(map) = record.as_object_mut() {
            for (key, value) in map.iter_mut() {
                if !PLAIN_KEYS.contains(&key.as_str()) {
                    *value = Value::String(HIDDEN.into());
                }
            }
        }
    }
}

/// Hides the value `spot` names in `record`; false when the spot names no value this reply holds exactly.
fn mask_spot(record: &mut Value, spot: Spot<'_>) -> bool {
    let hide = |v: &mut Value| *v = Value::String(HIDDEN.into());
    match spot {
        // A locator is shown as one object (node get) or as its source column (node list): either hides both.
        Spot::Named("locator" | "locator_source") => {
            for key in ["locator", "locator_source"] {
                if let Some(v) = record.get_mut(key) {
                    hide(v);
                }
            }
            true
        }
        Spot::Named(name) => {
            if let Some(v) = record.get_mut(name) {
                hide(v);
            }
            true
        }
        Spot::Entry { list, index } => {
            // `key:<i>` indexes the record's `entity_key`, or entity show's `keys` at the top of its reply.
            let (key, part) = match list {
                "key" if record.get("entity_key").is_some() => ("entity_key", None),
                "key" => ("keys", None),
                "evidence" => ("evidence", None),
                "status_reason" => ("status_history", Some("reason")),
                "status_evidence" => ("status_history", Some("evidence")),
                "evidence_removed" => ("evidence_removed", None),
                _ => return false,
            };
            if let Some(item) = record.get_mut(key).and_then(|items| items.get_mut(index)) {
                // An entity member shows each key it shares a second time, in `matched[].key` (R394-1): the copies
                // of this key, by value, are hidden with it.
                let shared = (key == "entity_key").then(|| item.clone());
                match part.and_then(|part| item.get_mut(part)) {
                    Some(v) => hide(v),
                    None => hide(item),
                }
                if let (Some(shared), Some(Value::Array(matched))) =
                    (shared, record.get_mut("matched"))
                {
                    for m in matched {
                        if let Some(k) = m.get_mut("key").filter(|k| **k == shared) {
                            hide(k);
                        }
                    }
                }
            }
            true
        }
        Spot::Custom(_) | Spot::Record => false,
    }
}

/// Renders an envelope as (stdout, stderr) text.
pub fn render(env: &Envelope, out: Style, err: Style) -> (String, String) {
    if env.command == "daemon start" && env.errors.is_empty() {
        // 007 section 3: in human mode the ready line goes to stderr; stdout stays empty.
        let d = &env.data;
        let line = format!(
            "{} pid {}, socket {}",
            err.paint(GREEN, "umx daemon ready:"),
            clean(&text(&d["pid"])),
            clean(&text(&d["socket"]))
        );
        return (String::new(), format!("{}{line}\n", banner(err)));
    }
    let mut stdout = String::new();
    if BANNER_ON.contains(&env.command.as_str()) && env.errors.is_empty() {
        stdout.push_str(&banner(out));
    }
    if env.command == "graph guide" && env.errors.is_empty() {
        stdout.push_str(&guide(&masked_guide(&env.data), out));
    } else if env.command == "graph spine propose" && env.errors.is_empty() {
        stdout.push_str(&spines(&env.data, out));
    } else {
        // F224: every value a record's secret_suspected names is hidden here; JSON output keeps it.
        stdout.push_str(&data(&masked(&env.data), &env.command, out));
    }
    if env.command == "check" && env.errors.is_empty() {
        for g in env.data["spines"].as_array().into_iter().flatten() {
            stdout.push_str(&spine_line(&g["graph"], &g["spine"], out));
        }
    }
    let mut stderr = String::new();
    for issue in &env.warnings {
        stderr.push_str(&issue_line(err.paint(YELLOW, "warning:"), issue, err));
    }
    for issue in &env.errors {
        stderr.push_str(&issue_line(err.paint(RED, "error:"), issue, err));
    }
    // Hints last, one line each (ws002 X1, architect): advice, never a problem.
    for hint in &env.hints {
        stderr.push_str(&format!(
            "{} {} ({} {})\n",
            err.paint(CYAN, "hint:"),
            clean(hint.message()),
            err.paint(CYAN, "next:"),
            err.paint(BOLD, &clean(hint.next()))
        ));
    }
    (stdout, stderr)
}

fn issue_line(label: String, issue: &Issue, style: Style) -> String {
    // A `umx batch` issue names its file line first, unless its message already starts with it.
    let at = match issue.line() {
        Some(l) if !issue.message().starts_with("Line ") => format!("line {l}: "),
        _ => String::new(),
    };
    format!(
        "{label} {at}{} {}\n  {} {}\n",
        clean(issue.message()),
        style.paint(DIM, &format!("[{}]", issue.code())),
        style.paint(CYAN, "next:"),
        style.paint(BOLD, &clean(issue.next()))
    )
}

fn data(data: &Value, command: &str, style: Style) -> String {
    if data.is_null() {
        return String::new();
    }
    // Built-in docs, compiled into the binary: trusted text.
    if let Some(markdown) = data["markdown"].as_str() {
        return markdown
            .lines()
            .map(|l| {
                let l = if l.starts_with('#') {
                    style.paint(HEADING, l)
                } else {
                    l.to_owned()
                };
                l + "\n"
            })
            .collect();
    }
    // edge show (F220): the row is the edge, as a node read's data is the node: every key under the sentence. Chosen
    // by the command, never by the row's keys, since a custom field may be named `edge`, `deleted` or `changed`
    // (R389-1).
    if command == "edge show"
        && let Some(sentence) = data["sentence"].as_str()
    {
        let mut row = data.clone();
        if let Some(m) = row.as_object_mut() {
            m.remove("sentence");
        }
        let mut lines = vec![style.paint(BOLD, &clean(sentence))];
        flatten("", &row, style, &mut lines);
        return lines.iter().map(|l| format!("{l}\n")).collect();
    }
    // edge add, delete and edit (req-59, F75): the sentence first, so the direction is the first thing read.
    if let Some(sentence) = data["sentence"].as_str() {
        let first = if data["deleted"] == true {
            format!("deleted: {}", clean(sentence))
        } else {
            clean(sentence)
        };
        let mut lines = vec![style.paint(BOLD, &first)];
        flatten("", &data["edge"], style, &mut lines);
        if let Some(changed) = data["changed"].as_array() {
            let names: Vec<&str> = changed.iter().filter_map(Value::as_str).collect();
            lines.push(match names.as_slice() {
                [] => "changed: nothing".to_owned(),
                _ => format!("changed: {}", names.join(", ")),
            });
        }
        return lines.iter().map(|l| format!("{l}\n")).collect();
    }
    if let (Some(columns), Some(rows)) = (data["columns"].as_array(), data["rows"].as_array()) {
        let cols: Vec<&str> = columns.iter().filter_map(Value::as_str).collect();
        let mut out = String::new();
        if let Some(sentences) = data["sentences"].as_array() {
            for sentence in sentences {
                let id = sentence["id"].as_str().unwrap_or("(not minted)");
                out.push_str(&format!(
                    "line {} {} {}: {}\n",
                    clean(&text(&sentence["line"])),
                    clean(sentence["op"].as_str().unwrap_or("edge")),
                    clean(id),
                    clean(sentence["sentence"].as_str().unwrap_or("")),
                ));
            }
            if let Some(omitted) = data["sentences_omitted"].as_u64().filter(|n| *n > 0) {
                out.push_str(&format!("{omitted} more edge sentences omitted\n"));
            }
        }
        // ws003 s09: a replayed batch has no rows; say so, then the refs it bound.
        if data["replayed"] == true {
            out.push_str(&format!(
                "replayed: recorded {}, nothing written\n",
                clean(&text(&data["recorded_at"]))
            ));
            for (name, id) in data["refs"].as_object().into_iter().flatten() {
                out.push_str(&format!("ref {} {}\n", clean(name), clean(&text(id))));
            }
        }
        out.push_str(&table(&cols, rows, data, style));
        // umx doctor: what --fix did, then the verdict last, where it is read first.
        for f in data["fixed"].as_array().into_iter().flatten() {
            let done = if f["done"] == true {
                "fixed"
            } else {
                "not fixed"
            };
            out.push_str(&format!(
                "{done}: {} {}\n",
                clean(&text(&f["fix"])),
                clean(&text(&f["path"]))
            ));
        }
        if let Some(verdict) = data["verdict"].as_str() {
            let paint = match verdict {
                "ok" => GREEN,
                "degraded" => YELLOW,
                _ => RED,
            };
            out.push_str(&format!(
                "{} {}\n",
                style.paint(BOLD, "verdict:"),
                style.paint(paint, &clean(verdict))
            ));
        }
        return out;
    }
    let mut lines = Vec::new();
    // A `next` list (graph new, req-71) goes last, one runnable command per line. umx builds these from its own
    // words and checked names, but they are cleaned like any data.
    let mut data = data.clone();
    let next = data.as_object_mut().and_then(|m| m.remove("next"));
    flatten("", &data, style, &mut lines);
    if let Some(Value::Array(commands)) = next {
        lines.push(style.paint(CYAN, "next:"));
        lines.extend(
            commands
                .iter()
                .map(|c| format!("  {}", style.paint(BOLD, &clean(&text(c))))),
        );
    }
    lines.iter().map(|l| format!("{l}\n")).collect()
}

/// A search hit's links (search-08) on one line: `part_of -> <id> <title>; supports <- <id> <title>; +2 more`, every
/// part cleaned. A hidden value (a flagged title, F224) prints as it is; no links prints nothing.
fn links_line(links: &Value, more: &Value) -> Option<String> {
    let more = more.as_u64().unwrap_or(0);
    let mut parts: Vec<String> = match links {
        Value::Array(links) => links
            .iter()
            .map(|l| {
                let arrow = if l["direction"] == "in" { "<-" } else { "->" };
                format!(
                    "{} {arrow} {} {}",
                    clean(&text(&l["relation"])),
                    clean(&text(&l["id"])),
                    clean(&text(&l["title"]))
                )
            })
            .collect(),
        other => vec![clean(&text(other))],
    };
    if more > 0 {
        parts.push(format!("+{more} more"));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// A header row, a rule and one aligned row per record; ids dim. Widths are terminal columns, before colour.
/// Column names can come from the command line (`--columns`), so they are cleaned like graph text; rows are
/// looked up by the raw name.
fn table(cols: &[&str], rows: &[Value], data: &Value, style: Style) -> String {
    // A search hit's cut (PRD 002 search-07) is printed under its row, one indented line, never as a column: the
    // table stays as wide as it was. It is graph text, so it goes through [`clean`] like every cell.
    let cut = cols.contains(&"text") && cols.contains(&"text_field");
    // Its links (search-08) likewise: one indented line under the text, `+N more` for the rest.
    let linked = cols.contains(&"links") && cols.contains(&"links_more");
    let shown: Vec<&str> = cols
        .iter()
        .copied()
        .filter(|c| !(cut && matches!(*c, "text" | "text_field")))
        .filter(|c| !(linked && matches!(*c, "links" | "links_more")))
        .collect();
    let cols = shown.as_slice();
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| cols.iter().map(|c| clean(&cell(&row[*c]))).collect())
        .collect();
    let header: Vec<String> = cols.iter().map(|c| clean(c)).collect();
    let width = |i: usize| {
        cells
            .iter()
            .map(|r| width_of(&r[i]))
            .chain([width_of(&header[i])])
            .max()
            .unwrap_or(0)
    };
    let widths: Vec<usize> = (0..cols.len()).map(width).collect();
    // Pads each cell to its column, then paints it, so colour codes never count towards the width.
    let line = |cells: &[String], paint: &dyn Fn(usize, &str) -> String| {
        let mut s = String::new();
        for (i, cell) in cells.iter().enumerate() {
            if i + 1 < cells.len() {
                let pad = widths[i] - width_of(cell) + 2;
                s.push_str(&paint(i, cell));
                s.push_str(&" ".repeat(pad));
            } else {
                s.push_str(&paint(i, cell.trim_end()));
            }
        }
        s + "\n"
    };
    let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
    let mut out = line(&header, &|_, c| style.paint(HEADING, c));
    out.push_str(&line(&rule, &|_, c| style.paint(DIM, c)));
    for (row, record) in cells.iter().zip(rows) {
        out.push_str(&line(row, &|i, c| {
            if cols[i] == "id" {
                style.paint(DIM, c)
            } else {
                c.to_owned()
            }
        }));
        if cut && let Some(text) = record["text"].as_str() {
            let field = clean(record["text_field"].as_str().unwrap_or("text"));
            out.push_str(&format!(
                "    {} {}\n",
                style.paint(DIM, &format!("{field}:")),
                clean(text)
            ));
        }
        if linked && let Some(line) = links_line(&record["links"], &record["links_more"]) {
            out.push_str(&format!("    {} {line}\n", style.paint(DIM, "links:")));
        }
    }
    if rows.is_empty() {
        out.push_str(&style.paint(DIM, "(none)"));
        out.push('\n');
    }
    if data["truncated"] == true {
        // A null total: the service stopped early instead of counting everything (a lazy walk).
        let more = match data["total"].as_u64() {
            Some(total) => format!("showing {} of {total}", rows.len()),
            None => format!(
                "showing {}; more follow (page on with --offset)",
                rows.len()
            ),
        };
        out.push_str(&style.paint(DIM, &more));
        out.push('\n');
    }
    out
}

fn flatten(prefix: &str, value: &Value, style: Style, lines: &mut Vec<String>) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (key, v) in map {
                let key = clean(key);
                let key = if prefix.is_empty() {
                    key
                } else {
                    format!("{prefix}.{key}")
                };
                flatten(&key, v, style, lines);
            }
        }
        _ => {
            let shown = clean(&text(value));
            let shown = match value {
                Value::Bool(true) => style.paint(GREEN, &shown),
                Value::Bool(false) => style.paint(RED, &shown),
                _ => shown,
            };
            lines.push(format!("{}: {shown}", style.paint(CYAN, prefix)));
        }
    }
}

/// A table cell. A derived edge (a similar_to entry in umx query rows: no id, no address) reads as
/// `<from> -[similar_to <score>]-> <to>`, never as its JSON with nulls.
fn cell(value: &Value) -> String {
    if value["derived"] == true
        && let (Some(kind), Some(from), Some(to)) = (
            value["type"].as_str(),
            value["from"].as_str(),
            value["to"].as_str(),
        )
    {
        return format!("{from} -[{kind} {}]-> {to}", text(&value["score"]));
    }
    // A search hit's word_terms: `vendor ~ vendors (stem)`, or the word alone when it matched itself (req-92).
    if let Value::Array(terms) = value
        && !terms.is_empty()
        && terms.iter().all(|t| t["form"].is_string())
    {
        return terms
            .iter()
            .map(|t| match t["form"].as_str() {
                Some("exact") => text(&t["query"]),
                _ => format!(
                    "{} ~ {} ({})",
                    text(&t["query"]),
                    text(&t["term"]),
                    text(&t["form"])
                ),
            })
            .collect::<Vec<_>>()
            .join(", ");
    }
    text(value)
}

/// `umx graph guide` (plan 079): the purpose and conventions, then each type, status and relation with its count,
/// meaning and example. Every string but the headings is graph text, so each goes through [`clean`].
fn guide(d: &Value, style: Style) -> String {
    let g = &d["graph"];
    let mut out = format!(
        "{} {} ({} nodes, {} edges)\n",
        style.paint(BOLD, &format!("Graph {}:", clean(&text(&g["alias"])))),
        clean(&text(&g["title"])),
        text(&g["nodes"]),
        text(&g["edges"])
    );
    for (heading, key) in [("Purpose", "purpose"), ("Conventions", "conventions")] {
        out.push_str(&format!("\n{}\n", style.paint(HEADING, heading)));
        match d[key].as_str() {
            Some(t) => t
                .lines()
                .for_each(|l| out.push_str(&format!("  {}\n", clean(l)))),
            None => out.push_str(&missing(&d[&format!("{key}_next")], style)),
        }
    }
    let entry = |e: &Value, out: &mut String| {
        let mut what = text(&e["count"]);
        if e["declared"] == false {
            what.push_str(", not declared");
        }
        if e["symmetric"] == true {
            what.push_str(", symmetric");
        }
        out.push_str(&format!(
            "  {} ({what})",
            style.paint(BOLD, &clean(&text(&e["name"])))
        ));
        match e["description"].as_str() {
            Some(t) => out.push_str(&format!(": {}\n", clean(t))),
            None => out.push_str(&format!(":\n{}", missing(&e["next"], style))),
        }
        let x = &e["example"];
        if let Some(sentence) = x["sentence"].as_str() {
            out.push_str(&format!(
                "    {} {}\n",
                style.paint(DIM, "e.g."),
                clean(sentence)
            ));
        } else if x.is_object() {
            out.push_str(&format!(
                "    {} {} {}\n",
                style.paint(DIM, "e.g."),
                clean(&text(&x["id"])),
                clean(&text(&x["title"]))
            ));
        }
    };
    for (heading, key) in [
        ("Types", "types"),
        ("Statuses", "statuses"),
        ("Relations", "relations"),
    ] {
        out.push_str(&format!("\n{}\n", style.paint(HEADING, heading)));
        for e in d[key].as_array().into_iter().flatten() {
            entry(e, &mut out);
            for f in e["fields"].as_array().into_iter().flatten() {
                out.push_str(&format!(
                    "    field {} ({})",
                    clean(&text(&f["name"])),
                    clean(&text(&f["kind"]))
                ));
                match f["description"].as_str() {
                    Some(t) => out.push_str(&format!(": {}\n", clean(t))),
                    None => out.push_str(&format!(
                        ":\n{}",
                        missing(&f["next"], style).replacen("  ", "    ", 1)
                    )),
                }
            }
        }
    }
    out.push_str(&format!("\n{}\n", style.paint(HEADING, "Spine")));
    out.push_str(&spine_line(&g["alias"], &d["spine"], style));
    out
}

/// A cover as `placed of total (field f, link l, evidence e, rule r)`.
fn cover(c: &Value) -> String {
    let n = |k: &str| c[k].as_u64().unwrap_or(0);
    format!(
        "{} of {} placed (field {}, link {}, evidence {}, rule {})",
        n("total") - n("not_placed"),
        n("total"),
        n("by_field"),
        n("by_link"),
        n("by_evidence"),
        n("by_rule")
    )
}

/// The spine line of `graph guide` and `check` (B4): the best offered candidate and its cover, or none, then the
/// command that lists every candidate.
fn spine_line(graph: &Value, line: &Value, style: Style) -> String {
    let best = &line["best"];
    let what = if best.is_object() {
        format!(
            "{} ({}), {}",
            clean(&text(&best["candidate"])),
            clean(&text(&best["clock"])),
            cover(&best["cover"])
        )
    } else {
        "none offered".to_owned()
    };
    format!(
        "  {} {}: {what}\n  {} {}\n",
        style.paint(BOLD, "spine"),
        clean(&text(graph)),
        style.paint(CYAN, "see:"),
        clean(&text(&line["next"]))
    )
}

/// `umx graph spine propose`: the offered candidates with cover and spread, the count of the rest, what is in the
/// way, and the commands that list more. Names are vocabulary names; every string still goes through [`clean`].
fn spines(d: &Value, style: Style) -> String {
    let mut out = format!(
        "{} {} nodes\n",
        style.paint(BOLD, &format!("Spines for {}:", clean(&text(&d["graph"])))),
        text(&d["total"])
    );
    if let Some(u) = d["unplaced"].as_object() {
        out.push_str(&format!(
            "\n{} {} of {} (from {})\n",
            style.paint(HEADING, "Not placed on"),
            clean(&text(&u["candidate"])),
            text(&u["total"]),
            text(&u["offset"])
        ));
        for id in u["ids"].as_array().into_iter().flatten() {
            out.push_str(&format!("  {}\n", style.paint(DIM, &clean(&text(id)))));
        }
    }
    out.push_str(&format!("\n{}\n", style.paint(HEADING, "Offered")));
    let offered: Vec<&Value> = d["candidates"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c["offered"] == true)
        .collect();
    if offered.is_empty() {
        out.push_str("  none\n");
    }
    for c in offered {
        let s = &c["spread"];
        out.push_str(&format!(
            "  {} ({}, {}): {}; {} bands, largest {}, {} distinct values\n",
            style.paint(BOLD, &clean(&text(&c["name"]))),
            clean(&text(&c["source"])),
            clean(&text(&c["clock"])),
            cover(&c["cover"]),
            text(&s["bands"]),
            text(&s["largest_band_share"]),
            text(&s["distinct_values"])
        ));
        let decl = &c["decl"];
        if decl.is_object() {
            let list = |v: &Value| {
                v.as_array()
                    .into_iter()
                    .flatten()
                    .map(|x| clean(&text(x)))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            out.push_str(&format!(
                "    order {}; place {}; inside {}\n",
                list(&decl["order"]),
                list(&decl["place"]),
                clean(&text(&decl["anchors"]["inside"]))
            ));
        }
    }
    let ways = d["in_the_way"].as_array().into_iter().flatten();
    let mut header = false;
    for w in ways {
        if !header {
            out.push_str(&format!("\n{}\n", style.paint(HEADING, "In the way")));
            header = true;
        }
        out.push_str(&format!(
            "  {} {} ({} nodes), e.g. {}\n    {} {}\n",
            style.paint(YELLOW, &clean(&text(&w["kind"]))),
            clean(&text(&w["about"])),
            text(&w["count"]),
            w["examples"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|x| clean(&text(x)))
                .collect::<Vec<_>>()
                .join(" "),
            style.paint(CYAN, "next:"),
            style.paint(BOLD, &clean(&text(&w["next"])))
        ));
    }
    let h = &d["hints"];
    let not_offered = &h["not_offered"];
    if not_offered["count"].as_u64().unwrap_or(0) > 0 {
        out.push_str(&format!(
            "\n{} not offered (why_not in): {}\n",
            text(&not_offered["count"]),
            style.paint(BOLD, &clean(&text(&not_offered["next"])))
        ));
    }
    let not_placed = &h["not_placed"];
    if not_placed["count"].as_u64().unwrap_or(0) > 0 {
        out.push_str(&format!(
            "{} nodes have no place on {}: {}\n",
            text(&not_placed["count"]),
            clean(&text(&not_placed["candidate"])),
            style.paint(BOLD, &clean(&text(&not_placed["next"])))
        ));
    }
    out
}

/// A missing purpose, conventions or description: the command that adds it.
fn missing(next: &Value, style: Style) -> String {
    match next.as_str() {
        Some(c) => format!(
            "  {} {}\n",
            style.paint(YELLOW, "missing, add with:"),
            clean(c)
        ),
        None => format!("  {}\n", style.paint(YELLOW, "missing")),
    }
}

fn text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "none".to_owned(),
        other => other.to_string(),
    }
}

/// Terminal columns a cleaned string takes: 2 for East Asian wide and emoji characters, 0 for combining marks
/// and zero-width characters, else 1. An approximation without a Unicode table crate; good enough to align.
fn width_of(s: &str) -> usize {
    s.chars()
        .map(|c| match c as u32 {
            0x0300..=0x036F | 0x200B..=0x200D | 0xFE00..=0xFE0F => 0,
            0x1100..=0x115F
            | 0x2E80..=0x303E
            | 0x3041..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA000..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1F64F
            | 0x1F900..=0x1F9FF
            | 0x20000..=0x3FFFD => 2,
            _ => 1,
        })
        .sum()
}

/// Graph text made safe for a terminal: escape sequences (CSI, OSC, DCS and the like, 7-bit or C1) and other
/// control characters removed, tabs and line breaks turned into spaces, bidi overrides dropped.
pub fn clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    // Skips a string-type sequence body up to BEL, ST (ESC \) or C1 ST.
    fn skip_string(chars: &mut std::iter::Peekable<std::str::Chars>) {
        while let Some(c) = chars.next() {
            match c {
                '\x07' | '\u{9c}' => return,
                '\x1b' if chars.peek() == Some(&'\\') => {
                    chars.next();
                    return;
                }
                _ => {}
            }
        }
    }
    fn skip_csi(chars: &mut std::iter::Peekable<std::str::Chars>) {
        for c in chars.by_ref() {
            if ('@'..='~').contains(&c) {
                return;
            }
        }
    }
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                Some('[') => skip_csi(&mut chars),
                Some(']' | 'P' | 'X' | '^' | '_') => skip_string(&mut chars),
                // ESC, intermediate bytes, then one final byte (such as ESC ( B).
                Some(' '..='/') => {
                    while chars.next_if(|c| (' '..='/').contains(c)).is_some() {}
                    chars.next();
                }
                _ => {}
            },
            '\u{9b}' => skip_csi(&mut chars),
            '\u{9d}' | '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => skip_string(&mut chars),
            '\t' | '\n' | '\r' => out.push(' '),
            // Bidi marks, embeddings, overrides and isolates: they reorder what follows them on screen.
            '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}' => {}
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use umx_protocol::json;

    const PLAIN: Style = Style { on: false };
    const COLOUR: Style = Style { on: true };

    #[test]
    fn clean_strips_escape_sequences_and_controls() {
        let planted = "A\x1b[31mred\x1b[0m \x1b]8;;https://x.example\x07link\x1b]8;;\x1b\\ \x1b]0;title\x1b\\\
                       \u{9b}2J\u{9d}osc\u{9c}b\u{7}el\u{8}\nnext\tcol\u{202e}rtl \x1bc\x1b(B end";
        assert_eq!(clean(planted), "Ared link bel next colrtl  end");
        assert!(!clean(planted).chars().any(|c| c.is_control()));
        assert_eq!(clean("Approval delays ✓"), "Approval delays ✓");
    }

    #[test]
    fn a_replayed_batch_says_so_and_lists_its_refs() {
        let replay = json!({
            "columns": ["line", "op"], "rows": [], "total": 0, "truncated": false,
            "replayed": true, "recorded_at": "2026-11-17T00:42:00.123Z",
            "refs": {"jo": "01JC8XA4H7B3KT9M2NZ5QW6RDE"},
        });
        let out = data(&replay, "batch", PLAIN);
        assert!(
            out.starts_with(
                "replayed: recorded 2026-11-17T00:42:00.123Z, nothing written\nref jo 01JC8XA4H7B3KT9M2NZ5QW6RDE\n"
            ),
            "{out:?}"
        );
        let mut first = replay;
        first["replayed"] = json!(false);
        assert!(!data(&first, "batch", PLAIN).contains("replayed"));
    }

    #[test]
    fn batch_echo_is_cleaned_and_precedes_the_unchanged_table() {
        let base = json!({
            "columns": ["line", "op"], "rows": [{"line": 3, "op": "edge add"}],
            "total": 1, "truncated": false,
        });
        let ordinary = data(&base, "batch", PLAIN);
        let mut echoed = base;
        echoed["sentences"] = json!([{
            "line": 3, "op": "edge add", "id": null,
            "sentence": "A\x1b[31mred\x1b[0m\nsupports\tB\u{202e}",
        }]);
        echoed["sentences_omitted"] = json!(2);
        let out = data(&echoed, "batch", PLAIN);
        assert!(
            out.starts_with(
                "line 3 edge add (not minted): Ared supports B\n2 more edge sentences omitted\n"
            ),
            "{out:?}"
        );
        assert!(out.ends_with(&ordinary));
        assert!(!out.contains('\x1b') && !out.contains('\u{202e}'));
    }

    #[test]
    fn tables_align_and_graph_text_cannot_reach_the_terminal() {
        let env = Envelope::ok(
            "node list",
            json!({
                "columns": ["id", "title"],
                "rows": [
                    {"id": "01AB", "title": "evil\x1b]52;c;aGk=\x07\x1b[2J title"},
                    {"id": "01ABCDEF", "title": "ok"},
                ],
                "total": 2, "truncated": false,
            }),
        );
        let (out, _) = render(&env, PLAIN, PLAIN);
        assert_eq!(
            out,
            "id        title\n────────  ──────────\n01AB      evil title\n01ABCDEF  ok\n"
        );
        let (out, _) = render(&env, COLOUR, COLOUR);
        assert!(out.contains("\x1b[2m01AB\x1b[0m"), "{out:?}");
        assert!(!out.contains("]52;") && !out.contains("[2J"), "{out:?}");
    }

    #[test]
    fn an_unknown_total_says_more_follow() {
        let env = Envelope::ok(
            "walk",
            json!({ "columns": ["id"], "rows": [{ "id": "01AB" }], "total": null, "truncated": true }),
        );
        let (out, _) = render(&env, PLAIN, PLAIN);
        assert!(
            out.ends_with("showing 1; more follow (page on with --offset)\n"),
            "{out}"
        );
    }

    #[test]
    fn hostile_column_names_are_cleaned_and_rows_still_found() {
        // Review B1: `--columns` names reach the header.
        let hostile = "\x1b]8;;https://x.example\x07f\x1b]8;;\x07\x1b[31m\u{202e}ld";
        let env = Envelope::ok(
            "node list",
            json!({ "columns": ["id", hostile], "rows": [{ "id": "01AB", hostile: "v" }], "total": 1, "truncated": false }),
        );
        for style in [PLAIN, COLOUR] {
            let (out, _) = render(&env, style, style);
            assert!(
                !out.contains("]8;") && !out.contains("[31m") && !out.contains('\u{202e}'),
                "{out:?}"
            );
            assert!(out.contains("fld"), "{out:?}");
        }
        let (out, _) = render(&env, PLAIN, PLAIN);
        assert_eq!(out, "id    fld\n────  ───\n01AB  v\n");
    }

    #[test]
    fn clean_drops_every_bidi_control() {
        // Review B2: U+061C is a format character, not a control.
        for c in [
            '\u{061c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}',
            '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
        ] {
            assert_eq!(clean(&format!("a{c}b")), "ab", "U+{:04X}", c as u32);
        }
    }

    #[test]
    fn wide_characters_keep_columns_aligned() {
        let env = Envelope::ok(
            "node list",
            json!({ "columns": ["title", "status"], "rows": [{ "title": "承認", "status": "open" }, { "title": "ab", "status": "done" }], "total": 2, "truncated": false }),
        );
        let (out, _) = render(&env, PLAIN, PLAIN);
        assert_eq!(
            out,
            "title  status\n─────  ──────\n承認   open\nab     done\n"
        );
    }

    #[test]
    fn plain_style_has_no_escape_codes_and_the_banner_shows_either_way() {
        let env = Envelope::ok(
            "version",
            json!({ "cli": { "version": "1" }, "running": false }),
        );
        let (plain, _) = render(&env, PLAIN, PLAIN);
        assert!(!plain.contains('\x1b'), "{plain:?}");
        assert!(plain.contains(BANNER[0].1.trim_end()), "{plain}");
        let (colour, _) = render(&env, COLOUR, COLOUR);
        assert!(colour.contains("\x1b[38;5;51m"), "{colour:?}");
    }
}
