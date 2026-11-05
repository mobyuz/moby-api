//! Pairing secrets and session proofs. The library never generates random
//! bytes: the robot makes the nonce and the desktop makes the key.

use std::fmt;

use hmac::{Hmac, KeyInit, Mac};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::StableId;

type HmacSha256 = Hmac<Sha256>;

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

fn encode_hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

/// Accepts exactly `2 * N` lowercase hexadecimal digits.
fn decode_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let digits = text.as_bytes();
    if digits.len() != 2 * N {
        return None;
    }
    let nibble = |digit: u8| match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    };
    let mut bytes = [0; N];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = (nibble(digits[2 * index])? << 4) | nibble(digits[2 * index + 1])?;
    }
    Some(bytes)
}

/// Fixed-size binary data written as lowercase hexadecimal.
macro_rules! hex_string {
    ($(#[$meta:meta])* $name:ident, $bytes:literal, $chars:literal, $pattern:literal, $error:literal) => {
        $(#[$meta])*
        #[derive(Clone, Serialize, Deserialize, JsonSchema)]
        #[cfg_attr(feature = "generate", derive(ts_rs::TS))]
        #[serde(try_from = "String")]
        #[schemars(!try_from, transparent)]
        pub struct $name(
            #[schemars(length(min = $chars, max = $chars), regex(pattern = $pattern))] String,
        );

        impl $name {
            pub const BYTES: usize = $bytes;
            pub const HEX_CHARS: usize = $chars;

            pub fn from_bytes(bytes: [u8; $bytes]) -> Self {
                Self(encode_hex(&bytes))
            }

            pub fn to_bytes(&self) -> [u8; $bytes] {
                decode_hex(&self.0).expect("validated hexadecimal")
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = &'static str;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                decode_hex::<$bytes>(&value).ok_or($error)?;
                Ok(Self(value))
            }
        }
    };
}

hex_string!(
    /// A fresh random value the robot sends in `hello`: 16 bytes.
    #[derive(Debug, PartialEq, Eq)]
    Nonce, 16, 32, r"^[0-9a-f]{32}(?![\s\S])", "expected exactly 32 lowercase hexadecimal digits"
);
hex_string!(
    /// HMAC-SHA256 over the session input: 32 bytes.
    #[derive(Debug, PartialEq, Eq)]
    Proof, 32, 64, r"^[0-9a-f]{64}(?![\s\S])", "expected exactly 64 lowercase hexadecimal digits"
);
hex_string!(
    /// The shared pairing secret: 32 random bytes. `Debug` never prints it.
    /// Serialization still writes the key, because `pair_set_key` sends it.
    PairKey, 32, 64, r"^[0-9a-f]{64}(?![\s\S])", "expected exactly 64 lowercase hexadecimal digits"
);

impl fmt::Debug for PairKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PairKey(<redacted>)")
    }
}

/// Compares in constant time. Both sides always hold 64 digits.
impl PartialEq for PairKey {
    fn eq(&self, other: &Self) -> bool {
        self.0
            .bytes()
            .zip(other.0.bytes())
            .fold(0, |difference, (a, b)| difference | (a ^ b))
            == 0
    }
}

impl Eq for PairKey {}

/// The exact bytes the proof covers: the 16 nonce bytes, the device ID, a
/// zero byte, then the session ID. IDs cannot contain zero, so no two
/// distinct (device, session) pairs produce the same input.
pub fn session_proof_input(nonce: &Nonce, device_id: &StableId, session_id: &StableId) -> Vec<u8> {
    let mut input = Vec::with_capacity(Nonce::BYTES + device_id.as_str().len() + 1 + 64);
    input.extend_from_slice(&nonce.to_bytes());
    input.extend_from_slice(device_id.as_str().as_bytes());
    input.push(0);
    input.extend_from_slice(session_id.as_str().as_bytes());
    input
}

impl PairKey {
    fn mac(&self, nonce: &Nonce, device_id: &StableId, session_id: &StableId) -> HmacSha256 {
        let mut mac =
            HmacSha256::new_from_slice(&self.to_bytes()).expect("HMAC accepts any key length");
        mac.update(&session_proof_input(nonce, device_id, session_id));
        mac
    }

    /// The desktop's proof for `hello_ack`, bound to this hello and session.
    pub fn session_proof(
        &self,
        nonce: &Nonce,
        device_id: &StableId,
        session_id: &StableId,
    ) -> Proof {
        let tag: [u8; Proof::BYTES] = self
            .mac(nonce, device_id, session_id)
            .finalize()
            .into_bytes()
            .into();
        Proof::from_bytes(tag)
    }

    /// Check a proof in constant time.
    pub fn verify_session_proof(
        &self,
        nonce: &Nonce,
        device_id: &StableId,
        session_id: &StableId,
        proof: &Proof,
    ) -> bool {
        self.mac(nonce, device_id, session_id)
            .verify_slice(&proof.to_bytes())
            .is_ok()
    }
}
