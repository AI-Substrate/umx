//! The places umx names in a record: the read flags (`secret_suspected`, F188, F190), `check --lint`'s places (F139)
//! and a redact's candidates (F214). Every place string is written here, counted from [`LIST_BASE`] (F217), and read
//! back by [`parse`]. umx-protocol mirrors `parse` for the client (r-cli-is-a-client); a parity test in umx-daemon
//! keeps the two the same (F224).

use alloc::format;
use alloc::string::String;

/// Where a position in a list starts counting in every place umx names: from 1 (F217, security's ruling: one base
/// everywhere). Only here, and in umx-protocol's mirror.
pub const LIST_BASE: usize = 1;

/// A custom field's place: its number among the record's custom fields as its line holds them, never its name.
pub fn custom_place(n: usize) -> String {
    format!("field {n}")
}

/// A field umx names by its name (title, note, a reserved field, a node's string field): `field:<name>`.
pub fn named_place(name: &str) -> String {
    format!("field:{name}")
}

/// A place on edge `id`: `edge:<id>/<place>` (R377-3).
pub fn edge_place(id: &str, place: &str) -> String {
    format!("edge:{id}/{place}")
}

/// The place of the `i`-th entry (from 0 in storage) of list `list`: `evidence:1` for the first pointer, `field 2:1`
/// for the first element of a list-valued custom field.
pub fn list_place(list: &str, i: usize) -> String {
    format!("{list}:{}", i + LIST_BASE)
}

/// A place read back: on an edge (`edge:<id>/...`) or on the record itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Place<'a> {
    /// The edge id of an `edge:<id>/` place.
    pub edge: Option<&'a str>,
    pub spot: Spot<'a>,
}

/// Where in a record a place is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spot<'a> {
    /// `field:<name>`.
    Named(&'a str),
    /// `field <n>`: the n-th custom field, from 1.
    Custom(usize),
    /// `<list>:<n>`: the entry at `index` (from 0, as stored) of `list`, a standard list (`evidence`, `key`) or a
    /// list-valued custom field (`field 2`).
    Entry { list: &'a str, index: usize },
    /// `record`: a line umx could not read as one object.
    Record,
}

/// The place `text` names, or None when it is not one umx writes.
pub fn parse(text: &str) -> Option<Place<'_>> {
    match text.strip_prefix("edge:") {
        Some(rest) => {
            let (id, place) = rest.split_once('/')?;
            if id.is_empty() || place.starts_with("edge:") {
                return None;
            }
            Some(Place {
                edge: Some(id),
                spot: spot(place)?,
            })
        }
        None => Some(Place {
            edge: None,
            spot: spot(text)?,
        }),
    }
}

fn spot(text: &str) -> Option<Spot<'_>> {
    if text == "record" {
        return Some(Spot::Record);
    }
    if let Some(name) = text.strip_prefix("field:") {
        return (!name.is_empty() && !name.contains(':')).then_some(Spot::Named(name));
    }
    if let Some(n) = custom(text) {
        return Some(Spot::Custom(n));
    }
    let (list, n) = text.rsplit_once(':')?;
    let index = number(n)?.checked_sub(LIST_BASE)?;
    let named = !list.is_empty() && list.bytes().all(|b| b.is_ascii_lowercase() || b == b'_');
    (named || custom(list).is_some()).then_some(Spot::Entry { list, index })
}

/// The n of `field <n>`, from 1.
fn custom(text: &str) -> Option<usize> {
    number(text.strip_prefix("field ")?).filter(|n| *n >= 1)
}

/// A plain decimal with no sign, no leading zero and no spaces.
fn number(text: &str) -> Option<usize> {
    let plain = !text.is_empty()
        && text.bytes().all(|b| b.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    if plain { text.parse().ok() } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_place_umx_writes_reads_back_as_written() {
        let at = |edge, spot| Some(Place { edge, spot });
        let field = custom_place(3);
        assert_eq!(parse(&field), at(None, Spot::Custom(3)));
        assert_eq!(parse(&named_place("note")), at(None, Spot::Named("note")));
        assert_eq!(
            parse(&list_place("evidence", 0)),
            at(
                None,
                Spot::Entry {
                    list: "evidence",
                    index: 0
                }
            )
        );
        assert_eq!(
            parse(&list_place(&field, 1)),
            at(
                None,
                Spot::Entry {
                    list: "field 3",
                    index: 1
                }
            )
        );
        assert_eq!(
            parse(&edge_place("01ABC", &custom_place(2))),
            at(Some("01ABC"), Spot::Custom(2))
        );
        assert_eq!(parse("record"), at(None, Spot::Record));
    }

    #[test]
    fn a_text_umx_never_writes_is_no_place() {
        for text in [
            "",
            "field:",
            "field 0",
            "field 01",
            "field -1",
            "field 2x",
            "evidence:0",
            "evidence:",
            "key:01",
            "Key:1",
            "graph:title",
            "edge:/field 1",
            "edge:01A",
            "edge:01A/edge:01B/field 1",
            "shape",
            "field:a:b",
            "evidence:+1",
        ] {
            assert_eq!(parse(text), None, "{text:?}");
        }
    }
}
