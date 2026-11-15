//! 128-bit sortable ids, written as 26-character Crockford base32 (the ULID text form).

use alloc::string::String;
use core::fmt;
use core::str::FromStr;

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Length of an id's text form.
pub const ID_LEN: usize = 26;

/// Shortest id prefix accepted when naming a node (workshop 007 rule 7).
pub const MIN_PREFIX: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(pub u128);

impl Id {
    pub fn to_text(self) -> String {
        self.text_bytes().iter().map(|&b| b as char).collect()
    }

    /// The text form as ASCII bytes, with nothing allocated (hot loops: the similar_to digest, ws004 s34b).
    pub fn text_bytes(self) -> [u8; ID_LEN] {
        let mut out = [0u8; ID_LEN];
        let mut v = self.0;
        for slot in out.iter_mut().rev() {
            *slot = ALPHABET[(v & 31) as usize];
            v >>= 5;
        }
        out
    }
}

/// Maps one Crockford character (any case; I and L read as 1, O as 0) to its value.
pub fn crockford_value(c: char) -> Option<u8> {
    let c = c.to_ascii_uppercase();
    let c = match c {
        'I' | 'L' => '1',
        'O' => '0',
        other => other,
    };
    ALPHABET
        .iter()
        .position(|&a| a as char == c)
        .map(|p| p as u8)
}

/// Normalises id text (or a prefix of it) to canonical upper case; None if any character is not Crockford.
pub fn normalise_id_text(text: &str) -> Option<String> {
    text.chars()
        .map(|c| crockford_value(c).map(|v| ALPHABET[v as usize] as char))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdParseError;

impl fmt::Display for IdParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not a 26-character id")
    }
}

impl FromStr for Id {
    type Err = IdParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.chars().count() != ID_LEN {
            return Err(IdParseError);
        }
        let mut chars = s.chars();
        // The first character carries only 3 bits.
        let first = crockford_value(chars.next().ok_or(IdParseError)?).ok_or(IdParseError)?;
        if first > 7 {
            return Err(IdParseError);
        }
        let mut v = first as u128;
        for c in chars {
            v = (v << 5) | crockford_value(c).ok_or(IdParseError)? as u128;
        }
        Ok(Id(v))
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_text())
    }
}

impl serde::Serialize for Id {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_text())
    }
}

impl<'de> serde::Deserialize<'de> for Id {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = <String as serde::Deserialize>::deserialize(d)?;
        s.parse()
            .map_err(|_| serde::de::Error::custom("not a 26-character id"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trips_and_sorts_like_the_number() {
        let ids = [
            Id(0),
            Id(1),
            Id(31),
            Id(32),
            Id(u128::MAX),
            Id(0x0194_1234_5678_9abc_def0_1234_5678_9abc),
        ];
        for id in ids {
            let text = id.to_text();
            assert_eq!(text.len(), ID_LEN);
            assert_eq!(text.parse::<Id>().unwrap(), id);
        }
        assert!(Id(31).to_text() < Id(32).to_text());
        assert_eq!(Id(u128::MAX).to_text(), "7ZZZZZZZZZZZZZZZZZZZZZZZZZ");
    }

    #[test]
    fn parse_is_case_insensitive_and_refuses_bad_text() {
        let id = Id(123_456_789);
        assert_eq!(id.to_text().to_lowercase().parse::<Id>().unwrap(), id);
        assert!("8ZZZZZZZZZZZZZZZZZZZZZZZZZ".parse::<Id>().is_err());
        assert!("UUUUUUUUUUUUUUUUUUUUUUUUUU".parse::<Id>().is_err());
        assert!("0123".parse::<Id>().is_err());
    }
}
