//! Saved scopes are machine-local graph identities, not graph contents or copy names.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

use crate::{Id, name::is_snake_case, ports::GraphDir};

pub const SCOPES_FILE: &str = "scopes.toml";
pub const MAX_SCOPES: usize = 128;
pub const MAX_MEMBERS: usize = 128;
pub const MAX_BYTES: usize = crate::ports::config_files::SMALL_FILE_CAP as usize - 4096;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Member {
    pub id: Id,
    pub alias: String,
    pub folder: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    pub members: Vec<Member>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scopes {
    pub umx_scopes: u32,
    pub scopes: BTreeMap<String, Saved>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Invalid,
    Limit,
    Newer(u64),
}

impl Default for Scopes {
    fn default() -> Self {
        Self {
            umx_scopes: 1,
            scopes: BTreeMap::new(),
            extra: BTreeMap::new(),
        }
    }
}

impl Scopes {
    fn validate(&self) -> Result<(), Error> {
        if self.umx_scopes > 1 {
            return Err(Error::Newer(u64::from(self.umx_scopes)));
        }
        if self.umx_scopes != 1
            || self.scopes.iter().any(|(name, saved)| {
                !is_snake_case(name)
                    || saved.members.is_empty()
                    || saved.members.iter().enumerate().any(|(i, m)| {
                        !is_snake_case(&m.alias)
                            || GraphDir::parse(&m.folder).is_err()
                            || saved.members[..i].iter().any(|other| other.id == m.id)
                    })
            })
        {
            return Err(Error::Invalid);
        }
        if self.scopes.len() > MAX_SCOPES
            || self.scopes.values().any(|s| s.members.len() > MAX_MEMBERS)
        {
            return Err(Error::Limit);
        }
        Ok(())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_BYTES {
            return Err(Error::Limit);
        }
        let text = core::str::from_utf8(bytes).map_err(|_| Error::Invalid)?;
        let value: toml::Value = toml::from_str(text).map_err(|_| Error::Invalid)?;
        if let Some(version) = value.get("umx_scopes").and_then(toml::Value::as_integer)
            && version > 1
        {
            return Err(Error::Newer(version as u64));
        }
        let scopes: Self = value.try_into().map_err(|_| Error::Invalid)?;
        scopes.validate()?;
        Ok(scopes)
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        self.validate()?;
        let bytes = toml::to_string(self)
            .map_err(|_| Error::Invalid)?
            .into_bytes();
        if bytes.len() > MAX_BYTES {
            return Err(Error::Limit);
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_preserve_unknowns_and_refuse_invalid_or_oversized_files() {
        let text = alloc::format!(
            "umx_scopes = 1\nfuture = 'keep'\n[scopes.both]\nfuture = 7\n[[scopes.both.members]]\nid = '{}'\nalias = 'g'\nfolder = '/g'\nfuture = true\n",
            Id(1)
        );
        let scopes = Scopes::decode(text.as_bytes()).unwrap();
        assert_eq!(Scopes::decode(&scopes.encode().unwrap()).unwrap(), scopes);
        assert_eq!(
            Scopes::decode(b"umx_scopes = 2\n[scopes]"),
            Err(Error::Newer(2))
        );
        assert_eq!(
            Scopes::decode(b"umx_scopes = 3\nfuture = true"),
            Err(Error::Newer(3))
        );
        assert_eq!(
            Scopes::decode(b"umx_scopes = 0\n[scopes]"),
            Err(Error::Invalid)
        );
        assert_eq!(
            Scopes::decode(&alloc::vec![b' '; MAX_BYTES + 1]),
            Err(Error::Limit)
        );
        let mut too_many = scopes.clone();
        let member = scopes.scopes["both"].members[0].clone();
        too_many.scopes.get_mut("both").unwrap().members = (0..=MAX_MEMBERS)
            .map(|n| Member {
                id: Id(n as u128),
                ..member.clone()
            })
            .collect();
        assert_eq!(too_many.encode(), Err(Error::Limit));
        let mut too_many = scopes.clone();
        for n in 0..MAX_SCOPES {
            too_many
                .scopes
                .insert(alloc::format!("scope_{n}"), scopes.scopes["both"].clone());
        }
        assert_eq!(too_many.encode(), Err(Error::Limit));
    }
}
