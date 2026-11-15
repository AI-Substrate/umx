//! The grammar of the places umx names in a record (`secret_suspected`, `check --lint`'s places, a redact's
//! candidates), read back for the client: umx-core's `place::parse`, mirrored here (r-cli-is-a-client) as `check_ref`
//! is. The parity test in umx-daemon parses every place core writes here and in core alike, and rejects alike (F224).
//! umx writes places only in umx-core; nothing here writes one.

/// Where a position in a list starts counting in every place umx names: from 1 (F217). umx-core's `LIST_BASE`.
pub const LIST_BASE: usize = 1;

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
