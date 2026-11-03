use std::{error::Error, fmt};

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, de};

/// An opaque registry key: 1-64 ASCII characters, beginning with a lowercase
/// letter and then containing only lowercase letters, digits, `_`, or `-`.
/// TypeScript represents this as a string; validate untrusted JSON at runtime.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(transparent)]
pub struct StableId(
    // ECMAScript `$` also matches before a final newline. Use an actual
    // end-of-input assertion so JSON Schema agrees with the Rust validator.
    #[schemars(
        length(min = 1, max = 64),
        regex(pattern = "^[a-z][a-z0-9_-]{0,63}(?![\\s\\S])")
    )]
    String,
);

impl StableId {
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidStableId> {
        let value = value.into();
        let bytes = value.as_bytes();
        if !(1..=64).contains(&bytes.len())
            || !bytes[0].is_ascii_lowercase()
            || !bytes.iter().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(byte)
            })
        {
            return Err(InvalidStableId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for StableId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

impl TryFrom<String> for StableId {
    type Error = InvalidStableId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl fmt::Display for StableId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Does not echo an untrusted value into error messages or logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidStableId;

impl fmt::Display for InvalidStableId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("expected a registry ID matching ^[a-z][a-z0-9_-]{0,63}$")
    }
}

impl Error for InvalidStableId {}
