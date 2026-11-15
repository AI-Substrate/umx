//! The Cypher date functions (D71 step 10), answered through the provider's function hook (VENDORED row 28) over
//! `umx_core::time`, the one date parser. LadybugDB's names: `date`, `timestamp`, `current_date`,
//! `current_timestamp`, `date_part` (`datepart`), `date_trunc` (`datetrunc`), and umx's own `date_diff`.
//!
//! Values stay text: a date result is a string, a part or a gap an integer. Every answer is in UTC (R4). A null
//! gives null, and so does text that is not a date (openCypher's rule for conversions) and a part or cut finer than
//! the value (`date_part('day', '2025-01')`). A bad unit, a value of the wrong kind or a wrong count is an error.

use alloc::format;
use alloc::string::String;
use umx_core::time::{self, DateSpan, Unit};
use umx_cypher::CypherValue;

/// Every date function, lower case: the provider answers these names, and check accepts them.
pub const NAMES: [&str; 9] = [
    "date",
    "timestamp",
    "current_date",
    "current_timestamp",
    "date_part",
    "datepart",
    "date_trunc",
    "datetrunc",
    "date_diff",
];

/// Calls date function `name` (one of [`NAMES`]) on its evaluated arguments; `now_ms` is the query's one instant.
pub(crate) fn call(
    name: &str,
    args: &[CypherValue],
    now_ms: Option<u64>,
) -> Result<CypherValue, String> {
    let want = match name {
        "current_date" | "current_timestamp" => 0,
        "date" | "timestamp" => 1,
        "date_diff" => 3,
        _ => 2,
    };
    if args.len() != want {
        return Err(format!("`{name}` expects exactly {want} arguments"));
    }
    let text = |s: Option<String>| s.map_or(CypherValue::Null, CypherValue::Str);
    let int = |n: Option<i64>| n.map_or(CypherValue::Null, CypherValue::Int);
    match name {
        "current_date" | "current_timestamp" => {
            let ms = now_ms.ok_or_else(|| format!("`{name}` has no clock"))?;
            let secs = i64::try_from(ms / 1000).map_err(|_| format!("`{name}` has no clock"))?;
            let now = DateSpan {
                start: secs,
                end: secs + 1,
            };
            Ok(text(if name == "current_date" {
                time::date_text(now)
            } else {
                time::timestamp_text(now)
            }))
        }
        "date" | "timestamp" => Ok(match span(name, &args[0])? {
            None => CypherValue::Null,
            Some(s) if name == "date" => text(time::date_text(s)),
            Some(s) => text(time::timestamp_text(s)),
        }),
        "date_part" | "datepart" => {
            let unit = unit(name, &args[0], false)?;
            Ok(match (unit, span(name, &args[1])?) {
                (Some(u), Some(s)) => int(time::date_part(u, s)),
                _ => CypherValue::Null,
            })
        }
        "date_trunc" | "datetrunc" => {
            let unit = unit(name, &args[0], true)?;
            Ok(match (unit, span(name, &args[1])?) {
                (Some(u), Some(s)) => text(time::date_trunc(u, s)),
                _ => CypherValue::Null,
            })
        }
        _ => {
            let unit = unit(name, &args[0], true)?;
            let (a, b) = (span(name, &args[1])?, span(name, &args[2])?);
            Ok(match (unit, a, b) {
                (Some(u), Some(a), Some(b)) => int(time::date_diff(u, a, b)),
                _ => CypherValue::Null,
            })
        }
    }
}

/// A date argument's span: None for null and for text that is not a date; an error for any other value.
fn span(name: &str, v: &CypherValue) -> Result<Option<DateSpan>, String> {
    match v {
        CypherValue::Null => Ok(None),
        CypherValue::Str(s) => Ok(time::date_span(s)),
        _ => Err(format!("`{name}` expects a date string")),
    }
}

/// A unit argument: None for null; an error for a non-string or a name no unit has (or `dayofweek` where a cut is
/// needed).
fn unit(name: &str, v: &CypherValue, cut: bool) -> Result<Option<Unit>, String> {
    match v {
        CypherValue::Null => Ok(None),
        CypherValue::Str(s) => match Unit::parse(s) {
            Some(u) if !cut || u.is_cut() => Ok(Some(u)),
            _ => Err(format!("`{name}` has no unit `{s}`")),
        },
        _ => Err(format!("`{name}` expects a unit string")),
    }
}
