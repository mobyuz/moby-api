use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// An RFC 3339 timestamp with an explicit offset and up to nine fractional
/// digits. Leap seconds are excluded from the scheduling contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "generate", derive(ts_rs::TS))]
#[serde(try_from = "String")]
#[schemars(!try_from, transparent)]
pub struct ZonedDateTime(
    #[schemars(
        extend("format" = "date-time"),
        regex(pattern = r"^[0-9]{4}-[0-9]{2}-[0-9]{2}[Tt][0-9]{2}:[0-9]{2}:[0-5][0-9](?:\.[0-9]{1,9})?(?:[Zz]|[+-][0-9]{2}:[0-9]{2})(?![\s\S])")
    )]
    String,
);

impl ZonedDateTime {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ZonedDateTime {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        // The parser accepts more timestamp spellings than the wire profile.
        let bytes = value.as_bytes();
        if !matches!(bytes.get(10), Some(b'T' | b't'))
            || bytes.get(17) == Some(&b'6')
            || (bytes.get(19) == Some(&b'.')
                && bytes
                    .get(20..30)
                    .is_some_and(|fraction| fraction.iter().all(u8::is_ascii_digit)))
            || OffsetDateTime::parse(&value, &Rfc3339).is_err()
        {
            return Err(
                "expected an RFC 3339 timestamp with an offset and at most nine fractional digits",
            );
        }
        Ok(Self(value))
    }
}
