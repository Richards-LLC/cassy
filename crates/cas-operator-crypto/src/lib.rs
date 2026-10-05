//! HPKE envelopes for the Commander operator inbox (cloud wire v1).
//!
//! One suite everywhere: DHKEM(P-256, HKDF-SHA256) / HKDF-SHA256 /
//! AES-256-GCM (`0x0010/0x0001/0x0002`, base mode), as pinned by the cloud
//! contract §6.6 and implemented in the browser by `@hpke/core` 1.9.0.
//!
//! Formats, by owner:
//!
//! | Format | Owner | Section |
//! | --- | --- | --- |
//! | Epoch wrap (epoch private scalar → device) | cloud | contract §6.4 |
//! | Enrollment key check | cloud | contract §5.1, §5.4 |
//! | Observer envelope v1 (`psc-op-observer-v1`) | cloud | contract §7.4 |
//! | Session event envelope v1 (`cas-op-event-v1`) | cas-src | cas-9b7d DESIGN D3 |
//! | Command envelope v1 (`cas-op-command-v1`) | cas-src | cas-9b7d DESIGN D4 |
//!
//! Keys cross this crate's boundary as bytes only: a P-256 private key is the
//! 32-byte big-endian scalar and a public key the 65-byte uncompressed SEC1
//! point, matching `serializePrivateKey`/`serializePublicKey` in `@hpke/core`.
//! The browser mirror is `hub-web/src/inbox/hpke.ts`; both sides are pinned to
//! the same committed fixtures.

use aes_gcm::aead::{Aead as _, Nonce, Payload};
use aes_gcm::{Aes256Gcm, KeyInit as _};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hpke::aead::AesGcm256;
use hpke::kdf::HkdfSha256;
use hpke::kem::DhP256HkdfSha256;
use hpke::rand_core::{TryCryptoRng, TryRng};
use hpke::{Deserializable as _, Kem as _, OpModeR, OpModeS, Serializable as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::convert::Infallible;
use zeroize::Zeroizing;

type Kem = DhP256HkdfSha256;
type PrivateKey = <Kem as hpke::Kem>::PrivateKey;
type PublicKey = <Kem as hpke::Kem>::PublicKey;
type EncappedKey = <Kem as hpke::Kem>::EncappedKey;

/// Size of a serialized P-256 private key (big-endian scalar).
pub const SECRET_KEY_LEN: usize = 32;
/// Size of a serialized P-256 public key (uncompressed SEC1 point).
pub const PUBLIC_KEY_LEN: usize = 65;
/// AES-256-GCM content key size.
pub const CONTENT_KEY_LEN: usize = 32;
/// AES-GCM nonce size.
pub const NONCE_LEN: usize = 12;

/// HPKE `info` of the cloud's per-device epoch wrap (§6.4).
pub const EPOCH_WRAP_INFO: &str = "psc-op-epoch-wrap-v1";
/// HPKE `info` of the enrollment encryption-key check (§5.1, §5.4).
pub const ENROLLMENT_CHECK_INFO: &str = "psc-op-enc-check-v1";

/// Observer envelope v1 (cloud-owned, §7.4).
pub const OBSERVER_ALG: &str = "psc-op-observer-v1";
pub const OBSERVER_KEY_INFO: &str = "psc-op-observer-key-v1";
pub const OBSERVER_CONTENT_TYPE: &str = "application/vnd.psc.operator.machine-presence+json; v=1";

/// Session event envelope v1 (cas-src-owned, DESIGN D3).
pub const EVENT_ALG: &str = "cas-op-event-v1";
pub const EVENT_KEY_INFO: &str = "cas-op-event-key-v1";
pub const EVENT_CONTENT_TYPE: &str = "application/vnd.cas.operator.turn+json; v=1";

/// Command envelope v1 (cas-src-owned, DESIGN D4).
pub const COMMAND_ALG: &str = "cas-op-command-v1";
pub const COMMAND_INFO: &str = "cas-op-command-v1";

/// Upper bound on any decoded envelope this crate opens (§7.1, §10.1).
pub const MAX_ENVELOPE_BYTES: usize = 65_536;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CryptoError {
    #[error("malformed {0}")]
    Malformed(&'static str),
    #[error("unsupported envelope version or algorithm")]
    Unsupported,
    #[error("envelope epoch {found} does not match the row's key epoch {expected}")]
    EpochMismatch { expected: String, found: String },
    #[error("envelope is larger than 65536 bytes")]
    TooLarge,
    #[error("invalid P-256 key")]
    InvalidKey,
    #[error("ciphertext failed authentication")]
    OpenFailed,
    #[error("sealing failed")]
    SealFailed,
}

pub type Result<T> = std::result::Result<T, CryptoError>;

// ---------------------------------------------------------------- encoding

pub fn b64url_encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn b64url_decode(text: &str, what: &'static str) -> Result<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|_| CryptoError::Malformed(what))
}

/// The wire digest: `sha256:` + 64 lowercase hex of the given bytes (§4.3).
pub fn digest(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    let mut out = String::with_capacity(7 + 64);
    out.push_str("sha256:");
    for byte in hash {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

// ---------------------------------------------------------------- keys

/// A P-256 key pair as bytes. The secret is zeroized on drop.
pub struct KeyPair {
    pub secret: Zeroizing<[u8; SECRET_KEY_LEN]>,
    pub public: [u8; PUBLIC_KEY_LEN],
}

fn private_key(secret: &[u8]) -> Result<PrivateKey> {
    if secret.len() != SECRET_KEY_LEN {
        return Err(CryptoError::InvalidKey);
    }
    PrivateKey::from_bytes(secret).map_err(|_| CryptoError::InvalidKey)
}

fn public_key(public: &[u8]) -> Result<PublicKey> {
    if public.len() != PUBLIC_KEY_LEN {
        return Err(CryptoError::InvalidKey);
    }
    PublicKey::from_bytes(public).map_err(|_| CryptoError::InvalidKey)
}

fn encapped_key(enc: &[u8]) -> Result<EncappedKey> {
    EncappedKey::from_bytes(enc).map_err(|_| CryptoError::Malformed("enc"))
}

fn key_pair_from(sk: &PrivateKey, pk: &PublicKey) -> KeyPair {
    let mut secret = Zeroizing::new([0u8; SECRET_KEY_LEN]);
    secret.copy_from_slice(&sk.to_bytes());
    let mut public = [0u8; PUBLIC_KEY_LEN];
    public.copy_from_slice(&pk.to_bytes());
    KeyPair { secret, public }
}

/// A fresh random key pair (machine command key, test recipients).
pub fn generate_key_pair() -> KeyPair {
    let (sk, pk) = Kem::gen_keypair();
    key_pair_from(&sk, &pk)
}

/// RFC 9180 DeriveKeyPair, for deterministic fixtures only.
pub fn derive_key_pair(ikm: &[u8]) -> KeyPair {
    let (sk, pk) = Kem::derive_keypair(ikm);
    key_pair_from(&sk, &pk)
}

/// The public key of a serialized private key.
pub fn public_key_of(secret: &[u8]) -> Result<[u8; PUBLIC_KEY_LEN]> {
    let pk = Kem::sk_to_pk(&private_key(secret)?);
    let mut public = [0u8; PUBLIC_KEY_LEN];
    public.copy_from_slice(&pk.to_bytes());
    Ok(public)
}

// ---------------------------------------------------------------- HPKE core

/// Feeds a fixed ephemeral IKM to the HPKE sender. HPKE draws exactly one
/// 32-byte IKM per encapsulation (`gen_keypair_with_rng` →
/// `derive_keypair`), which is what `@hpke/core` reproduces with
/// `ekm = deriveKeyPair(ikmE)`. Only the deterministic `*_with` functions use
/// it; production sealing uses the system RNG.
struct FixedIkm([u8; 32]);

impl TryRng for FixedIkm {
    type Error = Infallible;

    fn try_next_u32(&mut self) -> std::result::Result<u32, Infallible> {
        let mut bytes = [0u8; 4];
        self.try_fill_bytes(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn try_next_u64(&mut self) -> std::result::Result<u64, Infallible> {
        let mut bytes = [0u8; 8];
        self.try_fill_bytes(&mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> std::result::Result<(), Infallible> {
        for (index, byte) in dst.iter_mut().enumerate() {
            *byte = self.0[index % self.0.len()];
        }
        Ok(())
    }
}

impl TryCryptoRng for FixedIkm {}

/// `(enc, ciphertext)` of an HPKE base-mode single-shot seal.
pub struct HpkeSealed {
    pub enc: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

fn hpke_seal(
    recipient: &[u8],
    info: &[u8],
    plaintext: &[u8],
    aad: &[u8],
    ikm_ephemeral: Option<&[u8; 32]>,
) -> Result<HpkeSealed> {
    let pk = public_key(recipient)?;
    let (enc, ciphertext) = match ikm_ephemeral {
        Some(ikm) => hpke::single_shot_seal_with_rng::<AesGcm256, HkdfSha256, Kem>(
            &OpModeS::Base,
            &pk,
            info,
            plaintext,
            aad,
            &mut FixedIkm(*ikm),
        ),
        None => hpke::single_shot_seal::<AesGcm256, HkdfSha256, Kem>(
            &OpModeS::Base,
            &pk,
            info,
            plaintext,
            aad,
        ),
    }
    .map_err(|_| CryptoError::SealFailed)?;
    Ok(HpkeSealed {
        enc: enc.to_bytes().to_vec(),
        ciphertext,
    })
}

/// HPKE base-mode seal to `recipient` (65-byte public key).
pub fn seal(recipient: &[u8], info: &[u8], plaintext: &[u8], aad: &[u8]) -> Result<HpkeSealed> {
    hpke_seal(recipient, info, plaintext, aad, None)
}

/// Deterministic seal with a fixed ephemeral IKM (fixtures only).
pub fn seal_with_ikm(
    recipient: &[u8],
    info: &[u8],
    plaintext: &[u8],
    aad: &[u8],
    ikm_ephemeral: &[u8; 32],
) -> Result<HpkeSealed> {
    hpke_seal(recipient, info, plaintext, aad, Some(ikm_ephemeral))
}

/// HPKE base-mode open with `secret` (32-byte private key).
pub fn open(
    secret: &[u8],
    enc: &[u8],
    info: &[u8],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    let sk = private_key(secret)?;
    let enc = encapped_key(enc)?;
    hpke::single_shot_open::<AesGcm256, HkdfSha256, Kem>(
        &OpModeR::Base,
        &sk,
        &enc,
        info,
        ciphertext,
        aad,
    )
    .map(Zeroizing::new)
    .map_err(|_| CryptoError::OpenFailed)
}

fn random_bytes<const N: usize>() -> [u8; N] {
    use rand::RngCore as _;
    let mut bytes = [0u8; N];
    rand::rng().fill_bytes(&mut bytes);
    bytes
}

fn aes_gcm_seal(key: &[u8], nonce: &[u8], plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::InvalidKey)?;
    let nonce = Nonce::<Aes256Gcm>::try_from(nonce).map_err(|_| CryptoError::Malformed("nonce"))?;
    cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::SealFailed)
}

fn aes_gcm_open(
    key: &[u8],
    nonce: &[u8],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::InvalidKey)?;
    let nonce = Nonce::<Aes256Gcm>::try_from(nonce).map_err(|_| CryptoError::Malformed("nonce"))?;
    cipher
        .decrypt(
            &nonce,
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| CryptoError::OpenFailed)
}

// ---------------------------------------------------------------- cloud §6.4

/// Binding of an epoch wrap: `account_id|feed_generation|epoch|device_id`.
pub struct EpochWrapIds<'a> {
    pub account_id: &'a str,
    pub feed_generation: &'a str,
    pub epoch: &'a str,
    pub device_id: &'a str,
}

impl EpochWrapIds<'_> {
    pub fn aad(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.account_id, self.feed_generation, self.epoch, self.device_id
        )
    }
}

/// Open a `/keys/wraps` entry: the epoch private scalar, sealed to the device
/// encryption key. The result is checked to be a valid P-256 private key.
pub fn open_epoch_wrap(
    device_secret: &[u8],
    enc: &[u8],
    ciphertext: &[u8],
    ids: &EpochWrapIds<'_>,
) -> Result<Zeroizing<[u8; SECRET_KEY_LEN]>> {
    let plain = open(
        device_secret,
        enc,
        EPOCH_WRAP_INFO.as_bytes(),
        ciphertext,
        ids.aad().as_bytes(),
    )?;
    if plain.len() != SECRET_KEY_LEN {
        return Err(CryptoError::InvalidKey);
    }
    private_key(&plain)?;
    let mut secret = Zeroizing::new([0u8; SECRET_KEY_LEN]);
    secret.copy_from_slice(&plain);
    Ok(secret)
}

/// Open the enrollment `encryption_key_check` (§5.1, §5.4); the caller sends
/// the 32-byte plaintext back, base64url, to complete enrollment.
pub fn open_enrollment_check(
    secret: &[u8],
    enc: &[u8],
    ciphertext: &[u8],
    enrollment_id: &str,
) -> Result<Zeroizing<Vec<u8>>> {
    open(
        secret,
        enc,
        ENROLLMENT_CHECK_INFO.as_bytes(),
        ciphertext,
        enrollment_id.as_bytes(),
    )
}

// ---------------------------------------------------------------- envelopes

/// The shared JSON shape of the observer and session envelopes. Field order
/// is the wire order; serde preserves it, so the bytes match
/// `JSON.stringify` of the same object literal in the browser.
#[derive(Serialize, Deserialize)]
struct WrappedEnvelope {
    v: u8,
    alg: String,
    epoch: String,
    enc: String,
    wk: String,
    n: String,
    ct: String,
}

/// A sealed event: the exact ciphertext bytes to upload and their digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SealedEnvelope {
    pub bytes: Vec<u8>,
    pub digest: String,
}

struct WrappedSpec<'a> {
    alg: &'static str,
    key_info: &'static str,
    epoch: &'a str,
    aad: String,
}

struct WrappedRandomness<'a> {
    content_key: &'a [u8; CONTENT_KEY_LEN],
    nonce: &'a [u8; NONCE_LEN],
    ikm_ephemeral: Option<&'a [u8; 32]>,
}

fn seal_wrapped(
    spec: &WrappedSpec<'_>,
    epoch_public: &[u8],
    plaintext: &[u8],
    randomness: &WrappedRandomness<'_>,
) -> Result<SealedEnvelope> {
    let aad = spec.aad.as_bytes();
    let wrapped = hpke_seal(
        epoch_public,
        spec.key_info.as_bytes(),
        randomness.content_key,
        aad,
        randomness.ikm_ephemeral,
    )?;
    let ct = aes_gcm_seal(randomness.content_key, randomness.nonce, plaintext, aad)?;
    let envelope = WrappedEnvelope {
        v: 1,
        alg: spec.alg.to_owned(),
        epoch: spec.epoch.to_owned(),
        enc: b64url_encode(&wrapped.enc),
        wk: b64url_encode(&wrapped.ciphertext),
        n: b64url_encode(randomness.nonce),
        ct: b64url_encode(&ct),
    };
    let bytes = serde_json::to_vec(&envelope).map_err(|_| CryptoError::SealFailed)?;
    if bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(CryptoError::TooLarge);
    }
    let digest = digest(&bytes);
    Ok(SealedEnvelope { bytes, digest })
}

fn open_wrapped(
    spec: &WrappedSpec<'_>,
    epoch_secret: &[u8],
    envelope: &[u8],
) -> Result<Zeroizing<Vec<u8>>> {
    if envelope.len() > MAX_ENVELOPE_BYTES {
        return Err(CryptoError::TooLarge);
    }
    let parsed: WrappedEnvelope =
        serde_json::from_slice(envelope).map_err(|_| CryptoError::Malformed("envelope"))?;
    if parsed.v != 1 || parsed.alg != spec.alg {
        return Err(CryptoError::Unsupported);
    }
    if parsed.epoch != spec.epoch {
        return Err(CryptoError::EpochMismatch {
            expected: spec.epoch.to_owned(),
            found: parsed.epoch,
        });
    }
    let aad = spec.aad.as_bytes();
    let content_key = open(
        epoch_secret,
        &b64url_decode(&parsed.enc, "enc")?,
        spec.key_info.as_bytes(),
        &b64url_decode(&parsed.wk, "wk")?,
        aad,
    )?;
    if content_key.len() != CONTENT_KEY_LEN {
        return Err(CryptoError::Malformed("content key"));
    }
    let nonce = b64url_decode(&parsed.n, "n")?;
    if nonce.len() != NONCE_LEN {
        return Err(CryptoError::Malformed("n"));
    }
    aes_gcm_open(&content_key, &nonce, &b64url_decode(&parsed.ct, "ct")?, aad)
}

// ------------------------------------------------- observer envelope (§7.4)

/// Row values an observer envelope is bound to.
pub struct ObserverIds<'a> {
    pub account_id: &'a str,
    pub feed_generation: &'a str,
    pub key_epoch: &'a str,
    pub event_id: &'a str,
    pub hub_id: &'a str,
}

impl<'a> ObserverIds<'a> {
    pub fn aad(&self) -> String {
        format!(
            "{OBSERVER_ALG}|{}|{}|{}|{}|{}|{OBSERVER_CONTENT_TYPE}",
            self.account_id, self.feed_generation, self.key_epoch, self.event_id, self.hub_id
        )
    }

    fn spec(&self) -> WrappedSpec<'a> {
        WrappedSpec {
            alg: OBSERVER_ALG,
            key_info: OBSERVER_KEY_INFO,
            epoch: self.key_epoch,
            aad: self.aad(),
        }
    }
}

/// Open a cloud-observer notice. The caller still runs the §7.4 assertion
/// checks (signature, claims, digest) before persisting or ACKing it.
pub fn open_observer_notice(
    epoch_secret: &[u8],
    envelope: &[u8],
    ids: &ObserverIds<'_>,
) -> Result<Zeroizing<Vec<u8>>> {
    open_wrapped(&ids.spec(), epoch_secret, envelope)
}

/// Reproduce a cloud observer notice byte-for-byte (fixture parity only; the
/// cloud is the only producer of observer notices).
pub fn seal_observer_notice_with(
    epoch_public: &[u8],
    plaintext: &[u8],
    ids: &ObserverIds<'_>,
    content_key: &[u8; CONTENT_KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    ikm_ephemeral: &[u8; 32],
) -> Result<SealedEnvelope> {
    seal_wrapped(
        &ids.spec(),
        epoch_public,
        plaintext,
        &WrappedRandomness {
            content_key,
            nonce,
            ikm_ephemeral: Some(ikm_ephemeral),
        },
    )
}

// ------------------------------------------ session event envelope (D3)

/// Row values a session event is bound to. Every value is an opaque routing
/// ID (§4.3); human names travel inside the plaintext.
pub struct EventIds<'a> {
    pub account_id: &'a str,
    pub feed_generation: &'a str,
    pub key_epoch: &'a str,
    pub event_id: &'a str,
    pub hub_id: &'a str,
    pub project_id: &'a str,
    pub session_id: &'a str,
}

impl<'a> EventIds<'a> {
    pub fn aad(&self) -> String {
        format!(
            "{EVENT_ALG}|{}|{}|{}|{}|{}|{}|{}|{EVENT_CONTENT_TYPE}",
            self.account_id,
            self.feed_generation,
            self.key_epoch,
            self.event_id,
            self.hub_id,
            self.project_id,
            self.session_id
        )
    }

    fn spec(&self) -> WrappedSpec<'a> {
        WrappedSpec {
            alg: EVENT_ALG,
            key_info: EVENT_KEY_INFO,
            epoch: self.key_epoch,
            aad: self.aad(),
        }
    }
}

/// Seal a session event to the epoch public key with fresh randomness.
pub fn seal_event(
    epoch_public: &[u8],
    plaintext: &[u8],
    ids: &EventIds<'_>,
) -> Result<SealedEnvelope> {
    let content_key = Zeroizing::new(random_bytes::<CONTENT_KEY_LEN>());
    let nonce = random_bytes::<NONCE_LEN>();
    seal_wrapped(
        &ids.spec(),
        epoch_public,
        plaintext,
        &WrappedRandomness {
            content_key: &content_key,
            nonce: &nonce,
            ikm_ephemeral: None,
        },
    )
}

/// Deterministic [`seal_event`] (fixtures only).
pub fn seal_event_with(
    epoch_public: &[u8],
    plaintext: &[u8],
    ids: &EventIds<'_>,
    content_key: &[u8; CONTENT_KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    ikm_ephemeral: &[u8; 32],
) -> Result<SealedEnvelope> {
    seal_wrapped(
        &ids.spec(),
        epoch_public,
        plaintext,
        &WrappedRandomness {
            content_key,
            nonce,
            ikm_ephemeral: Some(ikm_ephemeral),
        },
    )
}

pub fn open_event(
    epoch_secret: &[u8],
    envelope: &[u8],
    ids: &EventIds<'_>,
) -> Result<Zeroizing<Vec<u8>>> {
    open_wrapped(&ids.spec(), epoch_secret, envelope)
}

// ---------------------------------------------- command envelope (D4)

#[derive(Serialize, Deserialize)]
struct CommandEnvelope {
    v: u8,
    alg: String,
    kid: String,
    enc: String,
    ct: String,
}

/// Values a command is bound to (§10.1 routing plus the machine key ID).
pub struct CommandIds<'a> {
    pub account_id: &'a str,
    pub machine_id: &'a str,
    pub command_id: &'a str,
    pub hub_id: &'a str,
    pub project_id: &'a str,
    pub session_id: &'a str,
    pub operation: &'a str,
    pub machine_key_id: &'a str,
}

impl CommandIds<'_> {
    pub fn aad(&self) -> String {
        format!(
            "{COMMAND_ALG}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.account_id,
            self.machine_id,
            self.command_id,
            self.hub_id,
            self.project_id,
            self.session_id,
            self.operation,
            self.machine_key_id
        )
    }
}

fn seal_command_inner(
    machine_public: &[u8],
    plaintext: &[u8],
    ids: &CommandIds<'_>,
    ikm_ephemeral: Option<&[u8; 32]>,
) -> Result<SealedEnvelope> {
    let sealed = hpke_seal(
        machine_public,
        COMMAND_INFO.as_bytes(),
        plaintext,
        ids.aad().as_bytes(),
        ikm_ephemeral,
    )?;
    let envelope = CommandEnvelope {
        v: 1,
        alg: COMMAND_ALG.to_owned(),
        kid: ids.machine_key_id.to_owned(),
        enc: b64url_encode(&sealed.enc),
        ct: b64url_encode(&sealed.ciphertext),
    };
    let bytes = serde_json::to_vec(&envelope).map_err(|_| CryptoError::SealFailed)?;
    if bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(CryptoError::TooLarge);
    }
    let digest = digest(&bytes);
    Ok(SealedEnvelope { bytes, digest })
}

/// Seal a command to the machine command key (devices; tests on the hub).
pub fn seal_command(
    machine_public: &[u8],
    plaintext: &[u8],
    ids: &CommandIds<'_>,
) -> Result<SealedEnvelope> {
    seal_command_inner(machine_public, plaintext, ids, None)
}

/// Deterministic [`seal_command`] (fixtures only).
pub fn seal_command_with(
    machine_public: &[u8],
    plaintext: &[u8],
    ids: &CommandIds<'_>,
    ikm_ephemeral: &[u8; 32],
) -> Result<SealedEnvelope> {
    seal_command_inner(machine_public, plaintext, ids, Some(ikm_ephemeral))
}

/// Open a reserved command with the machine's command private key.
pub fn open_command(
    machine_secret: &[u8],
    envelope: &[u8],
    ids: &CommandIds<'_>,
) -> Result<Zeroizing<Vec<u8>>> {
    if envelope.len() > MAX_ENVELOPE_BYTES {
        return Err(CryptoError::TooLarge);
    }
    let parsed: CommandEnvelope =
        serde_json::from_slice(envelope).map_err(|_| CryptoError::Malformed("envelope"))?;
    if parsed.v != 1 || parsed.alg != COMMAND_ALG {
        return Err(CryptoError::Unsupported);
    }
    if parsed.kid != ids.machine_key_id {
        return Err(CryptoError::Malformed("kid"));
    }
    open(
        machine_secret,
        &b64url_decode(&parsed.enc, "enc")?,
        COMMAND_INFO.as_bytes(),
        &b64url_decode(&parsed.ct, "ct")?,
        ids.aad().as_bytes(),
    )
}
