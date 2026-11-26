//! ws008 t2b (req-117): std tags through the CLI: node add `--tag`, node edit `--tag`/`--untag`, batch `tags`, `tag`
//! and `untag`, `ontology add tag`, and node list `--tag`/`--no-tag`. A module of the cli tests (it uses their `Home`
//! and helpers), kept in its own file.

use super::*;

/// Makes graph `plain` at `<home>/p` without std, the daemon already running; returns the folder.
fn plain(home: &Home) -> String {
    let path = home.path.join("p").display().to_string();
    home.ok(&["graph", "new", &path, "--alias", "plain", "--no-std"]);
    path
}

/// Adds a `risk` with these `--tag` values to `demo` and returns its id.
fn tagged(home: &Home, title: &str, tags: &[&str]) -> String {
    let mut args = vec!["node", "add", "--type", "risk", "--title", title];
    for t in tags {
        args.extend(["--tag", t]);
    }
    let env = home.ok(&args);
    env.data["node"]["id"].as_str().unwrap().to_owned()
}

/// The node line of `id` in `<path>/nodes.jsonl`.
fn line_of(path: &str, id: &str) -> Value {
    let text = fs::read_to_string(Path::new(path).join("nodes.jsonl")).unwrap();
    let line = text.lines().find(|l| l.contains(id)).unwrap();
    umx_protocol::parse_json(line).unwrap()
}

/// The ids a list reply returns, in order.
fn ids(env: &Envelope) -> Vec<String> {
    env.data["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_owned())
        .collect()
}

/// Rewrites `<path>/nodes.jsonl` with `from` replaced by `to` while the graph is unloaded, then loads it again.
fn hand_edit(home: &Home, path: &str, from: &str, to: &str) -> (Envelope, u8) {
    home.ok(&["graph", "unload", "demo"]);
    let file = Path::new(path).join("nodes.jsonl");
    let text = fs::read_to_string(&file).unwrap();
    assert!(text.contains(from), "{from} in {text}");
    fs::write(&file, text.replace(from, to)).unwrap();
    home.umx(&["graph", "load", path])
}

pub(super) const TAG_CODES: [Code; 7] = [
    Code::TagInvalid,
    Code::TagLimit,
    Code::TagNeedsStd,
    Code::TagOffVocabulary,
    Code::TagNotCanonical,
    Code::TagUnusable,
    Code::TagFilterNotStd,
];

/// [`trigger`] for the tag codes, in `home`, which the caller keeps (to run the next as printed).
pub(super) fn tags_trigger(home: &mut Home, code: Code) -> (Envelope, u8) {
    match code {
        Code::TagInvalid => {
            home.graph();
            home.umx(&[
                "node",
                "add",
                "--type",
                "risk",
                "--title",
                "x",
                "--tag",
                "origin:Double-Diamond",
            ])
        }
        Code::TagLimit => {
            home.graph();
            let many: Vec<String> = (0..65).map(|i| format!("t{i}")).collect();
            home.umx(&[
                "node",
                "add",
                "--type",
                "risk",
                "--title",
                "x",
                "--tag",
                &many.join(","),
            ])
        }
        Code::TagNeedsStd => {
            home.start();
            let path = home.path.join("g").display().to_string();
            home.ok(&["graph", "new", &path, "--alias", "demo", "--no-std"]);
            home.umx(&[
                "node", "add", "--type", "risk", "--title", "x", "--tag", "a",
            ])
        }
        Code::TagOffVocabulary => {
            home.graph();
            home.ok(&["ontology", "add", "tag", "origin:double_diamond"]);
            home.umx(&[
                "node",
                "add",
                "--type",
                "risk",
                "--title",
                "x",
                "--tag",
                "origin:other",
            ])
        }
        Code::TagNotCanonical | Code::TagUnusable => {
            let path = home.graph();
            tagged(home, "x", &["a"]);
            let to = if code == Code::TagNotCanonical {
                r#""tags":["B","a"]"#
            } else {
                r#""tags":["a",3]"#
            };
            hand_edit(home, &path, r#""tags":["a"]"#, to)
        }
        Code::TagFilterNotStd => {
            home.graph();
            plain(home);
            home.umx(&["node", "list", "--graph", "demo,plain", "--tag", "a"])
        }
        _ => unreachable!("not a tag code"),
    }
}

/// ws008 t2b (D69): each tag code is raised with its slots filled and the status its kind gives, and its next runs
/// as printed (a `<title>` placeholder filled in, as a caller would).
#[test]
fn each_tag_code_is_raised_and_its_next_runs_as_printed() {
    for code in TAG_CODES {
        let spec = code.spec();
        let mut home = Home::new(&format!("{}-next", spec.code));
        let (env, exit) = tags_trigger(&mut home, code);
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
        let next = found.next().replace("<title>", "fixed");
        let (ran, next_exit) = run_as_printed(&home, &next);
        assert_eq!(next_exit, 0, "{}: {next} gave {}", spec.code, ran.to_json());
    }
}

/// req-117: a tag that is not snake_case is refused with tag_invalid, its next the same add with the snake_case
/// tag, and nothing is written; the next then stores it.
#[test]
fn a_tag_that_is_not_snake_case_is_refused_with_the_snake_case_form_in_next() {
    let mut home = Home::new("tag_invalid");
    let path = home.graph();
    let (env, exit) = home.umx(&[
        "node",
        "add",
        "--type",
        "risk",
        "--title",
        "x",
        "--tag",
        "origin:Double-Diamond",
    ]);
    assert_eq!(exit, 1, "{}", env.to_json());
    let found = issue(&env, Code::TagInvalid);
    assert!(
        found.next().contains("--tag origin:double_diamond"),
        "{}",
        found.next()
    );
    assert!(!env.to_json().contains("Double-Diamond"));
    assert_eq!(
        fs::read_to_string(Path::new(&path).join("nodes.jsonl")).unwrap(),
        ""
    );
    let (ran, exit) = run_as_printed(&home, &found.next().replace("<title>", "x"));
    assert_eq!(exit, 0, "{}", ran.to_json());
    assert_eq!(ran.data["node"]["tags"], json!(["origin:double_diamond"]));
}

/// req-117: tags are stored lowercased, deduplicated and sorted, so `--tag b --tag a --tag a` stores `[a, b]`; a
/// comma list is several tags, and `--set tags=` takes the same path.
#[test]
fn tags_are_stored_lowercased_deduplicated_and_sorted() {
    let mut home = Home::new("tag_order");
    let path = home.graph();
    home.ok(&["rule", "off", "std:risk_owned"]);
    let id = tagged(&home, "x", &["b", "a", "a"]);
    assert_eq!(line_of(&path, &id)["tags"], json!(["a", "b"]));
    let id = tagged(&home, "y", &["Needs_Review,origin:X", "needs_review"]);
    assert_eq!(
        line_of(&path, &id)["tags"],
        json!(["needs_review", "origin:x"])
    );
    let env = home.ok(&[
        "node", "add", "--type", "risk", "--title", "z", "--set", "tags=B,a",
    ]);
    let id = env.data["node"]["id"].as_str().unwrap();
    assert_eq!(line_of(&path, id)["tags"], json!(["a", "b"]));
    assert!(env.warnings.is_empty(), "{}", env.to_json());
    let (env, exit) = home.umx(&[
        "node", "add", "--type", "risk", "--title", "w", "--set", "tags=a", "--tag", "b",
    ]);
    assert_eq!(exit, Code::UsageInvalid.spec().exit);
    issue(&env, Code::UsageInvalid);
}

/// req-117: node list `--tag a --tag b` is AND, `--tag x,y` OR, `--tag 'facet:*'` a facet and `--no-tag x` leaves
/// out; they combine with `--type`.
#[test]
fn node_list_filters_by_tags() {
    let mut home = Home::new("tag_list");
    home.graph();
    let n1 = tagged(&home, "n1", &["origin:double_diamond", "needs_review"]);
    let n2 = tagged(&home, "n2", &["origin:double_diamond"]);
    let n3 = tagged(&home, "n3", &["needs_review", "x"]);
    let n4 = tagged(&home, "n4", &["y"]);
    let n5 = tagged(&home, "n5", &[]);
    let n6 = home.add("problem", "n6");
    home.ok(&["node", "edit", &n6, "--tag", "x"]);
    let list = |args: &[&str]| {
        let mut all = vec!["node", "list"];
        all.extend(args);
        ids(&home.ok(&all))
    };
    assert_eq!(
        list(&["--tag", "origin:double_diamond", "--tag", "needs_review"]),
        std::slice::from_ref(&n1)
    );
    assert_eq!(
        list(&["--tag", "x,y"]),
        [n3.clone(), n4.clone(), n6.clone()]
    );
    assert_eq!(
        list(&["--tag", "x,y", "--type", "risk"]),
        [n3.clone(), n4.clone()]
    );
    assert_eq!(list(&["--tag", "Origin:*"]), [n1.clone(), n2.clone()]);
    assert_eq!(
        list(&["--no-tag", "x"]),
        [n1.clone(), n2.clone(), n4.clone(), n5.clone()]
    );
    assert_eq!(
        list(&["--tag", "origin:*", "--no-tag", "needs_review"]),
        [n2]
    );
    let (env, exit) = home.umx(&["node", "list", "--tag", "a-b"]);
    assert_eq!(exit, 1);
    assert!(issue(&env, Code::TagInvalid).next().contains("--tag a_b"));
}

/// req-117: once a facet has a declared tag, an undeclared tag in it warns tag_off_vocabulary and is written; a
/// declared one, `<facet>:*`, a tag in another facet and a tag with no facet never warn.
#[test]
fn a_declared_facet_warns_on_an_undeclared_tag() {
    let mut home = Home::new("tag_vocab");
    let path = home.graph();
    home.ok(&["rule", "off", "std:risk_owned"]);
    let env = home.ok(&[
        "node",
        "add",
        "--type",
        "risk",
        "--title",
        "free",
        "--tag",
        "origin:any",
    ]);
    assert!(
        env.warnings.is_empty(),
        "free-form until declared: {}",
        env.to_json()
    );
    home.ok(&["ontology", "add", "tag", "origin:double_diamond"]);
    let again = home.ok(&["ontology", "add", "tag", "Origin:double_diamond"]);
    assert_eq!(again.data["unchanged"], true);
    assert_eq!(
        graph_json(&path)["ontology"]["tags"],
        json!(["origin:double_diamond"])
    );
    let env = home.ok(&[
        "node",
        "add",
        "--type",
        "risk",
        "--title",
        "a",
        "--tag",
        "origin:other",
    ]);
    let found = issue(&env, Code::TagOffVocabulary);
    assert_eq!(
        found.next(),
        "umx ontology add tag origin:other --graph demo"
    );
    let id = env.data["node"]["id"].as_str().unwrap();
    assert_eq!(line_of(&path, id)["tags"], json!(["origin:other"]));
    for ok in [
        ["origin:double_diamond", "needs_review"],
        ["phase:x", "loose"],
    ] {
        let env = home.ok(&[
            "node", "add", "--type", "risk", "--title", "b", "--tag", ok[0], "--tag", ok[1],
        ]);
        assert!(env.warnings.is_empty(), "{}", env.to_json());
    }
    home.ok(&["ontology", "add", "tag", "phase:*"]);
    home.ok(&["ontology", "add", "tag", "phase:one"]);
    let env = home.ok(&[
        "node",
        "add",
        "--type",
        "risk",
        "--title",
        "c",
        "--tag",
        "phase:two",
    ]);
    assert!(
        env.warnings.is_empty(),
        "phase:* declares every value: {}",
        env.to_json()
    );
    let shown = home.ok(&["ontology", "show"]);
    assert_eq!(
        shown.data["tags"],
        json!(["origin:double_diamond", "phase:*", "phase:one"])
    );
    let (env, exit) = home.umx(&["ontology", "add", "tag", "needs_review"]);
    assert_eq!(exit, 1);
    assert_eq!(
        issue(&env, Code::TagInvalid).next(),
        "umx docs get ontology"
    );
    let (env, exit) = home.umx(&["ontology", "add", "tag", "Bad-F:x"]);
    assert_eq!(exit, 1);
    let next = issue(&env, Code::TagInvalid).next().to_owned();
    assert_eq!(next, "umx ontology add tag bad_f:x");
    let (ran, exit) = run_as_printed(&home, &next);
    assert_eq!(exit, 0, "{}", ran.to_json());
    assert_eq!(ran.data["name"], "bad_f:x");
}

/// req-117: node edit `--tag` adds and `--untag` removes (a tag or `<facet>:*`), warning only on a tag the edit
/// added; removing the last tag drops the field.
#[test]
fn node_edit_adds_and_removes_tags() {
    let mut home = Home::new("tag_edit");
    let path = home.graph();
    home.ok(&["rule", "off", "std:risk_owned"]);
    home.ok(&["ontology", "add", "tag", "origin:a"]);
    let id = tagged(&home, "x", &["b"]);
    let env = home.ok(&[
        "node",
        "edit",
        &id,
        "--tag",
        "origin:a",
        "--tag",
        "A,origin:b",
    ]);
    assert_eq!(
        line_of(&path, &id)["tags"],
        json!(["a", "b", "origin:a", "origin:b"])
    );
    assert_eq!(env.warnings.len(), 1, "{}", env.to_json());
    issue(&env, Code::TagOffVocabulary);
    let env = home.ok(&["node", "edit", &id, "--untag", "origin:*", "--untag", "a"]);
    assert_eq!(env.data["changed"], json!(["tags"]));
    assert!(env.warnings.is_empty(), "{}", env.to_json());
    assert_eq!(line_of(&path, &id)["tags"], json!(["b"]));
    let env = home.ok(&["node", "edit", &id, "--untag", "zzz"]);
    assert_eq!(env.data["changed"], json!([]));
    home.ok(&["node", "edit", &id, "--untag", "b"]);
    assert!(line_of(&path, &id).get("tags").is_none());
    let (env, exit) = home.umx(&["node", "edit", &id, "--untag", "B-C"]);
    assert_eq!(exit, 1);
    assert_eq!(
        issue(&env, Code::TagInvalid).next(),
        format!("umx node edit umx:demo/{id} --untag b_c")
    );
}

/// req-117: a hand-edited tags value loads with a warning, is found by its stored form, and the node's next write
/// stores it so, keeping what umx cannot read.
#[test]
fn hand_edited_tags_are_found_as_stored_and_canonical_after_the_next_write() {
    let mut home = Home::new("tag_hand");
    let path = home.graph();
    let id = tagged(&home, "x", &["a"]);
    let (env, exit) = hand_edit(&home, &path, r#""tags":["a"]"#, r#""tags":["B","a","a",3]"#);
    assert_eq!(exit, 0, "{}", env.to_json());
    issue(&env, Code::TagNotCanonical);
    issue(&env, Code::TagUnusable);
    assert_eq!(
        ids(&home.ok(&["node", "list", "--tag", "b"])),
        std::slice::from_ref(&id)
    );
    home.ok(&["node", "edit", &id, "--title", "y"]);
    assert_eq!(line_of(&path, &id)["tags"], json!(["a", "b", 3]));
}

/// req-117: in a graph without std `tags` stays an ordinary custom field: nothing normalises it, and every tag verb
/// and filter is refused with tag_needs_std; a list over a std and a plain graph skips the plain one and says so.
#[test]
fn a_graph_without_std_keeps_tags_an_ordinary_field() {
    let mut home = Home::new("tag_plain");
    home.graph();
    let path = plain(&home);
    let env = home.ok(&[
        "node",
        "add",
        "--graph",
        "plain",
        "--type",
        "risk",
        "--title",
        "x",
        "--set-json",
        r#"tags=["B","a","a"]"#,
    ]);
    let id = env.data["node"]["id"].as_str().unwrap().to_owned();
    assert_eq!(line_of(&path, &id)["tags"], json!(["B", "a", "a"]));
    home.ok(&["node", "edit", &id, "--graph", "plain", "--title", "y"]);
    assert_eq!(line_of(&path, &id)["tags"], json!(["B", "a", "a"]));
    assert!(
        !graph_json(&path)["ontology"]
            .as_object()
            .unwrap()
            .contains_key("tags")
    );
    for args in [
        vec![
            "node", "add", "--type", "risk", "--title", "x", "--tag", "a",
        ],
        vec!["node", "edit", &id, "--tag", "a"],
        vec!["node", "edit", &id, "--untag", "a"],
        vec!["ontology", "add", "tag", "a:b"],
        vec!["node", "list", "--tag", "a"],
        vec!["node", "list", "--no-tag", "a"],
    ] {
        let mut args = args;
        args.extend(["--graph", "plain"]);
        let (env, exit) = home.umx(&args);
        assert_eq!(exit, 1, "{args:?}: {}", env.to_json());
        issue(&env, Code::TagNeedsStd);
    }
    let env = home.ok(&[
        "node", "add", "--graph", "demo", "--type", "risk", "--title", "std", "--tag", "a",
    ]);
    let a = env.data["node"]["id"].as_str().unwrap().to_owned();
    let env = home.ok(&["node", "list", "--graph", "demo,plain", "--tag", "a"]);
    assert_eq!(ids(&env), [a]);
    assert_eq!(
        issue(&env, Code::TagFilterNotStd).message(),
        "The tag filter skipped 1 of the graphs read: they do not use std, so they have no std tags."
    );
}

/// req-117: batch `node add` lines take `tags` and `node edit` lines `tag` and `untag`, checked as the verbs are; a
/// bad tag fails the whole batch and an off-vocabulary tag is one of its warnings.
#[test]
fn batch_lines_take_tags_tag_and_untag() {
    let mut home = Home::new("tag_batch");
    let path = home.graph();
    home.ok(&["ontology", "add", "tag", "origin:a"]);
    let id = tagged(&home, "old", &["x", "y"]);
    let ops = home.ops_file(&format!(
        "{}\n{}\n",
        r#"{"op":"node add","ref":"n","type":"risk","title":"new","tags":["b","A","origin:z"]}"#,
        format_args!(r#"{{"op":"node edit","node":"{id}","tag":["origin:a"],"untag":["x"]}}"#),
    ));
    let env = home.ok(&["batch", "--file", &ops]);
    issue(&env, Code::TagOffVocabulary);
    let new = env.data["refs"]["n"].as_str().unwrap();
    assert_eq!(line_of(&path, new)["tags"], json!(["a", "b", "origin:z"]));
    assert_eq!(line_of(&path, &id)["tags"], json!(["origin:a", "y"]));
    let before = fs::read_to_string(Path::new(&path).join("nodes.jsonl")).unwrap();
    let bad = home.ops_file(&format!(
        "{}\n{}\n",
        r#"{"op":"node add","type":"risk","title":"ok","tags":["fine"]}"#,
        format_args!(r#"{{"op":"node edit","node":"{id}","tag":["Not OK!"]}}"#),
    ));
    let (env, exit) = home.umx(&["batch", "--file", &bad]);
    assert_ne!(exit, 0, "{}", env.to_json());
    assert!(env.to_json().contains("tag_invalid"), "{}", env.to_json());
    assert_eq!(
        fs::read_to_string(Path::new(&path).join("nodes.jsonl")).unwrap(),
        before
    );
}

/// req-117: a graph that adopts std gets its tag index in the same call: tags written before, as an ordinary field,
/// are found by their stored form at once, and the next write of the node stores them so.
#[test]
fn adopting_std_turns_the_tag_index_on() {
    let mut home = Home::new("tag_adopt");
    home.start();
    let path = plain(&home);
    let env = home.ok(&[
        "node",
        "add",
        "--type",
        "risk",
        "--title",
        "x",
        "--set-json",
        r#"tags=["B","a"]"#,
    ]);
    let id = env.data["node"]["id"].as_str().unwrap().to_owned();
    home.ok(&["ontology", "adopt-std"]);
    assert_eq!(
        ids(&home.ok(&["node", "list", "--tag", "b"])),
        std::slice::from_ref(&id)
    );
    home.ok(&["node", "edit", &id, "--tag", "c"]);
    assert_eq!(line_of(&path, &id)["tags"], json!(["a", "b", "c"]));
}

/// A fake AWS access key id, built at run time so no credential-shaped text is in the source.
fn token() -> String {
    format!("AKIA{}", "Q7Z3X9K2Q7Z3X9K2")
}

/// Security B1: a tag is stored lowercased, so the scan reads it upper cased too. A write warns naming field `tags`
/// and the rule, never the value; a batch refuses; node get and node list flag the field; a hand-edited lowercase
/// key is flagged on read. A plain tag is the control.
#[test]
fn a_secret_shaped_tag_is_found_though_tags_are_lowercased() {
    let mut home = Home::new("tag_secret");
    let path = home.graph();
    let key = token();
    let lower = key.to_ascii_lowercase();
    let plain = tagged(&home, "plain", &["plain"]);
    let env = home.ok(&[
        "node", "add", "--type", "risk", "--title", "x", "--tag", &key,
    ]);
    let id = env.data["node"]["id"].as_str().unwrap().to_owned();
    let found = issue(&env, Code::SecretSuspected);
    assert!(
        found.message().contains("field tags"),
        "{}",
        found.message()
    );
    assert!(found.message().contains("aws"), "{}", found.message());
    assert!(!env.to_json().contains(&key) && !found.message().contains(&lower));
    let got = home.ok(&["node", "get", &id]);
    assert_eq!(got.data["node"]["secret_suspected"], json!(["field:tags"]));
    assert!(
        home.ok(&["node", "get", &plain]).data["node"]
            .get("secret_suspected")
            .is_none()
    );
    let listed = home.ok(&["node", "list", "--columns", "id,tags"]);
    let rows = listed.data["rows"].as_array().unwrap();
    let row = |n: &str| rows.iter().find(|r| r["id"] == n).unwrap().clone();
    assert_eq!(row(&id)["secret_suspected"], json!(["field:tags"]));
    assert!(row(&plain).get("secret_suspected").is_none());
    let before = fs::read_to_string(Path::new(&path).join("nodes.jsonl")).unwrap();
    let ops = home.ops_file(&format!(
        "{{\"op\":\"node add\",\"type\":\"risk\",\"title\":\"b\",\"tags\":[\"{lower}\"]}}\n"
    ));
    let (env, exit) = home.umx(&["batch", "--file", &ops]);
    assert_ne!(exit, 0, "{}", env.to_json());
    assert!(!env.to_json().contains(&lower), "{}", env.to_json());
    assert_eq!(
        fs::read_to_string(Path::new(&path).join("nodes.jsonl")).unwrap(),
        before
    );
    let hand = tagged(&home, "hand", &["swap_me"]);
    let (env, exit) = hand_edit(&home, &path, "\"swap_me\"", &format!("\"{lower}\""));
    assert_eq!(exit, 0, "{}", env.to_json());
    let got = home.ok(&["node", "get", &hand]);
    assert_eq!(got.data["node"]["secret_suspected"], json!(["field:tags"]));
}

/// Security N1: a secret-shaped tag that is refused never has its derived text echoed; the next is the docs.
#[test]
fn a_refused_secret_shaped_tag_gives_the_docs_as_next() {
    let mut home = Home::new("tag_secret_next");
    home.graph();
    let bad = format!("{}-x", token());
    let (env, exit) = home.umx(&[
        "node", "add", "--type", "risk", "--title", "x", "--tag", &bad,
    ]);
    assert_eq!(exit, 1, "{}", env.to_json());
    assert_eq!(
        issue(&env, Code::TagInvalid).next(),
        "umx docs get ontology"
    );
    assert!(
        !env.to_json().to_ascii_lowercase().contains("q7z3"),
        "{}",
        env.to_json()
    );
}

/// Junglefowl F2: tag_invalid's next repeats the whole command, the graph first, every tag kept and the refused one
/// in snake_case form; each runs as printed.
#[test]
fn tag_invalid_next_repeats_every_tag() {
    let mut home = Home::new("tag_retry");
    home.graph();
    let (env, _) = home.umx(&[
        "node", "add", "--graph", "demo", "--type", "risk", "--title", "x", "--tag", "a,B",
        "--tag", "Bad-One", "--tag", "c",
    ]);
    let next = issue(&env, Code::TagInvalid).next().to_owned();
    assert_eq!(
        next,
        "umx node add --type risk --title <title> --graph demo --tag a,b --tag bad_one --tag c"
    );
    let (ran, exit) = run_as_printed(&home, &next.replace("<title>", "x"));
    assert_eq!(exit, 0, "{}", ran.to_json());
    assert_eq!(ran.data["node"]["tags"], json!(["a", "b", "bad_one", "c"]));
    let (env, _) = home.umx(&[
        "node", "list", "--graph", "demo", "--type", "risk", "--tag", "a", "--tag", "x:Y-z,c",
        "--no-tag", "origin:*",
    ]);
    let next = issue(&env, Code::TagInvalid).next().to_owned();
    assert_eq!(
        next,
        "umx node list --graph demo --type risk --tag a --tag x:y_z,c --no-tag 'origin:*'"
    );
    let (ran, exit) = run_as_printed(&home, &next);
    assert_eq!(exit, 0, "{}", ran.to_json());
    // Junglefowl F6: a retry past the length a next keeps as written gives the docs instead.
    let long: Vec<String> = (0..10)
        .map(|i| format!("t{i}_{}", "a".repeat(19)))
        .collect();
    let mut args = vec![
        "node", "add", "--graph", "demo", "--type", "risk", "--title", "x",
    ];
    for t in &long {
        args.extend(["--tag", t.as_str()]);
    }
    args.extend(["--tag", "Bad-One"]);
    let (env, _) = home.umx(&args);
    let next = issue(&env, Code::TagInvalid).next().to_owned();
    assert_eq!(next, "umx docs get ontology");
    let (ran, exit) = run_as_printed(&home, &next);
    assert_eq!(exit, 0, "{}", ran.to_json());
}

/// Junglefowl F4: a refusal's reason reads as plain words.
#[test]
fn tag_invalid_says_why_in_plain_words() {
    let mut home = Home::new("tag_reason");
    home.graph();
    let (env, _) = home.umx(&[
        "node", "add", "--type", "risk", "--title", "x", "--tag", "A-B",
    ]);
    assert_eq!(
        issue(&env, Code::TagInvalid).message(),
        "A tag was refused: each part of a tag must be a lowercase letter then up to 63 lowercase letters digits \
         or underscores. Nothing changed."
    );
    let (env, _) = home.umx(&[
        "node", "add", "--type", "risk", "--title", "x", "--tag", "a:b:c",
    ]);
    assert_eq!(
        issue(&env, Code::TagInvalid).message(),
        "A tag was refused: a tag has at most one facet. Nothing changed."
    );
    for args in [
        vec!["node", "add", "--type", "risk", "--title", "x", "--tag", ""],
        vec!["node", "list", "--tag", ""],
        vec!["node", "list", "--no-tag", " , "],
    ] {
        let (env, exit) = home.umx(&args);
        assert_eq!(exit, 1, "{args:?}");
        assert_eq!(
            issue(&env, Code::TagInvalid).message(),
            "A tag was refused: a tag is empty. Nothing changed.",
            "{args:?}"
        );
    }
}

/// Junglefowl F3: a hand-edited value past 64 tags keeps the tags the index leaves out through an edit.
#[test]
fn an_edit_keeps_hand_edited_tags_past_64() {
    let mut home = Home::new("tag_past_64");
    let path = home.graph();
    let id = tagged(&home, "x", &["swap_me"]);
    let many: Vec<String> = (0..66).map(|i| format!("\"t{i:02}\"")).collect();
    let (env, exit) = hand_edit(
        &home,
        &path,
        "[\"swap_me\"]",
        &format!("[{}]", many.join(",")),
    );
    assert_eq!(exit, 0, "{}", env.to_json());
    issue(&env, Code::TagUnusable);
    home.ok(&["node", "edit", &id, "--untag", "t00", "--untag", "t65"]);
    let tags = line_of(&path, &id)["tags"].as_array().unwrap().clone();
    assert_eq!(tags.len(), 64);
    assert!(!tags.contains(&json!("t00")) && !tags.contains(&json!("t65")));
    assert!(tags.contains(&json!("t64")));
}

/// Junglefowl N2: an on_key resolve that keeps a node's other tags says to add them with `node edit --tag`.
#[test]
fn a_resolve_holding_other_tags_names_node_edit_tag() {
    let mut home = Home::new("tag_on_key");
    home.graph();
    let add = |tag: &str| {
        home.umx(&[
            "node",
            "add",
            "--type",
            "person",
            "--title",
            "Ann",
            "--set",
            "entity_key=email:ann@example.test",
            "--tag",
            tag,
            "--on-key",
            "resolve",
        ])
    };
    let (first, _) = add("a");
    let id = first.data["node"]["id"].as_str().unwrap().to_owned();
    let (env, exit) = add("b");
    assert_eq!(exit, 0, "{}", env.to_json());
    let next = issue(&env, Code::OnKeyFieldHeld).next().to_owned();
    assert_eq!(next, format!("umx node edit umx:demo/{id} --tag <tag>"));
    let (ran, exit) = run_as_printed(&home, &next.replace("<tag>", "b"));
    assert_eq!(exit, 0, "{}", ran.to_json());
}

/// Security N2: a saved scope that names a secured std graph lists its tagged nodes (naming it counts); without it the
/// secured graph is not read.
#[test]
fn a_scope_naming_a_secured_graph_filters_its_tags() {
    let mut home = Home::new("tag_secured");
    home.secured_graph();
    let path = home.path.join("o").display().to_string();
    home.ok(&["graph", "new", &path, "--alias", "open"]);
    let add = |g: &str| {
        let env = home.ok(&[
            "node", "add", "--graph", g, "--type", "risk", "--title", "x", "--tag", "a",
        ]);
        env.data["node"]["id"].as_str().unwrap().to_owned()
    };
    let (s, o) = (add("demo"), add("open"));
    home.ok(&["scope", "save", "both", "--graph", "demo,open"]);
    let mut got = ids(&home.ok(&["node", "list", "--scope", "both", "--tag", "a"]));
    got.sort();
    let mut want = vec![s, o.clone()];
    want.sort();
    assert_eq!(got, want);
    assert_eq!(
        ids(&home.ok(&["node", "list", "--graph", "open", "--tag", "a"])),
        [o]
    );
}

/// Junglefowl T1 to T6: `--set tags` on an edit, a scope skipping two plain graphs, tag_limit on an edit, graph.json
/// tags sorted, `--tag` with `--where`, and an edit keeping unusable entries while a tags value that is no list warns.
#[test]
fn edits_scopes_limits_and_stored_shapes() {
    let mut home = Home::new("tag_more");
    let path = home.graph();
    home.ok(&["ontology", "add", "tag", "z:a"]);
    home.ok(&["ontology", "add", "tag", "b:x"]);
    assert_eq!(graph_json(&path)["ontology"]["tags"], json!(["b:x", "z:a"]));
    let id = tagged(&home, "x", &["a"]);
    let env = home.ok(&["node", "edit", &id, "--set", "tags=Z:b,c"]);
    issue(&env, Code::TagOffVocabulary);
    assert_eq!(line_of(&path, &id)["tags"], json!(["c", "z:b"]));
    let (env, exit) = home.umx(&["node", "edit", &id, "--set", "tags=c", "--tag", "d"]);
    assert_eq!(exit, Code::UsageInvalid.spec().exit, "{}", env.to_json());
    let full: Vec<String> = (0..64).map(|i| format!("t{i}")).collect();
    let big = tagged(&home, "big", &[&full.join(",")]);
    let (env, exit) = home.umx(&["node", "edit", &big, "--tag", "one_more"]);
    assert_eq!(exit, 1, "{}", env.to_json());
    issue(&env, Code::TagLimit);
    let w = home.ok(&[
        "node", "add", "--type", "risk", "--title", "w", "--set", "cost=3", "--tag", "c",
    ]);
    let w = w.data["node"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        ids(&home.ok(&["node", "list", "--tag", "c", "--where", "cost>=3"])),
        std::slice::from_ref(&w)
    );
    let keep = tagged(&home, "keep", &["swap_me"]);
    let (env, _) = hand_edit(&home, &path, "[\"swap_me\"]", "[\"a\",3]");
    issue(&env, Code::TagUnusable);
    home.ok(&["node", "edit", &keep, "--tag", "b"]);
    assert_eq!(line_of(&path, &keep)["tags"], json!(["a", "b", 3]));
    let (env, _) = hand_edit(&home, &path, "[\"a\",\"b\",3]", "\"a\"");
    issue(&env, Code::TagUnusable);
    for alias in ["p1", "p2"] {
        let p = home.path.join(alias).display().to_string();
        home.ok(&["graph", "new", &p, "--alias", alias, "--no-std"]);
    }
    home.ok(&["scope", "save", "three", "--graph", "demo,p1,p2"]);
    let env = home.ok(&["node", "list", "--scope", "three", "--tag", "c"]);
    assert_eq!(
        issue(&env, Code::TagFilterNotStd).message(),
        "The tag filter skipped 2 of the graphs read: they do not use std, so they have no std tags."
    );
}
