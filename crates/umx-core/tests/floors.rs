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
