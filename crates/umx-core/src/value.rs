//! Typed field values (workshop 007 rule 5). Values are JSON values limited to the six field kinds.

use crate::ontology::FieldKind;
use crate::time::{date_span, looks_like_date, normalise_date};
use alloc::string::{String, ToString};
use serde_json::Value;

/// How a `--set` style argument was given.
#[derive(Clone, Debug, PartialEq)]
pub enum SetValue {
    /// `--set k=v`: parsed by the declared kind, else guessed.
    Raw(String),
    /// `--set-str k=v`: always a string.
    Str(String),
    /// `--set-json k=<json>`: exactly this value.
    Json(Value),
}

/// The value to store, and whether it failed to parse to its kind (then it is stored as given, with a
/// `field_kind_mismatch` warning). `kind` is the declared kind, else the kind fixed by the field's first use.
pub fn typed_value(value: &SetValue, kind: Option<FieldKind>) -> (Value, bool) {
    let v = match value {
        SetValue::Str(s) => Value::String(s.clone()),
        SetValue::Json(v) => v.clone(),
        SetValue::Raw(s) => match kind {
            Some(k) => parse_as(s, k).unwrap_or_else(|| Value::String(s.clone())),
            None => guess(s),
        },
    };
    let Some(kind) = kind else { return (v, false) };
    let v = match (&v, kind) {
        (Value::String(s), FieldKind::Date) => normalise_date(s).map(Value::String).unwrap_or(v),
        _ => v,
    };
    let fits = fits(&v, kind);
    (v, !fits)
}

/// The kind a value has, for fixing an undeclared field's kind on first use.
pub fn kind_of(v: &Value) -> Option<FieldKind> {
    match v {
        Value::String(_) => Some(FieldKind::String),
        Value::Number(n) if n.is_f64() => Some(FieldKind::Float),
        Value::Number(_) => Some(FieldKind::Int),
        Value::Bool(_) => Some(FieldKind::Bool),
        Value::Array(_) => Some(FieldKind::List),
        _ => None,
    }
}

/// Parses a `--where` value: by the field's kind if known, else by the same inference as `--set`.
pub fn where_value(s: &str, kind: Option<FieldKind>) -> Value {
    typed_value(&SetValue::Raw(s.to_string()), kind).0
}

fn parse_as(s: &str, kind: FieldKind) -> Option<Value> {
    match kind {
        FieldKind::String => Some(Value::String(s.to_string())),
        FieldKind::Int => s.parse::<i64>().ok().map(Value::from),
        FieldKind::Float => s
            .parse::<f64>()
            .ok()
            .filter(|f| f.is_finite())
            .map(Value::from),
        FieldKind::Bool => match s {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        FieldKind::Date => normalise_date(s).map(Value::String),
        FieldKind::List => Some(Value::Array(
            s.split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(|p| Value::String(p.to_string()))
                .collect(),
        )),
    }
}

fn guess(s: &str) -> Value {
    if let Ok(i) = s.parse::<i64>() {
        return Value::from(i);
    }
    if s.contains('.')
        && let Some(f) = s.parse::<f64>().ok().filter(|f| f.is_finite())
    {
        return Value::from(f);
    }
    match s {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => Value::String(s.to_string()),
    }
}

/// True if `v` is a value of `kind`.
pub fn fits(v: &Value, kind: FieldKind) -> bool {
    match kind {
        FieldKind::String => v.is_string(),
        FieldKind::Int => v.is_i64() || v.is_u64(),
        FieldKind::Float => v.is_number(),
        FieldKind::Bool => v.is_boolean(),
        FieldKind::Date => v.as_str().is_some_and(looks_like_date),
        FieldKind::List => v.as_array().is_some_and(|a| a.iter().all(Value::is_string)),
    }
}

/// How `a` stands against `b` for `--where` and Cypher's comparisons. Any two texts that read as dates compare as
/// dates ([`crate::time::DateSpan::relation`]: `Less` wholly before, `Greater` wholly after, `Equal` overlapping);
/// other texts compare as text.
pub fn compare_values(a: &Value, b: &Value) -> Option<core::cmp::Ordering> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64()?.partial_cmp(&y.as_f64()?),
        (Value::String(x), Value::String(y)) => Some(compare_text(x, y)),
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

/// [`compare_values`] for two texts: [`date_relation`] when both read as dates, else text order.
pub fn compare_text(a: &str, b: &str) -> core::cmp::Ordering {
    date_relation(a, b).unwrap_or_else(|| a.cmp(b))
}

/// How date `a` stands against date `b` ([`crate::time::DateSpan::relation`]); None unless both read as dates. The
/// one date comparison: `--where` uses it through [`compare_text`], Cypher through its provider. Two texts of one
/// normalised shape (`DDDD`, `DDDD-DD`, `DDDD-DD-DD`, `DDDD-DD-DDTDD:DD:DDZ`) answer by text order without being
/// parsed: exact, since two dates of one precision compare as their text, and text is the answer when either is
/// not a date.
pub fn date_relation(a: &str, b: &str) -> Option<core::cmp::Ordering> {
    if a.len() == b.len() && date_shape(a) && date_shape(b) {
        return Some(a.cmp(b));
    }
    Some(date_span(a)?.relation(&date_span(b)?))
}

/// Whether `s` has the byte shape of a normalised date value, valid or not.
fn date_shape(s: &str) -> bool {
    const FULL: &[u8; 20] = b"0000-00-00T00:00:00Z";
    matches!(s.len(), 4 | 7 | 10 | 20)
        && s.bytes().zip(FULL).all(|(c, &p)| {
            if p == b'0' {
                c.is_ascii_digit()
            } else {
                c == p
            }
        })
}

/// The text a string sorts by (Cypher's `ORDER BY`, `min`, `max`), when it is not the string itself: the canonical
/// form of a date written another way (`2026-10-30T09:40:00+11:00` sorts as `2026-10-29T22:40:00Z`). Sort by this
/// key, else the string, then by the string: a total order. Canonical dates sort by text in the span order
/// ([`crate::time::DateSpan::order`]: start, then the longer span first), and every other string sorts as before.
pub fn text_sort_key(s: &str) -> Option<String> {
    // A stored date is canonical: checked by shape, so the sort builds no text for it.
    if looks_like_date(s) {
        return None;
    }
    normalise_date(s)
}

/// Two strings in sort order: by [`text_sort_key`], else the string, then by the string.
pub fn sort_texts(a: &str, b: &str) -> core::cmp::Ordering {
    let (ka, kb) = (text_sort_key(a), text_sort_key(b));
    ka.as_deref()
        .unwrap_or(a)
        .cmp(kb.as_deref().unwrap_or(b))
        .then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn raw_values_follow_the_declared_kind_else_a_guess() {
        let raw = |s: &str| SetValue::Raw(s.into());
        assert_eq!(
            typed_value(&raw("3"), Some(FieldKind::Float)),
            (json!(3.0), false)
        );
        assert_eq!(typed_value(&raw("3"), None), (json!(3), false));
        assert_eq!(typed_value(&raw("0.7"), None), (json!(0.7), false));
        assert_eq!(typed_value(&raw("true"), None), (json!(true), false));
        assert_eq!(typed_value(&raw("high"), None), (json!("high"), false));
        assert_eq!(
            typed_value(&raw("high"), Some(FieldKind::Int)),
            (json!("high"), true)
        );
        assert_eq!(
            typed_value(&raw("a, b"), Some(FieldKind::List)),
            (json!(["a", "b"]), false)
        );
        assert_eq!(
            typed_value(&SetValue::Str("3".into()), None),
            (json!("3"), false)
        );
        assert_eq!(
            typed_value(&SetValue::Str("3".into()), Some(FieldKind::Int)),
            (json!("3"), true)
        );
        assert_eq!(
            typed_value(&SetValue::Json(json!(["x"])), Some(FieldKind::List)),
            (json!(["x"]), false)
        );
        assert_eq!(
            typed_value(&raw("2026-11-09T15:00:00+10:00"), Some(FieldKind::Date)),
            (json!("2026-11-09T05:00:00Z"), false)
        );
        assert_eq!(
            typed_value(&SetValue::Str("2026-11-09".into()), Some(FieldKind::Date)),
            (json!("2026-11-09"), false)
        );
        assert_eq!(kind_of(&json!(3)), Some(FieldKind::Int));
        assert_eq!(kind_of(&json!(3.5)), Some(FieldKind::Float));
    }

    #[test]
    fn month_and_year_are_dates_and_a_bad_month_is_a_mismatch() {
        let raw = |s: &str| SetValue::Raw(s.into());
        assert_eq!(
            typed_value(&raw("2025-01"), Some(FieldKind::Date)),
            (json!("2025-01"), false)
        );
        assert_eq!(
            typed_value(&raw("2025"), Some(FieldKind::Date)),
            (json!("2025"), false)
        );
        assert_eq!(
            typed_value(&raw("2025-13"), Some(FieldKind::Date)),
            (json!("2025-13"), true)
        );
        assert_eq!(where_value("2025", Some(FieldKind::Date)), json!("2025"));
        // Undeclared, `2025` is a number, never a year.
        assert_eq!(where_value("2025", None), json!(2025));
    }

    #[test]
    fn texts_that_read_as_dates_compare_as_dates() {
        use core::cmp::Ordering::{Equal, Greater, Less};
        let c = |a: &str, b: &str| compare_values(&json!(a), &json!(b));
        assert_eq!(c("2026-10-29T22:40:00Z", "2026-10-30"), Some(Less));
        assert_eq!(c("2026-10-29T22:40:00Z", "2026-10-29"), Some(Equal));
        assert_eq!(c("2025-01-15", "2025-01"), Some(Equal));
        assert_eq!(c("2025", "2025-02"), Some(Equal));
        assert_eq!(c("2025-03", "2025-02"), Some(Greater));
        assert_eq!(
            c("2026-10-30T09:40:00+11:00", "2026-10-29T23:00:00Z"),
            Some(Less)
        );
        // Not both dates: text, as before.
        assert_eq!(c("2025-02", "soon"), Some(Less));
        assert_eq!(c("2025-1", "2025-01"), Some(Greater));
    }

    /// Two texts of one normalised shape skip the parse; the answer is the parse path's, valid dates or not.
    #[test]
    fn same_shape_texts_answer_as_the_parse_path_does() {
        let parsed = |a: &str, b: &str| match (date_span(a), date_span(b)) {
            (Some(x), Some(y)) => x.relation(&y),
            _ => a.cmp(b),
        };
        let shapes: [&[&str]; 4] = [
            &["2025", "2026", "1969", "0000", "9999"],
            &["2025-01", "2025-12", "2024-12", "2025-13", "2025-00"],
            &[
                "2025-01-15",
                "2025-01-16",
                "2024-02-29",
                "2025-02-29",
                "2026-02-30",
                "2025-01-00",
            ],
            &[
                "2026-10-29T22:40:00Z",
                "2026-10-29T22:40:01Z",
                "2026-10-30T00:00:00Z",
                "2026-10-29T25:00:00Z",
                "1969-12-31T23:59:59Z",
            ],
        ];
        for shape in shapes {
            for a in shape {
                assert!(date_shape(a), "{a}");
                for b in shape {
                    assert_eq!(compare_text(a, b), parsed(a, b), "{a} vs {b}");
                    assert_eq!(date_relation(a, b), Some(parsed(a, b)), "{a} vs {b}");
                }
            }
        }
        for s in [
            "2025-1",
            " 2025",
            "2025-01-01T00:00:00+00:00",
            "2025-01-01 00:00:00Z",
            "abcd",
        ] {
            assert!(!date_shape(s), "{s}");
        }
    }

    /// Random mixes of dates (every precision, canonical or not) and other texts: one order whatever the input
    /// order; dates in span order among themselves, other texts in text order, and canonical values in text order.
    #[test]
    fn text_sort_gives_one_total_order() {
        const POOL: &[&str] = &[
            "2025",
            "2025-01",
            "2025-01-01",
            "2025-01-01T00:00:00Z",
            "2025-01-01 00:00",
            "2025-12",
            "2024-12-31T23:59:59Z",
            "2025-01-01T10:00:00+10:00",
            "2026-10-29T22:40:00Z",
            "2026-10-30",
            "2026-10-30T09:40:00+11:00",
            "1969",
            "",
            " x",
            "soon",
            "2025-1",
            "2025-13",
            "Z",
            "abc",
            "2025-01-01T",
            "2025-x",
        ];
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let sorted = |items: &[&'static str]| {
            let mut v = items.to_vec();
            v.sort_by(|a, b| sort_texts(a, b));
            v
        };
        for _ in 0..500 {
            let n = (next() % 12) as usize + 1;
            let items: alloc::vec::Vec<&str> = (0..n)
                .map(|_| POOL[(next() % POOL.len() as u64) as usize])
                .collect();
            let mut shuffled = items.clone();
            for i in (1..shuffled.len()).rev() {
                shuffled.swap(i, (next() % (i as u64 + 1)) as usize);
            }
            let out = sorted(&items);
            assert_eq!(out, sorted(&shuffled));
            let dates: alloc::vec::Vec<_> = out.iter().filter_map(|s| date_span(s)).collect();
            assert!(
                dates.windows(2).all(|w| w[0].order(&w[1]).is_le()),
                "{out:?}"
            );
            let texts: alloc::vec::Vec<_> = out.iter().filter(|s| date_span(s).is_none()).collect();
            assert!(texts.windows(2).all(|w| w[0] <= w[1]), "{out:?}");
            let canonical: alloc::vec::Vec<_> =
                out.iter().filter(|s| text_sort_key(s).is_none()).collect();
            assert!(canonical.windows(2).all(|w| w[0] <= w[1]), "{out:?}");
        }
    }

    /// The sort key is pinned: a change to it reorders `ORDER BY` results.
    #[test]
    fn text_sort_key_is_the_canonical_form_of_a_date_written_otherwise() {
        for s in [
            "2025",
            "2025-01",
            "2025-01-01",
            "2026-10-29T22:40:00Z",
            "soon",
            " x",
            "2025-13",
        ] {
            assert_eq!(text_sort_key(s), None, "{s}");
        }
        assert_eq!(
            text_sort_key("2025-01-01T09:40:00+11:00").as_deref(),
            Some("2024-12-31T22:40:00Z")
        );
        assert_eq!(text_sort_key(" 2025").as_deref(), Some("2025"));
        let mut v = [
            "2025",
            " x",
            "abc",
            "2025-01-01T09:40:00+11:00",
            "2024-12-31T23:00:00Z",
        ];
        v.sort_by(|a, b| sort_texts(a, b));
        // Today's order, except that the offset literal sits at its instant.
        assert_eq!(
            v,
            [
                " x",
                "2025-01-01T09:40:00+11:00",
                "2024-12-31T23:00:00Z",
                "2025",
                "abc"
            ]
        );
    }
}
