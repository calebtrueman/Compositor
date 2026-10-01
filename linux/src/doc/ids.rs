use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

/// A layer, document or guide UUID. Written in uppercase, as the macOS app writes them, because
/// image filenames are named after the ID exactly as the manifest spells it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Id(pub Uuid);

impl Id {
    pub fn new() -> Self {
        Id(Uuid::new_v4())
    }

    pub fn upper(&self) -> String {
        self.0.hyphenated().to_string().to_uppercase()
    }
}

impl Default for Id {
    fn default() -> Self {
        Id::new()
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.upper())
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.upper())
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.upper())
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Uuid::parse_str(&text).map(Id).map_err(serde::de::Error::custom)
    }
}
