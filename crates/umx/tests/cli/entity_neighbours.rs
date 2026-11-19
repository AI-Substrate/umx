//! The cli tests: `umx neighbours` inherits the edges of the node's entity members (ws008 E3, req-127).
//! A module of cli.rs, whose `Home` and helpers it uses, in its own file so no file nears the seed gate's
//! size cap. Synthetic graphs only.

use super::*;

/// Adds a node and gives its address.
fn add(home: &Home, graph: &str, kind: &str, title: &str, extra: &[&str]) -> String {
    let mut args = vec![
        "node", "add", "--graph", graph, "--type", kind, "--title", title,
    ];
    args.extend_from_slice(extra);
    home.ok(&args).data["node"]["address"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Makes graph `alias` at `<home>/<alias>` (std, as `graph new` makes it); gives its graph id.
fn graph(home: &Home, alias: &str, extra: &[&str]) -> String {
    let path = home.path.join(alias).display().to_string();
    let mut args = vec!["graph", "new", path.as_str(), "--alias", alias];
    args.extend_from_slice(extra);
    home.ok(&args).data["graph"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn blocks(home: &Home, a: &str, b: &str, graph: &str) {
    home.ok(&["edge", "add", a, "blocks", b, "--graph", graph]);
}

fn id(address: &str) -> &str {
    address.rsplit('/').next().unwrap()
}

/// Each row as (far end id, match_status, via), in the reply's order; "-" for a missing field.
fn rows(env: &Envelope) -> Vec<(String, String, String)> {
    let s = |v: &Value| v.as_str().unwrap_or("-").to_owned();
    env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (s(&r["id"]), s(&r["match_status"]), s(&r["via"])))
        .collect()
}

fn row(far: &str, status: &str, via: &str) -> (String, String, String) {
    (id(far).to_owned(), status.to_owned(), via.to_owned())
}

/// The `entities_grouped` hint's count slots, read from its message: (grouped, through proposals).
#[track_caller]
fn grouped(env: &Envelope) -> (u64, u64) {
    let message = hint_of(env, Code::EntitiesGrouped).message().to_owned();
    let n: Vec<u64> = message
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|w| w.parse().ok())
        .collect();
    (n[0], n[1])
}

fn ungrouped(env: &Envelope) -> bool {
    !env.hints.iter().any(|h| h.code() == "entities_grouped")
}

/// The bytes of every graph file under `<home>/<alias>` for each alias.
fn stored(home: &Home, aliases: &[&str]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for alias in aliases {
        for file in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
            out.push(fs::read(home.path.join(alias).join(file)).unwrap_or_default());
        }
    }
    out
}

/// [`trigger`] for `entities_grouped`: two graphs share a key; neighbours on one member.
pub(super) fn grouped_trigger(home: &mut Home) -> (Envelope, u8) {
    home.start();
    graph(home, "g1", &[]);
    graph(home, "g2", &[]);
    let key = ["--set", "entity_key=email:jo@x.com"];
    let a = add(home, "g1", "person", "Jo", &key);
    let b = add(home, "g2", "person", "Jo", &key);
    let x = add(home, "g2", "problem", "Late", &[]);
    blocks(home, &b, &x, "g2");
    home.umx(&["neighbours", &a])
}

/// req-127 done_when 1 and 7: with two graphs sharing a key, neighbours on one member lists the other's edges with
/// via, graph and match_status confirmed, names the covered graphs, and ends with the hint, whose next runs as
/// printed and lists the members by graph. Nothing is stored.
#[test]
fn a_shared_key_inherits_the_other_members_edges_with_via_graph_and_status() {
    let mut home = Home::new("e3-key");
    home.start();
    graph(&home, "g1", &[]);
    let g2 = graph(&home, "g2", &[]);
    let key = ["--set", "entity_key=email:jo@x.com"];
    let a = add(&home, "g1", "person", "Jo", &key);
    let b = add(&home, "g2", "person", "Jo", &key);
    let x = add(&home, "g2", "problem", "Late", &[]);
    let y = add(&home, "g1", "problem", "Early", &[]);
    blocks(&home, &b, &x, "g2");
    blocks(&home, &a, &y, "g1");
    let before = stored(&home, &["g1", "g2"]);
    let env = home.ok(&["neighbours", &a]);
    assert_eq!(
        rows(&env),
        [row(&y, "own", "-"), row(&x, "confirmed", &b)],
        "{}",
        env.to_json()
    );
    let inherited = &env.data["rows"][1];
    assert_eq!(inherited["graph"], g2.as_str(), "{}", env.to_json());
    assert_eq!(inherited["from"], id(&b));
    assert_eq!(inherited["address"], x.as_str());
    assert_eq!(env.data["graphs"], json!(["g1", "g2"]), "{}", env.to_json());
    assert_eq!(grouped(&env), (2, 0), "{}", env.to_json());
    let next = hint_of(&env, Code::EntitiesGrouped).next().to_owned();
    assert_eq!(next, format!("umx entity show {a}"));
    let (shown, exit) = home.run_next(&next);
    assert_eq!(exit, 0, "{}", shown.to_json());
    assert_eq!(
        e2_members(&shown),
        [a.clone(), b.clone()],
        "{}",
        shown.to_json()
    );
    assert_eq!(shown.data["graphs"], json!(["g1", "g2"]));
    // From the other member, the first's edge comes back the same way.
    let back = home.ok(&["neighbours", &b]);
    assert_eq!(
        rows(&back),
        [row(&x, "own", "-"), row(&y, "confirmed", &a)],
        "{}",
        back.to_json()
    );
    assert_eq!(
        stored(&home, &["g1", "g2"]),
        before,
        "a read stores nothing"
    );
}

/// req-127 done_when 2: `entity link` between two unkeyed nodes groups them the same way. The link itself is the
/// node's own edge; an identity edge is never inherited.
#[test]
fn an_entity_link_between_unkeyed_nodes_inherits_the_same_way() {
    let mut home = Home::new("e3-link");
    let n = e2_people(&mut home, &["g1", "g2"], &[]);
    let x = add(&home, "g2", "problem", "Late", &[]);
    blocks(&home, &n[1], &x, "g2");
    home.ok(&["entity", "link", &n[0], &n[1]]);
    let env = home.ok(&["neighbours", &n[0]]);
    let got = rows(&env);
    assert_eq!(
        got,
        [row(&n[1], "own", "-"), row(&x, "confirmed", &n[1])],
        "{}",
        env.to_json()
    );
    assert_eq!(env.data["rows"][0]["relation"], "same_as");
    assert_eq!(env.data["graphs"], json!(["g1", "g2"]));
    assert_eq!(grouped(&env), (2, 0), "{}", env.to_json());
    // A member's proposal to an outsider: with proposals left out, its possibly_same_as edge is still never inherited.
    let m = add(&home, "g2", "person", "Jo M", &[]);
    home.ok(&[
        "edge",
        "add",
        &n[1],
        "possibly_same_as",
        &m,
        "--graph",
        "g2",
    ]);
    let confirmed = home.ok(&["neighbours", &n[0], "--entities", "confirmed"]);
    assert_eq!(
        rows(&confirmed),
        [row(&n[1], "own", "-"), row(&x, "confirmed", &n[1])],
        "{}",
        confirmed.to_json()
    );
}

/// req-127 done_when 3: a possibly_same_as partner's edges come back as proposed and count in the hint;
/// `--entities confirmed` hides them. A proposal never brings in its partner's own confirmed group.
#[test]
fn a_proposal_is_inherited_as_proposed_and_confirmed_hides_it() {
    let mut home = Home::new("e3-proposed");
    let key = ["--set", "entity_key=email:cy@x.com"];
    let n = e2_people(&mut home, &["g1", "g2", "g3"], &[&[], &key, &key]);
    let z = add(&home, "g2", "problem", "Proposed edge", &[]);
    let w = add(&home, "g3", "problem", "Partner group edge", &[]);
    blocks(&home, &n[1], &z, "g2");
    blocks(&home, &n[2], &w, "g3");
    home.ok(&[
        "edge",
        "add",
        &n[0],
        "possibly_same_as",
        &n[1],
        "--graph",
        "g1",
    ]);
    let env = home.ok(&["neighbours", &n[0]]);
    assert_eq!(
        rows(&env),
        [row(&n[1], "own", "-"), row(&z, "proposed", &n[1])],
        "{}",
        env.to_json()
    );
    assert_eq!(grouped(&env), (2, 1), "{}", env.to_json());
    let confirmed = home.ok(&["neighbours", &n[0], "--entities", "confirmed"]);
    assert_eq!(
        rows(&confirmed),
        [row(&n[1], "-", "-")],
        "{}",
        confirmed.to_json()
    );
    assert!(ungrouped(&confirmed), "{}", confirmed.to_json());
    assert_eq!(confirmed.data["graphs"], json!(["g1", "g2", "g3"]));
}

/// req-127 done_when 4: `--entities off` gives the rows of before E3 byte for byte: the same reply whether or not the
/// node has an entity, with no graphs, via, match_status or hint; an ungrouped node with entities on lists the same
/// rows too.
#[test]
fn entities_off_gives_todays_reply_byte_for_byte() {
    let mut home = Home::new("e3-off");
    home.start();
    graph(&home, "g1", &[]);
    graph(&home, "g2", &[]);
    let key = ["--set", "entity_key=email:jo@x.com"];
    let a = add(&home, "g1", "person", "Jo", &key);
    let b = add(&home, "g2", "person", "Jo", &key);
    let x = add(&home, "g2", "problem", "Late", &[]);
    let y = add(&home, "g1", "problem", "Early", &[]);
    blocks(&home, &b, &x, "g2");
    blocks(&home, &a, &y, "g1");
    let reply = |env: &Envelope| {
        format!(
            "{} {:?} {:?}",
            env.data,
            env.warnings.iter().map(Issue::code).collect::<Vec<_>>(),
            env.hints.iter().map(Issue::code).collect::<Vec<_>>()
        )
    };
    let off = home.ok(&["neighbours", &a, "--entities", "off"]);
    assert!(off.data.get("graphs").is_none(), "{}", off.to_json());
    assert_eq!(rows(&off), [row(&y, "-", "-")], "{}", off.to_json());
    assert!(ungrouped(&off));
    home.ok(&["entity", "key", &b, "remove", "email:jo@x.com"]);
    let alone = home.ok(&["neighbours", &a, "--entities", "off"]);
    assert_eq!(reply(&alone), reply(&off), "off is the same grouped or not");
    let mut on = home.ok(&["neighbours", &a]);
    assert_eq!(on.data["graphs"], json!(["g1", "g2"]));
    on.data.as_object_mut().unwrap().remove("graphs");
    assert_eq!(
        reply(&on),
        reply(&off),
        "an ungrouped reply keeps today's rows"
    );
    // similar_to is a ranking, never inherited: such a read never groups, even through the traversal (a list).
    home.ok(&["entity", "key", &b, "add", "email:jo@x.com"]);
    let (similar, exit) = home.umx(&[
        "neighbours",
        &a,
        "--relation",
        "similar_to",
        "--graph",
        "g1,g2",
    ]);
    assert_eq!(exit, 0, "{}", similar.to_json());
    assert!(similar.data["rows"].is_array(), "{}", similar.to_json());
    assert!(
        similar.data.get("graphs").is_none(),
        "{}",
        similar.to_json()
    );
    assert!(ungrouped(&similar), "{}", similar.to_json());
}

/// req-127 done_when 5 (d-20): with a secured graph and an open one, a read that names neither shows nothing from the
/// secured graph: no member, row, id or alias of it, not in `graphs` even when it bridges two open members, and no
/// count in the hint. Named, its member's edges are inherited.
#[test]
fn an_unnamed_secured_graph_gives_nothing_even_as_a_bridge() {
    let mut home = Home::new("e3-secured");
    home.secured_graph();
    graph(&home, "open", &[]);
    graph(&home, "open2", &[]);
    let key = ["--set", "entity_key=email:jo@x.com"];
    let o = add(&home, "open", "person", "Jo", &key);
    let s = add(&home, "demo", "person", "Jo", &key);
    let t = add(&home, "demo", "problem", "Hidden", &[]);
    blocks(&home, &s, &t, "demo");
    let unnamed = home.ok(&["neighbours", &o]);
    assert_eq!(
        rows(&unnamed),
        Vec::<(String, String, String)>::new(),
        "{}",
        unnamed.to_json()
    );
    assert_eq!(
        unnamed.data["graphs"],
        json!(["open", "open2"]),
        "{}",
        unnamed.to_json()
    );
    assert!(ungrouped(&unnamed));
    let text = unnamed.to_json();
    for hidden in [id(&s), id(&t), "demo"] {
        assert!(!text.contains(hidden), "{hidden} in {text}");
    }
    // A bridge: the open node is linked to the secured member (stored in open), and the secured member to open2's
    // node (stored in open2). The secured member joins the walk, so open2's node groups, but it is never shown.
    let p = add(&home, "open2", "person", "Jo P", &[]);
    let q = add(&home, "open2", "problem", "Bridged", &[]);
    blocks(&home, &p, &q, "open2");
    home.ok(&["entity", "link", &o, &s, "--graph", "open"]);
    home.ok(&["entity", "link", &p, &s, "--graph", "open2"]);
    let bridged = home.ok(&["neighbours", &o, "--relation", "blocks"]);
    assert_eq!(
        rows(&bridged),
        [row(&q, "confirmed", &p)],
        "{}",
        bridged.to_json()
    );
    assert_eq!(
        bridged.data["graphs"],
        json!(["open", "open2"]),
        "{}",
        bridged.to_json()
    );
    assert_eq!(grouped(&bridged), (2, 0), "{}", bridged.to_json());
    let text = bridged.to_json();
    for hidden in [id(&s), id(&t), "demo"] {
        assert!(!text.contains(hidden), "{hidden} in {text}");
    }
    // Control: named, the secured member's edge is inherited and its graph covered.
    let named = home.ok(&["neighbours", &o, "--graph", "open,demo"]);
    assert!(
        rows(&named).contains(&row(&t, "confirmed", &s)),
        "{}",
        named.to_json()
    );
    assert!(
        named.data["graphs"]
            .as_array()
            .unwrap()
            .contains(&json!("demo")),
        "{}",
        named.to_json()
    );
}

/// req-127 done_when 6 (d-21): two copies of one graph never group, named or not.
#[test]
fn two_copies_of_one_graph_do_not_group() {
    let mut home = Home::new("e3-copies");
    let key: &[&str] = &["--set", "entity_key=email:jo@x.com"];
    let n = e2_people(&mut home, &["g"], &[key]);
    let x = add(&home, "g", "problem", "Late", &[]);
    blocks(&home, &n[0], &x, "g");
    let to = home.path.join("g2");
    fs::create_dir_all(&to).unwrap();
    for file in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
        fs::copy(home.path.join("g").join(file), to.join(file)).unwrap();
    }
    home.ok(&["graph", "load", to.to_str().unwrap(), "--as", "g2"]);
    let env = home.ok(&["neighbours", &n[0]]);
    assert_eq!(rows(&env), [row(&x, "-", "-")], "{}", env.to_json());
    assert_eq!(env.data["graphs"], json!(["g"]), "{}", env.to_json());
    assert!(ungrouped(&env), "{}", env.to_json());
    // Both copies named: still one graph, no group.
    let named = home.ok(&["neighbours", &n[0], "--graph", "g,g2"]);
    assert_eq!(named.data["graphs"], json!(["g"]), "{}", named.to_json());
    assert!(ungrouped(&named), "{}", named.to_json());
    // Two copies of another graph and no primary: entity show refuses, so neighbours gives the node's own rows.
    let h = home.path.join("h");
    home.ok(&["graph", "new", h.to_str().unwrap(), "--alias", "h"]);
    for alias in ["h1", "h2"] {
        let to = home.path.join(alias);
        fs::create_dir_all(&to).unwrap();
        for file in ["graph.json", "nodes.jsonl", "edges.jsonl"] {
            fs::copy(h.join(file), to.join(file)).unwrap();
        }
        home.ok(&["graph", "load", to.to_str().unwrap(), "--as", alias]);
    }
    home.ok(&["graph", "unload", "h"]);
    let (soft, exit) = home.umx(&["neighbours", &n[0], "--graph", "g"]);
    assert_eq!(exit, 0, "{}", soft.to_json());
    assert_eq!(rows(&soft), [row(&x, "-", "-")], "{}", soft.to_json());
    assert!(soft.data.get("graphs").is_none(), "{}", soft.to_json());
}

/// `--tag` filters inherited rows' far ends as it does the node's own.
#[test]
fn a_tag_filter_tests_inherited_rows_too() {
    let mut home = Home::new("e3-tags");
    home.start();
    graph(&home, "g1", &[]);
    graph(&home, "g2", &[]);
    let key = ["--set", "entity_key=email:jo@x.com"];
    let a = add(&home, "g1", "person", "Jo", &key);
    let b = add(&home, "g2", "person", "Jo", &key);
    let tagged = ["--tag", "urgent"];
    let y = add(&home, "g1", "risk", "Own tagged", &tagged);
    let x1 = add(&home, "g2", "risk", "Inherited tagged", &tagged);
    let x2 = add(&home, "g2", "risk", "Inherited plain", &[]);
    blocks(&home, &a, &y, "g1");
    blocks(&home, &b, &x1, "g2");
    blocks(&home, &b, &x2, "g2");
    let env = home.ok(&["neighbours", &a, "--tag", "urgent"]);
    assert_eq!(
        rows(&env),
        [row(&y, "own", "-"), row(&x1, "confirmed", &b)],
        "{}",
        env.to_json()
    );
}

/// `--limit`/`--offset` page the own rows then the inherited ones; `total` counts both. A member's edge back into the
/// group is never inherited.
#[test]
fn paging_runs_over_own_then_inherited_rows_and_group_edges_stay_out() {
    let mut home = Home::new("e3-paging");
    home.start();
    for g in ["g1", "g2", "g3"] {
        graph(&home, g, &[]);
    }
    let key = ["--set", "entity_key=email:jo@x.com"];
    let a = add(&home, "g1", "person", "Jo", &key);
    let b = add(&home, "g2", "person", "Jo", &key);
    let c = add(&home, "g3", "person", "Jo", &key);
    let own: Vec<String> = (0..2)
        .map(|i| add(&home, "g1", "problem", &format!("Own {i}"), &[]))
        .collect();
    let far: Vec<String> = (0..2)
        .map(|i| add(&home, "g2", "problem", &format!("Far {i}"), &[]))
        .collect();
    for o in &own {
        blocks(&home, &a, o, "g1");
    }
    for f in &far {
        blocks(&home, &b, f, "g2");
    }
    // Member to member: listed for neither as inherited.
    blocks(&home, &b, &c, "g2");
    let all = home.ok(&["neighbours", &a]);
    let want = [
        row(&own[0], "own", "-"),
        row(&own[1], "own", "-"),
        row(&far[0], "confirmed", &b),
        row(&far[1], "confirmed", &b),
    ];
    assert_eq!(rows(&all), want, "{}", all.to_json());
    assert_eq!(grouped(&all), (3, 0));
    let first = home.ok(&["neighbours", &a, "--limit", "3"]);
    assert_eq!(rows(&first), want[..3], "{}", first.to_json());
    assert_eq!(first.data["total"], 4);
    assert_eq!(first.data["truncated"], true);
    let rest = home.ok(&["neighbours", &a, "--limit", "3", "--offset", "3"]);
    assert_eq!(rows(&rest), want[3..], "{}", rest.to_json());
}

/// `--scope` pins the covered graphs, and the hint's next keeps it and runs as printed.
#[test]
fn a_scope_pins_the_covered_graphs_and_the_next_keeps_it() {
    let mut home = Home::new("e3-scope");
    home.start();
    for g in ["g1", "g2", "g3"] {
        graph(&home, g, &[]);
    }
    let key = ["--set", "entity_key=email:jo@x.com"];
    let a = add(&home, "g1", "person", "Jo", &key);
    let b = add(&home, "g2", "person", "Jo", &key);
    let c = add(&home, "g3", "person", "Jo", &key);
    let x = add(&home, "g2", "problem", "In scope", &[]);
    let z = add(&home, "g3", "problem", "Out of scope", &[]);
    blocks(&home, &b, &x, "g2");
    blocks(&home, &c, &z, "g3");
    home.ok(&["scope", "save", "pair", "--graph", "g1,g2"]);
    let env = home.ok(&["neighbours", &a, "--scope", "pair"]);
    assert_eq!(rows(&env), [row(&x, "confirmed", &b)], "{}", env.to_json());
    assert_eq!(env.data["graphs"], json!(["g1", "g2"]), "{}", env.to_json());
    assert_eq!(grouped(&env), (2, 0));
    let next = hint_of(&env, Code::EntitiesGrouped).next().to_owned();
    assert_eq!(next, format!("umx entity show {a} --scope pair"));
    let (shown, exit) = home.run_next(&next);
    assert_eq!(exit, 0, "{}", shown.to_json());
    assert_eq!(e2_members(&shown), [a.clone(), b.clone()]);
    home.ok(&["scope", "save", "alone", "--graph", "g1"]);
    let alone = home.ok(&["neighbours", &a, "--scope", "alone"]);
    assert_eq!(alone.data["graphs"], json!(["g1"]));
    assert!(ungrouped(&alone), "{}", alone.to_json());
}

/// The synthetic shape of req-127's live check: three graphs, nine groups of nodes whose titles match exactly across
/// them, organisations first. Organisations group by a shared key, people by `entity link`, projects by a key in two
/// graphs and a proposal into the third. Every member of every group inherits the others' edges with the right
/// status, and `--entities confirmed` drops the proposed ones.
#[test]
fn nine_title_matched_groups_across_three_graphs_inherit_by_group() {
    let mut home = Home::new("e3-nine");
    home.start();
    let graphs = ["alpha", "beta", "gamma"];
    for g in graphs {
        graph(&home, g, &[]);
    }
    let groups: [(&str, &str, &str); 9] = [
        ("organisation", "Acme Holdings", "key"),
        ("organisation", "Globex Partners", "key"),
        ("organisation", "Initech Group", "key"),
        ("person", "Ana Cruz", "link"),
        ("person", "Sam Lee", "link"),
        ("person", "Kai Moana", "link"),
        ("project", "Apollo", "proposal"),
        ("project", "Zephyr", "proposal"),
        ("project", "Mercury", "proposal"),
    ];
    // members[g][i]: group g's node in graphs[i]; far[g][i]: the one edge it has of its own.
    let mut members: Vec<Vec<String>> = Vec::new();
    let mut far: Vec<Vec<String>> = Vec::new();
    for (g, (kind, title, how)) in groups.iter().enumerate() {
        let key = format!("entity_key=ref:group-{g}");
        let mut m = Vec::new();
        let mut f = Vec::new();
        for (i, graph) in graphs.iter().enumerate() {
            let keyed = *how == "key" || (*how == "proposal" && i < 2);
            let extra: Vec<&str> = if keyed {
                vec!["--set", key.as_str()]
            } else {
                Vec::new()
            };
            let node = add(&home, graph, kind, title, &extra);
            let edge_to = add(&home, graph, "problem", &format!("{title} note {i}"), &[]);
            blocks(&home, &node, &edge_to, graph);
            m.push(node);
            f.push(edge_to);
        }
        match *how {
            "link" => {
                home.ok(&["entity", "link", &m[0], &m[1]]);
                home.ok(&["entity", "link", &m[1], &m[2]]);
            }
            "proposal" => {
                home.ok(&[
                    "edge",
                    "add",
                    &m[1],
                    "possibly_same_as",
                    &m[2],
                    "--graph",
                    "beta",
                ]);
            }
            _ => {}
        }
        members.push(m);
        far.push(f);
    }
    for (g, (_, title, how)) in groups.iter().enumerate() {
        for i in 0..3 {
            let env = home.ok(&["neighbours", &members[g][i], "--relation", "blocks"]);
            let got = rows(&env);
            assert_eq!(
                got[0],
                row(&far[g][i], "own", "-"),
                "{title}/{i}: {}",
                env.to_json()
            );
            // Projects: alpha and beta share a key; beta proposes gamma. From gamma, only beta (one hop, never
            // beta's confirmed group).
            let status = |j: usize| match (*how, i, j) {
                ("proposal", 2, _) | ("proposal", _, 2) => "proposed",
                _ => "confirmed",
            };
            let mut want: Vec<_> = (0..3)
                .filter(|j| *j != i)
                .filter(|j| !(*how == "proposal" && i == 2 && *j == 0))
                .map(|j| row(&far[g][j], status(j), &members[g][j]))
                .collect();
            let mut inherited = got[1..].to_vec();
            want.sort();
            inherited.sort();
            assert_eq!(inherited, want, "{title}/{i}: {}", env.to_json());
            // The node's own graph first, as entity show lists them.
            let mut covered = vec![graphs[i]];
            covered.extend(graphs.iter().filter(|g| **g != graphs[i]));
            assert_eq!(env.data["graphs"], json!(covered), "{title}/{i}");
            let proposed = want.iter().filter(|r| r.1 == "proposed").count() as u64;
            assert_eq!(
                grouped(&env),
                (1 + want.len() as u64, proposed),
                "{title}/{i}: {}",
                env.to_json()
            );
            let confirmed = home.ok(&[
                "neighbours",
                &members[g][i],
                "--relation",
                "blocks",
                "--entities",
                "confirmed",
            ]);
            assert!(
                rows(&confirmed).iter().all(|r| r.1 != "proposed"),
                "{title}/{i}: {}",
                confirmed.to_json()
            );
        }
    }
}

/// Each row as (far end id, way, relation, match_status), in the reply's order.
fn ways(env: &Envelope) -> Vec<(String, String, String, String)> {
    let s = |v: &Value| v.as_str().unwrap_or("-").to_owned();
    env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                s(&r["id"]),
                s(&r["way"]),
                s(&r["relation"]),
                s(&r["match_status"]),
            )
        })
        .collect()
}

/// `--direction` and `--relation` apply to an inherited row relative to the member it came from: an edge arriving at
/// the member is `in`, one leaving it `out`, and a relation filter keeps only that relation.
#[test]
fn direction_and_relation_apply_to_inherited_rows_from_the_member() {
    let mut home = Home::new("e3-direction");
    home.start();
    graph(&home, "g1", &[]);
    graph(&home, "g2", &[]);
    let key = ["--set", "entity_key=email:jo@x.com"];
    let a = add(&home, "g1", "person", "Jo", &key);
    let b = add(&home, "g2", "person", "Jo", &key);
    let x = add(&home, "g2", "problem", "Leaves the member", &[]);
    let y = add(&home, "g2", "problem", "Arrives at the member", &[]);
    let z = add(&home, "g2", "problem", "Mentioned", &[]);
    blocks(&home, &b, &x, "g2");
    blocks(&home, &y, &b, "g2");
    home.ok(&["edge", "add", &b, "mentions", &z, "--graph", "g2"]);
    let r = |far: &str, way: &str, relation: &str| {
        (
            id(far).to_owned(),
            way.to_owned(),
            relation.to_owned(),
            "confirmed".to_owned(),
        )
    };
    let out = home.ok(&["neighbours", &a, "--direction", "out"]);
    assert_eq!(
        ways(&out),
        [r(&x, "out", "blocks"), r(&z, "out", "mentions")],
        "{}",
        out.to_json()
    );
    let into = home.ok(&["neighbours", &a, "--direction", "in"]);
    assert_eq!(ways(&into), [r(&y, "in", "blocks")], "{}", into.to_json());
    assert_eq!(into.data["rows"][0]["from"], id(&y));
    assert_eq!(into.data["rows"][0]["to"], id(&b));
    assert_eq!(into.data["rows"][0]["via"], b.as_str());
    let mentions = home.ok(&["neighbours", &a, "--relation", "mentions"]);
    assert_eq!(
        ways(&mentions),
        [r(&z, "out", "mentions")],
        "{}",
        mentions.to_json()
    );
}

/// A confirmed member's own possibly_same_as partner is proposed for the entity, as `entity show` lists it: its edges
/// are inherited as proposed and it counts in the hint.
#[test]
fn a_members_own_proposal_is_inherited_as_proposed() {
    let mut home = Home::new("e3-member-proposal");
    let key = ["--set", "entity_key=email:jo@x.com"];
    let n = e2_people(&mut home, &["g1", "g2", "g3"], &[&key, &key, &[]]);
    let w = add(&home, "g3", "problem", "The partner's edge", &[]);
    blocks(&home, &n[2], &w, "g3");
    home.ok(&[
        "edge",
        "add",
        &n[1],
        "possibly_same_as",
        &n[2],
        "--graph",
        "g2",
    ]);
    let env = home.ok(&["neighbours", &n[0]]);
    assert_eq!(
        rows(&env),
        [row(&w, "proposed", &n[2])],
        "{}",
        env.to_json()
    );
    assert_eq!(grouped(&env), (3, 1), "{}", env.to_json());
    let shown = home.ok(&["entity", "show", &n[0]]);
    assert_eq!(shown.data["proposed"][0]["address"], n[2].as_str());
    assert_eq!(shown.data["proposed"][0]["with"], n[1].as_str());
}

/// Only copies with no primary fall back to the node's own rows; any other refusal of the entity read is the reply's.
/// An address in one graph with `--graph` naming another is graph_mismatch, as `entity show` says; `--entities off`
/// reads as before.
#[test]
fn an_entity_refusal_other_than_copies_is_the_replys() {
    let mut home = Home::new("e3-refusal");
    home.start();
    graph(&home, "g1", &[]);
    graph(&home, "g2", &[]);
    let a = add(&home, "g1", "person", "Jo", &[]);
    let (env, exit) = home.umx(&["neighbours", &a, "--graph", "g2"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    issue(&env, Code::GraphMismatch);
    let (shown, _) = home.umx(&["entity", "show", &a, "--graph", "g2"]);
    assert_eq!(env.errors[0].message(), shown.errors[0].message());
    let off = home.ok(&["neighbours", &a, "--graph", "g2", "--entities", "off"]);
    assert_eq!(rows(&off), Vec::<(String, String, String)>::new());
}
