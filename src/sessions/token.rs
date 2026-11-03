use std::{
    collections::HashMap,
    env,
    time::{SystemTime, UNIX_EPOCH},
};

use ed25519_dalek::{SigningKey, pkcs8::EncodePrivateKey};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::accounts::AuthError;

pub(super) const ACCESS_SECONDS: u64 = 15 * 60;
const ISSUER: &str = "moby-api";
const AUDIENCE: &str = "moby-desktop";
const TOKEN_TYPE: &str = "at+jwt";

#[derive(Clone)]
pub struct AccessKeys {
    kid: String,
    signing: EncodingKey,
    verification: HashMap<String, DecodingKey>,
}

#[derive(Deserialize, Serialize)]
pub(super) struct AccessClaims {
    pub(super) sub: Uuid,
    pub(super) sid: Uuid,
    jti: Uuid,
    iss: String,
    aud: String,
    iat: u64,
    exp: u64,
}

impl AccessKeys {
    pub fn ephemeral() -> Result<Self, &'static str> {
        let mut seed = [0; 32];
        getrandom::fill(&mut seed).map_err(|_| "could not generate an ephemeral signing key")?;
        Self::from_seed(seed, "")
    }

    pub(crate) fn from_env(loopback: bool) -> Result<Self, &'static str> {
        let previous = match env::var("MOBY_AUTH_PREVIOUS_PUBLIC_KEYS") {
            Ok(value) => value,
            Err(env::VarError::NotPresent) => String::new(),
            Err(env::VarError::NotUnicode(_)) => return Err("invalid previous access keys"),
        };
        match env::var("MOBY_AUTH_SIGNING_SEED") {
            Ok(seed) => Self::from_seed(parse_seed(&seed)?, &previous),
            Err(env::VarError::NotPresent)
                if previous.is_empty()
                    && (loopback
                        || env::var("MOBY_AUTH_DEV_EPHEMERAL").as_deref() == Ok("true")) =>
            {
                Self::ephemeral()
            }
            _ => Err("MOBY_AUTH_SIGNING_SEED is required for a stable signing key"),
        }
    }

    pub fn public_key_from_seed(seed: &str) -> Result<String, &'static str> {
        Ok(hex::encode(
            SigningKey::from_bytes(&parse_seed(seed)?)
                .verifying_key()
                .as_bytes(),
        ))
    }

    fn from_seed(seed: [u8; 32], previous: &str) -> Result<Self, &'static str> {
        let key = SigningKey::from_bytes(&seed);
        let public = key.verifying_key();
        let kid = fingerprint(public.as_bytes());
        let private = key
            .to_pkcs8_der()
            .map_err(|_| "could not encode the signing key")?;
        let signing = EncodingKey::from_ed_der(private.as_bytes());
        let mut verification =
            HashMap::from([(kid.clone(), DecodingKey::from_ed_der(public.as_bytes()))]);
        if !previous.is_empty() {
            for (index, item) in previous.split(',').enumerate() {
                if index >= 4 {
                    return Err("at most four previous signing keys may be configured");
                }
                let mut public = [0; 32];
                hex::decode_to_slice(item.trim(), &mut public)
                    .map_err(|_| "invalid previous access key")?;
                let key = ed25519_dalek::VerifyingKey::from_bytes(&public)
                    .map_err(|_| "invalid previous access key")?;
                if key.is_weak() {
                    return Err("invalid previous access key");
                }
                verification.insert(fingerprint(&public), DecodingKey::from_ed_der(&public));
            }
        }
        Ok(Self {
            kid,
            signing,
            verification,
        })
    }

    pub(super) fn sign(&self, user: Uuid, session: Uuid) -> Result<String, AuthError> {
        let now = timestamp()?;
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some(self.kid.clone());
        header.typ = Some(TOKEN_TYPE.into());
        jsonwebtoken::encode(
            &header,
            &AccessClaims {
                sub: user,
                sid: session,
                jti: Uuid::new_v4(),
                iss: ISSUER.into(),
                aud: AUDIENCE.into(),
                iat: now,
                exp: now + ACCESS_SECONDS,
            },
            &self.signing,
        )
        .map_err(|_| AuthError::Unavailable)
    }

    pub(super) fn verify(&self, token: &str) -> Result<AccessClaims, AuthError> {
        if token.len() > 2048 {
            return Err(AuthError::Unauthorized);
        }
        let header = jsonwebtoken::decode_header(token).map_err(|_| AuthError::Unauthorized)?;
        if header.alg != Algorithm::EdDSA || header.typ.as_deref() != Some(TOKEN_TYPE) {
            return Err(AuthError::Unauthorized);
        }
        let key = header
            .kid
            .as_ref()
            .and_then(|kid| self.verification.get(kid))
            .ok_or(AuthError::Unauthorized)?;
        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.leeway = 0;
        validation.set_required_spec_claims(&["exp", "sub", "iss", "aud"]);
        validation.set_issuer(&[ISSUER]);
        validation.set_audience(&[AUDIENCE]);
        let claims = jsonwebtoken::decode::<AccessClaims>(token, key, &validation)
            .map_err(|_| AuthError::Unauthorized)?
            .claims;
        if claims.iat > timestamp()? || claims.exp.saturating_sub(claims.iat) != ACCESS_SECONDS {
            return Err(AuthError::Unauthorized);
        }
        Ok(claims)
    }
}

fn parse_seed(value: &str) -> Result<[u8; 32], &'static str> {
    let mut seed = [0; 32];
    hex::decode_to_slice(value, &mut seed)
        .map_err(|_| "MOBY_AUTH_SIGNING_SEED must be 32 hex-encoded bytes")?;
    Ok(seed)
}

fn fingerprint(public: &[u8; 32]) -> String {
    hex::encode(&Sha256::digest(public)[..16])
}

fn timestamp() -> Result<u64, AuthError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| AuthError::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_expired_access_token_is_rejected() {
        let keys = AccessKeys::from_seed([3; 32], "").unwrap();
        let exp = timestamp().ok().unwrap() - 60;
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some(keys.kid.clone());
        header.typ = Some(TOKEN_TYPE.into());
        let token = jsonwebtoken::encode(
            &header,
            &AccessClaims {
                sub: Uuid::new_v4(),
                sid: Uuid::new_v4(),
                jti: Uuid::new_v4(),
                iss: ISSUER.into(),
                aud: AUDIENCE.into(),
                iat: exp - ACCESS_SECONDS,
                exp,
            },
            &keys.signing,
        )
        .unwrap();
        assert!(keys.verify(&token).is_err());
    }

    #[test]
    fn rotation_accepts_an_explicit_previous_key() {
        let original_seed = [1; 32];
        let original = AccessKeys::from_seed(original_seed, "").unwrap();
        let user = Uuid::new_v4();
        let session = Uuid::new_v4();
        let token = original.sign(user, session).ok().unwrap();
        let previous = AccessKeys::public_key_from_seed(&hex::encode(original_seed)).unwrap();
        let rotated = AccessKeys::from_seed([2; 32], &previous).unwrap();
        let claims = rotated.verify(&token).ok().unwrap();
        assert_eq!((claims.sub, claims.sid), (user, session));
        assert!(
            AccessKeys::from_seed([2; 32], "")
                .unwrap()
                .verify(&token)
                .is_err()
        );
    }
}
