//! Row comparison for the Cypher conformance fixture, shared by umx's side (crates/umx-cypher/tests/design.rs)
//! and LadybugDB's (tools/lbug-conformance, through `#[path]`). The engines are the two origins; this is the
//! one ruler both are measured with.
//!
//! Cells are text. A cell is a float when it parses as f64 and is written with a `.`, an exponent, `inf` or
//! `NaN`; two float cells match within `FLOAT_TOLERANCE` (cypher-expert design input section 1.1: "Compare
//! floats to 1e-9"). Every other cell, including an integer against a float, must match exactly. Engines must
//! print floats at full precision (Rust's `{:?}`), never rounded.

/// Absolute tolerance for float cells.
pub const FLOAT_TOLERANCE: f64 = 1e-9;

fn float(cell: &str) -> Option<f64> {
    let written_as_float =
        cell.contains(['.', 'e', 'E']) || cell.contains("inf") || cell.contains("NaN");
    written_as_float.then(|| cell.parse::<f64>().ok()).flatten()
}

/// One cell against the expected cell.
pub fn cell_matches(got: &str, want: &str) -> bool {
    match (float(got), float(want)) {
        (Some(g), Some(w)) => (g - w).abs() <= FLOAT_TOLERANCE,
        _ => got == want,
    }
}

/// Rows against the expected rows, in order, cell by cell.
pub fn rows_match(got: &[Vec<String>], want: &[Vec<String>]) -> bool {
    got.len() == want.len()
        && got
            .iter()
            .zip(want)
            .all(|(g, w)| g.len() == w.len() && g.iter().zip(w).all(|(a, b)| cell_matches(a, b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_match_within_the_tolerance_and_no_further() {
        // a02's mean, 2.9 / 6, as LadybugDB prints it and as the fixture records it.
        assert!(cell_matches("0.48333333333333334", "0.48333333333333334"));
        assert!(cell_matches("0.4833333333333333", "0.48333333333333334"));
        assert!(cell_matches("2.9000000000000004", "2.9"));
        // Equal when rounded to two places, as round 1 compared; wrong by 3.3e-3, so they must fail.
        assert!(!cell_matches("0.480001", "0.48333333333333334"));
        assert!(!cell_matches("0.48", "0.48333333333333334"));
        assert!(cell_matches("0.4833333333333334", "0.48333333333333334"));
        assert!(
            cell_matches("0.483333334", "0.48333333333333334"),
            "6.7e-10 off is within 1e-9"
        );
        assert!(
            !cell_matches("0.48333334", "0.48333333333333334"),
            "6.7e-9 off is not"
        );
    }

    #[test]
    fn other_cells_match_exactly() {
        assert!(cell_matches("6", "6"));
        assert!(!cell_matches("6.0", "6"), "an integer is not a float");
        assert!(!cell_matches("D1", "D2"));
        assert!(!cell_matches("null", "0.0"));
        assert!(rows_match(
            &[vec!["3".into(), "2.9000000000000004".into()]],
            &[vec!["3".into(), "2.9".into()]]
        ));
        assert!(!rows_match(
            &[vec!["3".into()]],
            &[vec!["3".into(), "x".into()]]
        ));
        assert!(!rows_match(&[], &[vec!["3".into()]]));
    }
}
