//! Bounded numbers and text for control messages. Each type checks its range
//! when constructed and when deserialized, and its JSON Schema states the same
//! bounds. Consumers that cannot use serde read `MIN` and `MAX` instead.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// An integer with an inclusive range. The `$int_name` literal is the type
/// name serde needs in its `try_from` attribute.
macro_rules! bounded_int {
    ($(#[$meta:meta])* $name:ident, $int:ty, $int_name:literal, $min:literal, $max:literal, $error:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
        #[cfg_attr(feature = "generate", derive(ts_rs::TS))]
        #[serde(try_from = $int_name)]
        #[schemars(!try_from, transparent)]
        pub struct $name(#[schemars(range(min = $min, max = $max))] $int);

        impl $name {
            pub const MIN: $int = $min;
            pub const MAX: $int = $max;

            pub const fn get(self) -> $int {
                self.0
            }
        }

        impl TryFrom<$int> for $name {
            type Error = &'static str;

            fn try_from(value: $int) -> Result<Self, Self::Error> {
                if (Self::MIN..=Self::MAX).contains(&value) {
                    Ok(Self(value))
                } else {
                    Err($error)
                }
            }
        }
    };
}

/// Text counted in Unicode code points. `$allowed` accepts one character.
macro_rules! bounded_text {
    ($(#[$meta:meta])* $name:ident, $min:literal, $max:literal, $pattern:literal, $allowed:expr, $error:literal) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
        #[cfg_attr(feature = "generate", derive(ts_rs::TS))]
        #[serde(try_from = "String")]
        #[schemars(!try_from, transparent)]
        pub struct $name(
            #[schemars(length(min = $min, max = $max), regex(pattern = $pattern))] String,
        );

        impl $name {
            pub const MIN_CHARS: usize = $min;
            pub const MAX_CHARS: usize = $max;

            pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
                Self::try_from(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = &'static str;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                let allowed: fn(char) -> bool = $allowed;
                // Stop counting early so a huge string costs little to reject.
                let count = value.chars().take(Self::MAX_CHARS + 1).count();
                if !(Self::MIN_CHARS..=Self::MAX_CHARS).contains(&count)
                    || !value.chars().all(allowed)
                {
                    return Err($error);
                }
                Ok(Self(value))
            }
        }
    };
}

bounded_int!(
    /// A nonzero audio stream, matching the frame header's stream ID.
    AudioStreamId, u16, "u16", 1, 65535, "expected an audio stream ID of 1–65535"
);
bounded_int!(
    /// How long an expression or servo move lasts. Zero holds until the next command.
    ActionDurationMs, u32, "u32", 0, 60_000, "expected a duration of 0–60000 ms"
);
bounded_int!(
    /// How long display text stays on screen.
    TextTtlMs, u32, "u32", 100, 60_000, "expected a text lifetime of 100–60000 ms"
);
bounded_int!(
    /// The window for the physical button press that confirms pairing.
    PairTimeoutMs, u32, "u32", 1_000, 30_000, "expected a pairing timeout of 1000–30000 ms"
);
bounded_int!(
    /// PCM samples per second per channel.
    SampleRate, u32, "u32", 8_000, 48_000, "expected a sample rate of 8000–48000 Hz"
);
bounded_int!(
    /// Interleaved PCM channels.
    ChannelCount, u8, "u8", 1, 2, "expected 1 or 2 channels"
);
bounded_int!(
    /// Audio per frame. One frame of PCM must fit in `MAX_PAYLOAD_LEN`.
    FrameMs, u8, "u8", 10, 60, "expected a frame length of 10–60 ms"
);
bounded_int!(
    /// Audio kept from before the wake word or button press.
    PrerollMs, u32, "u32", 0, 2_000, "expected a preroll of 0–2000 ms"
);
bounded_int!(
    /// A setting expressed from 0 to 100.
    Percent, u8, "u8", 0, 100, "expected a percentage of 0–100"
);
bounded_int!(
    /// Silence that ends an utterance.
    VadSilenceMs, u32, "u32", 200, 5_000, "expected silence of 200–5000 ms"
);
bounded_int!(
    /// The longest utterance before the robot stops listening.
    MaxUtteranceMs, u32, "u32", 1_000, 60_000, "expected an utterance limit of 1000–60000 ms"
);

bounded_text!(
    /// A short line for the robot's screen. Control characters are rejected.
    ScreenText, 1, 64, r"^[^\x00-\x1f\x7f-\x9f]+(?![\s\S])", |ch| !ch.is_control(),
    "expected screen text of 1–64 characters without control characters"
);
bounded_text!(
    /// A diagnostic for people, never a secret.
    ErrorDetail, 1, 128, r"^[^\x00-\x1f\x7f-\x9f]+(?![\s\S])", |ch| !ch.is_control(),
    "expected error detail of 1–128 characters without control characters"
);
bounded_text!(
    /// A hardware, firmware, or model version without whitespace.
    VersionText, 1, 32, r"^[\x21-\x7e]+(?![\s\S])", |ch| ch.is_ascii_graphic(),
    "expected a version of 1–32 printable ASCII characters"
);
