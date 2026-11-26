//! Parses graph, node, edge and ontology commands into typed operations (workshop 007 sections 4 to 7). The
//! daemon never sees argv; usage errors for these verbs come only from here.

use std::path::{Component, Path, PathBuf};
use umx_protocol::{
    Direction, EvidenceAction, KeyAction, OnKey, Op, PairRows, SetArg, SetValue, SettingArg, UiCmd,
    command_label, parse_json,
};

/// A parsed command: the operation and the common flags.
pub struct Parsed {
    pub op: Op,
    pub graph: Option<String>,
    pub scope: Option<String>,
    pub all: bool,
    pub include_named: bool,
    pub include_provenance: bool,
    pub by: Option<String>,
}

/// Why a command does not parse: the command words (from the known list) and what is wrong.
pub struct Usage {
    pub command: &'static str,
    pub what: &'static str,
}

struct Args {
    command: &'static str,
    positional: Vec<String>,
    flags: Vec<(String, String)>,
    switches: Vec<String>,
}

const SWITCHES: [&str; 31] = [
    "--clear-values",
    "--same-graph",
    "--raise",
    "--add",
    "--only",
    "--all",
    "--include-named",
    "--provenance",
    "--all-types",
    "--force",
    "--empty",
    "--repo",
    "--secured",
    "--dry-run",
    "--echo-sentences",
    "--symmetric",
    "--replace",
    "--reverse",
    "--reset",
    "--removed",
    "--rewrite",
    "--no-graphfile",
    "--graphfile",
    "--redact",
    "--lint",
    "--rules",
    "--sources",
    "--related",
    "--no-related",
    "--std",
    "--no-std",
];

/// Every command's own flags: the one list. The parser accepts a flag only when its command's row names it (the
/// offline rows are checked against their parsers in lib.rs), and docs.rs fails while a flag is missing from its
/// command's `--help` topic. Not here: [`COMMON`]; `graph set`'s flags, which are umx-core's graph settings;
/// the global `--json`, `--human` and `--help`.
pub const FLAGS: &[(&str, &[&str])] = &[
    ("rule list", &[]),
    (
        "rule add",
        &[
            "--type",
            "--status",
            "--tag",
            "--where",
            "--match",
            "--field",
            "--edge",
            "--from",
            "--to",
            "--to-node",
            "--description",
            "--dry-run",
        ],
    ),
    ("rule remove", &[]),
    ("rule off", &[]),
    ("rule on", &[]),
    ("rule show", &["--limit", "--offset"]),
    ("hub list", &[]),
    (
        "hub set",
        &[
            "--node",
            "--type-hub",
            "--via",
            "--type",
            "--status",
            "--tag",
            "--where",
            "--match",
        ],
    ),
    ("hub show", &["--limit", "--offset"]),
    ("hub fill", &["--after"]),
    ("hub remove", &[]),
    (
        "check",
        &[
            "--lint",
            "--sources",
            "--rules",
            "--rule",
            "--after",
            "--kind",
            "--offset",
        ],
    ),
    (
        "neighbours",
        &[
            "--relation",
            "--direction",
            "--limit",
            "--offset",
            "--provenance",
            "--tag",
            "--no-tag",
            "--entities",
        ],
    ),
    (
        "walk",
        &[
            "--depth",
            "--relation",
            "--direction",
            "--within",
            "--limit",
            "--offset",
            "--provenance",
            "--tag",
            "--no-tag",
        ],
    ),
    (
        "path",
        &["--max-depth", "--relation", "--direction", "--provenance"],
    ),
    ("resolve", &["--provenance"]),
    (
        "query",
        &[
            "--param",
            "--param-str",
            "--param-json",
            "--limit",
            "--offset",
            "--along",
            "--after",
            "--before",
            "--all",
            "--include-named",
            "--provenance",
        ],
    ),
    (
        "search",
        &[
            "--also",
            "--within",
            "--limit",
            "--along",
            "--after",
            "--before",
            "--all",
            "--include-named",
            "--provenance",
            "--tag",
            "--no-tag",
        ],
    ),
    (
        "batch",
        &[
            "--file",
            "--refs-from",
            "--dry-run",
            "--echo-sentences",
            "--reason",
            "--raised-in",
            "--related",
            "--no-related",
            "--idempotency-key",
        ],
    ),
    ("scope save", &["--replace"]),
    ("scope list", &[]),
    ("scope show", &[]),
    ("scope delete", &[]),
    (
        "graph new",
        &[
            "--alias",
            "--title",
            "--empty",
            "--embedding-model",
            "--secured",
            "--no-graphfile",
            "--graphfile",
            "--no-std",
        ],
    ),
    ("graph load", &["--as", "--graphfile", "--set"]),
    ("graph list", &["--all", "--provenance", "--graphfile"]),
    ("graph scan", &["--add", "--rewrite", "--drop", "--depth"]),
    ("graph set", &[]),
    ("graph confirm-policy", &[]),
    ("graph unload", &[]),
    ("graph forget", &[]),
    ("graph primary", &[]),
    ("graph rename", &[]),
    ("graph move", &[]),
    ("graph guide", &["--provenance"]),
    (
        "graph spine propose",
        &[
            "--unplaced",
            "--by-rule",
            "--limit",
            "--offset",
            "--provenance",
        ],
    ),
    ("graph spine set", &["--file"]),
    ("graph spine confirm", &[]),
    (
        "graph spine show",
        &["--by-rule", "--unplaced", "--limit", "--offset"],
    ),
    ("graph spine remove", &[]),
    ("similarity rebuild", &[]),
    (
        "similarity pairs",
        &[
            "--k",
            "--min",
            "--model",
            "--same-graph",
            "--limit",
            "--offset",
            "--rows",
            "--all",
            "--include-named",
        ],
    ),
    ("embeddings repair", &[]),
    ("embeddings allow", &["--node", "--field"]),
    ("embeddings reembed", &["--dry-run", "--model"]),
    ("embeddings probe", &["--embedder"]),
    ("embeddings status", &[]),
    ("entity key", &[]),
    ("entity link", &["--dry-run"]),
    ("entity unlink", &[]),
    ("entity deny", &["--reason"]),
    ("entity undeny", &[]),
    ("entity show", &[]),
    // Who reads provenance without --provenance (req-95), so the switch would be an accepted no-op: refused.
    ("who", &["--limit"]),
    ("node who", &[]),
    (
        "node add",
        &[
            "--type",
            "--title",
            "--status",
            "--note",
            "--set",
            "--set-str",
            "--set-json",
            "--parent",
            "--evidence",
            "--evidence-kind",
            "--locator",
            "--raised-in",
            "--on-key",
            "--related",
            "--no-related",
            "--tag",
        ],
    ),
    ("node get", &["--columns", "--provenance"]),
    (
        "node list",
        &[
            "--type",
            "--status",
            "--where",
            "--within",
            "--columns",
            "--limit",
            "--offset",
            "--along",
            "--after",
            "--before",
            "--provenance",
            "--tag",
            "--no-tag",
        ],
    ),
    (
        "node find",
        &[
            "--type",
            "--limit",
            "--within",
            "--along",
            "--after",
            "--before",
            "--provenance",
            "--tag",
            "--no-tag",
        ],
    ),
    (
        "node edit",
        &[
            "--title",
            "--type",
            "--note",
            "--set",
            "--set-str",
            "--set-json",
            "--unset",
            "--locator",
            "--tag",
            "--untag",
        ],
    ),
    ("resource show", &[]),
    ("resource refresh", &[]),
    ("node note", &["--append", "--evidence", "--evidence-kind"]),
    (
        "node evidence",
        &[
            "--evidence-kind",
            "--reason",
            "--at",
            "--removed",
            "--redact",
            "--all",
        ],
    ),
    (
        "node status",
        &[
            "--reason",
            "--label",
            "--in",
            "--evidence",
            "--evidence-kind",
        ],
    ),
    ("node history", &["--limit", "--provenance"]),
    (
        "edge add",
        &[
            "--set",
            "--set-str",
            "--set-json",
            "--evidence",
            "--evidence-kind",
        ],
    ),
    ("edge show", &[]),
    ("edge delete", &[]),
    (
        "edge edit",
        &[
            "--relation",
            "--reverse",
            "--set",
            "--set-str",
            "--set-json",
            "--unset",
        ],
    ),
    (
        "edge list",
        &[
            "--from",
            "--to",
            "--type",
            "--columns",
            "--limit",
            "--offset",
            "--along",
            "--after",
            "--before",
            "--provenance",
            "--where",
        ],
    ),
    ("ontology show", &["--std"]),
    ("ontology remove", &["--relation"]),
    (
        "ontology add",
        &[
            "--field",
            "--symmetric",
            "--all-types",
            "--relation",
            "--description",
            "--role",
            "--values",
            "--from",
            "--to",
        ],
    ),
    (
        "ontology set",
        &[
            "--columns",
            "--reset",
            "--role",
            "--values",
            "--clear-values",
        ],
    ),
    (
        "ontology adopt-std",
        &["--map", "--dry-run", "--field-to-tag"],
    ),
    ("config show", &[]),
    ("config check", &[]),
    ("config set", &["--repo"]),
    ("config unset", &["--repo"]),
    (
        "config import",
        &[
            "--from",
            "--as",
            "--model",
            "--dims",
            "--force",
            "--dry-run",
        ],
    ),
    ("root add", &[]),
    ("root list", &[]),
    ("root remove", &[]),
    ("ui focus", &["--hops", "--note"]),
    ("ui highlight", &["--add", "--note"]),
    ("ui filter", &["--show", "--hide", "--note"]),
    ("ui fit", &["--note"]),
    ("ui clear", &["--note"]),
    ("ui open", &["--only", "--note", "--raise"]),
    ("ui selection", &[]),
    ("daemon start", &["--idle-unload", "--service"]),
    ("doctor", &["--fix"]),
    ("models fetch", &["--dry-run"]),
];

/// The flags every typed command takes besides its own: `--graph` (one graph, or a list where
/// [`MULTI_GRAPH`] allows), `--scope` (only [`MULTI_GRAPH`] commands) and `--by` (who writes). Config commands
/// refuse `--graph` and `--by`.
pub const COMMON: [&str; 3] = ["--graph", "--by", "--scope"];

/// The commands that read across graphs: `--graph a,b` and `--scope`. `scope save` also takes `--graph a,b`.
pub const MULTI_GRAPH: [&str; 11] = [
    "similarity pairs",
    "node list",
    "edge list",
    "query",
    "search",
    "walk",
    "neighbours",
    "path",
    "check",
    "entity show",
    "who",
];

/// A command's row in [`FLAGS`]; none for a command with no row.
pub fn flags(command: &str) -> &'static [&'static str] {
    FLAGS
        .iter()
        .find(|(c, _)| *c == command)
        .map_or(&[], |(_, f)| f)
}

/// The switches of other commands that are also graph settings, so `graph set` reads them with a value (req-123:
/// `--related` is a switch on node add and batch, `graph set --related on|off` a setting). The settings table is
/// umx-core's, which the CLI does not read; docs' every_example_parses fails while one is missing here.
const GRAPH_SETTING_SWITCHES: [&str; 1] = ["--related"];

fn graph_setting(flag: &str) -> bool {
    GRAPH_SETTING_SWITCHES.contains(&flag)
}

impl Args {
    fn new(command: &'static str, words: &[&str]) -> Result<Args, Usage> {
        Args::allowing(command, words, flags(command))
    }

    fn allowing(command: &'static str, words: &[&str], allowed: &[&str]) -> Result<Args, Usage> {
        let fail = |what| Usage { command, what };
        let mut args = Args {
            command,
            positional: Vec::new(),
            flags: Vec::new(),
            switches: Vec::new(),
        };
        let mut it = words.iter();
        while let Some(word) = it.next() {
            if !word.starts_with("--") {
                args.positional.push((*word).to_owned());
                continue;
            }
            let (flag, inline) = match word.split_once('=') {
                Some((f, v)) => (f, Some(v.to_owned())),
                None => (*word, None),
            };
            if !allowed.contains(&flag) && !COMMON.contains(&flag) {
                return Err(fail("unknown flag for this command"));
            }
            // A graph setting always takes a value, even one named like another command's switch (--related).
            if SWITCHES.contains(&flag) && !(command == "graph set" && graph_setting(flag)) {
                if flag == "--echo-sentences" && inline.is_some() {
                    return Err(fail("--echo-sentences takes no value"));
                }
                if inline.is_some() {
                    return Err(fail("this flag takes no value"));
                }
                args.switches.push(flag.to_owned());
                continue;
            }
            let value = match inline {
                Some(v) => v,
                None => it
                    .next()
                    .ok_or(fail("a flag is missing its value"))?
                    .to_string(),
            };
            args.flags.push((flag.to_owned(), value));
        }
        if args.one("--graph").is_some() && args.one("--scope").is_some() {
            return Err(fail("--graph and --scope cannot be used together"));
        }
        // `node evidence <node> refresh --all` is every pointer (usage 220), not every graph.
        let all = command != "node evidence" && args.switches.iter().any(|s| s == "--all");
        if args.switches.iter().any(|s| s == "--include-named") && !all {
            return Err(fail("--include-named requires --all"));
        }
        if all && (args.one("--graph").is_some() || args.one("--scope").is_some()) {
            return Err(fail("--all cannot be combined with --graph or --scope"));
        }
        if args.one("--graph").is_some_and(|g| g.contains(','))
            && !MULTI_GRAPH.contains(&command)
            && command != "scope save"
            && command != "ui open"
        {
            return Err(fail(
                "this command takes one graph (use --graph with one name)",
            ));
        }
        if args.one("--scope").is_some() && !MULTI_GRAPH.contains(&command) {
            return Err(fail(
                "this command takes one graph (use --graph with one name)",
            ));
        }
        // F256: a second --graph or --scope used to replace the first, so a read named two graphs and answered
        // from one, often empty. Each is given once; several graphs go in one list.
        let given = |flag: &str| args.flags.iter().filter(|(f, _)| f == flag).count();
        if given("--graph") > 1 {
            return Err(fail(
                if MULTI_GRAPH.contains(&command) || command == "scope save" || command == "ui open"
                {
                    "--graph is given more than once (name every graph in one --graph a,b)"
                } else {
                    "--graph is given more than once (this command takes one graph: use --graph with one name)"
                },
            ));
        }
        if given("--scope") > 1 {
            return Err(fail(
                "--scope is given more than once (name one saved scope, or the graphs in one --graph a,b)",
            ));
        }
        Ok(args)
    }

    fn one(&self, flag: &str) -> Option<String> {
        self.flags
            .iter()
            .rev()
            .find(|(f, _)| f == flag)
            .map(|(_, v)| v.clone())
    }

    fn all(&self, flag: &str) -> Vec<String> {
        self.flags
            .iter()
            .filter(|(f, _)| f == flag)
            .map(|(_, v)| v.clone())
            .collect()
    }

    fn need(&self, flag: &str, what: &'static str) -> Result<String, Usage> {
        self.one(flag).ok_or(Usage {
            command: self.command,
            what,
        })
    }

    fn number(&self, flag: &str) -> Result<Option<usize>, Usage> {
        self.one(flag)
            .map(|v| {
                v.parse().map_err(|_| Usage {
                    command: self.command,
                    what: "--limit and --offset take a whole number",
                })
            })
            .transpose()
    }

    /// `--depth` or `--max-depth`: a whole number of 1 or more.
    fn depth(&self, flag: &str) -> Result<Option<usize>, Usage> {
        self.one(flag)
            .map(|v| match v.parse::<usize>() {
                Ok(n) if n > 0 => Ok(n),
                _ => Err(Usage {
                    command: self.command,
                    what: "--depth and --max-depth take a whole number of 1 or more",
                }),
            })
            .transpose()
    }

    /// `--direction out|in|both`.
    fn direction(&self) -> Result<Option<Direction>, Usage> {
        self.one("--direction")
            .map(|v| {
                Direction::parse(&v).ok_or(Usage {
                    command: self.command,
                    what: "--direction is out, in or both",
                })
            })
            .transpose()
    }

    /// One optional folder, absolute; the current folder when none is given (ws002 s17).
    fn folder_or_here(&self, cwd: &Path, what: &'static str) -> Result<String, Usage> {
        match self.positional.as_slice() {
            [] => Ok(absolute(".", cwd)),
            [one] => Ok(absolute(one, cwd)),
            _ => Err(Usage {
                command: self.command,
                what,
            }),
        }
    }

    fn has(&self, switch: &str) -> bool {
        self.switches.iter().any(|s| s == switch)
    }

    /// `--related` (Some(true)) or `--no-related` (Some(false)), req-123; neither leaves it to the settings.
    fn related(&self) -> Result<Option<bool>, Usage> {
        match (self.has("--related"), self.has("--no-related")) {
            (true, true) => Err(Usage {
                command: self.command,
                what: "give --related or --no-related, not both",
            }),
            (on, off) => Ok((on || off).then_some(on)),
        }
    }

    fn positionals(&self, n: usize, what: &'static str) -> Result<Vec<String>, Usage> {
        if self.positional.len() == n {
            Ok(self.positional.clone())
        } else {
            Err(Usage {
                command: self.command,
                what,
            })
        }
    }

    fn columns(&self) -> Option<Vec<String>> {
        self.one("--columns").map(|c| {
            c.split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect()
        })
    }

    /// `--columns` for node get, node list and edge list: `all` alone means every custom field on the page (F74, F137);
    /// beside other names it is refused, never read as a field named `all`.
    fn list_columns(&self) -> Result<Option<Vec<String>>, Usage> {
        let columns = self.columns();
        match columns.as_deref() {
            Some(names) if names.len() > 1 && names.iter().any(|c| c == "all") => Err(Usage {
                command: self.command,
                what: "--columns all takes no other names: give all alone, or name the columns",
            }),
            _ => Ok(columns),
        }
    }

    fn sets(&self) -> Result<Vec<SetArg>, Usage> {
        self.trio(["--set", "--set-str", "--set-json"])
    }

    /// `--param`, `--param-str` and `--param-json`, in the order given (umx query).
    fn params(&self) -> Result<Vec<SetArg>, Usage> {
        self.trio(["--param", "--param-str", "--param-json"])
    }

    /// `--along <spine> [--after <end>] [--before <end>]` (D71 step 12). An end without `--along` is refused.
    fn along(&self) -> Result<Option<umx_protocol::Along>, Usage> {
        let (after, before) = (self.one("--after"), self.one("--before"));
        match self.one("--along") {
            Some(spine) => Ok(Some(umx_protocol::Along {
                spine,
                after,
                before,
            })),
            None if after.is_some() || before.is_some() => Err(Usage {
                command: self.command,
                what: "--after and --before need --along <spine>",
            }),
            None => Ok(None),
        }
    }

    /// One `--set`-style trio: raw (typed as umx types a value), always a string, or JSON.
    fn trio(&self, [raw_flag, str_flag, json_flag]: [&str; 3]) -> Result<Vec<SetArg>, Usage> {
        let mut out = Vec::new();
        for (flag, raw) in &self.flags {
            let wrap: fn(String) -> Option<SetValue> = match flag.as_str() {
                f if f == raw_flag => |v| Some(SetValue::Raw(v)),
                f if f == str_flag => |v| Some(SetValue::Str(v)),
                f if f == json_flag => |v| parse_json(&v).map(SetValue::Json),
                _ => continue,
            };
            let (field, value) = raw.split_once('=').ok_or(Usage {
                command: self.command,
                what: "a --set value is <field>=<value>",
            })?;
            let value = wrap(value.to_owned()).ok_or(Usage {
                command: self.command,
                what: "a --set-json value is not valid JSON",
            })?;
            out.push(SetArg {
                field: field.to_owned(),
                value,
            });
        }
        Ok(out)
    }
}

/// An absolute, lexically normalised path: relative paths are taken from the current folder.
fn absolute(path: &str, cwd: &Path) -> String {
    let joined = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        cwd.join(path)
    };
    let mut out = PathBuf::from("/");
    for c in joined.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(p) => out.push(p),
            _ => {}
        }
    }
    out.to_string_lossy().into_owned()
}

/// A duration such as `500ms`, `90s`, `30m` or `2h`, in ms; a bare `0` is zero.
pub fn duration_ms(text: &str) -> Option<u64> {
    if text == "0" {
        return Some(0);
    }
    let split = text.find(|c: char| !c.is_ascii_digit())?;
    let (n, unit) = text.split_at(split);
    let scale = match unit {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        _ => return None,
    };
    n.parse::<u64>().ok()?.checked_mul(scale)
}

/// The operation label for a noun and verb, from [`Op::LABELS`] only.
pub fn known(noun: &str, verb: &str) -> Option<&'static str> {
    let label = command_label(&[noun, verb]);
    Op::LABELS.contains(&label).then_some(label)
}

/// `umx node evidence <node> list|add|remove|refresh [<pointer>]` (req-126; refresh usage 220). The action may also
/// come first (`node evidence remove <node> <pointer>`). Each action takes only its own flags; remove needs
/// `--reason`, and `remove --redact` (F183) takes no `--at` (it scrubs every pointer with that source). refresh takes
/// a pointer, `--at`, or neither (every pointer).
fn node_evidence(a: Args) -> Result<Parsed, Usage> {
    let fail = |what| Usage {
        command: "node evidence",
        what,
    };
    let words: Vec<&str> = a.positional.iter().map(String::as_str).collect();
    let (node, action, pointer) = match words.as_slice() {
        [first, second, rest @ ..]
            if EvidenceAction::parse(first).is_some()
                && EvidenceAction::parse(second).is_none() =>
        {
            (*second, EvidenceAction::parse(first), rest)
        }

        [node, action, rest @ ..] => (*node, EvidenceAction::parse(action), rest),
        _ => {
            return Err(fail(
                "node evidence takes <node> list, <node> add <pointer>, <node> remove <pointer> or <node> refresh",
            ));
        }
    };
    let action = action.ok_or(fail("the action is list, add, remove or refresh"))?;
    let pointer = match (action, pointer) {
        (EvidenceAction::List | EvidenceAction::Refresh, []) => None,
        (EvidenceAction::List, _) => {
            return Err(fail("node evidence <node> list takes no pointer"));
        }
        (_, [p]) => Some((*p).to_string()),
        (_, []) => {
            return Err(fail(
                "name the pointer after add or remove (quote it in the shell)",
            ));
        }
        (_, _) => return Err(fail("give one pointer; quote it if it has spaces")),
    };
    let removed = a.switches.iter().any(|s| s == "--removed");
    let redact = a.switches.iter().any(|s| s == "--redact");
    let all = a.switches.iter().any(|s| s == "--all");
    let reason = a.one("--reason");
    let at = a
        .one("--at")
        .map(|v| match v.parse::<usize>() {
            Ok(n) if n >= 1 => Ok(n),
            _ => Err(fail(
                "--at takes a position from node evidence <node> list (1 or more)",
            )),
        })
        .transpose()?;
    let evidence_kind = a.one("--evidence-kind");
    match action {
        EvidenceAction::List
            if reason.is_some() || at.is_some() || evidence_kind.is_some() || redact =>
        {
            return Err(fail("node evidence <node> list takes only --removed"));
        }
        EvidenceAction::Add if reason.is_some() || at.is_some() || removed || redact => {
            return Err(fail("node evidence <node> add takes only --evidence-kind"));
        }
        EvidenceAction::Remove if reason.is_none() => {
            return Err(fail(
                "removing evidence needs --reason (why it is wrong or stale)",
            ));
        }
        EvidenceAction::Remove if evidence_kind.is_some() || removed => {
            return Err(fail(
                "node evidence <node> remove takes --reason, and --at or --redact",
            ));
        }
        EvidenceAction::Refresh
            if reason.is_some() || evidence_kind.is_some() || removed || redact =>
        {
            return Err(fail(
                "node evidence <node> refresh takes a pointer, --at <n> or --all",
            ));
        }
        EvidenceAction::Refresh
            if usize::from(pointer.is_some()) + usize::from(at.is_some()) + usize::from(all)
                != 1 =>
        {
            return Err(fail(
                "node evidence <node> refresh takes one of a pointer, --at <n> or --all",
            ));
        }
        _ if all && action != EvidenceAction::Refresh => {
            return Err(fail("--all goes with node evidence <node> refresh only"));
        }
        EvidenceAction::Remove if redact && at.is_some() => {
            return Err(fail(
                "--redact scrubs every pointer with that source; drop --at",
            ));
        }
        _ => {}
    }
    Ok(Op::NodeEvidence {
        node: node.to_string(),
        action,
        pointer,
        evidence_kind,
        reason,
        at,
        removed,
        redact,
        all,
    }
    .with(a))
}
/// `--type`, `--status`, `--tag`, `--where` and `--match` as a pattern object (a rule's or a named hub's match).
fn pattern_flags(a: &Args, command: &'static str) -> Result<umx_protocol::Value, Usage> {
    let fail = |what| Usage { command, what };
    let mut pattern = umx_protocol::json!({});
    for flag in ["--type", "--status", "--tag"] {
        let values = list_flag(a, flag).map_err(fail)?;
        if !values.is_empty() {
            pattern[&flag[2..]] = umx_protocol::json!(values);
        }
    }
    for (flag, key) in [("--where", "field"), ("--match", "regex")] {
        for text in a.all(flag) {
            let (field, value) = text
                .split_once('=')
                .ok_or_else(|| fail("--where and --match take <field>=<value>"))?;
            if pattern.get(key).and_then(|v| v.get(field)).is_some() {
                return Err(fail("give each pattern field once"));
            }
            if pattern.get(key).is_none() {
                pattern[key] = umx_protocol::json!({});
            }
            pattern[key][field] = if key == "regex" {
                umx_protocol::json!(value)
            } else {
                parse_json(value).unwrap_or_else(|| umx_protocol::json!(value))
            };
        }
    }
    Ok(pattern)
}

/// A comma list flag given any number of times, trimmed, sorted and without repeats.
fn list_flag(a: &Args, flag: &str) -> Result<Vec<String>, &'static str> {
    let mut values = Vec::new();
    for value in a.all(flag) {
        for part in value.split(',') {
            if part.trim().is_empty() {
                return Err("rule lists need nonempty entries");
            }
            values.push(part.trim().to_owned());
        }
    }
    values.sort();
    values.dedup();
    Ok(values)
}

fn rule_add(a: Args) -> Result<Parsed, Usage> {
    let fail = |what| Usage {
        command: "rule add",
        what,
    };
    let mut rule = umx_protocol::json!({"name": a.positionals(1, "give one rule name")?.remove(0)});
    let pattern = pattern_flags(&a, "rule add")?;
    for flag in ["--from", "--to"] {
        let values = list_flag(&a, flag).map_err(fail)?;
        if !values.is_empty() {
            rule[&flag[2..]] = umx_protocol::json!(values);
        }
    }
    for (flag, key) in [
        ("--field", "field"),
        ("--edge", "edge"),
        ("--to-node", "to_node"),
        ("--description", "description"),
    ] {
        if a.all(flag).len() > 1 {
            return Err(fail("give each rule shape flag and description once"));
        }
        if let Some(value) = a.one(flag) {
            rule[key] = umx_protocol::json!(value);
        }
    }
    rule["match"] = pattern;
    Ok(Op::RuleAdd {
        rule,
        dry_run: a.has("--dry-run"),
    }
    .with(a))
}

/// `umx batch`'s flags: the `--file` to read (`-` is stdin), each `--refs-from` receipt (repeatable, in order), and
/// the operation with no lines or refs yet. The files are read by the caller, never named on the wire: the daemon
/// opens no path a request names.
pub fn batch(words: &[&str]) -> Result<(String, Vec<String>, Parsed), Usage> {
    let a = Args::new("batch", words)?;
    a.positionals(
        0,
        "give the ops with --file <ops.jsonl>, or --file - for stdin",
    )?;
    let file = a.need(
        "--file",
        "give the ops with --file <ops.jsonl>, or --file - for stdin",
    )?;
    let receipts = a.all("--refs-from");
    let idempotency_key = a.one("--idempotency-key");
    if idempotency_key
        .as_deref()
        .is_some_and(|k| !umx_protocol::idempotency_key_ok(k))
    {
        return Err(Usage {
            command: "batch",
            what: umx_protocol::IDEMPOTENCY_KEY_RULE,
        });
    }
    let op = Op::Batch {
        lines: Vec::new(),
        dry_run: a.switches.iter().any(|s| s == "--dry-run"),
        reason: a.one("--reason"),
        echo_sentences: a.switches.iter().any(|s| s == "--echo-sentences"),
        refs: Default::default(),
        refs_graph: None,
        raised_in: a.one("--raised-in"),
        related: a.related()?,
        idempotency_key,
    };
    Ok((file, receipts, op.with(a)))
}

/// Parses `words` (noun, verb, then arguments). `cwd` resolves relative graph paths.
pub fn parse(words: &[&str], cwd: &Path) -> Result<Parsed, Usage> {
    // Top-level verbs that are operations (workshop 007 rule 1).
    match words {
        ["hub", verb, rest @ ..] => {
            let command = known("hub", verb).ok_or(Usage {
                command: "hub list",
                what: "hub takes list, set, show, fill or remove",
            })?;
            let a = Args::new(command, rest)?;
            if *verb == "list" {
                a.positionals(0, "hub list takes no positional arguments")?;
                return Ok(Op::HubList.with(a));
            }
            let name = a.positionals(1, "give one hub name")?.remove(0);
            let op = match *verb {
                "set" => {
                    for flag in ["--node", "--type-hub", "--via"] {
                        if a.all(flag).len() > 1 {
                            return Err(Usage {
                                command: "hub set",
                                what: "give --node, --type-hub and --via once each",
                            });
                        }
                    }
                    let pattern = pattern_flags(&a, "hub set")?;
                    Op::HubSet {
                        name,
                        node: a.one("--node"),
                        type_hub: a.one("--type-hub"),
                        via: a.one("--via"),
                        pattern: (pattern != umx_protocol::json!({})).then_some(pattern),
                    }
                }
                "show" => Op::HubShow {
                    name,
                    limit: a.number("--limit")?,
                    offset: a.number("--offset")?,
                },
                "fill" => Op::HubFill {
                    name,
                    after: a.one("--after"),
                },
                "remove" => Op::HubRemove { name },
                _ => unreachable!("known hub verb"),
            };
            return Ok(op.with(a));
        }
        ["rule", verb, rest @ ..] => {
            let command = known("rule", verb).ok_or(Usage {
                command: "rule list",
                what: "rule takes list, add, remove, off, on or show",
            })?;
            let a = Args::new(command, rest)?;
            if *verb == "add" {
                return rule_add(a);
            }
            if *verb == "list" {
                a.positionals(0, "rule list takes no positional arguments")?;
                return Ok(Op::RuleList.with(a));
            }
            let name = a.positionals(1, "give one rule name")?.remove(0);
            let op = match *verb {
                "remove" => Op::RuleRemove { name },
                "off" => Op::RuleOff { name },
                "on" => Op::RuleOn { name },
                "show" => Op::RuleShow {
                    name,
                    limit: a.number("--limit")?,
                    offset: a.number("--offset")?,
                },
                _ => unreachable!("known rule verb"),
            };
            return Ok(op.with(a));
        }
        ["graph", "spine", "propose", rest @ ..] => {
            let a = Args::new("graph spine propose", rest)?;
            let alias = a.positionals(1, "name the graph's alias")?.remove(0);
            return Ok(Op::GraphSpinePropose {
                alias,
                unplaced: a.one("--unplaced"),
                by_rule: a.one("--by-rule"),
                limit: a.number("--limit")?,
                offset: a.number("--offset")?,
            }
            .with(a));
        }
        // ws008 TM5: the declaration's file is read by the CLI (lib.rs), never sent as a path.
        ["graph", "spine", "set", rest @ ..] => {
            let a = Args::new("graph spine set", rest)?;
            let alias = a.positionals(1, "name the graph's alias")?.remove(0);
            let file = a.need(
                "--file",
                "give the declaration with --file <decl.json> (- reads stdin)",
            )?;
            return Ok(Op::GraphSpineSet {
                alias,
                decl: umx_protocol::Value::String(file),
            }
            .with(a));
        }
        ["graph", "spine", "confirm", rest @ ..] => {
            let a = Args::new("graph spine confirm", rest)?;
            let mut words =
                a.positionals(2, "name the graph's alias and one candidate (type:<t>)")?;
            let candidate = words.remove(1);
            return Ok(Op::GraphSpineConfirm {
                alias: words.remove(0),
                candidate,
            }
            .with(a));
        }
        ["graph", "spine", "show", rest @ ..] => {
            let a = Args::new("graph spine show", rest)?;
            let mut words = a.positional.clone();
            if !(1..=2).contains(&words.len()) {
                return Err(Usage {
                    command: "graph spine show",
                    what: "name the graph's alias, and one spine if you want only that one",
                });
            }
            let named = (words.len() == 2).then(|| words.remove(1));
            // `--by-rule <name>` or `--unplaced <name>` (D71 step 12) names the spine whose rule placements, or
            // whose nodes it does not place, to list (B4a).
            let by_rule = a.one("--by-rule");
            let unplaced = a.one("--unplaced");
            if by_rule.is_some() && unplaced.is_some() {
                return Err(Usage {
                    command: "graph spine show",
                    what: "give --by-rule or --unplaced, not both",
                });
            }
            let listed = by_rule.clone().or(unplaced.clone());
            if named.is_some() && listed.is_some() {
                return Err(Usage {
                    command: "graph spine show",
                    what: "name the spine once: either after the alias or after --by-rule or --unplaced",
                });
            }
            return Ok(Op::GraphSpineShow {
                alias: words.remove(0),
                by_rule: by_rule.is_some(),
                unplaced: unplaced.is_some(),
                name: listed.or(named),
                limit: a.number("--limit")?,
                offset: a.number("--offset")?,
            }
            .with(a));
        }
        ["graph", "spine", "remove", rest @ ..] => {
            let a = Args::new("graph spine remove", rest)?;
            let mut words = a.positionals(2, "name the graph's alias and the spine")?;
            let name = words.remove(1);
            return Ok(Op::GraphSpineRemove {
                alias: words.remove(0),
                name,
            }
            .with(a));
        }
        ["check", rest @ ..] => {
            let a = Args::new("check", rest)?;
            a.positionals(0, "this command takes no arguments")?;
            // F139: --kind and --offset page --lint's findings.
            let lint = a.switches.iter().any(|s| s == "--lint");
            let rules = a.switches.iter().any(|s| s == "--rules");
            let rule = a.one("--rule");
            let after = a.one("--after");
            let sources = a.switches.iter().any(|s| s == "--sources");
            let kind = a.one("--kind");
            let offset = a.number("--offset")?;
            if (!lint && kind.is_some()) || (!lint && !rules && offset.is_some()) {
                return Err(Usage {
                    command: "check",
                    what: "--kind goes with --lint; --offset goes with --lint or --rules",
                });
            }
            if !rules && (rule.is_some() || after.is_some()) {
                return Err(Usage {
                    command: "check",
                    what: "--rule and --after go with --rules",
                });
            }
            return Ok(Op::Check {
                rules,
                rule,
                after,
                lint,
                sources,
                source_budget: None,
                kind,
                offset,
            }
            .with(a));
        }
        ["neighbours", rest @ ..] => {
            let a = Args::new("neighbours", rest)?;
            let node = a.positionals(1, "name one node")?.remove(0);
            return Ok(Op::Neighbours {
                node,
                relation: a.one("--relation"),
                direction: a.direction()?,
                limit: a.number("--limit")?,
                offset: a.number("--offset")?,
                tags: a.all("--tag"),
                no_tags: a.all("--no-tag"),
                entities: a
                    .one("--entities")
                    .map(|v| {
                        umx_protocol::Entities::parse(&v).ok_or(Usage {
                            command: "neighbours",
                            what: "--entities is on, off or confirmed",
                        })
                    })
                    .transpose()?,
            }
            .with(a));
        }
        ["walk", rest @ ..] => {
            let a = Args::new("walk", rest)?;
            let node = a.positionals(1, "name the node to walk from")?.remove(0);
            return Ok(Op::Walk {
                node,
                depth: a.depth("--depth")?,
                relation: a.one("--relation"),
                direction: a.direction()?,
                within: a.one("--within"),
                limit: a.number("--limit")?,
                offset: a.number("--offset")?,
                tags: a.all("--tag"),
                no_tags: a.all("--no-tag"),
            }
            .with(a));
        }
        ["resolve", rest @ ..] => {
            let a = Args::new("resolve", rest)?;
            let address = a
                .positionals(1, "give one address (umx:<graph>/<id>) or id")?
                .remove(0);
            return Ok(Op::Resolve { address }.with(a));
        }
        ["path", rest @ ..] => {
            let a = Args::new("path", rest)?;
            let mut ends = a.positionals(2, "path takes <from> <to>")?.into_iter();
            return Ok(Op::Path {
                from: ends.next().expect("2"),
                to: ends.next().expect("2"),
                max_depth: a.depth("--max-depth")?,
                relation: a.one("--relation"),
                direction: a.direction()?,
            }
            .with(a));
        }
        _ => {}
    }
    if let ["query", rest @ ..] = words {
        let a = Args::new("query", rest)?;
        let text = a
            .positionals(1, "give the query as one quoted argument")?
            .remove(0);
        let limit = a.number("--limit")?;
        if limit.is_some_and(|n| n == 0 || n > 100_000) {
            return Err(Usage {
                command: "query",
                what: "--limit takes a whole number from 1 to 100000",
            });
        }
        return Ok(Op::Query {
            text,
            limit,
            offset: a.number("--offset")?,
            params: a.params()?,
            along: a.along()?,
        }
        .with(a));
    }
    if let ["who", rest @ ..] = words {
        let a = Args::new("who", rest)?;
        let who = a
            .positionals(
                1,
                "name one agent or session: a prov_key, or a bare seat, user or session id",
            )?
            .remove(0);
        return Ok(Op::Who {
            who,
            limit: a.number("--limit")?,
        }
        .with(a));
    }
    if let ["search", rest @ ..] = words {
        let a = Args::new("search", rest)?;
        // A word is a run of letters or digits; text with none matches nothing.
        if !a
            .positional
            .iter()
            .any(|w| w.chars().any(char::is_alphanumeric))
        {
            return Err(Usage {
                command: "search",
                what: "give the words to look for",
            });
        }
        let text = a.positional.join(" ");
        // PRD 002 search-09: up to 4 more phrasings, refused by position, never by their text.
        let also = a.all("--also");
        if let Some(what) = umx_protocol::also_problem(&text, &also) {
            return Err(Usage {
                command: "search",
                what,
            });
        }
        return Ok(Op::Search {
            text,
            within: a.one("--within"),
            limit: a.number("--limit")?,
            along: a.along()?,
            also,
            tags: a.all("--tag"),
            no_tags: a.all("--no-tag"),
        }
        .with(a));
    }
    let (noun, verb, rest) = match words {
        [noun, verb, rest @ ..] => (*noun, *verb, rest),
        _ => {
            return Err(Usage {
                command: "unknown",
                what: "name a verb; the docs topic for the noun lists them",
            });
        }
    };
    let command = known(noun, verb).ok_or(Usage {
        command: "unknown",
        what: "unknown command",
    })?;
    let args = || Args::new(command, rest);
    let op = match command {
        "scope save" => {
            let a = args()?;
            let name = a.positionals(1, "name the scope to save")?.remove(0);
            a.need("--graph", "scope save needs --graph a,b")?;
            Op::ScopeSave {
                name,
                replace: a.switches.iter().any(|s| s == "--replace"),
            }
            .with(a)
        }
        "scope list" => {
            let a = args()?;
            a.positionals(0, "scope list takes no arguments")?;
            Op::ScopeList.with(a)
        }
        "scope show" | "scope delete" => {
            let a = args()?;
            let name = a.positionals(1, "name the saved scope")?.remove(0);
            if command == "scope show" {
                Op::ScopeShow { name }
            } else {
                Op::ScopeDelete { name }
            }
            .with(a)
        }
        "graph new" => {
            let a = args()?;
            if a.has("--no-graphfile") && a.has("--graphfile") {
                return Err(Usage {
                    command: a.command,
                    what: "--no-graphfile and --graphfile cannot be given together",
                });
            }
            let path = a.positionals(1, "name the graph's folder")?.remove(0);
            Op::GraphNew {
                path: absolute(&path, cwd),
                alias: a.need("--alias", "--alias <alias> is required")?,
                title: a.one("--title"),
                empty: a.switches.iter().any(|s| s == "--empty"),
                embedding_model: a.one("--embedding-model"),
                secured: a.has("--secured"),
                no_graphfile: a.has("--no-graphfile"),
                graphfile: a.has("--graphfile"),
                no_std: a.has("--no-std"),
            }
            .with(a)
        }
        "graph load" if args()?.has("--graphfile") => {
            let a = args()?;
            if a.one("--as").is_some() {
                return Err(Usage {
                    command: a.command,
                    what: "--as names one graph; graph load --graphfile loads each under its Graphfile alias",
                });
            }
            Op::GraphLoad {
                path: a.folder_or_here(cwd, "name at most one folder in the repo")?,
                as_alias: None,
                by_alias: None,
                graphfile: true,
                set: a.one("--set"),
            }
            .with(a)
        }
        "graph load" => {
            let a = args()?;
            if a.one("--set").is_some() {
                return Err(Usage {
                    command: a.command,
                    what: "--set names a Graphfile set: give it with --graphfile",
                });
            }
            let path = a
                .positionals(
                    1,
                    "name the graph's folder, or the alias of a graph this machine knows",
                )?
                .remove(0);
            // A bare word may be an alias in this machine's graph index (req-116); the daemon tries the folder first.
            let by_alias = (!path.contains('/') && !path.starts_with('.')).then(|| path.clone());
            Op::GraphLoad {
                path: absolute(&path, cwd),
                as_alias: a.one("--as"),
                by_alias,
                graphfile: false,
                set: None,
            }
            .with(a)
        }
        "graph list" => {
            let a = args()?;
            let graphfile = a
                .has("--graphfile")
                .then(|| a.folder_or_here(cwd, "name at most one folder in the repo"))
                .transpose()?;
            if graphfile.is_none() {
                a.positionals(0, "this command takes no arguments")?;
            } else if a.has("--all") {
                return Err(Usage {
                    command: a.command,
                    what: "--all and --graphfile are two lists: give one",
                });
            }
            let all = a.switches.iter().any(|s| s == "--all");
            // `graph list --all` is this machine's graph index (req-116), not search and query's request-wide `--all`.
            Parsed {
                all: false,
                ..Op::GraphList { all, graphfile }.with(a)
            }
        }
        "graph scan" => {
            let a = args()?;
            Op::GraphScan {
                path: a.folder_or_here(cwd, "name at most one folder to scan")?,
                add: a.has("--add"),
                rewrite: a.has("--rewrite"),
                drop: a.one("--drop"),
                depth: a
                    .depth("--depth")?
                    .map(|d| u32::try_from(d).unwrap_or(u32::MAX)),
                whole_repo: a.positional.is_empty(),
            }
            .with(a)
        }
        "similarity rebuild" => {
            let a = args()?;
            a.positionals(
                0,
                "this command takes no arguments; name the graph with --graph",
            )?;
            Op::SimilarityRebuild.with(a)
        }
        "similarity pairs" => {
            let a = args()?;
            a.positionals(
                0,
                "this command takes no arguments; name the graphs with --graph a,b",
            )?;
            let bad = |what| Usage {
                command: "similarity pairs",
                what,
            };
            let k = a.number("--k")?;
            if k.is_some_and(|n| n == 0 || n > 10) {
                return Err(bad("--k takes a whole number from 1 to 10"));
            }
            let limit = a.number("--limit")?;
            if limit.is_some_and(|n| n == 0 || n > 250_000) {
                return Err(bad("--limit takes a whole number from 1 to 250000"));
            }
            let rows = match a.one("--rows").as_deref() {
                None | Some("full") => PairRows::Full,
                Some("ids") => PairRows::Ids,
                Some(_) => return Err(bad("--rows takes full or ids")),
            };
            Op::SimilarityPairs {
                k,
                min: a.one("--min"),
                model: a.one("--model"),
                same_graph: a.switches.iter().any(|s| s == "--same-graph"),
                limit,
                offset: a.number("--offset")?,
                rows,
            }
            .with(a)
        }
        "embeddings repair" => {
            let a = args()?;
            a.positionals(0, "this command takes no arguments")?;
            Op::EmbeddingsRepair.with(a)
        }
        "embeddings allow" => {
            let a = args()?;
            a.positionals(0, "embeddings allow takes --node <node> --field <field>")?;
            Op::EmbeddingsAllow {
                node: a.need("--node", "--node <node> is required")?,
                field: a.need("--field", "--field <field> is required")?,
            }
            .with(a)
        }
        "embeddings reembed" => {
            let a = args()?;
            a.positionals(
                0,
                "this command takes no arguments; name the graph with --graph",
            )?;
            Op::EmbeddingsReembed {
                dry_run: a.switches.iter().any(|s| s == "--dry-run"),
                model: a.one("--model"),
            }
            .with(a)
        }
        "node add" => {
            let a = args()?;
            a.positionals(
                0,
                "node add takes flags only: --type <type> --title <title>",
            )?;
            let kind = a.need("--type", "--type <type> is required")?;
            // A resource node's title defaults to its source's last part (ws006 S3b).
            let locator = a.one("--locator");
            let title = match locator {
                Some(_) => a.one("--title").unwrap_or_default(),
                None => a.need("--title", "--title <title> is required")?,
            };
            let on_key = match a.one("--on-key").as_deref() {
                None => None,
                Some("resolve") => Some(OnKey::Resolve),
                Some(_) => {
                    return Err(Usage {
                        command: "node add",
                        what: "--on-key takes resolve",
                    });
                }
            };
            Op::NodeAdd {
                tags: a.all("--tag"),
                kind,
                title,
                status: a.one("--status"),
                note: a.one("--note"),
                sets: a.sets()?,
                parent: a.one("--parent"),
                evidence: a.all("--evidence"),
                evidence_kind: a.one("--evidence-kind"),
                locator,
                raised_in: a.one("--raised-in"),
                on_key,
                related: a.related()?,
            }
            .with(a)
        }
        "resource show" => {
            let a = args()?;
            let node = a
                .positionals(1, "name one resource node by id, prefix or address")?
                .remove(0);
            Op::ResourceShow { node }.with(a)
        }
        "resource refresh" => {
            let a = args()?;
            let node = a
                .positionals(1, "name one resource node by id, prefix or address")?
                .remove(0);
            Op::ResourceRefresh { node }.with(a)
        }
        "node get" => {
            let a = args()?;
            let node = a
                .positionals(1, "name one node by id, prefix or address")?
                .remove(0);
            Op::NodeGet {
                node,
                columns: a.list_columns()?,
            }
            .with(a)
        }
        "node list" => {
            let a = args()?;
            a.positionals(0, "node list takes flags only")?;
            Op::NodeList {
                tags: a.all("--tag"),
                no_tags: a.all("--no-tag"),
                kind: a.one("--type"),
                status: a.one("--status"),
                wheres: a.all("--where"),
                within: a.one("--within"),
                columns: a.list_columns()?,
                limit: a.number("--limit")?,
                offset: a.number("--offset")?,
                along: a.along()?,
            }
            .with(a)
        }
        "node find" => {
            let a = args()?;
            if a.positional.is_empty() {
                return Err(Usage {
                    command,
                    what: "give the words to look for",
                });
            }
            Op::NodeFind {
                text: a.positional.join(" "),
                kind: a.one("--type"),
                limit: a.number("--limit")?,
                within: a.one("--within"),
                along: a.along()?,
                tags: a.all("--tag"),
                no_tags: a.all("--no-tag"),
            }
            .with(a)
        }
        "edge add" => {
            let a = args()?;
            let mut p = a
                .positionals(3, "edge add takes <from> <relation> <to>")?
                .into_iter();
            let (from, relation, to) = (
                p.next().expect("3"),
                p.next().expect("3"),
                p.next().expect("3"),
            );
            Op::EdgeAdd {
                from,
                relation,
                to,
                sets: a.sets()?,
                evidence: a.all("--evidence"),
                evidence_kind: a.one("--evidence-kind"),
            }
            .with(a)
        }
        "edge show" => {
            let a = args()?;
            let edge = a
                .positionals(
                    1,
                    "name one edge by its id or address umx:<graph>/<edge id>",
                )?
                .remove(0);
            Op::EdgeShow { edge }.with(a)
        }
        "edge delete" => {
            let a = args()?;
            let edge = a
                .positionals(1, "name one edge by its id or address")?
                .remove(0);
            Op::EdgeDelete { edge }.with(a)
        }
        "edge edit" => {
            let a = args()?;
            let edge = a
                .positionals(1, "name one edge by its id or address")?
                .remove(0);
            let reverse = a.switches.iter().any(|s| s == "--reverse");
            let relation = a.one("--relation");
            let (sets, unsets) = (a.sets()?, a.all("--unset"));
            if relation.is_none() && !reverse && sets.is_empty() && unsets.is_empty() {
                return Err(Usage {
                    command: "edge edit",
                    what: "give --relation <relation>, --reverse, --set <field>=<value> or --unset <field>",
                });
            }
            Op::EdgeEdit {
                edge,
                relation,
                reverse,
                sets,
                unsets,
            }
            .with(a)
        }
        "edge list" => {
            let a = args()?;
            a.positionals(0, "edge list takes flags only")?;
            Op::EdgeList {
                from: a.one("--from"),
                to: a.one("--to"),
                kind: a.one("--type"),
                columns: a.list_columns()?,
                limit: a.number("--limit")?,
                offset: a.number("--offset")?,
                along: a.along()?,
                wheres: a.all("--where"),
            }
            .with(a)
        }
        "graph set" => {
            // Any `--<setting> <value>`: the daemon checks names against the one settings table (umx-core), so
            // a setting added there needs no change here. Switches such as --empty stay unknown flags.
            let named: Vec<&str> = rest
                .iter()
                .filter(|w| w.starts_with("--"))
                .map(|w| w.split_once('=').map_or(*w, |(f, _)| f))
                .filter(|f| !SWITCHES.contains(f) || graph_setting(f))
                .collect();
            let a = Args::allowing(command, rest, &named)?;
            let alias = a.positionals(1, "name the graph's alias")?.remove(0);
            let settings = a
                .flags
                .iter()
                .filter(|(f, _)| f != "--graph" && f != "--by")
                .map(|(f, v)| SettingArg {
                    name: f.trim_start_matches("--").to_owned(),
                    value: v.clone(),
                })
                .collect();
            Op::GraphSet { alias, settings }.with(a)
        }
        "graph confirm-policy" => {
            let a = args()?;
            let alias = a.positionals(1, "name the graph's alias")?.remove(0);
            Op::GraphConfirmPolicy { alias }.with(a)
        }
        "graph unload" => {
            let a = args()?;
            let alias = a.positionals(1, "name the graph's alias")?.remove(0);
            Op::GraphUnload { alias }.with(a)
        }
        "graph forget" => {
            let a = args()?;
            let alias = a.positionals(1, "name the graph's alias")?.remove(0);
            Op::GraphForget { alias }.with(a)
        }
        "graph rename" => {
            let a = args()?;
            let mut p = a.positionals(2, "name the graph's alias and its new alias")?;
            let to = p.remove(1);
            let from = p.remove(0);
            Op::GraphRename { from, to }.with(a)
        }
        "graph move" => {
            let a = args()?;
            let mut p = a.positionals(2, "name the graph's alias and the folder to move it to")?;
            let to = absolute(&p.remove(1), cwd);
            let alias = p.remove(0);
            Op::GraphMove { alias, to }.with(a)
        }
        "graph primary" => {
            let a = args()?;
            let alias = a.positionals(1, "name the copy's alias")?.remove(0);
            Op::GraphPrimary { alias }.with(a)
        }
        "graph guide" => {
            let a = args()?;
            let alias = a.positionals(1, "name the graph's alias")?.remove(0);
            Op::GraphGuide { alias }.with(a)
        }
        "node edit" => {
            let a = args()?;
            let node = a
                .positionals(1, "name one node by id, prefix or address")?
                .remove(0);
            Op::NodeEdit {
                tag: a.all("--tag"),
                untag: a.all("--untag"),
                node,
                title: a.one("--title"),
                kind: a.one("--type"),
                note: a.one("--note"),
                sets: a.sets()?,
                unsets: a.all("--unset"),
                locator: a.one("--locator"),
            }
            .with(a)
        }
        "entity key" => {
            let a = args()?;
            let what = "entity key takes <node> add|remove <key>";
            let mut p = a.positionals(3, what)?.into_iter();
            let node = p.next().expect("3");
            let action = KeyAction::parse(&p.next().expect("3")).ok_or(Usage {
                command: a.command,
                what,
            })?;
            Op::EntityKey {
                node,
                action,
                key: p.next().expect("3"),
            }
            .with(a)
        }
        "entity deny" | "entity undeny" => {
            let a = args()?;
            let what = "entity deny and undeny take two nodes: <a> <b>";
            let mut p = a.positionals(2, what)?.into_iter();
            let (x, y) = (p.next().expect("2"), p.next().expect("2"));
            if a.command == "entity deny" {
                Op::EntityDeny {
                    a: x,
                    b: y,
                    reason: a.one("--reason"),
                }
            } else {
                Op::EntityUndeny { a: x, b: y }
            }
            .with(a)
        }
        "entity link" | "entity unlink" => {
            let a = args()?;
            let what = "entity link and unlink take two nodes: <a> <b>";
            let mut p = a.positionals(2, what)?.into_iter();
            let (x, y) = (p.next().expect("2"), p.next().expect("2"));
            if a.command == "entity link" {
                let dry_run = a.switches.iter().any(|s| s == "--dry-run");
                Op::EntityLink {
                    a: x,
                    b: y,
                    dry_run,
                }
            } else {
                Op::EntityUnlink { a: x, b: y }
            }
            .with(a)
        }
        "entity show" => {
            let a = args()?;
            let node = a
                .positionals(1, "name one node by id, prefix or address")?
                .remove(0);
            Op::EntityShow { node }.with(a)
        }
        "node note" => {
            let a = args()?;
            let node = a
                .positionals(1, "name one node by id, prefix or address")?
                .remove(0);
            Op::NodeNote {
                node,
                append: a.need("--append", "--append <text> is required")?,
                evidence: a.all("--evidence"),
                evidence_kind: a.one("--evidence-kind"),
            }
            .with(a)
        }
        "node evidence" => node_evidence(args()?)?,
        "node status" => {
            let a = args()?;
            let mut p = a
                .positionals(2, "node status takes <node> <status>")?
                .into_iter();
            Op::NodeStatus {
                node: p.next().expect("2"),
                status: p.next().expect("2"),
                reason: a.one("--reason"),
                label: a.one("--label"),
                place: a.one("--in"),
                evidence: a.one("--evidence"),
                evidence_kind: a.one("--evidence-kind"),
            }
            .with(a)
        }
        "node history" => {
            let a = args()?;
            let node = a
                .positionals(1, "name one node by id, prefix or address")?
                .remove(0);
            Op::NodeHistory {
                node,
                limit: a.number("--limit")?,
            }
            .with(a)
        }
        "node who" => {
            let a = args()?;
            let node = a
                .positionals(1, "name one node by id, prefix or address")?
                .remove(0);
            Op::NodeWho { node }.with(a)
        }
        // ws008 T3 (req-101): `ontology adopt-std [--map <from>=<to>]... [--dry-run]`.
        "ontology adopt-std" => {
            let a = args()?;
            a.positionals(
                0,
                "ontology adopt-std takes --map <from>=<to>, --field-to-tag <field> and --dry-run only",
            )?;
            Op::OntologyAdoptStd {
                map: a.all("--map"),
                dry_run: a.has("--dry-run"),
                field_to_tag: a.all("--field-to-tag"),
            }
            .with(a)
        }
        "ontology show" => {
            let a = args()?;
            a.positionals(0, "this command takes no arguments")?;
            Op::OntologyShow {
                std: a.has("--std"),
            }
            .with(a)
        }
        "embeddings probe" => {
            let a = args()?;
            a.positionals(0, "embeddings probe takes no positional arguments")?;
            Op::EmbeddingsProbe {
                embedder: a.one("--embedder"),
            }
            .with(a)
        }
        "embeddings status" => {
            let a = args()?;
            a.positionals(0, "this command takes no arguments")?;
            if a.one("--graph").is_some() {
                return Err(Usage {
                    command,
                    what: "embeddings status lists every loaded graph and takes no --graph",
                });
            }
            Op::EmbeddingsStatus.with(a)
        }
        "config import" => {
            let a = args()?;
            a.positionals(0, "config import takes --from flowspace3")?;
            if a.one("--graph").is_some() || a.one("--by").is_some() {
                return Err(Usage {
                    command,
                    what: "config commands take no --graph or --by",
                });
            }
            let from = a.need("--from", "--from flowspace3 is required")?;
            if from != "flowspace3" {
                return Err(Usage {
                    command,
                    what: "config import takes --from flowspace3 (the one tool umx imports from)",
                });
            }
            Op::ConfigImport {
                from,
                name: a.one("--as"),
                model: a.one("--model"),
                dims: a
                    .one("--dims")
                    .map(|d| d.parse::<u32>())
                    .transpose()
                    .map_err(|_| Usage {
                        command,
                        what: "--dims takes a whole number such as 256",
                    })?,
                force: a.switches.iter().any(|s| s == "--force"),
                dry_run: a.switches.iter().any(|s| s == "--dry-run"),
            }
            .with(a)
        }
        "config show" | "config check" | "config set" | "config unset" => {
            let a = args()?;
            if a.one("--graph").is_some() || a.one("--by").is_some() {
                return Err(Usage {
                    command,
                    what: "config commands take no --graph or --by",
                });
            }
            let cwd = Some(absolute(".", cwd));
            let repo = a.switches.iter().any(|s| s == "--repo");
            match command {
                "config show" => {
                    a.positionals(0, "this command takes no arguments")?;
                    Op::ConfigShow { cwd }.with(a)
                }
                "config check" => {
                    a.positionals(0, "this command takes no arguments")?;
                    Op::ConfigCheck { cwd }.with(a)
                }
                "config set" => {
                    let mut p = a
                        .positionals(2, "config set takes <key> <value>")?
                        .into_iter();
                    let (key, value) = (p.next().expect("2"), p.next().expect("2"));
                    Op::ConfigSet {
                        key,
                        value,
                        repo,
                        cwd,
                    }
                    .with(a)
                }
                _ => {
                    let key = a.positionals(1, "config unset takes <key>")?.remove(0);
                    Op::ConfigUnset { key, repo, cwd }.with(a)
                }
            }
        }
        "ontology set" => {
            let a = args()?;
            let what = "use ontology set type <type> --columns <a,b,...> or --reset, or ontology set field <type> <field> --role data|content, --values a,b,c or --clear-values";
            let p = a.positional.clone();
            let reset = a.switches.iter().any(|s| s == "--reset");
            let clear_values = a.switches.iter().any(|s| s == "--clear-values");
            let values = a.one("--values");
            if clear_values && values.is_some() {
                return Err(Usage {
                    command,
                    what: "give --values or --clear-values, not both",
                });
            }
            let op = match p.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
                // F96: a type's default node list columns.
                ["type", name]
                    if a.one("--role").is_none() && values.is_none() && !clear_values =>
                {
                    let columns = match (a.columns(), reset) {
                        (Some(c), false) if !c.is_empty() => Some(c),
                        (None, true) => None,
                        (Some(_), true) => {
                            return Err(Usage {
                                command,
                                what: "give --columns or --reset, not both",
                            });
                        }
                        _ => return Err(Usage { command, what }),
                    };
                    Op::OntologySetType {
                        name: (*name).to_owned(),
                        columns,
                    }
                }
                // D56: a field's role.
                ["field", t, f]
                    if a.one("--columns").is_none()
                        && !reset
                        && (a.one("--role").is_some() || values.is_some() || clear_values) =>
                {
                    Op::OntologySetField {
                        type_name: (*t).to_owned(),
                        field: (*f).to_owned(),
                        role: a.one("--role"),
                        values: if clear_values {
                            Some(Vec::new())
                        } else {
                            values.map(|s| s.split(',').map(str::to_owned).collect())
                        },
                    }
                }
                _ => return Err(Usage { command, what }),
            };
            op.with(a)
        }
        "ui focus" | "ui highlight" | "ui filter" | "ui fit" | "ui clear" => {
            let a = args()?;
            let note = a.one("--note");
            let command = match command {
                "ui focus" => {
                    let node = a.positionals(1, "name one node to focus on")?.remove(0);
                    let hops = match a.one("--hops") {
                        None => 1,
                        Some(h) => h.parse().map_err(|_| Usage {
                            command,
                            what: "--hops is a whole number from 1 to 3",
                        })?,
                    };
                    UiCmd::Focus { node, hops, note }
                }
                "ui highlight" => UiCmd::Highlight {
                    nodes: a.positional.clone(),
                    add: a.switches.iter().any(|s| s == "--add"),
                    note,
                },
                "ui filter" => {
                    a.positionals(
                        0,
                        "give names with --show or --hide <relation|type|status>:<name>",
                    )?;
                    UiCmd::Filter {
                        show: a.all("--show"),
                        hide: a.all("--hide"),
                        note,
                    }
                }
                "ui fit" => UiCmd::Fit {
                    nodes: a.positional.clone(),
                    note,
                },
                _ => {
                    a.positionals(0, "ui clear takes no nodes")?;
                    UiCmd::Clear { note }
                }
            };
            Op::UiCommand { command }.with(a)
        }
        // ws009 M1b1: the graphs to open are the command's own list, not the request's graph; the daemon checks each.
        "ui open" => {
            let a = args()?;
            a.positionals(0, "name the graphs with --graph a,b")?;
            let graphs = a
                .need("--graph", "name the graphs with --graph a,b")?
                .split(',')
                .map(str::to_owned)
                .collect();
            let op = Op::UiCommand {
                command: UiCmd::Open {
                    left_out: 0,
                    graphs,
                    only: a.switches.iter().any(|s| s == "--only"),
                    note: a.one("--note"),
                    raise: a.switches.iter().any(|s| s == "--raise"),
                },
            };
            Parsed {
                graph: None,
                ..op.with(a)
            }
        }
        "ui selection" => {
            let a = args()?;
            a.positionals(0, "this command takes no arguments")?;
            Op::UiSelection.with(a)
        }
        "ui wait" | "ui select" => {
            return Err(Usage {
                command,
                what: "only the graph view sends this",
            });
        }
        "root add" | "root list" | "root remove" => {
            let a = args()?;
            if a.one("--graph").is_some() || a.one("--by").is_some() {
                return Err(Usage {
                    command,
                    what: "root commands take no --graph or --by",
                });
            }
            match command {
                "root add" => {
                    let mut p = a
                        .positionals(2, "root add takes <name> <folder>")?
                        .into_iter();
                    let (name, folder) = (p.next().expect("2"), p.next().expect("2"));
                    Op::RootAdd {
                        name,
                        folder: absolute(&folder, cwd),
                    }
                    .with(a)
                }
                "root list" => {
                    a.positionals(0, "this command takes no arguments")?;
                    Op::RootList.with(a)
                }
                _ => {
                    let name = a.positionals(1, "root remove takes <name>")?.remove(0);
                    Op::RootRemove { name }.with(a)
                }
            }
        }
        // F76: `ontology remove type|status|relation <name>`, `ontology remove field <type> <name>`.
        "ontology remove" => {
            let a = args()?;
            let p = a.positional.clone();
            let words: Vec<&str> = p.iter().map(String::as_str).collect();
            let op = match (words.as_slice(), a.one("--relation")) {
                (["field", field], Some(relation)) => Op::OntologyRemoveRelationField {
                    relation,
                    field: (*field).to_owned(),
                },
                (_, Some(_)) => {
                    return Err(Usage {
                        command,
                        what: "--relation takes ontology remove field --relation <relation> <name>",
                    });
                }
                (["type", name], None) => Op::OntologyRemoveType {
                    name: (*name).to_owned(),
                },
                (["status", name], None) => Op::OntologyRemoveStatus {
                    name: (*name).to_owned(),
                },
                (["relation", name], None) => Op::OntologyRemoveRelation {
                    name: (*name).to_owned(),
                },
                (["field", kind, field], None) => Op::OntologyRemoveField {
                    type_name: (*kind).to_owned(),
                    field: (*field).to_owned(),
                },
                _ => {
                    return Err(Usage {
                        command,
                        what: "use ontology remove type|status|relation <name>, or ontology remove field <type> <name>, or ontology remove field --relation <relation> <name>",
                    });
                }
            };
            op.with(a)
        }
        _ => {
            let a = args()?;
            let p = a.positional.clone();
            let description = a.one("--description");
            let all_types = a.switches.iter().any(|s| s == "--all-types");
            let values = a.one("--values");
            if values.is_some()
                && (p.first().map(String::as_str) != Some("field") || a.one("--relation").is_some())
            {
                return Err(Usage {
                    command,
                    what: "--values is for ontology add field <type> <field>:string only",
                });
            }
            if all_types && p.first().map(String::as_str) != Some("field") {
                return Err(Usage {
                    command,
                    what: "--all-types is for ontology add field only",
                });
            }
            // F92: `field --relation <relation> <name>:<kind>`, a field on the relation's edges.
            if let Some(relation) = a.one("--relation") {
                return match p.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
                    _ if a.one("--description").is_some() => Err(Usage {
                        command,
                        what: "--description is not taken with --relation yet (describe the relation with umx graph set <alias> --description relation:<name>=<text>)",
                    }),
                    ["field", f] if !all_types => Ok(Op::OntologyAddRelationField {
                        relation,
                        field: (*f).to_owned(),
                    }
                    .with(a)),
                    _ => Err(Usage {
                        command,
                        what: "--relation takes ontology add field --relation <relation> <name>:<kind> (no types, no --all-types)",
                    }),
                };
            }
            let op = match p.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
                ["type", name] => Op::OntologyAddType {
                    name: (*name).to_owned(),
                    fields: a.all("--field"),
                    description,
                },
                ["status", name] => Op::OntologyAddStatus {
                    name: (*name).to_owned(),
                    description,
                },
                // ws008 t2b (req-117): `tag <facet>:<value>` or `tag <facet>:*`.
                ["tag", _] if description.is_some() => {
                    return Err(Usage {
                        command,
                        what: "ontology add tag takes no --description",
                    });
                }
                ["tag", tag] => Op::OntologyAddTag {
                    tag: (*tag).to_owned(),
                },
                ["relation", name] => Op::OntologyAddRelation {
                    name: (*name).to_owned(),
                    symmetric: a.switches.iter().any(|s| s == "--symmetric"),
                    description,
                    from: a.all("--from"),
                    to: a.all("--to"),
                },
                // F71: `field <type>[,<type>...] [<type>...] <name>:<kind>`, or `field --all-types <name>:<kind>`.
                ["field", rest @ .., f] if !rest.is_empty() != all_types => {
                    let mut types: Vec<String> = Vec::new();
                    for t in rest.iter().flat_map(|t| t.split(',')) {
                        if t.is_empty() {
                            return Err(Usage {
                                command,
                                what: "a type list has an empty name (write risk,issue with no spaces or empty items)",
                            });
                        }
                        if !types.iter().any(|seen| seen == t) {
                            types.push(t.to_owned());
                        }
                    }
                    Op::OntologyAddField {
                        types,
                        field: (*f).to_owned(),
                        all_types,
                        description,
                        role: a.one("--role"),
                        values: values
                            .map_or_else(Vec::new, |s| s.split(',').map(str::to_owned).collect()),
                    }
                }
                _ => {
                    return Err(Usage {
                        command,
                        what: "use ontology add type|status|relation <name>, or ontology add tag <facet>:<value>|<facet>:*, or ontology add field <type>[,<type>...] <name>:<kind>, or ontology add field --all-types <name>:<kind>, or ontology add field --relation <relation> <name>:<kind>",
                    });
                }
            };
            op.with(a)
        }
    };
    Ok(op)
}

trait With {
    fn with(self, a: Args) -> Parsed;
}

impl With for Op {
    fn with(self, a: Args) -> Parsed {
        let evidence = matches!(self, Op::NodeEvidence { .. });
        Parsed {
            op: self,
            graph: a.one("--graph"),
            scope: a.one("--scope"),
            all: !evidence && a.switches.iter().any(|s| s == "--all"),
            include_named: a.switches.iter().any(|s| s == "--include-named"),
            include_provenance: a.switches.iter().any(|s| s == "--provenance"),
            by: a.one("--by"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reviewer N1 (ws004 s28): every `related_skipped` fix umx-services builds parses as printed. The list is the
    /// golden file umx-services' `every_reason_s_fix_is_a_whole_command_as_the_golden_file_lists_it` pins.
    #[test]
    fn every_related_skipped_fix_parses() {
        let golden = include_str!("../../umx-services/tests/fixtures/related/fixes.txt");
        for line in golden.lines().filter(|l| !l.is_empty()) {
            let words: Vec<&str> = line
                .strip_prefix("umx ")
                .expect("a umx command")
                .split(' ')
                .collect();
            let ok = match words.as_slice() {
                ["batch", rest @ ..] => batch(rest).is_ok(),
                ["docs", "get", topic] => crate::docs::get(topic).errors.is_empty(),
                _ => parse(&words, Path::new("/work")).is_ok(),
            };
            assert!(ok, "does not parse: {line}");
        }
    }

    /// req-123: --related and --no-related set node add's and batch's flag, never both; graph set reads --related
    /// as a setting with a value.
    #[test]
    fn related_is_a_switch_on_writes_and_a_setting_on_graph_set() {
        let add = |extra: &[&'static str]| {
            let mut words = vec!["node", "add", "--type", "problem", "--title", "t"];
            words.extend_from_slice(extra);
            parse(&words, Path::new("/work")).map(|p| p.op)
        };
        let flag = |op: Result<Op, Usage>| match op.ok() {
            Some(Op::NodeAdd { related, .. }) => related,
            _ => panic!("node add parses"),
        };
        assert_eq!(flag(add(&[])), None);
        assert_eq!(flag(add(&["--related"])), Some(true));
        assert_eq!(flag(add(&["--no-related"])), Some(false));
        let Err(both) = add(&["--related", "--no-related"]) else {
            panic!("refused")
        };
        assert_eq!(both.what, "give --related or --no-related, not both");
        let Op::Batch { related, .. } = batch(&["--file", "-", "--no-related"]).ok().unwrap().2.op
        else {
            panic!("batch parses")
        };
        assert_eq!(related, Some(false));
        let set = parse(
            &["graph", "set", "g", "--related", "off"],
            Path::new("/work"),
        )
        .ok()
        .expect("graph set parses");
        let Op::GraphSet { settings, .. } = set.op else {
            panic!("graph set")
        };
        assert_eq!(
            settings
                .iter()
                .map(|s| (s.name.as_str(), s.value.as_str()))
                .collect::<Vec<_>>(),
            [("related", "off")]
        );
    }

    #[test]
    fn ui_open_takes_its_graph_list_as_the_command_and_not_as_the_request_graph() {
        let parsed = parse(
            &[
                "ui",
                "open",
                "--graph",
                "alpha,beta",
                "--only",
                "--note",
                "why",
                "--raise",
            ],
            Path::new("/work"),
        )
        .ok()
        .expect("ui open parses");
        assert_eq!(parsed.graph, None);
        assert_eq!(
            parsed.op,
            Op::UiCommand {
                command: UiCmd::Open {
                    left_out: 0,
                    graphs: vec!["alpha".into(), "beta".into()],
                    only: true,
                    note: Some("why".into()),
                    raise: true,
                },
            }
        );
        let plain = parse(&["ui", "open", "--graph", "alpha"], Path::new("/work"))
            .ok()
            .expect("one graph parses");
        assert!(matches!(
            plain.op,
            Op::UiCommand {
                command: UiCmd::Open {
                    only: false,
                    raise: false,
                    ..
                }
            }
        ));
        for words in [
            vec!["ui", "open"],
            vec!["ui", "open", "alpha"],
            vec!["ui", "open", "--graph", "alpha", "--scope", "s"],
            vec!["ui", "clear", "--graph", "alpha,beta"],
            vec!["ui", "clear", "--only"],
        ] {
            assert!(parse(&words, Path::new("/work")).is_err(), "{words:?}");
        }
    }

    #[test]
    fn provenance_is_a_per_read_switch_and_never_a_write_flag() {
        for words in [
            vec!["node", "list"],
            vec!["node", "get", "abc"],
            vec!["node", "find", "Session"],
            vec!["node", "history", "abc"],
            vec!["edge", "list"],
            vec!["graph", "list"],
            vec!["graph", "guide", "g"],
            vec!["resolve", "abc"],
            vec!["search", "Session"],
            vec!["query", "MATCH (n) RETURN n"],
            vec!["walk", "abc"],
            vec!["neighbours", "abc"],
            vec!["path", "abc", "def"],
        ] {
            let plain = parse(&words, Path::new("/work")).ok().expect("read parses");
            assert!(plain.op.is_read());
            assert!(!plain.include_provenance);
            let mut shown = words.clone();
            shown.push("--provenance");
            assert!(
                parse(&shown, Path::new("/work"))
                    .ok()
                    .expect("read opt-in parses")
                    .include_provenance
            );
            for value in ["false", "true", "", "other"] {
                let inline = format!("--provenance={value}");
                let mut invalid = words.clone();
                invalid.push(&inline);
                assert!(parse(&invalid, Path::new("/work")).is_err(), "{invalid:?}");
            }
        }
        for words in [
            vec!["node", "add", "--type", "problem", "--title", "New"],
            vec!["edge", "delete", "abc"],
            vec!["node", "edit", "abc", "--title", "Edit"],
            vec!["batch", "--file", "-"],
            vec!["graph", "set", "g", "--title", "Title"],
            vec!["ontology", "show"],
            vec!["embeddings", "status"],
            vec!["check"],
        ] {
            let mut words = words;
            words.push("--provenance");
            assert!(parse(&words, Path::new("/work")).is_err());
        }
    }

    fn p(line: &str) -> Result<Parsed, Usage> {
        let words: Vec<&str> = line.split(' ').collect();
        parse(&words, Path::new("/work"))
    }

    /// R289-2: only a scan naming no folder covers the whole repo; a named folder (even `.`) narrows it.
    #[test]
    fn a_bare_graph_scan_asks_for_the_whole_repo() {
        let scope = |line: &str| match p(line).ok().unwrap().op {
            Op::GraphScan {
                path, whole_repo, ..
            } => (path, whole_repo),
            _ => panic!("{line}"),
        };
        assert_eq!(scope("graph scan"), ("/work".to_string(), true));
        assert_eq!(scope("graph scan --add"), ("/work".to_string(), true));
        assert_eq!(scope("graph scan ."), ("/work".to_string(), false));
        assert_eq!(scope("graph scan docs"), ("/work/docs".to_string(), false));
    }

    /// F194: one call may ask for every row a view needs (ws004 s34a), so 250000 passes and the next number does not.
    #[test]
    fn similarity_pairs_limit_runs_to_250000() {
        let limit = |n: &str| p(&format!("similarity pairs --graph a,b --limit {n}")).map(|x| x.op);
        assert!(matches!(
            limit("250000"),
            Ok(Op::SimilarityPairs {
                limit: Some(250_000),
                ..
            })
        ));
        for bad in ["0", "250001"] {
            let u = limit(bad).expect_err(bad);
            assert_eq!(
                u.what, "--limit takes a whole number from 1 to 250000",
                "{bad}"
            );
        }
    }

    /// ws004 s34b: `--rows ids` asks for lean rows; `full` is the default and the only other word.
    #[test]
    fn similarity_pairs_rows_takes_full_or_ids() {
        let rows = |flag: &str| {
            p(format!("similarity pairs --graph a,b {flag}").trim()).map(|x| match x.op {
                Op::SimilarityPairs { rows, .. } => rows,
                other => panic!("{other:?}"),
            })
        };
        assert_eq!(rows("").ok(), Some(PairRows::Full));
        assert_eq!(rows("--rows full").ok(), Some(PairRows::Full));
        assert_eq!(rows("--rows ids").ok(), Some(PairRows::Ids));
        let u = rows("--rows titles").expect_err("titles");
        assert_eq!(u.what, "--rows takes full or ids");
    }

    #[test]
    fn batch_idempotency_key_takes_the_rule_and_names_it_never_the_value() {
        let (_, _, parsed) = batch(&["--file", "-", "--idempotency-key", "run:2026-11-17.w_8-a"])
            .ok()
            .unwrap();
        assert!(matches!(
            &parsed.op,
            Op::Batch { idempotency_key: Some(k), .. } if k == "run:2026-11-17.w_8-a"
        ));
        let long = "k".repeat(129);
        for bad in ["", "a b", "secret/value", "é", long.as_str()] {
            let usage = batch(&["--file", "-", "--idempotency-key", bad])
                .err()
                .unwrap();
            assert_eq!(usage.command, "batch");
            assert_eq!(usage.what, umx_protocol::IDEMPOTENCY_KEY_RULE, "{bad:?}");
            assert!(!usage.what.contains("secret"));
        }
        let (_, _, parsed) = batch(&["--file", "-"]).ok().unwrap();
        assert!(matches!(
            parsed.op,
            Op::Batch {
                idempotency_key: None,
                ..
            }
        ));
    }

    #[test]
    fn batch_echo_sentences_is_a_plain_switch() {
        let (_, _, parsed) = batch(&["--file", "-", "--echo-sentences"]).ok().unwrap();
        assert!(matches!(
            parsed.op,
            Op::Batch {
                echo_sentences: true,
                ..
            }
        ));
        for flag in [
            "--echo-sentences=false",
            "--echo-sentences=true",
            "--echo-sentences=",
        ] {
            let usage = batch(&["--file", "-", flag]).err().unwrap();
            assert_eq!(usage.command, "batch");
            assert_eq!(usage.what, "--echo-sentences takes no value", "{flag}");
        }
        assert!(batch(&["--file", "-", "--echo-sentences", "false"]).is_err());
    }

    #[test]
    fn batch_refs_from_repeats_in_order_and_never_reaches_the_wire_as_a_path() {
        let (file, receipts, parsed) = batch(&[
            "--refs-from",
            "one.json",
            "--file",
            "ops.jsonl",
            "--refs-from=two.json",
        ])
        .ok()
        .unwrap();
        assert_eq!(file, "ops.jsonl");
        assert_eq!(receipts, ["one.json", "two.json"]);
        assert!(matches!(
            &parsed.op,
            Op::Batch { refs, refs_graph: None, .. } if refs.is_empty()
        ));
        let (_, receipts, _) = batch(&["--file", "-"]).ok().unwrap();
        assert!(receipts.is_empty());
        assert!(batch(&["--file", "-", "--refs-from"]).is_err());
    }

    #[test]
    fn all_graph_flags_are_explicit_search_and_query_selectors() {
        for line in ["search words --all", "query MATCH --all --include-named"] {
            let parsed = p(line).ok().unwrap();
            assert!(parsed.all);
            assert_eq!(parsed.include_named, line.contains("--include-named"));
        }
        for line in [
            "search words --include-named",
            "query MATCH --include-named",
            "search words --all --graph a",
            "query MATCH --all --graph a,b",
            "search words --all --scope both",
            "query MATCH --all --scope both",
            "walk n --all",
            "node list --all",
            "scope save both --all",
            "search words --all=false",
            "query MATCH --all --include-named=false",
        ] {
            assert!(p(line).is_err(), "{line}");
        }
    }

    #[test]
    fn scope_flags_never_narrow_or_broaden_an_unsupported_command_silently() {
        assert_eq!(
            p("node list --scope both").ok().unwrap().scope.as_deref(),
            Some("both")
        );
        for line in [
            "node list --scope both --graph a",
            "walk n --scope both --graph a",
            "node add --type problem --title n --scope both",
            "graph set a --graph a,b --title changed",
            "config set roots / --graph a,b",
        ] {
            assert!(p(line).is_err(), "{line}");
        }
        for command in ["walk n", "neighbours n", "path n m"] {
            assert_eq!(
                p(&format!("{command} --scope both"))
                    .ok()
                    .unwrap()
                    .scope
                    .as_deref(),
                Some("both")
            );
            assert_eq!(
                p(&format!("{command} --graph a,b"))
                    .ok()
                    .unwrap()
                    .graph
                    .as_deref(),
                Some("a,b")
            );
        }
    }

    #[test]
    fn verbs_parse_to_typed_operations() {
        let n = p("node add --type risk --title Slip --set likelihood=3 --set-json tags=[\"a\"] --graph demo --by me").ok().unwrap();
        assert_eq!(n.graph.as_deref(), Some("demo"));
        assert_eq!(n.by.as_deref(), Some("me"));
        let Op::NodeAdd { kind, sets, .. } = n.op else {
            panic!()
        };
        assert_eq!(kind, "risk");
        assert_eq!(sets.len(), 2);
        assert_eq!(sets[1].value, SetValue::Json(umx_protocol::json!(["a"])));
        let Op::NodeAdd {
            evidence,
            evidence_kind,
            ..
        } = p("node add --type t --title x --evidence a.md#L1 --evidence b.md#L2 --evidence-kind room")
            .ok()
            .unwrap()
            .op
        else {
            panic!()
        };
        assert_eq!(
            (evidence.len(), evidence_kind.as_deref()),
            (2, Some("room"))
        );
        let Op::GraphNew { path, .. } = p("graph new ../g/./h --alias x").ok().unwrap().op else {
            panic!()
        };
        assert_eq!(path, "/g/h");
        let Op::NodeFind { text, .. } = p("node find approval delays").ok().unwrap().op else {
            panic!()
        };
        assert_eq!(text, "approval delays");
        let Op::OntologyAddField {
            types,
            field,
            all_types,
            description,
            role,
            ..
        } = p("ontology add field risk impact:int --description=How_bad")
            .ok()
            .unwrap()
            .op
        else {
            panic!()
        };
        assert_eq!(
            (types, field.as_str(), all_types, description.as_deref()),
            (
                vec!["risk".to_owned()],
                "impact:int",
                false,
                Some("How_bad")
            )
        );
        assert_eq!(role, None);
        let Op::OntologyAddField { role, .. } = p("ontology add field doc file:string --role data")
            .ok()
            .unwrap()
            .op
        else {
            panic!()
        };
        assert_eq!(role.as_deref(), Some("data"));
        let Op::OntologySetField {
            type_name,
            field,
            role,
            ..
        } = p("ontology set field doc file --role content")
            .ok()
            .unwrap()
            .op
        else {
            panic!()
        };
        assert_eq!(
            (type_name.as_str(), field.as_str(), role.as_deref()),
            ("doc", "file", Some("content"))
        );
        assert!(
            p("ontology set field doc file").is_err(),
            "--role is required"
        );
        assert!(matches!(
            p("edge add a blocks b").ok().unwrap().op,
            Op::EdgeAdd { .. }
        ));
        assert!(matches!(
            p("node list --limit=5").ok().unwrap().op,
            Op::NodeList { limit: Some(5), .. }
        ));
    }

    #[test]
    fn ontology_add_field_takes_a_type_list_or_all_types() {
        let field = |line: &str| match p(line).ok().map(|x| x.op) {
            Some(Op::OntologyAddField {
                types,
                field,
                all_types,
                ..
            }) => Some((types.join("|"), field, all_types)),
            _ => None,
        };
        let some = |t: &str, all| Some((t.to_owned(), "impact:int".to_owned(), all));
        assert_eq!(
            field("ontology add field risk,issue,risk task impact:int"),
            some("risk|issue|task", false)
        );
        assert_eq!(
            field("ontology add field --all-types impact:int"),
            some("", true)
        );
        for bad in [
            "ontology add field --all-types risk impact:int",
            "ontology add field impact:int",
            "ontology add field risk,,issue impact:int",
            "ontology add type risk --all-types",
        ] {
            assert!(p(bad).is_err(), "{bad}");
        }
    }

    /// F137: `--columns all` alone is passed through for node get, node list and edge list; beside other names it is
    /// refused as usage, never sent as a column named `all`.
    #[test]
    fn columns_all_stands_alone() {
        for line in [
            "node list --columns all",
            "node get 01JC8XA4H7 --columns all",
            "edge list --columns all",
            "node list --columns all,",
        ] {
            assert!(p(line).is_ok(), "{line}");
        }
        for line in [
            "node list --columns all,owner",
            "node get 01JC8XA4H7 --columns title,all",
            "edge list --columns all,confidence",
        ] {
            let refused = p(line).err().map(|e| e.what);
            assert_eq!(
                refused,
                Some("--columns all takes no other names: give all alone, or name the columns"),
                "{line}"
            );
        }
    }

    /// F96: `ontology set type <t>` takes exactly one of `--columns a,b` or `--reset`.
    #[test]
    fn ontology_set_type_takes_columns_or_reset() {
        let set = |line: &str| match p(line).ok().map(|x| x.op) {
            Some(Op::OntologySetType { name, columns }) => Some((name, columns)),
            _ => None,
        };
        assert_eq!(
            set("ontology set type risk --columns title,likelihood"),
            Some((
                "risk".to_owned(),
                Some(vec!["title".to_owned(), "likelihood".to_owned()])
            ))
        );
        assert_eq!(
            set("ontology set type risk --reset"),
            Some(("risk".to_owned(), None))
        );
        for bad in [
            "ontology set type risk --columns title --reset",
            "ontology set type risk",
            "ontology set type risk --columns ,",
            "ontology set type --reset",
            "ontology set status open --reset",
            "ontology set type risk issue --reset",
        ] {
            assert!(p(bad).is_err(), "{bad}");
        }
        let both = p("ontology set type risk --columns title --reset")
            .err()
            .unwrap();
        assert_eq!(both.what, "give --columns or --reset, not both");
    }

    #[test]
    fn ontology_add_field_takes_a_relation() {
        // F92.
        let Some(Op::OntologyAddRelationField { relation, field }) =
            p("ontology add field --relation works_for starts_on:date")
                .ok()
                .map(|x| x.op)
        else {
            panic!()
        };
        assert_eq!(
            (relation.as_str(), field.as_str()),
            ("works_for", "starts_on:date")
        );
        for bad in [
            "ontology add field --relation works_for risk starts_on:date",
            "ontology add field --relation works_for --all-types starts_on:date",
            "ontology add field --relation works_for",
            "ontology add type risk --relation works_for",
            "ontology add field --relation works_for starts_on:date --description x",
        ] {
            assert!(p(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn ontology_remove_parses_each_form() {
        // F76.
        let op = |line: &str| p(line).ok().map(|x| x.op);
        assert_eq!(
            op("ontology remove type risk"),
            Some(Op::OntologyRemoveType {
                name: "risk".into()
            })
        );
        assert_eq!(
            op("ontology remove field risk impact"),
            Some(Op::OntologyRemoveField {
                type_name: "risk".into(),
                field: "impact".into(),
            })
        );
        assert_eq!(
            op("ontology remove field --relation works_for starts_on"),
            Some(Op::OntologyRemoveRelationField {
                relation: "works_for".into(),
                field: "starts_on".into(),
            })
        );
        for bad in [
            "ontology remove type",
            "ontology remove field risk",
            "ontology remove thing x",
            "ontology remove type risk --all-types",
            "ontology remove type risk --relation works_for",
            "ontology remove field --relation works_for",
        ] {
            assert!(p(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn s05b_verbs_parse() {
        let Op::NodeEdit {
            kind, sets, unsets, ..
        } = p("node edit 01JC8X --type risk --set a=1 --unset b --unset c")
            .ok()
            .unwrap()
            .op
        else {
            panic!()
        };
        assert_eq!(
            (kind.as_deref(), sets.len(), unsets.len()),
            (Some("risk"), 1, 2)
        );
        let Op::NodeNote {
            append, evidence, ..
        } = p("node note 01JC8X --append more --evidence a.md#L1 --evidence b.md#L2")
            .ok()
            .unwrap()
            .op
        else {
            panic!()
        };
        assert_eq!((append.as_str(), evidence.len()), ("more", 2));
        let Op::NodeStatus { status, label, .. } = p("node status 01JC8X agreed --label resolved")
            .ok()
            .unwrap()
            .op
        else {
            panic!()
        };
        assert_eq!(
            (status.as_str(), label.as_deref()),
            ("agreed", Some("resolved"))
        );
        assert!(matches!(
            p("node history 01JC8X --limit 5").ok().unwrap().op,
            Op::NodeHistory { limit: Some(5), .. }
        ));
        assert!(matches!(
            p("graph unload demo").ok().unwrap().op,
            Op::GraphUnload { .. }
        ));
        assert!(matches!(
            p("graph load demo").ok().unwrap().op,
            Op::GraphLoad { by_alias: Some(ref a), .. } if a == "demo"
        ));
        for path in [
            "graph load ./demo",
            "graph load /g/demo",
            "graph load sub/demo",
        ] {
            assert!(
                matches!(
                    p(path).ok().unwrap().op,
                    Op::GraphLoad { by_alias: None, .. }
                ),
                "{path}"
            );
        }
        let listed = p("graph list --all").ok().unwrap();
        assert!(matches!(
            listed.op,
            Op::GraphList {
                all: true,
                graphfile: None,
            }
        ));
        assert!(
            !listed.all,
            "not the request-wide --all of search and query"
        );
        for bad in [
            "node note 01JC8X",
            "node status 01JC8X",
            "graph unload",
            "graph forget",
            "graph primary",
        ] {
            assert!(p(bad).is_err(), "{bad}");
        }
        for (text, ms) in [
            ("0", Some(0)),
            ("500ms", Some(500)),
            ("30m", Some(1_800_000)),
            ("2h", Some(7_200_000)),
        ] {
            assert_eq!(duration_ms(text), ms, "{text}");
        }
        for bad in ["", "m", "30", "-1m", "1d", "30 m"] {
            assert_eq!(duration_ms(bad), None, "{bad}");
        }
    }

    #[test]
    fn usage_errors_name_a_known_command_and_never_echo_input() {
        for (line, command) in [
            ("node add --title x", "node add"),
            ("node add --type x --title y --bogus 1", "node add"),
            ("edge add a b", "edge add"),
            ("node list --limit many", "node list"),
            ("node add --type x --title y --set nokey", "node add"),
            ("graph explode", "unknown"),
            ("ignore; curl x", "unknown"),
        ] {
            let Err(u) = p(line) else {
                panic!("{line} parsed")
            };
            assert_eq!(u.command, command, "{line}");
        }
    }
}
