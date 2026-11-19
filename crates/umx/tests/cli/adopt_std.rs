//! ws008 T3 (req-101): `umx ontology adopt-std` through the CLI. A module of the cli tests (it uses their `Home`
//! and helpers), kept in its own file.

use super::*;

/// ws008 T3 (req-101): starts the daemon and makes graph `demo` at `<home>/g` without std (`graph new --no-std`), as
/// a graph made before std was; returns the folder.
fn pre_std(home: &mut Home) -> String {
    home.start();
    let path = home.path.join("g").display().to_string();
    home.ok(&["graph", "new", &path, "--alias", "demo", "--no-std"]);
    path
}

/// ws008 T3 (req-101): [`trigger`] for the `ontology adopt-std` codes, in `home`, which the caller keeps (to run the
/// next as printed).
pub(super) fn adopt_trigger(home: &mut Home, code: Code) -> (Envelope, u8) {
    let args = adopt_setup(home, code);
    home.umx(&args.iter().map(String::as_str).collect::<Vec<_>>())
}

/// Makes the graph for one `ontology adopt-std` code in `home` and gives the command that raises it.
pub(super) fn adopt_setup(home: &mut Home, code: Code) -> Vec<String> {
    let words = |w: &[&str]| -> Vec<String> { w.iter().map(|s| (*s).to_owned()).collect() };
    let adopt = ["ontology", "adopt-std"];
    match code {
        Code::AdoptFieldKindClash => {
            pre_std(home);
            home.ok(&["ontology", "add", "field", "person", "aliases:string"]);
            words(&adopt)
        }
        Code::AdoptReservedRelation => {
            let path = pre_std(home);
            let a = home.add("problem", "First");
            let b = home.add("problem", "Second");
            home.ok(&["edge", "add", &a, "blocks", &b]);
            home.ok(&["graph", "unload", "demo"]);
            let file = Path::new(&path).join("edges.jsonl");
            let text = fs::read_to_string(&file).unwrap();
            fs::write(&file, text.replace("\"blocks\"", "\"authored_by\"")).unwrap();
            home.ok(&["graph", "load", &path]);
            words(&adopt)
        }
        Code::AdoptMapUnknown => {
            pre_std(home);
            words(&["ontology", "adopt-std", "--map", "individual=person"])
        }
        Code::AdoptMapAmbiguous => {
            pre_std(home);
            home.ok(&["ontology", "add", "type", "mentor"]);
            home.ok(&["ontology", "add", "relation", "mentor"]);
            words(&["ontology", "adopt-std", "--map", "mentor=coach"])
        }
        Code::AdoptMapMerge => {
            pre_std(home);
            home.ok(&["ontology", "add", "field", "individual", "bio:string"]);
            home.ok(&["ontology", "add", "field", "person", "bio:int"]);
            words(&["ontology", "adopt-std", "--map", "individual=person"])
        }
        Code::AdoptMapSymmetry => {
            pre_std(home);
            home.ok(&["ontology", "add", "relation", "knows", "--symmetric"]);
            words(&["ontology", "adopt-std", "--map", "knows=blocks"])
        }
        Code::AdoptMapDuplicateEdge => {
            pre_std(home);
            let a = home.add("problem", "First");
            let b = home.add("problem", "Second");
            home.ok(&["edge", "add", &a, "blocks", &b]);
            home.ok(&["edge", "add", &a, "supports", &b]);
            words(&["ontology", "adopt-std", "--map", "supports=blocks"])
        }
        Code::AdoptValueKind => {
            pre_std(home);
            home.ok(&[
                "node",
                "add",
                "--type",
                "person",
                "--title",
                "Ada",
                "--set",
                "aliases=Ada L",
            ]);
            words(&adopt)
        }
        Code::StdFieldConflictStored => {
            let path = home.graph();
            home.ok(&["graph", "unload", "demo"]);
            edit_graph_json(&path, |g| {
                g["ontology"]["types"].as_array_mut().unwrap().push(
                    json!({ "name": "person", "fields": [{ "name": "job_title", "kind": "int" }] }),
                );
            });
            vec!["graph".into(), "load".into(), path]
        }
        Code::AlreadyStd => {
            home.graph();
            words(&adopt)
        }
        Code::AdoptRenameSuggested => {
            pre_std(home);
            home.ok(&["ontology", "add", "type", "organization"]);
            words(&["ontology", "adopt-std", "--dry-run"])
        }
        Code::AdoptFieldToTagRefused => {
            pre_std(home);
            words(&["ontology", "adopt-std", "--field-to-tag", "nothing"])
        }
        Code::AdoptTagValueInvalid => {
            pre_std(home);
            home.ok(&[
                "node",
                "add",
                "--type",
                "problem",
                "--title",
                "x",
                "--set",
                "origin=123",
            ]);
            words(&["ontology", "adopt-std", "--field-to-tag", "origin"])
        }
        _ => unreachable!("{code:?}"),
    }
}

/// ws008 T3 (req-101): the graph's three files, as bytes.
fn graph_files(path: &str) -> [Vec<u8>; 3] {
    ["graph.json", "nodes.jsonl", "edges.jsonl"].map(|f| fs::read(Path::new(path).join(f)).unwrap())
}

/// The lines of `file` in graph folder `path`, by the id each holds.
fn lines_by_id(path: &str, file: &str) -> std::collections::BTreeMap<String, String> {
    fs::read_to_string(Path::new(path).join(file))
        .unwrap()
        .lines()
        .map(|l| {
            let v: Value = umx_protocol::parse_json(l).unwrap();
            (v["id"].as_str().unwrap().to_owned(), l.to_owned())
        })
        .collect()
}

const ADOPT_CODES: [Code; 13] = [
    Code::AdoptFieldToTagRefused,
    Code::AdoptTagValueInvalid,
    Code::AdoptFieldKindClash,
    Code::AdoptReservedRelation,
    Code::AdoptMapUnknown,
    Code::AdoptMapAmbiguous,
    Code::AdoptMapMerge,
    Code::AdoptMapSymmetry,
    Code::AdoptMapDuplicateEdge,
    Code::AdoptValueKind,
    Code::StdFieldConflictStored,
    Code::AlreadyStd,
    Code::AdoptRenameSuggested,
];

/// ws008 T3 (req-101, D69): each adopt-std code is raised with its slots filled and the kind its row says, and its
/// next runs as printed.
#[test]
fn each_adopt_std_code_is_raised_and_its_next_runs_as_printed() {
    for code in ADOPT_CODES {
        let spec = code.spec();
        let mut home = Home::new(&format!("{}-next", spec.code));
        let (env, exit) = adopt_trigger(&mut home, code);
        let found = issue(&env, code);
        assert_eq!(exit, spec.exit, "{}: {}", spec.code, env.to_json());
        for slot in slots(spec.message).chain(slots(spec.next)) {
            let slot = format!("<{slot}>");
            assert!(
                !found.message().contains(&slot) && !found.next().contains(&slot),
                "{} left {slot}: {}",
                spec.code,
                found.message()
            );
        }
        let status = match spec.kind {
            Kind::Error => Status::Error,
            Kind::Warning => Status::Warning,
            Kind::Hint => Status::Ok,
        };
        assert_eq!(env.status, status, "{}: {}", spec.code, env.to_json());
        let (next, next_exit) = run_as_printed(&home, found.next());
        assert_eq!(
            next_exit,
            0,
            "{}: {} gave {}",
            spec.code,
            found.next(),
            next.to_json()
        );
    }
}

/// ws008 T3 (req-101): a graph built to the interim std sheet adopts with no line change; graph.json keeps only
/// person's `name` (std v1 has no such field) and says `std: 1`. A second run is `already_std` and writes nothing.
#[test]
fn an_interim_sheet_graph_adopts_std_with_no_line_change_and_a_second_run_writes_nothing() {
    let mut home = Home::new("adopt_interim");
    let path = pre_std(&mut home);
    let sheet: Value = umx_protocol::parse_json(include_str!(
        "../../../umx-core/fixtures/std/interim-sheet.json"
    ))
    .unwrap();
    let kind = |f: &str| match f {
        "aliases" => "list",
        "starts_on" | "ends_on" | "due_on" => "date",
        _ => "string",
    };
    home.ok(&["graph", "unload", "demo"]);
    edit_graph_json(&path, |g| {
        let types: Vec<Value> = sheet["types"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                let fields: Vec<Value> = sheet["fields"][t.as_str().unwrap()]
                    .as_array()
                    .map(|fs| {
                        fs.iter()
                            .map(|f| f.as_str().unwrap())
                            // `title` is a reserved key in every graph: no graph can declare it.
                            .filter(|f| *f != "title")
                            .map(|f| json!({ "name": f, "kind": kind(f) }))
                            .collect()
                    })
                    .unwrap_or_default();
                json!({ "name": t, "fields": fields })
            })
            .collect();
        g["ontology"]["types"] = json!(types);
        g["ontology"]["relations"] = sheet["relations"].clone();
    });
    home.ok(&["graph", "load", &path]);
    let ada = home
        .ok(&[
            "node",
            "add",
            "--type",
            "person",
            "--title",
            "Ada",
            "--set",
            "org=Acme",
            "--set-json",
            "aliases=[\"Ada L\"]",
            "--set",
            "name=Ada Lovelace",
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let meeting = home
        .ok(&[
            "node",
            "add",
            "--type",
            "meeting",
            "--title",
            "Kick-off",
            "--set",
            "starts_on=2026-11-02",
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    home.ok(&["edge", "add", &ada, "attended", &meeting]);
    let [_, nodes, edges] = graph_files(&path);

    // Another session than the one that wrote the lines: with no --map, adopting stamps nothing and records no session.
    let (env, exit) = home.umx_env(
        &["ontology", "adopt-std"],
        &[("COPILOT_AGENT_SESSION_ID", "AdoptSession_02")],
    );
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(env.data["unchanged"], json!(false), "{}", env.to_json());
    assert_eq!(env.data["std"], json!(1));
    assert_eq!(env.data["renamed"], json!([]));
    assert_eq!(
        env.data["extended"],
        json!([{ "type": "person", "field": "name" }])
    );
    assert!(env.warnings.is_empty(), "{}", env.to_json());
    let [graph_json, nodes_after, edges_after] = graph_files(&path);
    assert_eq!((nodes_after, edges_after), (nodes.clone(), edges.clone()));
    let stored: Value =
        umx_protocol::parse_json(std::str::from_utf8(&graph_json).unwrap()).unwrap();
    assert_eq!(stored["ontology"]["std"], json!(1));
    assert_eq!(
        stored["ontology"]["types"],
        json!([{ "name": "person", "fields": [{ "name": "name", "kind": "string" }] }])
    );
    assert_eq!(stored["ontology"]["relations"], json!([]));
    // std's fields are the type's now, then the graph's own.
    let show = home.ok(&["ontology", "show"]);
    let fields = &named(&show.data["types"], "person")["fields"];
    let names: Vec<&str> = fields
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["aliases", "job_title", "org", "name"]);

    let (again, exit) = home.umx(&["ontology", "adopt-std"]);
    assert_eq!(exit, 0, "{}", again.to_json());
    assert_eq!(again.data["unchanged"], json!(true));
    assert_eq!(
        issue(&again, Code::AlreadyStd).next(),
        "umx ontology show --graph demo"
    );
    assert_eq!(graph_files(&path), [graph_json, nodes, edges]);
}

/// ws008 T3 (req-101): `--map` rewrites only the lines of the type or relation it names, stamped as `node edit
/// --type` stamps, and every id, end and address stays.
#[test]
fn adopt_std_map_rewrites_only_those_lines_and_keeps_every_id() {
    let mut home = Home::new("adopt_map");
    let path = pre_std(&mut home);
    home.ok(&["ontology", "add", "field", "individual", "bio:string"]);
    let ada = home.add("individual", "Ada");
    let bo = home.add("individual", "Bo");
    let p = home.add("problem", "Approvals");
    let kept = edge_id(&home.ok(&["edge", "add", &ada, "blocks", &p]));
    let moved = edge_id(&home.ok(&["edge", "add", &bo, "supports", &p]));
    let (nodes, edges) = (
        lines_by_id(&path, "nodes.jsonl"),
        lines_by_id(&path, "edges.jsonl"),
    );

    // Another session than the one that made the lines, so the stamps it leaves are told apart.
    let (env, exit) = home.umx_env(
        &[
            "ontology",
            "adopt-std",
            "--map",
            "individual=std:person",
            "--map",
            "supports=backs",
        ],
        &[("COPILOT_AGENT_SESSION_ID", "AdoptSession_01")],
    );
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(
        env.data["renamed"],
        json!([
            { "what": "type", "from": "individual", "to": "person", "nodes": 2, "edges": 0 },
            { "what": "relation", "from": "supports", "to": "backs", "nodes": 0, "edges": 1 },
        ]),
        "{}",
        env.to_json()
    );
    let (nodes_after, edges_after) = (
        lines_by_id(&path, "nodes.jsonl"),
        lines_by_id(&path, "edges.jsonl"),
    );
    // Every line keeps its id; the only new lines are the provenance of the session that ran the adoption.
    let provenance = ["session", "agent", "platform", "harness", "model"];
    assert!(nodes.keys().all(|id| nodes_after.contains_key(id)));
    for (id, line) in nodes_after
        .iter()
        .filter(|(id, _)| !nodes.contains_key(*id))
    {
        let v: Value = umx_protocol::parse_json(line).unwrap();
        assert!(
            provenance.contains(&v["type"].as_str().unwrap()),
            "{id}: {line}"
        );
    }
    assert_eq!(nodes_after[&p], nodes[&p]);
    for id in [&ada, &bo] {
        let v: Value = umx_protocol::parse_json(&nodes_after[id]).unwrap();
        assert_eq!(
            (v["type"].as_str(), v["id"].as_str()),
            (Some("person"), Some(id.as_str()))
        );
        let was: Value = umx_protocol::parse_json(&nodes[id]).unwrap();
        assert!(v["edited_by_session"].is_string(), "{v}");
        assert_ne!(v["edited_by_session"], was["edited_by_session"], "{v}");
        assert_eq!(v["authored_by_session"], was["authored_by_session"], "{v}");
    }
    assert!(edges.keys().all(|id| edges_after.contains_key(id)));
    for (id, line) in edges_after
        .iter()
        .filter(|(id, _)| !edges.contains_key(*id))
    {
        let v: Value = umx_protocol::parse_json(line).unwrap();
        assert!(
            v["type"] != "backs" && v["type"] != "blocks",
            "{id}: {line}"
        );
    }
    assert_eq!(edges_after[&kept], edges[&kept]);
    let (was, now): (Value, Value) = (
        umx_protocol::parse_json(&edges[&moved]).unwrap(),
        umx_protocol::parse_json(&edges_after[&moved]).unwrap(),
    );
    assert_eq!(now["type"], "backs");
    // Stamped as `edge edit --relation` stamps an edge.
    assert!(
        now["updated"].is_string() && now["updated_by"].is_string(),
        "{now}"
    );
    assert!(was.get("updated").is_none(), "{was}");
    assert_eq!((&now["from"], &now["to"]), (&was["from"], &was["to"]));
    assert!(now["edited_by_session"].is_string());
    assert_ne!(now["edited_by_session"], was["edited_by_session"]);
    // The address still resolves, to the renamed node, which keeps its field.
    let got = home.ok(&["node", "get", &format!("umx:demo/{ada}")]);
    assert_eq!(got.data["node"]["type"], "person");
    let stored = type_in_file(&path, "person");
    assert_eq!(
        stored["fields"],
        json!([{ "name": "bio", "kind": "string" }])
    );
    let gj: Value =
        umx_protocol::parse_json(&fs::read_to_string(Path::new(&path).join("graph.json")).unwrap())
            .unwrap();
    let relations = strings(&gj["ontology"]["relations"]);
    assert!(
        relations.contains(&"backs") && !relations.contains(&"supports"),
        "{relations:?}"
    );
    assert!(
        gj["ontology"]["types"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["name"] != "individual")
    );
}

/// ws008 T3 (req-101): `--dry-run` writes nothing and prints the plan, with renames only suggested.
#[test]
fn adopt_std_dry_run_writes_nothing_and_suggests_renames_it_never_applies() {
    let mut home = Home::new("adopt_dry");
    let path = pre_std(&mut home);
    home.ok(&["ontology", "add", "type", "organization"]);
    home.add("individual", "Ada");
    let before = graph_files(&path);
    let env = home.ok(&[
        "ontology",
        "adopt-std",
        "--dry-run",
        "--map",
        "individual=person",
    ]);
    assert_eq!(env.data["dry_run"], json!(true));
    assert_eq!(env.data["renamed"][0]["nodes"], json!(1));
    assert!(strings(&env.data["custom"]["types"]).contains(&"organization"));
    let hint = issue(&env, Code::AdoptRenameSuggested);
    assert_eq!(
        hint.next(),
        "umx ontology adopt-std --map organization=organisation --dry-run --graph demo"
    );
    assert_eq!(graph_files(&path), before);
    let show = home.ok(&["ontology", "show"]);
    assert_eq!(show.data["std"], json!(null), "{}", show.to_json());
}

/// ws008 T3 (req-101): each conflict kind is refused with its fix, and the graph's files are left as they were.
#[test]
fn each_adopt_std_conflict_is_refused_and_writes_nothing() {
    for code in [
        Code::AdoptFieldKindClash,
        Code::AdoptReservedRelation,
        Code::AdoptMapUnknown,
        Code::AdoptMapAmbiguous,
        Code::AdoptMapMerge,
        Code::AdoptMapSymmetry,
        Code::AdoptMapDuplicateEdge,
    ] {
        let mut home = Home::new(&format!("{}-nowrite", code.spec().code));
        let args = adopt_setup(&mut home, code);
        let path = home.path.join("g").display().to_string();
        let before = graph_files(&path);
        let (env, exit) = home.umx(&args.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!((env.status, exit), (Status::Error, 1), "{}", env.to_json());
        issue(&env, code);
        // With --dry-run too: the same refusal.
        let mut dry = args.clone();
        dry.push("--dry-run".into());
        let (env, _) = home.umx(&dry.iter().map(String::as_str).collect::<Vec<_>>());
        issue(&env, code);
        assert_eq!(graph_files(&path), before, "{}", code.spec().code);
    }
}

/// ws008 T3 (req-101): a secured graph adopts std like any other; an open graph's adoption names nothing of a secured
/// graph loaded beside it, even one with the same names.
#[test]
fn a_secured_graph_adopts_std_and_an_open_graphs_reply_names_none() {
    let mut home = Home::new("adopt_secured");
    home.start();
    let vault = home.path.join("v").display().to_string();
    let made = home.ok(&[
        "graph",
        "new",
        &vault,
        "--alias",
        "vault",
        "--secured",
        "--no-std",
    ]);
    let vault_id = made.data["graph"]["id"].as_str().unwrap().to_owned();
    let open = home.path.join("g").display().to_string();
    home.ok(&["graph", "new", &open, "--alias", "demo", "--no-std"]);
    let secret = home
        .ok(&[
            "node",
            "add",
            "--type",
            "individual",
            "--title",
            "Vault person",
            "--graph",
            "vault",
        ])
        .data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    home.ok(&[
        "node",
        "add",
        "--type",
        "individual",
        "--title",
        "Ada",
        "--graph",
        "demo",
    ]);

    let env = home.ok(&[
        "ontology",
        "adopt-std",
        "--graph",
        "demo",
        "--map",
        "individual=person",
        "--dry-run",
    ]);
    let env2 = home.ok(&[
        "ontology",
        "adopt-std",
        "--graph",
        "demo",
        "--map",
        "individual=person",
    ]);
    for e in [&env, &env2] {
        let text = e.to_json();
        assert!(
            !text.contains("vault") && !text.contains(&vault_id) && !text.contains(&secret),
            "{text}"
        );
    }
    let env = home.ok(&[
        "ontology",
        "adopt-std",
        "--graph",
        "vault",
        "--map",
        "individual=person",
    ]);
    assert_eq!(
        env.data["renamed"][0]["nodes"],
        json!(1),
        "{}",
        env.to_json()
    );
    let got = home.ok(&["node", "get", &format!("umx:vault/{secret}")]);
    assert_eq!(got.data["node"]["type"], "person");
    let show = home.ok(&["ontology", "show", "--graph", "vault"]);
    assert_eq!(show.data["std"], json!(1));
}

/// ws008 T3 (req-101): stored `authored_by` edges block adoption until a `--map` gives them a name of the graph's own,
/// which keeps their ids.
#[test]
fn stored_authored_by_edges_adopt_once_a_map_renames_them() {
    let mut home = Home::new("adopt_authored_by");
    adopt_setup(&mut home, Code::AdoptReservedRelation);
    let path = home.path.join("g").display().to_string();
    let before = lines_by_id(&path, "edges.jsonl");
    let (id, _) = before
        .iter()
        .find(|(_, l)| l.contains("\"authored_by\""))
        .unwrap();
    let env = home.ok(&["ontology", "adopt-std", "--map", "authored_by=written_by"]);
    assert_eq!(
        env.data["renamed"][0]["edges"],
        json!(1),
        "{}",
        env.to_json()
    );
    let after = lines_by_id(&path, "edges.jsonl");
    let v: Value = umx_protocol::parse_json(&after[id]).unwrap();
    assert_eq!(v["type"], "written_by");
    assert!(home.ok(&["ontology", "show"]).data["std"] == json!(1));
}

/// ws008 T3 (req-101): a `--map` takes only what every other write takes. A chain or a swap of maps is refused, and
/// so is a target another write refuses: a relation named `std`, `not_same_as` or a derived one, a type named `std`
/// or `graph`; `not_same_as` edges are never renamed; nor is an edge whose own `updated` is not umx's stamp.
/// Nothing is written.
#[test]
fn adopt_std_map_refuses_chains_swaps_and_names_other_writes_refuse() {
    let mut home = Home::new("adopt_map_refused");
    let path = pre_std(&mut home);
    let a = home.add("problem", "First");
    let b = home.add("problem", "Second");
    home.ok(&["edge", "add", &a, "supports", &b]);
    home.ok(&["entity", "deny", &a, &b, "--reason", "Two problems"]);
    let before = graph_files(&path);
    for (maps, code) in [
        (
            &["supports=blocks", "blocks=depends_on"][..],
            "usage_invalid",
        ),
        (
            &["blocks=depends_on", "supports=blocks"][..],
            "usage_invalid",
        ),
        (&["supports=blocks", "blocks=supports"][..], "usage_invalid"),
        (&["supports=authored_by"][..], "derived_relation"),
        (&["supports=std"][..], "reserved_name"),
        (&["supports=not_same_as"][..], "entity_link_invalid"),
        (&["problem=std"][..], "reserved_name"),
        (&["problem=graph"][..], "reserved_name"),
        (&["not_same_as=refuted"][..], "entity_link_invalid"),
    ] {
        let mut args = vec!["ontology", "adopt-std"];
        for m in maps {
            args.extend(["--map", m]);
        }
        let (env, _) = home.umx(&args);
        assert_eq!(env.status, Status::Error, "{maps:?}: {}", env.to_json());
        assert_eq!(env.errors[0].code(), code, "{maps:?}: {}", env.to_json());
        // Its own words reach the message whole.
        if code == "usage_invalid" {
            assert!(
                !env.errors[0].message().contains('?'),
                "{}",
                env.errors[0].message()
            );
        }
        assert_eq!(graph_files(&path), before, "{maps:?}");
    }
    // An edge holding its own `updated` keeps it: the rename is refused, as `edge edit --relation` refuses.
    home.ok(&["graph", "unload", "demo"]);
    let file = Path::new(&path).join("edges.jsonl");
    let text = fs::read_to_string(&file).unwrap();
    let own = text.replace(
        "\"type\":\"supports\"",
        "\"type\":\"supports\",\"updated\":\"last week\"",
    );
    assert_ne!(own, text);
    fs::write(&file, own).unwrap();
    home.ok(&["graph", "load", &path]);
    let before = graph_files(&path);
    let (env, _) = home.umx(&["ontology", "adopt-std", "--map", "supports=backs"]);
    issue(&env, Code::EdgeReservedFieldInUse);
    assert_eq!(graph_files(&path), before);
}

/// ws008 T3 (req-101): while nodes hold a field declared with another kind than std's, adopt_field_kind_clash's next
/// lists them; an edge value of a std relation field of another kind warns adopt_value_kind with the edge. Both nexts
/// run as printed.
#[test]
fn a_kind_clash_points_at_its_holders_and_edge_values_are_checked() {
    let mut home = Home::new("adopt_holders");
    pre_std(&mut home);
    home.ok(&["ontology", "add", "field", "person", "aliases:string"]);
    home.ok(&[
        "node",
        "add",
        "--type",
        "person",
        "--title",
        "Ada",
        "--set",
        "aliases=Ada L",
    ]);
    let (env, _) = home.umx(&["ontology", "adopt-std"]);
    let clash = issue(&env, Code::AdoptFieldKindClash);
    assert_eq!(
        clash.next(),
        "umx node list --type person --columns aliases --graph demo"
    );
    assert!(
        clash.message().contains("--unset aliases"),
        "{}",
        clash.message()
    );
    let (listed, exit) = run_as_printed(&home, clash.next());
    assert_eq!(
        (exit, listed.data["rows"].as_array().unwrap().len()),
        (0, 1)
    );

    let mut home = Home::new("adopt_edge_values");
    pre_std(&mut home);
    let ada = home.add("person", "Ada");
    let acme = home.add("organisation", "Acme");
    let edge = edge_id(&home.ok(&[
        "edge",
        "add",
        &ada,
        "works_for",
        &acme,
        "--set",
        "starts_on=soon",
    ]));
    let env = home.ok(&["ontology", "adopt-std"]);
    let w = issue(&env, Code::AdoptValueKind);
    assert!(
        w.message().starts_with("1 works_for edges hold starts_on"),
        "{}",
        w.message()
    );
    assert_eq!(w.next(), format!("umx edge show umx:demo/{edge}"));
    assert_eq!(run_as_printed(&home, w.next()).1, 0);
}

/// Adds a `<kind>` titled `title` to `demo` with these `--set`/`--set-json` words; returns its id.
fn holder(home: &Home, kind: &str, title: &str, sets: &[&str]) -> String {
    let mut args = vec!["node", "add", "--type", kind, "--title", title];
    args.extend(sets);
    home.ok(&args).data["node"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The stored line of `id` in `path`'s nodes.jsonl.
fn node_line(path: &str, id: &str) -> Value {
    umx_protocol::parse_json(&lines_by_id(path, "nodes.jsonl")[id]).unwrap()
}

/// req-117's done_when: `ontology adopt-std --field-to-tag origin` moves every origin value into an `origin:<value>`
/// tag (tag_invalid's snake_case fix applied, merged with the tags a node holds), ids unchanged and each moved line
/// stamped, lists the values it cannot normalise (they stay), and `--dry-run` writes nothing. Run again on the std
/// graph once the last value can move, it removes the field's declaration.
#[test]
fn field_to_tag_moves_every_value_into_a_tag_with_ids_unchanged() {
    let mut home = Home::new("adopt_field_to_tag");
    let path = pre_std(&mut home);
    home.ok(&["ontology", "add", "field", "problem", "origin:string"]);
    let a = holder(&home, "problem", "a", &["--set", "origin=Double Diamond"]);
    let b = holder(&home, "decision", "b", &["--set", "origin=double-diamond"]);
    let c = holder(
        &home,
        "problem",
        "c",
        &["--set", "origin=Lean", "--set-json", "tags=[\"keep\"]"],
    );
    let d = holder(&home, "problem", "d", &["--set-str", "origin=123"]);
    let e = holder(
        &home,
        "problem",
        "e",
        &["--set-json", "origin=[\"Agile\",\"x y\",7]"],
    );
    let f = holder(&home, "problem", "f", &[]);
    // An empty list leaves the node with the moved values; a value whose tag would be cut stays whole.
    let g = holder(&home, "problem", "g", &["--set-json", "origin=[]"]);
    let long = "word ".repeat(600);
    let h = holder(
        &home,
        "problem",
        "h",
        &["--set-str", &format!("origin={long}")],
    );
    let before = graph_files(&path);
    let lines = lines_by_id(&path, "nodes.jsonl");
    let plan = home.ok(&[
        "ontology",
        "adopt-std",
        "--field-to-tag",
        "origin",
        "--dry-run",
    ]);
    assert_eq!(graph_files(&path), before, "a dry run writes nothing");
    assert_eq!(
        plan.data["field_to_tag"],
        json!([{ "field": "origin", "nodes": 4, "tags": 5, "kept": 3, "collapsed": 1, "declaration_removed": false }])
    );
    let w = issue(&plan, Code::AdoptTagValueInvalid);
    assert_eq!(w.code(), "adopt_tag_value_invalid");
    // Another session than the one that made the lines, so the stamp it leaves is told apart.
    let (env, exit) = home.umx_env(
        &["ontology", "adopt-std", "--field-to-tag", "origin"],
        &[("COPILOT_AGENT_SESSION_ID", "AdoptSession_02")],
    );
    assert_eq!(exit, 0, "{}", env.to_json());
    assert_eq!(env.data["field_to_tag"], plan.data["field_to_tag"]);
    let warned: Vec<(String, Vec<(String, String)>)> = env
        .warnings
        .iter()
        .filter(|w| w.code() == "adopt_tag_value_invalid")
        .map(|w| {
            (
                w.message().to_owned(),
                w.candidates()
                    .iter()
                    .map(|c| {
                        (
                            c["id"].as_str().unwrap_or_default().to_owned(),
                            c["title"].as_str().unwrap_or_default().to_owned(),
                        )
                    })
                    .collect(),
            )
        })
        .collect();
    assert_eq!(warned.len(), 3, "{warned:?}");
    assert!(warned[0].0.contains("no snake_case tag"), "{warned:?}");
    assert_eq!(warned[0].1, [(d.clone(), "123".to_owned())]);
    assert!(warned[1].0.contains("not text"), "{warned:?}");
    assert_eq!(warned[1].1, [(e.clone(), "<not text>".to_owned())]);
    assert!(warned[2].0.contains("longer than a tag"), "{warned:?}");
    assert_eq!(warned[2].1[0].0, h);
    assert!(
        warned[2].1[0].1.starts_with("word word") && warned[2].1[0].1.chars().count() == 81,
        "{warned:?}"
    );
    let after = lines_by_id(&path, "nodes.jsonl");
    // Ids unchanged: every node is still there by its id, and the only new lines are provenance records of the run
    // (its session, and its agent when this machine's environment names one).
    assert!(
        lines.keys().all(|id| after.contains_key(id)),
        "ids unchanged"
    );
    for (id, l) in after.iter().filter(|(id, _)| !lines.contains_key(*id)) {
        let v: Value = umx_protocol::parse_json(l).unwrap();
        let kind = v["type"].as_str().unwrap_or_default();
        assert!(
            umx_core::ontology::PROVENANCE_TYPES.contains(&kind),
            "{id}: {l}"
        );
    }
    let line = |id: &str| node_line(&path, id);
    assert_eq!(line(&a)["tags"], json!(["origin:double_diamond"]));
    assert!(line(&a).get("origin").is_none());
    assert_eq!(line(&b)["tags"], json!(["origin:double_diamond"]));
    assert_eq!(line(&c)["tags"], json!(["keep", "origin:lean"]));
    assert_eq!(line(&d)["origin"], json!("123"));
    assert!(line(&d).get("tags").is_none());
    assert_eq!(line(&e)["tags"], json!(["origin:agile", "origin:x_y"]));
    assert_eq!(line(&e)["origin"], json!([7]));
    assert_eq!(
        after[&f], lines[&f],
        "a node without the field is not written"
    );
    assert!(line(&g).get("origin").is_none(), "an empty list leaves");
    assert!(line(&g).get("tags").is_none());
    assert_eq!(line(&h)["origin"], json!(long), "a long value stays whole");
    assert!(line(&h).get("tags").is_none());
    let (old, new) = (umx_protocol::parse_json(&lines[&a]).unwrap(), line(&a));
    assert_eq!(new["created"], old["created"]);
    assert!(new["edited_by_session"].is_string(), "{new}");
    assert_ne!(
        new["edited_by_session"], old["edited_by_session"],
        "a moved line is stamped"
    );
    assert_eq!(graph_json(&path)["ontology"]["std"], json!(1));
    let found = home.ok(&["node", "list", "--tag", "origin:*"]);
    let mut got: Vec<&str> = found.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    got.sort();
    let mut want = vec![a.as_str(), b.as_str(), c.as_str(), e.as_str()];
    want.sort();
    assert_eq!(got, want);
    // On the std graph, alone: the last values move and the declaration goes.
    home.ok(&["node", "edit", &d, "--set", "origin=Kept Value"]);
    home.ok(&["node", "edit", &e, "--unset", "origin"]);
    home.ok(&["node", "edit", &h, "--set-json", "origin=[]"]);
    let env = home.ok(&["ontology", "adopt-std", "--field-to-tag", "origin"]);
    assert_eq!(
        env.data["field_to_tag"][0]["declaration_removed"],
        json!(true)
    );
    assert_eq!(line(&d)["tags"], json!(["origin:kept_value"]));
    let types = graph_json(&path)["ontology"]["types"].to_string();
    assert!(!types.contains("origin"), "{types}");
}

/// A fake AWS access key id, built at run time so no credential-shaped text is in the source.
fn token() -> String {
    format!("AKIA{}", "Q7Z3X9K2Q7Z3X9K2")
}

/// A value that looks like a secret stays in its field and is listed masked; a node that would pass 64 tags moves
/// none of its values. `--map` and `--field-to-tag` land in one write.
#[test]
fn field_to_tag_masks_secrets_keeps_the_cap_and_joins_a_map() {
    let mut home = Home::new("adopt_field_to_tag_more");
    let path = pre_std(&mut home);
    let key = token();
    let s = holder(
        &home,
        "problem",
        "s",
        &["--set-str", &format!("origin={key}")],
    );
    let full: Vec<String> = (0..64).map(|i| format!("\"t{i}\"")).collect();
    let tags = format!("tags=[{}]", full.join(","));
    let big = holder(
        &home,
        "problem",
        "big",
        &["--set", "origin=one", "--set-json", &tags],
    );
    let m = holder(&home, "individual", "m", &["--set", "origin=Lean"]);
    // A key inside an object (as a value and as a key), inside a nested list, and lowercased as plain text.
    let lower = key.to_ascii_lowercase();
    let obj = holder(
        &home,
        "problem",
        "obj",
        &[
            "--set-json",
            &format!("origin={{\"note\":\"{key}\",\"{lower}\":1}}"),
        ],
    );
    let nested = holder(
        &home,
        "problem",
        "nested",
        &["--set-json", &format!("origin=[[\"{key}\"]]")],
    );
    let low = holder(
        &home,
        "problem",
        "low",
        &["--set-str", &format!("origin={lower}")],
    );
    let env = home.ok(&[
        "ontology",
        "adopt-std",
        "--map",
        "individual=person",
        "--field-to-tag",
        "origin",
    ]);
    let json = env.to_json();
    assert!(
        !json.contains(&key) && !json.contains(&key.to_ascii_lowercase()),
        "{json}"
    );
    let by_reason = |r: &str| {
        env.warnings
            .iter()
            .find(|w| w.code() == "adopt_tag_value_invalid" && w.message().contains(r))
            .unwrap_or_else(|| panic!("{r}: {json}"))
            .candidates()
            .iter()
            .map(|c| {
                (
                    c["id"].as_str().unwrap_or_default().to_owned(),
                    c["title"].as_str().unwrap_or_default().to_owned(),
                )
            })
            .collect::<Vec<_>>()
    };
    let mut secrets = by_reason("looks like a secret");
    secrets.sort();
    let mut want: Vec<(String, String)> = [&s, &obj, &nested, &low]
        .into_iter()
        .map(|id| (id.clone(), "<masked>".to_owned()))
        .collect();
    want.sort();
    assert_eq!(secrets, want);
    for id in [&obj, &nested, &low] {
        assert!(
            node_line(&path, id).get("origin").is_some(),
            "{id} keeps it"
        );
    }
    // Counted from the final outcome: the node past the cap keeps its value, so it counts as kept.
    assert_eq!(
        env.data["field_to_tag"],
        json!([{ "field": "origin", "nodes": 1, "tags": 1, "kept": 5, "collapsed": 0, "declaration_removed": false }])
    );
    assert_eq!(
        by_reason("more than 64 tags"),
        [(big.clone(), "one".to_owned())]
    );
    assert_eq!(node_line(&path, &s)["origin"], json!(key));
    assert_eq!(node_line(&path, &big)["origin"], json!("one"));
    let moved = node_line(&path, &m);
    assert_eq!(
        (moved["type"].clone(), moved["tags"].clone()),
        (json!("person"), json!(["origin:lean"]))
    );
    assert!(moved.get("origin").is_none());
}

/// Each `--field-to-tag` refusal writes nothing and its next runs as printed: a std field, `tags`, a field only edges
/// have, one no node holds, one a spine reads, a field given twice, and `--map` on a graph that already uses std.
#[test]
fn field_to_tag_refusals_write_nothing_and_their_nexts_run() {
    let mut home = Home::new("adopt_field_to_tag_refused");
    let path = pre_std(&mut home);
    let a = holder(&home, "problem", "a", &["--set", "job_title=Lead"]);
    let b = holder(&home, "problem", "b", &["--set", "origin=x"]);
    home.ok(&["edge", "add", &a, "blocks", &b, "--set", "why=because"]);
    home.ok(&["ontology", "add", "field", "segment", "held_on:date"]);
    let mut decl = tm5_decl("day", "segment", false);
    decl["order"] = json!(["held_on"]);
    let file = home.ops_file(&decl.to_string());
    home.ok(&["graph", "spine", "set", "demo", "--file", &file]);
    holder(&home, "segment", "s", &["--set", "held_on=2026-01-01"]);
    let before = graph_files(&path);
    for (field, reason) in [
        ("job_title", "it is a std field"),
        ("tags", "it is a std field"),
        ("why", "only edges have it"),
        ("nothing", "no node type declares it"),
        ("held_on", "a spine reads it"),
    ] {
        let (env, exit) = home.umx(&["ontology", "adopt-std", "--field-to-tag", field]);
        assert_eq!(exit, 1, "{field}: {}", env.to_json());
        let p = issue(&env, Code::AdoptFieldToTagRefused);
        assert!(p.message().contains(reason), "{field}: {}", p.message());
        let (ran, exit) = run_as_printed(&home, p.next());
        assert_eq!(exit, 0, "{}: {}", p.next(), ran.to_json());
    }
    let (env, exit) = home.umx(&[
        "ontology",
        "adopt-std",
        "--field-to-tag",
        "origin",
        "--field-to-tag",
        "origin",
    ]);
    assert_eq!(exit, Code::UsageInvalid.spec().exit, "{}", env.to_json());
    assert_eq!(graph_files(&path), before);
    home.ok(&["ontology", "adopt-std"]);
    let (env, exit) = home.umx(&[
        "ontology",
        "adopt-std",
        "--map",
        "problem=issue",
        "--field-to-tag",
        "origin",
    ]);
    assert_eq!(exit, Code::UsageInvalid.spec().exit, "{}", env.to_json());
    let p = issue(&env, Code::UsageInvalid);
    assert!(p.message().contains("already uses std"), "{}", p.message());
    let (ran, exit) = run_as_printed(&home, p.next());
    assert_eq!(exit, 0, "{}: {}", p.next(), ran.to_json());
    assert!(node_line(&path, &b).get("tags").is_none(), "nothing moved");
}
