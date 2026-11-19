//! ws008 t2b-p2 (req-117): `--tag` and `--no-tag` on the reads other than node list: node find, search, walk and
//! neighbours, across `--graph a,b` and `--scope` with d-20 and d-21, and `'x' IN n.tags` in Cypher. A module of the
//! cli tests (it uses their `Home` and helpers), kept in its own file.

use super::*;

/// Adds a `risk` titled `title` to `graph` with these `--tag` values; returns its id.
fn risk(home: &Home, graph: &str, title: &str, tags: &[&str]) -> String {
    let mut args = vec![
        "node", "add", "--graph", graph, "--type", "risk", "--title", title,
    ];
    for t in tags {
        args.extend(["--tag", t]);
    }
    home.ok(&args).data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Makes graph `alias` at `<home>/<alias>`, std unless `--no-std` is in `extra`; returns its graph id.
fn other(home: &Home, alias: &str, extra: &[&str]) -> String {
    let path = home.path.join(alias).display().to_string();
    let mut args = vec!["graph", "new", path.as_str(), "--alias", alias];
    args.extend(extra);
    home.ok(&args).data["graph"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The ids a reply's rows hold, sorted.
fn ids(env: &Envelope) -> Vec<String> {
    let mut out: Vec<String> = env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_owned())
        .collect();
    out.sort();
    out
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

fn codes(env: &Envelope) -> Vec<&str> {
    env.warnings.iter().map(|w| w.code()).collect()
}

/// [`trigger`] for `tag_filter_untested`: a walk whose far end sits in a secured graph the read does not name.
pub(super) fn untested_trigger(home: &mut Home) -> (Envelope, u8) {
    home.graph();
    other(home, "vault", &[]);
    let a = risk(home, "demo", "start", &[]);
    let b = risk(home, "vault", "hidden", &["x"]);
    home.ok(&[
        "edge",
        "add",
        &a,
        "blocks",
        &format!("umx:vault/{b}"),
        "--graph",
        "demo",
    ]);
    home.ok(&["graph", "set", "vault", "--policy", "secured"]);
    home.umx(&["walk", &a, "--tag", "x"])
}

/// req-117: search narrows every leg to the tagged nodes before it ranks, so `--limit 1` gives the best tagged hit,
/// never an empty page; node find keeps its tagged matches before the page is cut. AND, OR, a facet and `--no-tag`
/// read as node list's.
#[test]
fn search_and_node_find_filter_by_tags_before_the_page_is_cut() {
    let mut home = Home::new("tag_reads_search");
    home.graph();
    let u = risk(&home, "demo", "approval delay", &[]);
    let t = risk(
        &home,
        "demo",
        "approval delay in the long signoff",
        &["a", "origin:x"],
    );
    let z = risk(&home, "demo", "approval delay at the other desk", &["b"]);
    let search = |extra: &[&str]| {
        let mut args = vec!["search", "approval", "delay"];
        args.extend(extra);
        home.ok(&args)
    };
    assert_eq!(
        ids(&search(&["--limit", "1"])),
        std::slice::from_ref(&u),
        "the untagged hit ranks first"
    );
    assert_eq!(
        ids(&search(&["--limit", "1", "--tag", "a"])),
        std::slice::from_ref(&t)
    );
    assert_eq!(
        ids(&search(&["--tag", "a,b"])),
        sorted(vec![t.clone(), z.clone()])
    );
    assert_eq!(
        ids(&search(&["--tag", "origin:*"])),
        std::slice::from_ref(&t)
    );
    assert_eq!(
        ids(&search(&["--tag", "a", "--tag", "b"])),
        Vec::<String>::new()
    );
    assert_eq!(
        ids(&search(&["--no-tag", "a"])),
        sorted(vec![u.clone(), z.clone()])
    );
    let find = |extra: &[&str]| {
        let mut args = vec!["node", "find", "approval"];
        args.extend(extra);
        home.ok(&args)
    };
    assert_eq!(ids(&find(&["--tag", "a"])), std::slice::from_ref(&t));
    assert_eq!(ids(&find(&["--limit", "1", "--tag", "b"])), [z]);
    assert_eq!(ids(&find(&["--no-tag", "a,b"])), [u]);
    // A tag filter that leaves every title match out is not a miss.
    let none = find(&["--tag", "nobody"]);
    assert_eq!(ids(&none), Vec::<String>::new());
    assert!(
        !codes(&none).contains(&"find_no_match"),
        "{}",
        none.to_json()
    );
}

/// req-117: walk filters the nodes it lists, like --within, and walks on through the rest; neighbours filters the
/// far ends.
#[test]
fn walk_lists_tagged_nodes_and_neighbours_filters_far_ends() {
    let mut home = Home::new("tag_reads_walk");
    home.graph();
    let a = risk(&home, "demo", "a", &[]);
    let b = risk(&home, "demo", "b", &[]);
    let c = risk(&home, "demo", "c", &["x"]);
    home.ok(&["edge", "add", &a, "blocks", &b]);
    home.ok(&["edge", "add", &b, "blocks", &c]);
    let walk = |extra: &[&str]| {
        let mut args = vec!["walk", a.as_str(), "--depth", "2"];
        args.extend(extra);
        home.ok(&args)
    };
    assert_eq!(
        ids(&walk(&["--tag", "x"])),
        std::slice::from_ref(&c),
        "b is walked through, not listed"
    );
    assert_eq!(ids(&walk(&["--no-tag", "x"])), std::slice::from_ref(&b));
    let near = |extra: &[&str]| {
        let mut args = vec!["neighbours", b.as_str()];
        args.extend(extra);
        home.ok(&args)
    };
    assert_eq!(ids(&near(&[])), sorted(vec![a.clone(), c.clone()]));
    assert_eq!(ids(&near(&["--tag", "x"])), [c]);
    assert_eq!(ids(&near(&["--no-tag", "x"])), [a]);
}

/// Std across graphs: a graph without std is skipped and counted once, however many rows it holds, in a list, a scope
/// or an unnamed read; read alone it is tag_needs_std.
#[test]
fn graphs_without_std_are_skipped_in_lists_scopes_and_unnamed_reads() {
    let mut home = Home::new("tag_reads_scope");
    home.graph();
    other(&home, "second", &[]);
    other(&home, "plain", &["--no-std"]);
    let a = risk(&home, "demo", "start", &[]);
    let b = risk(&home, "second", "far", &["x"]);
    let p = home.ok(&[
        "node",
        "add",
        "--graph",
        "plain",
        "--type",
        "risk",
        "--title",
        "plain one",
    ]);
    let p = p.data["node"]["id"].as_str().unwrap().to_owned();
    let q = home.ok(&[
        "node",
        "add",
        "--graph",
        "plain",
        "--type",
        "risk",
        "--title",
        "plain two",
    ]);
    let q = q.data["node"]["id"].as_str().unwrap().to_owned();
    for to in [
        format!("umx:second/{b}"),
        format!("umx:plain/{p}"),
        format!("umx:plain/{q}"),
    ] {
        home.ok(&["edge", "add", &a, "blocks", &to, "--graph", "demo"]);
    }
    let both = home.ok(&["walk", &a, "--tag", "x"]);
    assert_eq!(ids(&both), std::slice::from_ref(&b));
    assert_eq!(
        issue(&both, Code::TagFilterNotStd).message(),
        "The tag filter skipped 1 of the graphs read: they do not use std, so they have no std tags."
    );
    let near = home.ok(&["neighbours", &a, "--no-tag", "x"]);
    assert_eq!(ids(&near), Vec::<String>::new());
    issue(&near, Code::TagFilterNotStd);
    let found = home.ok(&["search", "far", "--graph", "demo,second", "--tag", "x"]);
    assert_eq!(ids(&found), std::slice::from_ref(&b));
    let skipped = home.ok(&["search", "plain", "--graph", "demo,plain", "--tag", "x"]);
    assert_eq!(ids(&skipped), Vec::<String>::new());
    issue(&skipped, Code::TagFilterNotStd);
    home.ok(&["scope", "save", "mixed", "--graph", "demo,plain"]);
    issue(
        &home.ok(&["walk", &a, "--scope", "mixed", "--tag", "x"]),
        Code::TagFilterNotStd,
    );
    for args in [
        vec!["search", "plain", "--graph", "plain", "--tag", "x"],
        vec!["node", "find", "plain", "--graph", "plain", "--tag", "x"],
        vec!["walk", &p, "--graph", "plain", "--no-tag", "x"],
        vec!["neighbours", &p, "--graph", "plain", "--tag", "x"],
    ] {
        let (env, exit) = home.umx(&args);
        assert_eq!(exit, 1, "{args:?}: {}", env.to_json());
        issue(&env, Code::TagNeedsStd);
    }
}

/// d-20: a far end in a secured graph the read does not name is never tested, under `--tag` or `--no-tag`: it is
/// left out and only counted, and the count names no graph. Naming the graph reads it, so its row is tested.
#[test]
fn a_far_end_in_a_graph_the_read_cannot_show_is_counted_never_tested() {
    let mut home = Home::new("tag_reads_hidden");
    home.graph();
    let second = other(&home, "second", &[]);
    let a = risk(&home, "demo", "start", &[]);
    let b = risk(&home, "second", "far", &["x"]);
    home.ok(&[
        "edge",
        "add",
        &a,
        "blocks",
        &format!("umx:second/{b}"),
        "--graph",
        "demo",
    ]);
    home.ok(&["graph", "set", "second", "--policy", "secured"]);
    for flag in ["--tag", "--no-tag"] {
        for verb in ["walk", "neighbours"] {
            let env = home.ok(&[verb, &a, flag, "x"]);
            assert_eq!(ids(&env), Vec::<String>::new(), "{verb} {flag}");
            assert_eq!(
                issue(&env, Code::TagFilterUntested).message(),
                "The tag filter left out 1 rows whose node this read cannot show; their tags were not read."
            );
            let json = env.to_json();
            assert!(
                !json.contains("second") && !json.contains(&second),
                "{json}"
            );
        }
    }
    let named = home.ok(&["walk", &a, "--graph", "demo,second", "--tag", "x"]);
    assert_eq!(ids(&named), [b]);
    assert!(!codes(&named).contains(&"tag_filter_untested"));
}

/// d-21: named copies of one graph are each read and filtered alike.
#[test]
fn named_copies_are_each_filtered() {
    let mut home = Home::new("tag_reads_copies");
    let path = home.graph();
    let t = risk(&home, "demo", "copied approval", &["x"]);
    risk(&home, "demo", "copied approval too", &[]);
    let copy = home.path.join("copy");
    fs::create_dir_all(&copy).unwrap();
    for file in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
        fs::copy(Path::new(&path).join(file), copy.join(file)).unwrap();
    }
    home.ok(&["graph", "load", &copy.display().to_string(), "--as", "copy"]);
    let env = home.ok(&["search", "copied", "--graph", "demo,copy", "--tag", "x"]);
    let rows = env.data["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{}", env.to_json());
    assert!(rows.iter().all(|r| r["id"] == t.as_str()));
}

/// req-117: a tag that is not snake_case on a read is refused with tag_invalid; its next repeats the verb, the start
/// or a placeholder for the words, the graph (a list's first std graph) and every tag, fixed, and runs as printed.
#[test]
fn a_bad_tag_on_a_read_is_refused_and_its_next_runs_as_printed() {
    let mut home = Home::new("tag_reads_invalid");
    home.graph();
    other(&home, "plain", &["--no-std"]);
    let a = risk(&home, "demo", "approval", &["origin:double_diamond"]);
    // A start in the graph without std, linked to the std one: an unnamed walk or neighbours from it reads both.
    let p = home.ok(&[
        "node", "add", "--graph", "plain", "--type", "risk", "--title", "start",
    ]);
    let p = p.data["node"]["id"].as_str().unwrap().to_owned();
    home.ok(&[
        "edge",
        "add",
        &p,
        "blocks",
        &format!("umx:demo/{a}"),
        "--graph",
        "plain",
    ]);
    let cases: Vec<(Vec<&str>, &str)> = vec![
        (vec!["walk", &p], ""),
        (vec!["neighbours", &p], ""),
        (vec!["search", "approval"], "<words>"),
        (
            vec!["node", "find", "approval", "--graph", "demo"],
            "<text>",
        ),
        (vec!["walk", &a], ""),
        (vec!["neighbours", &a], ""),
        (vec!["walk", &a, "--graph", "plain,demo"], ""),
        (
            vec!["search", "approval", "--graph", "plain,demo"],
            "<words>",
        ),
    ];
    for (mut args, placeholder) in cases {
        args.extend(["--tag", "b", "--tag", "origin:Double-Diamond"]);
        let (env, exit) = home.umx(&args);
        assert_eq!(exit, 1, "{args:?}: {}", env.to_json());
        let next = issue(&env, Code::TagInvalid).next().to_owned();
        assert!(
            next.contains("--tag b --tag origin:double_diamond"),
            "{args:?}: {next}"
        );
        if args.contains(&"plain,demo") {
            assert!(next.contains("--graph demo"), "{next}");
        }
        let next = match placeholder {
            "" => next,
            p => next.replace(p, "approval"),
        };
        let (ran, exit) = run_as_printed(&home, &next);
        assert_eq!(exit, 0, "{next}: {}", ran.to_json());
    }
    // A list read from a start without std has no single graph to repeat it on: the next is the docs.
    for verb in ["walk", "neighbours"] {
        let (env, exit) = home.umx(&[
            verb,
            &p,
            "--graph",
            "plain,demo",
            "--tag",
            "origin:Double-Diamond",
        ]);
        assert_eq!(exit, 1, "{verb}: {}", env.to_json());
        let next = issue(&env, Code::TagInvalid).next().to_owned();
        assert_eq!(next, "umx docs get ontology", "{verb}");
        let (ran, exit) = run_as_printed(&home, &next);
        assert_eq!(exit, 0, "{next}: {}", ran.to_json());
    }
}

/// req-117: `'x' IN n.tags` matches in a std graph, reading the stored form even when a hand edit left another; in a
/// graph without std `tags` is an ordinary custom field, read as written.
#[test]
fn cypher_in_reads_std_tags_in_stored_form() {
    let mut home = Home::new("tag_reads_cypher");
    let path = home.graph();
    other(&home, "plain", &["--no-std"]);
    let t = risk(&home, "demo", "tagged", &["needs_review"]);
    let h = risk(&home, "demo", "hand", &["swap_me"]);
    risk(&home, "demo", "bare", &[]);
    let query = "MATCH (n) WHERE 'needs_review' IN n.tags RETURN n.id AS id";
    let got = |env: &Envelope| ids(env);
    assert_eq!(
        got(&home.ok(&["query", query, "--graph", "demo"])),
        std::slice::from_ref(&t)
    );
    home.ok(&["graph", "unload", "demo"]);
    let file = Path::new(&path).join("nodes.jsonl");
    let text = fs::read_to_string(&file).unwrap();
    fs::write(&file, text.replace("[\"swap_me\"]", "[\"Needs_Review\"]")).unwrap();
    home.ok(&["graph", "load", &path]);
    assert_eq!(
        got(&home.ok(&["query", query, "--graph", "demo"])),
        sorted(vec![t, h])
    );
    for (title, tags) in [("raw", "needs_review"), ("cased", "Needs_Review")] {
        home.ok(&[
            "node",
            "add",
            "--graph",
            "plain",
            "--type",
            "risk",
            "--title",
            title,
            "--set-json",
            &format!("tags=[\"{tags}\"]"),
        ]);
    }
    let plain = home.ok(&["query", query, "--graph", "plain"]);
    assert_eq!(
        plain.data["rows"].as_array().unwrap().len(),
        1,
        "{}",
        plain.to_json()
    );
}

/// Cypher's NOT here is two-valued: `NOT 'x' IN n.tags` keeps a node with no tags (`n.tags` reads null) as well as
/// one whose tags leave `x` out; `n.tags IS NULL OR NOT 'x' IN n.tags` says the same plainly.
#[test]
fn cypher_not_in_tags_keeps_untagged_nodes() {
    let mut home = Home::new("tag_reads_not");
    home.graph();
    risk(&home, "demo", "ta", &["x"]);
    let tb = risk(&home, "demo", "tb", &["y"]);
    let tc = risk(&home, "demo", "tc", &[]);
    let both = sorted(vec![tb, tc]);
    // two-valued NOT keeps rows whose value is null; a three-valued executor would drop them, so this test flips on purpose then
    let not = "MATCH (n:risk) WHERE NOT 'x' IN n.tags RETURN n.id AS id";
    assert_eq!(ids(&home.ok(&["query", not])), both);
    let plain = "MATCH (n:risk) WHERE n.tags IS NULL OR NOT 'x' IN n.tags RETURN n.id AS id";
    assert_eq!(ids(&home.ok(&["query", plain])), both);
}

/// A walk or neighbours that names no graph, while several are loaded, may cover several: a start in a graph without
/// std is skipped and counted, not refused, and the std graphs it reaches are filtered.
#[test]
fn an_unnamed_read_from_a_graph_without_std_skips_that_graph() {
    let mut home = Home::new("tag_reads_unnamed_plain");
    home.graph();
    other(&home, "plain", &["--no-std"]);
    let p = home.ok(&[
        "node", "add", "--graph", "plain", "--type", "risk", "--title", "start",
    ]);
    let p = p.data["node"]["id"].as_str().unwrap().to_owned();
    let t = risk(&home, "demo", "tagged", &["x"]);
    let u = risk(&home, "demo", "untagged", &[]);
    for to in [&t, &u] {
        home.ok(&[
            "edge",
            "add",
            &p,
            "blocks",
            &format!("umx:demo/{to}"),
            "--graph",
            "plain",
        ]);
    }
    for verb in ["walk", "neighbours"] {
        let env = home.ok(&[verb, &p, "--tag", "x"]);
        assert_eq!(ids(&env), std::slice::from_ref(&t), "{verb}");
        assert_eq!(
            issue(&env, Code::TagFilterNotStd).message(),
            "The tag filter skipped 1 of the graphs read: they do not use std, so they have no std tags.",
            "{verb}"
        );
        let (env, exit) = home.umx(&[verb, &p, "--graph", "plain", "--tag", "x"]);
        assert_eq!(exit, 1, "{verb}: {}", env.to_json());
        issue(&env, Code::TagNeedsStd);
    }
}

/// d-21: named copies of one graph without std are one graph skipped, counted once.
#[test]
fn copies_of_a_graph_without_std_are_counted_once() {
    let mut home = Home::new("tag_reads_plain_copies");
    home.graph();
    other(&home, "plain", &["--no-std"]);
    home.ok(&[
        "node", "add", "--graph", "plain", "--type", "risk", "--title", "copied",
    ]);
    let from = home.path.join("plain");
    let copy = home.path.join("plaincopy");
    fs::create_dir_all(&copy).unwrap();
    for file in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
        fs::copy(from.join(file), copy.join(file)).unwrap();
    }
    home.ok(&[
        "graph",
        "load",
        &copy.display().to_string(),
        "--as",
        "plaincopy",
    ]);
    let env = home.ok(&[
        "search",
        "copied",
        "--graph",
        "demo,plain,plaincopy",
        "--tag",
        "x",
    ]);
    assert_eq!(
        issue(&env, Code::TagFilterNotStd).message(),
        "The tag filter skipped 1 of the graphs read: they do not use std, so they have no std tags."
    );
}

/// The tags narrow search's legs together with `--within` and `--along`: a tagged node outside the subtree or the
/// range, and an untagged one inside it, are both left out.
#[test]
fn search_tags_narrow_with_within_and_along() {
    let mut home = Home::new("tag_reads_narrow");
    home.graph();
    let root = risk(&home, "demo", "root", &[]);
    let child = |title: &str, tags: &[&str], on: &str| {
        let mut args = vec![
            "node",
            "add",
            "--type",
            "risk",
            "--title",
            title,
            "--parent",
            root.as_str(),
            "--set",
            on,
        ];
        for t in tags {
            args.extend(["--tag", t]);
        }
        home.ok(&args).data["node"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let inside = child("approval delay inside", &["a"], "starts_on=2026-12-10");
    child("approval delay untagged", &[], "starts_on=2026-12-11");
    let outside = home.ok(&[
        "node",
        "add",
        "--type",
        "risk",
        "--title",
        "approval delay outside",
        "--tag",
        "a",
        "--set",
        "starts_on=2025-01-01",
    ]);
    let outside = outside.data["node"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        ids(&home.ok(&["search", "approval", "delay", "--tag", "a"])),
        sorted(vec![inside.clone(), outside])
    );
    assert_eq!(
        ids(&home.ok(&[
            "search", "approval", "delay", "--within", &root, "--tag", "a",
        ])),
        std::slice::from_ref(&inside)
    );
    assert_eq!(
        ids(&home.ok(&[
            "search",
            "approval",
            "delay",
            "--along",
            "date_field:starts_on",
            "--after",
            "2026-12-01",
            "--before",
            "2026-12-31",
            "--tag",
            "a",
        ])),
        [inside]
    );
}

/// AND, OR and a facet on walk and neighbours, and several unreadable rows counted as one number.
#[test]
fn walk_and_neighbours_read_and_or_and_facets() {
    let mut home = Home::new("tag_reads_groups");
    home.graph();
    let hub = risk(&home, "demo", "hub", &[]);
    let ab = risk(&home, "demo", "ab", &["a", "b"]);
    let a = risk(&home, "demo", "a only", &["a", "origin:x"]);
    let b = risk(&home, "demo", "b only", &["b"]);
    for to in [&ab, &a, &b] {
        home.ok(&["edge", "add", &hub, "blocks", to]);
    }
    for verb in ["walk", "neighbours"] {
        let read = |extra: &[&str]| {
            let mut args = vec![verb, hub.as_str()];
            args.extend(extra);
            ids(&home.ok(&args))
        };
        assert_eq!(
            read(&["--tag", "a", "--tag", "b"]),
            std::slice::from_ref(&ab),
            "{verb} AND"
        );
        assert_eq!(
            read(&["--tag", "a,b"]),
            sorted(vec![ab.clone(), a.clone(), b.clone()]),
            "{verb} OR"
        );
        assert_eq!(
            read(&["--tag", "origin:*"]),
            std::slice::from_ref(&a),
            "{verb} facet"
        );
        assert_eq!(
            read(&["--tag", "b", "--no-tag", "a"]),
            std::slice::from_ref(&b),
            "{verb} not"
        );
    }
    other(&home, "vault", &[]);
    for title in ["one", "two"] {
        let hidden = risk(&home, "vault", title, &["a"]);
        home.ok(&[
            "edge",
            "add",
            &hub,
            "blocks",
            &format!("umx:vault/{hidden}"),
            "--graph",
            "demo",
        ]);
    }
    home.ok(&["graph", "set", "vault", "--policy", "secured"]);
    for verb in ["walk", "neighbours"] {
        let env = home.ok(&[verb, &hub, "--tag", "a"]);
        assert_eq!(ids(&env), sorted(vec![ab.clone(), a.clone()]), "{verb}");
        assert_eq!(
            issue(&env, Code::TagFilterUntested).message(),
            "The tag filter left out 2 rows whose node this read cannot show; their tags were not read.",
            "{verb}"
        );
    }
}

/// A fake AWS access key id, built at run time so no credential-shaped text is in the source.
fn token() -> String {
    format!("AKIA{}", "Q7Z3X9K2Q7Z3X9K2")
}

/// Security: `n.tags` reads a std graph's tags lowercased, so a key in a tag is flagged in a query row when scanned
/// upper cased too, as the tag write and node get do: one umx wrote and one a hand edit left upper case. A plain tag
/// is the control.
#[test]
fn a_secret_shaped_tag_is_flagged_in_a_query_row() {
    let mut home = Home::new("tag_reads_secret_query");
    let path = home.graph();
    let key = token();
    let written = risk(&home, "demo", "written", &[&key]);
    let hand = risk(&home, "demo", "hand", &["swap_me"]);
    let plain = risk(&home, "demo", "plain", &["plain"]);
    home.ok(&["graph", "unload", "demo"]);
    let file = Path::new(&path).join("nodes.jsonl");
    let text = fs::read_to_string(&file).unwrap();
    fs::write(&file, text.replace("\"swap_me\"", &format!("\"{key}\""))).unwrap();
    home.ok(&["graph", "load", &path]);
    let flags = |id: &str| {
        let q = format!("MATCH (n) WHERE n.id = '{id}' RETURN n.tags AS t");
        let env = home.ok(&["query", &q]);
        // JSON carries the value with its flag, as node get does; --human hides a flagged cell, in either case.
        let shown = home.human(&["query", &q]);
        assert!(
            !shown.contains(&key) && !shown.contains(&key.to_ascii_lowercase()),
            "{shown}"
        );
        env.data["rows"][0]["secret_suspected"].clone()
    };
    assert_eq!(flags(&written), json!(["field:t"]));
    assert_eq!(flags(&hand), json!(["field:t"]));
    assert!(flags(&plain).is_null());
}

/// Security: a success next that repeats the request's tags (search_results_cut) never echoes one that looks like a
/// secret: it gives `<tag>` instead.
#[test]
fn a_success_next_never_echoes_a_secret_shaped_tag() {
    let mut home = Home::new("tag_reads_secret_next");
    home.graph();
    let key = token();
    let lower = key.to_ascii_lowercase();
    for title in ["approval one", "approval two"] {
        risk(&home, "demo", title, &[&key]);
    }
    risk(&home, "demo", "approval plain", &["plain"]);
    // The key as stored (lowercased) is a tag in stored form, the one a next would otherwise repeat.
    let cut = home.ok(&["search", "approval", "--tag", &lower, "--limit", "1"]);
    let next = issue(&cut, Code::SearchResultsCut).next();
    assert!(next.contains("--tag <tag>"), "{next}");
    let json = cut.to_json();
    assert!(!json.contains(&lower) && !json.contains(&key), "{json}");
    let shown = home.ok(&["search", "approval", "--no-tag", "plain", "--limit", "1"]);
    assert!(
        issue(&shown, Code::SearchResultsCut)
            .next()
            .contains("--no-tag plain"),
        "{}",
        shown.to_json()
    );
}
