//! The scanner against serde (s05): over hand-written and mutated node and edge lines, whatever the scanner accepts
//! serde accepts with the same values, and the load as a whole gives the codec's records or its exact error.

use crate::Graph;
use crate::json::{self, Num};
use crate::line::{self, FieldVal};
use alloc::borrow::Cow;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use serde_json::{Value, json};
use umx_core::codec;
use umx_core::{Edge, Id, Node};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn id(n: u128) -> Id {
    Id((0x0194_0000_0000_u128 << 80) | n)
}

fn node(n: u128) -> Node {
    Node {
        id: id(n),
        kind: "problem".into(),
        title: "Approval delays".into(),
        status: "open".into(),
        note: String::new(),
        evidence: vec![],
        status_history: vec![],
        created: "2026-11-10T01:00:00Z".into(),
        updated: "2026-11-10T01:00:00Z".into(),
        created_by: "tester".into(),
        locator: None,
        authored_by_session: None,
        edited_by_session: None,
        fields: Default::default(),
    }
}

fn canonical(n: &Node) -> Vec<u8> {
    let mut b = codec::encode_nodes([n]);
    b.pop();
    b
}

/// Node lines to start from: the forms umx writes, with every kind of value, and hand edits.
fn node_seeds() -> Vec<Vec<u8>> {
    let mut seeds = Vec::new();
    let mut a = node(1);
    a.title = "Tab\there \"quoted\" back\\slash \u{1} é ☃ 𝄞 \u{2028}".into();
    a.note = "line one\nline two\r\n/".into();
    a.evidence = vec![
        json!({"source": "t/call-001.ndjson", "seq": [100, 141], "kind": "room", "exact": false}),
        json!({"kind": null, "nested": {"a": [1.5, -0.0, 1e300, {"b": []}]}}),
    ];
    a.status_history = vec![json!({"to": "done", "at": "2026-11-10T02:00:00Z"})];
    for (k, v) in [
        ("likelihood", json!(3)),
        ("weight", json!(1.0)),
        ("neg", json!(-7)),
        ("big", json!(u64::MAX)),
        ("bigger", json!(1.8446744073709552e19)),
        ("small", json!(i64::MIN)),
        ("tiny", json!(-1.5e-7)),
        ("owner", json!("team \"7\" ☃")),
        ("done", json!(true)),
        ("off", json!(false)),
        ("tags", json!(["a", "b"])),
        ("meta", json!({"x": 1})),
        ("nothing", Value::Null),
        ("empty", json!("")),
    ] {
        a.fields.insert(k.into(), v);
    }
    seeds.push(canonical(&a));
    let mut b = node(2);
    b.created = "2026-11-10T01:00:00.250Z".into();
    seeds.push(canonical(&b));
    let t = id(3).to_text();
    for hand in [
        format!(
            r#"  {{ "title" : "Bravo",  "id":"{t}", "type":"risk","status":"open","note":"","evidence":[ ],"status_history":[],"created":"","updated":"later","created_by":"hand", "colour":"red", "weight": 1.0, "count": 3 }} "#
        ),
        format!(
            r#"{{"ti\u0074le":"\u00e9\ud83d\ude00\/","id":"{t}","type":"risk","status":"open","note":"\b\f","evidence":[],"status_history":[],"created":"x","updated":"y","created_by":"h","n":-0,"e":1E+2,"f":0.5e-3}}"#
        ),
        format!(
            "{{\"id\":\"{t}\",\"type\":\"risk\",\"title\":\"T\",\"status\":\"open\",\"note\":\"\",\"evidence\":[],\"status_history\":[],\"created\":\"\",\"updated\":\"\",\"created_by\":\"\"}}\r"
        ),
    ] {
        seeds.push(hand.into_bytes());
    }
    seeds
}

fn edge_seeds() -> Vec<Vec<u8>> {
    let (a, b, e) = (id(1).to_text(), id(2).to_text(), id(10).to_text());
    vec![
        format!(
            r#"{{"id":"{e}","type":"blocks","from":"{a}","to":"{b}","created":"2026-11-10T03:00:00Z","created_by":"tester"}}"#
        )
        .into_bytes(),
        format!(
            r#"{{ "to":"{b}","id":"{e}","type":"part_of","from":"{a}","created":"2026-11-10T03:00:00+00:00","created_by":"h","via":"merge \"x\"","confidence":1.0,"n":[1,{{"k":null}}] }}"#
        )
        .into_bytes(),
    ]
}

const ALPHABET: &[u8] = b"\"\\{}[],:0123456789-+.eEtrufalsn \t\r\x01\x7fu/bfx\xc3\xa9\xff";

fn mutate(rng: &mut Rng, seed: &[u8]) -> Vec<u8> {
    let mut m = seed.to_vec();
    for _ in 0..1 + rng.below(3) {
        if m.is_empty() {
            m.push(b'{');
        }
        let at = rng.below(m.len());
        match rng.below(6) {
            0 => {
                m.remove(at);
            }
            1 => m.insert(at, ALPHABET[rng.below(ALPHABET.len())]),
            2 => m[at] = ALPHABET[rng.below(ALPHABET.len())],
            3 => {
                let end = (at + 1 + rng.below(12)).min(m.len());
                let piece = m[at..end].to_vec();
                let to = rng.below(m.len());
                m.splice(to..to, piece);
            }
            4 => {
                let other = rng.below(m.len());
                m.swap(at, other);
            }
            _ => {
                let end = (at + 1 + rng.below(12)).min(m.len());
                m.drain(at..end);
            }
        }
    }
    m.retain(|&b| b != b'\n');
    m
}

/// The value a column entry stands for, read back as the graph reads it.
fn column_value(line: &[u8], v: FieldVal) -> Option<Value> {
    match v {
        FieldVal::Num(n) => Some(n.value()),
        FieldVal::Bool(b) => Some(Value::Bool(b)),
        FieldVal::Str { rel, len, esc } => {
            let raw = core::str::from_utf8(&line[rel as usize..(rel + len) as usize]).unwrap();
            Some(Value::String(json::text(raw, esc).into_owned()))
        }
        FieldVal::Other => None,
    }
}

/// Checks one node line: if the scanner accepts it, serde does, with the same values everywhere.
fn check_node_line(line: &[u8]) -> bool {
    let mut fields: Vec<(Cow<'_, str>, FieldVal)> = Vec::new();
    let Ok(hot) = line::node_hot(line, &mut fields) else {
        return false;
    };
    let text = String::from_utf8_lossy(line);
    let n = match codec::decode_nodes(line) {
        Ok(d) => d.records.into_iter().next().unwrap(),
        Err(e) => panic!("scanner accepted what serde refuses ({e}): {text}"),
    };
    assert_eq!(
        (
            hot.id,
            &*hot.kind,
            &*hot.title,
            &*hot.status,
            &*hot.created,
            &*hot.updated,
            &*hot.by
        ),
        (
            n.id,
            n.kind.as_str(),
            n.title.as_str(),
            n.status.as_str(),
            n.created.as_str(),
            n.updated.as_str(),
            n.created_by.as_str()
        ),
        "{text}"
    );
    let names: Vec<&str> = fields.iter().map(|(k, _)| &**k).collect();
    assert_eq!(hot.authored, n.authored_by_session, "{text}");
    assert_eq!(hot.edited, n.edited_by_session, "{text}");
    assert_eq!(hot.locator, n.locator, "{text}");
    assert_eq!(
        names,
        n.fields.keys().map(String::as_str).collect::<Vec<_>>(),
        "{text}"
    );
    for (k, v) in &fields {
        let want = &n.fields[&**k];
        match column_value(line, *v) {
            Some(got) => {
                assert_eq!(&got, want, "{k} in {text}");
                assert_eq!(got.to_string(), want.to_string(), "{k} bytes in {text}");
            }
            None => assert!(
                matches!(want, Value::Array(_) | Value::Object(_) | Value::Null),
                "{k} in {text}"
            ),
        }
        assert_eq!(
            line::node_field(line, k).unwrap().as_ref(),
            Some(want),
            "{text}"
        );
    }
    let cold = line::node_cold(line).expect("a line the scanner took");
    assert_eq!(
        (cold.note, cold.evidence, cold.history, cold.fields),
        (n.note, n.evidence, n.status_history, n.fields),
        "{text}"
    );
    true
}

fn check_edge_line(line: &[u8]) -> bool {
    let Ok(e) = line::edge(line) else {
        return false;
    };
    let text = String::from_utf8_lossy(line);
    let want = match codec::decode_edges(line) {
        Ok(d) => d.records.into_iter().next().unwrap(),
        Err(err) => panic!("scanner accepted what serde refuses ({err}): {text}"),
    };
    let got = Edge {
        id: e.id,
        kind: e.kind.into_owned(),
        from_graph: e.from_graph,
        from: e.from,
        to_graph: e.to_graph,
        to: e.to,
        created: e.created.into_owned(),
        created_by: e.by.into_owned(),
        authored_by_session: e.authored,
        edited_by_session: e.edited,
        evidence: e.evidence,
        fields: e.fields,
    };
    assert_eq!(got, want, "{text}");
    for (k, v) in &got.fields {
        assert_eq!(v.to_string(), want.fields[k].to_string(), "{text}");
    }
    true
}

#[test]
fn the_scanner_takes_only_what_serde_takes_and_reads_it_the_same() {
    let mut rng = Rng(5);
    let (mut taken, mut serde_ok, mut total) = (0, 0, 0);
    for seed in node_seeds() {
        assert!(
            check_node_line(&seed),
            "a seed goes through the scanner: {}",
            String::from_utf8_lossy(&seed)
        );
        for _ in 0..4000 {
            let m = mutate(&mut rng, &seed);
            total += 1;
            taken += usize::from(check_node_line(&m));
            serde_ok += usize::from(!json::blank(&m) && codec::decode_nodes(&m).is_ok());
        }
    }
    // The instrument can say yes: some mutants are still records, and the scanner takes nearly all of them.
    assert!(
        serde_ok > total / 20 && taken * 10 >= serde_ok * 9,
        "{taken} taken, {serde_ok} serde, {total}"
    );
    let (mut taken, mut serde_ok) = (0, 0);
    for seed in edge_seeds() {
        assert!(check_edge_line(&seed));
        for _ in 0..4000 {
            let m = mutate(&mut rng, &seed);
            taken += usize::from(check_edge_line(&m));
            serde_ok += usize::from(!json::blank(&m) && codec::decode_edges(&m).is_ok());
        }
    }
    assert!(
        serde_ok > 100 && taken * 10 >= serde_ok * 9,
        "{taken} taken, {serde_ok} serde"
    );
}

/// ws006 S3 (graph-storage-expert): a locator in its pinned place, with both session keys after it, is taken by the
/// scanner with serde's values, a newer umx's extra key included; a locator that is not one is left to serde, which
/// keeps it as a custom field (S241-1).
#[test]
fn the_scanner_takes_a_locator_in_the_full_key_order() {
    let t = id(3).to_text();
    let (a, e) = (id(90).to_text(), id(91).to_text());
    let line = |locator: &str| {
        format!(
            r#"{{"id":"{t}","type":"transcript","title":"Call","status":"open","note":"","evidence":[],"status_history":[],"created":"c","updated":"u","created_by":"t","locator":{locator},"authored_by_session":"{a}","edited_by_session":"{e}","zeta":1}}"#
        )
    };
    let good = line(
        r#"{"kind":"file","format":"transcript_ndjson","source":"t/016.ndjson","hash":"sha256:9f2c","length":1843302,"seq_max":3511}"#,
    );
    assert!(check_node_line(good.as_bytes()), "the scanner took it");
    let mut fields = Vec::new();
    let hot = line::node_hot(good.as_bytes(), &mut fields).unwrap();
    assert_eq!(hot.locator.unwrap().key(), b"file\0t/016.ndjson".to_vec());
    assert_eq!(
        fields.iter().map(|(k, _)| &**k).collect::<Vec<_>>(),
        ["zeta"],
        "the locator is no custom field"
    );
    let newer = line(r#"{"kind":"file","source":"a","later":1}"#);
    assert!(check_node_line(newer.as_bytes()), "the scanner took it");
    for bad in [
        "null",
        r#""t/016.ndjson""#,
        r#"{"kind":"passage","of":"x","selector":"seq=1"}"#,
    ] {
        let l = line(bad);
        let mut fields = Vec::new();
        assert!(line::node_hot(l.as_bytes(), &mut fields).is_err(), "{bad}");
        let n = codec::decode_nodes(l.as_bytes()).unwrap().records.remove(0);
        assert_eq!(
            (n.locator.is_none(), n.has_kept_locator_field()),
            (true, true),
            "{bad}"
        );
    }
}

/// ws006 S5b after ws007 P2: an edge line with both session keys and then its evidence list is taken by the scanner
/// with serde's values. Each key has its own seen-bit; sharing one sent every edited edge with evidence to the slow
/// path.
#[test]
fn the_scanner_takes_an_edited_edge_with_evidence() {
    let (t, f, a, e) = (id(3), id(4), id(90), id(91));
    let line = format!(
        r#"{{"id":"{t}","type":"feeds","from":"{f}","to":"{t}","created":"c","created_by":"t","authored_by_session":"{a}","edited_by_session":"{e}","evidence":[{{"kind":"document","source":"a.md"}}],"zeta":1}}"#
    );
    assert!(check_edge_line(line.as_bytes()), "the scanner took it");
    let hot = line::edge(line.as_bytes()).unwrap();
    assert_eq!((hot.edited, hot.evidence.len()), (Some(e), 1));
}

#[test]
fn surrogates_deep_nesting_and_repeated_keys_go_to_serde() {
    let t = id(3).to_text();
    let line = |extra: &str| {
        format!(
            r#"{{"id":"{t}","type":"r","title":"T","status":"s","note":"","evidence":[],"status_history":[],"created":"","updated":"","created_by":""{extra}}}"#
        )
    };
    let deep = format!(r#","x":{}1{}"#, "[".repeat(120), "]".repeat(120));
    for extra in [
        r#","x":"\ud83d""#,
        r#","x":"\ude00""#,
        r#","x":"\ud83d\u0041""#,
        r#","x":1,"x":2"#,
        r#","title":"again""#,
        deep.as_str(),
    ] {
        let l = line(extra);
        let mut fields = Vec::new();
        assert!(
            line::node_hot(l.as_bytes(), &mut fields).is_err(),
            "{extra}"
        );
        // The load gives serde's answer either way: its record, or its exact error.
        let file = format!("{l}\n").into_bytes();
        match (Graph::load_nodes(file.clone()), codec::decode_nodes(&file)) {
            (Ok((g, _)), Ok(d)) => {
                assert_eq!(g.nodes().next().unwrap().to_node(), d.records[0], "{extra}");
                assert_eq!(g.encode_nodes(), file, "{extra}");
                g.check_rows();
            }
            (Err(a), Err(b)) => assert_eq!(a, b, "{extra}"),
            (a, b) => panic!("{extra}: load {:?} codec {:?}", a.err(), b.err()),
        }
    }
}

/// A whole file of mutated lines: the graph's load against the codec, line numbers in errors included.
#[test]
fn a_mutated_file_loads_as_the_codec_decodes_it() {
    let mut rng = Rng(11);
    let seeds = node_seeds();
    let ids: Vec<Id> = (1..=3).map(id).collect();
    let (mut loaded, mut refused) = (0, 0);
    for round in 0..400 {
        let mut file = Vec::new();
        for i in 0..1 + rng.below(6) {
            let seed = &seeds[rng.below(seeds.len())];
            let mut line = if rng.below(4) == 0 {
                mutate(&mut rng, seed)
            } else {
                seed.clone()
            };
            // Vary the id so files have several records, out of order and repeated.
            if let Ok(mut n) = line::decode_node(&line, 0)
                && (i > 0 || round % 2 == 0)
            {
                n.id = ids[rng.below(ids.len())];
                line = canonical(&n);
            }
            file.extend_from_slice(&line);
            file.push(b'\n');
            if rng.below(8) == 0 {
                file.extend_from_slice(b" \r\n");
            }
        }
        if rng.below(5) == 0 {
            file.pop();
        }
        match (Graph::load_nodes(file.clone()), codec::decode_nodes(&file)) {
            (Ok((g, load)), Ok(mut d)) => {
                let repeats = load.repeats;
                // Out of order exactly when the next write would change the file's bytes (blank lines dropped, lines
                // moved into id order); a missing last newline alone is not.
                let mut written = file.clone();
                if !written.is_empty() && !written.ends_with(b"\n") {
                    written.push(b'\n');
                }
                assert_eq!(load.unsorted.is_some(), g.encode_nodes() != written);
                loaded += 1;
                d.records.sort_by_key(|n| n.id);
                let got: Vec<Node> = g.nodes().map(|n| n.to_node()).collect();
                assert_eq!(got, d.records);
                assert_eq!(repeats, d.repeats);
                assert_eq!(
                    g.kept_line_count(),
                    d.kept.len(),
                    "{}",
                    String::from_utf8_lossy(&file)
                );
                let mut want = codec::decode_nodes(&g.encode_nodes()).unwrap();
                want.records.sort_by_key(|n| n.id);
                assert_eq!(want.records, d.records);
                g.check_rows();
                for n in g.nodes() {
                    for f in n.custom_field_names() {
                        assert_eq!(
                            n.custom_field(f).as_ref(),
                            d.records
                                .iter()
                                .find(|r| r.id == n.id())
                                .unwrap()
                                .fields
                                .get(f)
                        );
                    }
                }
            }
            (Err(a), Err(b)) => {
                refused += 1;
                assert_eq!(a, b);
            }
            (a, b) => panic!(
                "load {:?} codec {:?}: {}",
                a.err(),
                b.err(),
                String::from_utf8_lossy(&file)
            ),
        }
    }
    assert!(loaded > 100 && refused > 20, "{loaded} {refused}");
}

/// Whole edge files against the pre-s05 load (codec's decode, then services' split of the records in use): ids
/// repeated and out of order, ends present or missing, so dangling copies come first, last or both (R1).
#[test]
fn a_shuffled_edge_file_loads_as_the_codec_and_the_old_split_did() {
    let mut rng = Rng(23);
    let nodes = codec::encode_nodes([&node(1), &node(2)]);
    let ends = [id(1), id(2), id(9)];
    let (mut repeated, mut dangled) = (0, 0);
    for _ in 0..1500 {
        let mut file = Vec::new();
        for _ in 0..1 + rng.below(6) {
            let e = Edge {
                id: id(10 + rng.below(3) as u128),
                kind: "blocks".into(),
                from_graph: None,
                from: ends[rng.below(3)],
                to_graph: None,
                to: ends[rng.below(3)],
                created: "2026-11-10T03:00:00Z".into(),
                created_by: format!("a{}", rng.below(9)),
                authored_by_session: None,
                edited_by_session: None,
                evidence: Vec::new(),
                fields: Default::default(),
            };
            file.extend(codec::encode_edges([&e]));
            if rng.below(6) == 0 {
                file.extend_from_slice(b"\n");
            }
        }
        let decoded = codec::decode_edges(&file).unwrap();
        let lines: Vec<&[u8]> = file.split(|&b| b == b'\n').collect();
        let mut kept = decoded.kept.clone();
        let (mut live, mut dangling) = (Vec::new(), Vec::new());
        for (e, no) in decoded.records.iter().zip(&decoded.record_lines) {
            let missing: Vec<(&'static str, Id)> = [("from", e.from), ("to", e.to)]
                .into_iter()
                .filter(|(_, n)| *n == id(9))
                .collect();
            if missing.is_empty() {
                live.push(e.clone());
                continue;
            }
            for (end, n) in missing {
                dangling.push(crate::Dangling {
                    edge: e.id,
                    end,
                    node: n,
                    line: *no,
                });
            }
            kept.push(codec::KeptLine {
                id: e.id,
                line: lines[no - 1].to_vec(),
            });
        }
        repeated += usize::from(!decoded.repeats.is_empty());
        dangled += usize::from(!dangling.is_empty());
        let (mut g, _) = Graph::load_nodes(nodes.clone()).unwrap();
        let got = g.load_edges(&file).unwrap();
        let text = String::from_utf8_lossy(&file);
        assert_eq!(got.repeats, decoded.repeats, "{text}");
        assert_eq!(got.dangling, dangling, "{text}");
        assert_eq!(
            g.encode_edges(),
            codec::encode_edges_keeping(&live, &kept),
            "{text}"
        );
        assert_eq!(g.edge_count(), live.len(), "{text}");
    }
    assert!(repeated > 300 && dangled > 300, "{repeated} {dangled}");
}

#[test]
fn numbers_round_trip_in_their_form() {
    for t in [
        "0",
        "-0",
        "7",
        "-7",
        "1.0",
        "1e5",
        "1E-3",
        "18446744073709551615",
        "18446744073709551616",
        "-9223372036854775808",
        "-9223372036854775809",
        "123456789012345678",
        "1234567890123456789",
        "0.1",
        "2.5e-308",
    ] {
        let want: Value = t.parse().unwrap();
        let n = Num::parse(t);
        assert_eq!(n.value(), want, "{t}");
        assert_eq!(n.value().to_string(), want.to_string(), "{t}");
        assert_eq!(n.as_f64(), want.as_f64().unwrap(), "{t}");
    }
}
