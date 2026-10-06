//! ES256 compact JWS for the operator inbox (cloud contract §4.2, §4.6).
//!
//! - [`RelayKey`]: the machine's own relay signing key. It signs PSC-PoP
//!   proofs (`typ=psc-op-pop+jwt`, `kid` = RFC 7638 thumbprint, never an
//!   embedded `jwk`). It is not the hub's DPoP key and never signs for a hub.
//! - [`IssuerKeys`]: verification of cloud issuer tokens against the issuer
//!   JWKS, with an exact `typ` per token kind so no token stands in for
//!   another. The JWKS is cached for at most one hour (Q3 default) and an
//!   unknown `kid` forces at most one refetch per minute; verify-only keys
//!   verify (the cloud publishes them 91 days).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use cas_operator_crypto::Zeroizing;
use p256::ecdsa::signature::{Signer, Verifier};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

pub const POP_TYP: &str = "psc-op-pop+jwt";
pub const TYP_EPOCH_MANIFEST: &str = "psc-op-epoch-manifest+jwt";
pub const TYP_MACHINE_BINDING: &str = "psc-op-machine-binding+jwt";
pub const TYP_ENROLLMENT: &str = "psc-op-enrollment+jwt";
pub const TYP_COMMAND_ADMISSION: &str = "psc-op-command-admission+jwt";
pub const TYP_MACHINE_OBSERVATION: &str = "psc-op-machine-observation+jwt";

pub const JWKS_MAX_AGE: Duration = Duration::from_secs(3600);
const UNKNOWN_KID_REFETCH: Duration = Duration::from_secs(60);
/// The server accepts at most 120 s (§4.2); 60 s leaves room for skew.
const PROOF_LIFETIME_S: i64 = 60;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum JwsError {
    #[error("malformed token: {0}")]
    Malformed(&'static str),
    #[error("token type {0} is not the expected type")]
    WrongType(String),
    #[error("issuer key {0} is not published")]
    UnknownKey(String),
    #[error("issuer signature does not verify")]
    BadSignature,
    #[error("token expired")]
    Expired,
    #[error("issuer JWKS unavailable")]
    JwksUnavailable,
    #[error("invalid P-256 key")]
    InvalidKey,
}

pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

fn unb64(text: &str, what: &'static str) -> Result<Vec<u8>, JwsError> {
    URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|_| JwsError::Malformed(what))
}

/// RFC 7638 SHA-256 thumbprint of a P-256 public key given as x and y.
pub fn thumbprint(x: &str, y: &str) -> String {
    let canonical = format!(r#"{{"crv":"P-256","kty":"EC","x":"{x}","y":"{y}"}}"#);
    b64(&Sha256::digest(canonical.as_bytes()))
}

/// What a PoP proof is bound to besides the request (§4.2).
pub enum ProofBinding<'a> {
    Grant {
        grant_id: &'a str,
        generation: &'a str,
    },
    Enrollment {
        enrollment_id: &'a str,
    },
}

/// One relay request as the proof covers it.
pub struct ProofRequest<'a> {
    pub audience: &'a str,
    pub method: &'a str,
    /// Path and query exactly as sent.
    pub path_and_query: &'a str,
    /// Base64url SHA-256 of the exact body bytes (zero bytes when empty).
    pub body_digest: &'a str,
    pub binding: ProofBinding<'a>,
    /// Unix seconds, already corrected for observed server skew.
    pub now: i64,
}

/// The machine's relay signing key (P-256). Deliberately no Debug.
pub struct RelayKey {
    key: SigningKey,
    x: String,
    y: String,
}

impl RelayKey {
    pub fn generate() -> Self {
        loop {
            let bytes = Zeroizing::new(rand::random::<[u8; 32]>());
            if let Ok(key) = Self::from_secret(&bytes[..]) {
                return key;
            }
        }
    }

    pub fn from_secret(secret: &[u8]) -> Result<Self, JwsError> {
        let key = SigningKey::from_slice(secret).map_err(|_| JwsError::InvalidKey)?;
        let point = key.verifying_key().to_encoded_point(false);
        let bytes = point.as_bytes();
        if bytes.len() != 65 {
            return Err(JwsError::InvalidKey);
        }
        Ok(Self {
            x: b64(&bytes[1..33]),
            y: b64(&bytes[33..65]),
            key,
        })
    }

    /// The 32-byte private scalar, for the 0600 principal file only.
    pub fn secret(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(self.key.to_bytes().to_vec())
    }

    pub fn public_jwk(&self) -> Value {
        json!({"kty": "EC", "crv": "P-256", "x": self.x, "y": self.y})
    }

    /// The grant's `signing_jkt`.
    pub fn thumbprint(&self) -> String {
        thumbprint(&self.x, &self.y)
    }

    pub fn proof(&self, request: &ProofRequest<'_>) -> String {
        let header = json!({"typ": POP_TYP, "alg": "ES256", "kid": self.thumbprint()});
        let mut claims = Map::new();
        claims.insert("aud".into(), request.audience.into());
        claims.insert("htm".into(), request.method.to_ascii_uppercase().into());
        claims.insert("htp".into(), request.path_and_query.into());
        claims.insert("bdg".into(), request.body_digest.into());
        claims.insert("iat".into(), request.now.into());
        claims.insert("exp".into(), (request.now + PROOF_LIFETIME_S).into());
        claims.insert("jti".into(), b64(&rand::random::<[u8; 16]>()).into());
        match request.binding {
            ProofBinding::Grant {
                grant_id,
                generation,
            } => {
                claims.insert("gid".into(), grant_id.into());
                claims.insert("gen".into(), generation.into());
            }
            ProofBinding::Enrollment { enrollment_id } => {
                claims.insert("enr".into(), enrollment_id.into());
            }
        }
        self.sign(&header, &Value::Object(claims))
    }

    fn sign(&self, header: &Value, claims: &Value) -> String {
        let input = format!(
            "{}.{}",
            b64(header.to_string().as_bytes()),
            b64(claims.to_string().as_bytes())
        );
        let signature: Signature = self.key.sign(input.as_bytes());
        format!("{input}.{}", b64(&signature.to_bytes()))
    }
}

/// A verified issuer token.
#[derive(Debug, Clone)]
pub struct Verified {
    pub kid: String,
    pub claims: Map<String, Value>,
}

struct Cache {
    /// `(kid, 65-byte SEC1 point)` of every usable published key.
    keys: Vec<(String, Vec<u8>)>,
    fetched: Instant,
}

/// Issuer JWKS fetcher; production uses `GET /api/operator/jwks`.
pub type JwksFetch = Box<dyn Fn() -> Result<Value, JwsError> + Send + Sync>;

pub struct IssuerKeys {
    fetch: JwksFetch,
    cache: Mutex<Option<Cache>>,
}

fn parse_jwks(value: &Value) -> Result<Vec<(String, Vec<u8>)>, JwsError> {
    let keys = value
        .get("keys")
        .and_then(Value::as_array)
        .ok_or(JwsError::JwksUnavailable)?;
    let mut out = Vec::new();
    for key in keys {
        let field = |name: &str| key.get(name).and_then(Value::as_str);
        if field("kty") != Some("EC") || field("crv") != Some("P-256") {
            continue;
        }
        if field("alg").is_some_and(|alg| alg != "ES256")
            || field("use").is_some_and(|usage| usage != "sig")
        {
            continue;
        }
        let (Some(kid), Some(x), Some(y)) = (field("kid"), field("x"), field("y")) else {
            continue;
        };
        let (Ok(x), Ok(y)) = (unb64(x, "x"), unb64(y, "y")) else {
            continue;
        };
        if x.len() != 32 || y.len() != 32 {
            continue;
        }
        let mut point = Vec::with_capacity(65);
        point.push(4);
        point.extend_from_slice(&x);
        point.extend_from_slice(&y);
        if VerifyingKey::from_sec1_bytes(&point).is_ok() {
            out.push((kid.to_owned(), point));
        }
    }
    Ok(out)
}

fn segment(text: &str, what: &'static str) -> Result<Map<String, Value>, JwsError> {
    let bytes = unb64(text, what)?;
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(JwsError::Malformed(what)),
    }
}

impl IssuerKeys {
    pub fn new(fetch: JwksFetch) -> Self {
        Self {
            fetch,
            cache: Mutex::new(None),
        }
    }

    fn key(&self, kid: &str) -> Result<VerifyingKey, JwsError> {
        let mut cache = self.cache.lock().map_err(|_| JwsError::JwksUnavailable)?;
        let stale = cache
            .as_ref()
            .is_none_or(|cache| cache.fetched.elapsed() >= JWKS_MAX_AGE);
        let mut refreshed = false;
        if stale {
            *cache = Some(Cache {
                keys: parse_jwks(&(self.fetch)()?)?,
                fetched: Instant::now(),
            });
            refreshed = true;
        }
        let find = |cache: &Option<Cache>| {
            cache.as_ref().and_then(|cache| {
                cache
                    .keys
                    .iter()
                    .find(|(id, _)| id == kid)
                    .and_then(|(_, point)| VerifyingKey::from_sec1_bytes(point).ok())
            })
        };
        if let Some(key) = find(&*cache) {
            return Ok(key);
        }
        let may_refetch = !refreshed
            && cache
                .as_ref()
                .is_some_and(|cache| cache.fetched.elapsed() >= UNKNOWN_KID_REFETCH);
        if may_refetch {
            *cache = Some(Cache {
                keys: parse_jwks(&(self.fetch)()?)?,
                fetched: Instant::now(),
            });
            if let Some(key) = find(&*cache) {
                return Ok(key);
            }
        }
        Err(JwsError::UnknownKey(kid.to_owned()))
    }

    /// Verify signature, `alg`, the exact `typ` and `exp > now`. The caller
    /// checks every claim that binds its use (account, hub, epoch, digest…).
    pub fn verify(&self, token: &str, typ: &str, now: i64) -> Result<Verified, JwsError> {
        let mut parts = token.split('.');
        let (Some(header_b64), Some(claims_b64), Some(signature_b64), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(JwsError::Malformed("segments"));
        };
        let header = segment(header_b64, "header")?;
        if header.get("alg").and_then(Value::as_str) != Some("ES256") {
            return Err(JwsError::Malformed("alg"));
        }
        let found_typ = header.get("typ").and_then(Value::as_str).unwrap_or("");
        if found_typ != typ {
            return Err(JwsError::WrongType(found_typ.to_owned()));
        }
        if header.contains_key("jwk") || header.contains_key("x5c") || header.contains_key("jku") {
            return Err(JwsError::Malformed("embedded key"));
        }
        let kid = header
            .get("kid")
            .and_then(Value::as_str)
            .ok_or(JwsError::Malformed("kid"))?
            .to_owned();
        let claims = segment(claims_b64, "claims")?;
        let signature = Signature::from_slice(&unb64(signature_b64, "signature")?)
            .map_err(|_| JwsError::BadSignature)?;
        let key = self.key(&kid)?;
        key.verify(format!("{header_b64}.{claims_b64}").as_bytes(), &signature)
            .map_err(|_| JwsError::BadSignature)?;
        match claims.get("exp").and_then(Value::as_i64) {
            Some(exp) if exp > now => {}
            _ => return Err(JwsError::Expired),
        }
        Ok(Verified { kid, claims })
    }
}

/// A claim as a string, or a malformed-token refusal naming it.
pub fn claim_str<'a>(verified: &'a Verified, name: &'static str) -> Result<&'a str, JwsError> {
    verified
        .claims
        .get(name)
        .and_then(Value::as_str)
        .ok_or(JwsError::Malformed(name))
}

#[cfg(test)]
pub(crate) mod test_issuer {
    //! A test issuer that signs like the cloud (§4.6), for verifier tests.
    use super::*;

    pub struct TestIssuer {
        pub kid: String,
        key: RelayKey,
    }

    impl TestIssuer {
        pub fn new(kid: &str) -> Self {
            Self {
                kid: kid.into(),
                key: RelayKey::generate(),
            }
        }

        pub fn jwks(&self) -> Value {
            json!({"keys": [{"kty": "EC", "crv": "P-256", "x": self.key.x, "y": self.key.y, "kid": self.kid, "alg": "ES256", "use": "sig"}]})
        }

        pub fn sign(&self, typ: &str, claims: Value) -> String {
            self.key.sign(
                &json!({"typ": typ, "alg": "ES256", "kid": self.kid}),
                &claims,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_issuer::TestIssuer;
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn issuer_keys(issuer: &Arc<TestIssuer>, fetches: &Arc<AtomicUsize>) -> IssuerKeys {
        let issuer = Arc::clone(issuer);
        let fetches = Arc::clone(fetches);
        IssuerKeys::new(Box::new(move || {
            fetches.fetch_add(1, Ordering::SeqCst);
            Ok(issuer.jwks())
        }))
    }

    #[test]
    fn rfc7638_thumbprint_matches_the_rfc_example_shape() {
        // RFC 7638 §3.1 orders members crv, kty, x, y with no whitespace; a
        // different member order would change the digest.
        let x = "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU";
        let y = "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0";
        let expected = b64(&Sha256::digest(
            format!(r#"{{"crv":"P-256","kty":"EC","x":"{x}","y":"{y}"}}"#).as_bytes(),
        ));
        assert_eq!(thumbprint(x, y), expected);
        assert_ne!(thumbprint(y, x), expected);
    }

    #[test]
    fn pop_proof_carries_the_exact_bindings_and_verifies() {
        let key = RelayKey::generate();
        let proof = key.proof(&ProofRequest {
            audience: "https://psc.test/api/operator",
            method: "post",
            path_and_query: "/api/operator/feed/events",
            body_digest: "47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU",
            binding: ProofBinding::Grant {
                grant_id: "g-1",
                generation: "3",
            },
            now: 1_000,
        });
        let parts: Vec<&str> = proof.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header = segment(parts[0], "header").expect("header");
        assert_eq!(header["typ"], POP_TYP);
        assert_eq!(header["kid"], key.thumbprint());
        assert!(!header.contains_key("jwk"));
        let claims = segment(parts[1], "claims").expect("claims");
        assert_eq!(claims["htm"], "POST");
        assert_eq!(claims["htp"], "/api/operator/feed/events");
        assert_eq!(claims["gid"], "g-1");
        assert_eq!(claims["gen"], "3");
        assert_eq!(claims["exp"], 1_060);
        assert!(!claims.contains_key("enr"));
        let point = key.key.verifying_key();
        let signature =
            Signature::from_slice(&unb64(parts[2], "sig").expect("sig")).expect("signature");
        point
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .expect("proof verifies with the relay key");

        let reloaded = RelayKey::from_secret(&key.secret()).expect("reload");
        assert_eq!(reloaded.thumbprint(), key.thumbprint());
    }

    #[test]
    fn issuer_tokens_verify_only_with_their_exact_type() {
        let issuer = Arc::new(TestIssuer::new("op-test-1"));
        let fetches = Arc::new(AtomicUsize::new(0));
        let keys = issuer_keys(&issuer, &fetches);
        let token = issuer.sign(TYP_MACHINE_BINDING, json!({"acct": "a", "exp": 2_000}));
        let verified = keys
            .verify(&token, TYP_MACHINE_BINDING, 1_000)
            .expect("verifies");
        assert_eq!(claim_str(&verified, "acct"), Ok("a"));
        assert_eq!(
            keys.verify(&token, TYP_COMMAND_ADMISSION, 1_000).err(),
            Some(JwsError::WrongType(TYP_MACHINE_BINDING.into()))
        );
        assert_eq!(
            keys.verify(&token, TYP_MACHINE_BINDING, 2_000).err(),
            Some(JwsError::Expired)
        );
        let mut tampered: Vec<&str> = token.split('.').collect();
        let forged = b64(br#"{"acct":"b","exp":2000}"#);
        tampered[1] = &forged;
        assert_eq!(
            keys.verify(&tampered.join("."), TYP_MACHINE_BINDING, 1_000)
                .err(),
            Some(JwsError::BadSignature)
        );
        // One fetch served every verification inside the cache window.
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_unknown_issuer_key_is_refused_without_hammering_the_jwks() {
        let issuer = Arc::new(TestIssuer::new("op-test-1"));
        let stranger = TestIssuer::new("op-unpublished");
        let fetches = Arc::new(AtomicUsize::new(0));
        let keys = issuer_keys(&issuer, &fetches);
        let token = stranger.sign(TYP_EPOCH_MANIFEST, json!({"exp": 2_000}));
        for _ in 0..5 {
            assert_eq!(
                keys.verify(&token, TYP_EPOCH_MANIFEST, 1_000).err(),
                Some(JwsError::UnknownKey("op-unpublished".into()))
            );
        }
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }
}
