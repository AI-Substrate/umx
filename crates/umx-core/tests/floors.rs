//! ws004 s26: the calibrated floor table in umx-core is the committed calibration record, row for row. The record is
//! docs/project/benchmarks/floor-calibration/floors.json, written by umx-bench's floor-calibration; a smoke run
//! there never feeds the table. Recalibrate (and update both) when a key's model, dims, prompt version or text form
//! changes.

use umx_core::similarity::CALIBRATED;

fn record() -> serde_json::Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/project/benchmarks/floor-calibration/floors.json"
    );
    let text = std::fs::read_to_string(path).expect("floors.json is committed");
    serde_json::from_str(&text).expect("floors.json parses")
}

#[test]
fn the_calibrated_table_equals_floors_json() {
    let record = record();
    let keys = record["keys"].as_object().expect("keys");
    let mut full = Vec::new();
    for (key, e) in keys {
        if e["smoke"] == serde_json::json!(true) {
            continue;
        }
        let model = e["model"].as_str().expect("model");
        let dims = e["dims"].as_u64().expect("dims") as usize;
        let prompt = e["prompt_version"].as_u64().map(|p| p as u32);
        let row = CALIBRATED
            .iter()
            .find(|c| c.model == model && c.dims == dims && c.prompt_version == prompt)
            .unwrap_or_else(|| panic!("{key} is in floors.json but not in CALIBRATED"));
        let number = |k: &str| e[k].as_f64().unwrap_or_else(|| panic!("{key}: {k}")) as f32;
        assert_eq!(row.search, number("search_floor"), "{key} search");
        assert_eq!(row.similar, number("similar_floor"), "{key} similar");
        assert_eq!(Some(row.run), e["run"].as_str(), "{key} run");
        full.push(key.clone());
    }
    assert_eq!(
        full.len(),
        CALIBRATED.len(),
        "every CALIBRATED row needs a full run in floors.json: {full:?}"
    );
}

/// The built-in search docs list every calibrated floor as the table holds it.
#[test]
fn the_search_docs_list_every_calibrated_floor() {
    let docs = include_str!("../../umx-cli/docs/search.md");
    for c in CALIBRATED {
        let key = match c.prompt_version {
            Some(p) => format!("{}@{}+p{p}", c.model, c.dims),
            None => format!("{}@{}", c.model, c.dims),
        };
        let row = format!("| `{key}` | {} | {} |", c.search, c.similar);
        assert!(docs.contains(&row), "search.md lacks {row}");
    }
}

/// PRD 002 search-10: each key's strength cuts in CALIBRATED are its floors.json `strength` record, value for value,
/// measured from the rows of that key's own floors run (never another's), with the provenance a reader needs: the
/// tool, command, umx commit, sets, seeds, the cache file's sha256 and that it was read offline. A key with no record
/// has no cuts, so its hits are never labelled.
#[test]
fn every_strength_cut_is_its_floors_json_record_with_provenance() {
    let record = record();
    for c in CALIBRATED {
        let key = match c.prompt_version {
            Some(p) => format!("{}@{}+p{p}", c.model, c.dims),
            None => format!("{}@{}", c.model, c.dims),
        };
        let st = &record["keys"][&key]["strength"];
        let Some(cuts) = c.strength else {
            // No cuts: no record, or a record whose cuts were withheld (a failed deployment check), kept for evidence.
            assert!(
                st.is_null()
                    || (st["cuts"].is_null()
                        && st["withheld"].is_string()
                        && st["deployment_check"]["pass"] == serde_json::json!(false)),
                "{key}: a strength record with cuts, but none in CALIBRATED"
            );
            continue;
        };
        assert!(
            st["deployment_check"].is_null()
                || st["deployment_check"]["pass"] == serde_json::json!(true),
            "{key}: cuts in CALIBRATED although its deployment check failed"
        );
        assert_eq!(
            st["floors_run"].as_str(),
            Some(c.run),
            "{key}: cuts from another run's rows"
        );
        assert_eq!(st["run"].as_str(), Some(cuts.run), "{key} run");
        let r = &st["cuts"];
        let number = |k: &str| r[k].as_f64().unwrap_or_else(|| panic!("{key}: {k}")) as f32;
        assert_eq!(cuts.strong_z, number("strong_z"), "{key}");
        assert_eq!(cuts.strong_z_per_ln, number("strong_z_per_ln"), "{key}");
        assert_eq!(cuts.strong_score, number("strong_score"), "{key}");
        assert_eq!(cuts.fair_z, number("fair_z"), "{key}");
        assert_eq!(cuts.fair_z_per_ln, number("fair_z_per_ln"), "{key}");
        assert_eq!(cuts.fair_score, number("fair_score"), "{key}");
        assert_eq!(
            Some(cuts.min_population as u64),
            r["min_population"].as_u64(),
            "{key}"
        );
        assert_eq!(Some(cuts.z_part), r["z_part"].as_bool(), "{key}");
        assert_eq!(
            Some(cuts.max_n_measured as u64),
            r["max_n_measured"].as_u64(),
            "{key}"
        );
        assert!(cuts.max_n_measured >= cuts.min_population, "{key}");
        assert!(
            cuts.strong_score >= cuts.fair_score && cuts.strong_z >= cuts.fair_z,
            "{key}"
        );
        assert!(
            cuts.strong_score > c.search,
            "{key}: a strong cut at or below the floor labels noise"
        );
        for k in ["tool", "command", "umx_commit", "run_date"] {
            assert!(st[k].as_str().is_some_and(|v| !v.is_empty()), "{key}: {k}");
        }
        assert_eq!(st["cache"]["offline"], serde_json::json!(true), "{key}");
        assert_eq!(st["cache"]["texts_sent"], serde_json::json!(0), "{key}");
        assert_eq!(
            st["cache"]["sha256"].as_str().map(str::len),
            Some(64),
            "{key}"
        );
        assert!(
            st["sample_seed"].is_u64() && st["bootstrap_seed"].is_u64(),
            "{key}"
        );
        for set in ["scifact", "nfcorpus", "arguana", "fixture", "tight"] {
            assert!(
                st["corpora"][set]["on_topic_queries"].as_u64() > Some(0),
                "{key}: {set}"
            );
        }
        assert!(
            st["off_topic_pairs_kept"]
                .as_array()
                .is_some_and(|k| !k.is_empty()),
            "{key}: the off-topic pairs and their spot checks are recorded"
        );
    }
}
