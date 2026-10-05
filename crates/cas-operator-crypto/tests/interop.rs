//! cas-9b7d S0 crypto gate (Rust side).
//!
//! Opens every case of the cloud's JS-sealed fixture
//! (petra-stella-cloud `tests/fixtures/operator-hpke-interop.json`, copied
//! byte-identical) including `observer_notices[0]`, the contract §6.6 / §16
//! Q11 gate, and reseals each case from the same inputs to prove the Rust
//! sender produces the JS bytes exactly. The cas-src envelope fixture
//! (`cas-operator-envelope-interop.json`, written by hub-web's @hpke/core
//! builder) is opened and reproduced the same way. The browser mirror is
//! `hub-web/src/inbox/hpke.test.ts`.

use cas_operator_crypto::{
    CommandIds, CryptoError, EPOCH_WRAP_INFO, EpochWrapIds, EventIds, ObserverIds, b64url_decode,
    b64url_encode, derive_key_pair, digest, generate_key_pair, open, open_command, open_epoch_wrap,
    open_event, open_observer_notice, public_key_of, seal_command, seal_command_with, seal_event,
    seal_event_with, seal_observer_notice_with, seal_with_ikm,
};
use serde_json::Value;

const CLOUD: &str = include_str!("fixtures/cloud-operator-hpke-interop.json");
const ENVELOPES: &str = include_str!("fixtures/cas-operator-envelope-interop.json");

fn json(text: &str) -> Value {
    serde_json::from_str(text).expect("fixture is JSON")
}

fn s<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("fixture field {key} is a string"))
}

fn bytes(value: &Value, key: &str) -> Vec<u8> {
    hex::decode(s(value, key)).unwrap_or_else(|_| panic!("fixture field {key} is hex"))
}

fn array32(value: &Value, key: &str) -> [u8; 32] {
    bytes(value, key)
        .try_into()
        .unwrap_or_else(|_| panic!("fixture field {key} is 32 bytes"))
}

fn array12(value: &Value, key: &str) -> [u8; 12] {
    bytes(value, key)
        .try_into()
        .unwrap_or_else(|_| panic!("fixture field {key} is 12 bytes"))
}

fn cloud_case<'a>(fixture: &'a Value, name: &str) -> &'a Value {
    fixture["cases"]
        .as_array()
        .expect("cases array")
        .iter()
        .find(|entry| entry["name"] == name)
        .unwrap_or_else(|| panic!("cloud fixture has case {name}"))
}

#[test]
fn cloud_fixture_pins_the_contract_suite() {
    let fixture = json(CLOUD);
    assert_eq!(fixture["suite"]["kem_id"], 16);
    assert_eq!(fixture["suite"]["kdf_id"], 1);
    assert_eq!(fixture["suite"]["aead_id"], 2);
    assert_eq!(fixture["suite"]["mode"], 0);
}

#[test]
fn rust_opens_and_reproduces_every_js_sealed_cloud_case() {
    let fixture = json(CLOUD);
    for name in ["epoch_wrap", "event_content_key"] {
        let case = cloud_case(&fixture, name);
        let derived = derive_key_pair(&bytes(case, "ikm_recipient_hex"));
        assert_eq!(
            &derived.secret[..],
            &bytes(case, "sk_recipient_hex")[..],
            "{name} sk"
        );
        assert_eq!(
            &derived.public[..],
            &bytes(case, "pk_recipient_hex")[..],
            "{name} pk"
        );
        assert_eq!(
            public_key_of(&bytes(case, "sk_recipient_hex")).expect("valid sk"),
            derived.public
        );

        let plain = open(
            &bytes(case, "sk_recipient_hex"),
            &bytes(case, "enc_hex"),
            s(case, "info_utf8").as_bytes(),
            &bytes(case, "ciphertext_hex"),
            s(case, "aad_utf8").as_bytes(),
        )
        .expect("Rust opens the JS seal");
        assert_eq!(
            &plain[..],
            &bytes(case, "plaintext_hex")[..],
            "{name} plaintext"
        );

        let resealed = seal_with_ikm(
            &bytes(case, "pk_recipient_hex"),
            s(case, "info_utf8").as_bytes(),
            &bytes(case, "plaintext_hex"),
            s(case, "aad_utf8").as_bytes(),
            &array32(case, "ikm_ephemeral_hex"),
        )
        .expect("Rust seals");
        assert_eq!(
            resealed.enc,
            bytes(case, "enc_hex"),
            "{name} enc byte-exact"
        );
        assert_eq!(
            resealed.ciphertext,
            bytes(case, "ciphertext_hex"),
            "{name} ciphertext byte-exact"
        );
    }
}

#[test]
fn device_path_unwraps_the_epoch_key_and_every_wrap_binding_is_load_bearing() {
    let fixture = json(CLOUD);
    let wrap = cloud_case(&fixture, "epoch_wrap");
    let event_key = cloud_case(&fixture, "event_content_key");
    assert_eq!(s(wrap, "info_utf8"), EPOCH_WRAP_INFO);
    let ids = &fixture["ids"];
    let account = s(ids, "accountId");
    let fgen = s(ids, "feedGeneration");
    let epoch = s(ids, "epoch");
    let device = s(ids, "deviceId");
    let good = EpochWrapIds {
        account_id: account,
        feed_generation: fgen,
        epoch,
        device_id: device,
    };
    assert_eq!(good.aad(), s(wrap, "aad_utf8"));

    let epoch_secret = open_epoch_wrap(
        &bytes(wrap, "sk_recipient_hex"),
        &bytes(wrap, "enc_hex"),
        &bytes(wrap, "ciphertext_hex"),
        &good,
    )
    .expect("device unwraps the epoch key");
    assert_eq!(&epoch_secret[..], &bytes(event_key, "sk_recipient_hex")[..]);

    let content_key = open(
        &epoch_secret[..],
        &bytes(event_key, "enc_hex"),
        s(event_key, "info_utf8").as_bytes(),
        &bytes(event_key, "ciphertext_hex"),
        s(event_key, "aad_utf8").as_bytes(),
    )
    .expect("epoch key opens the content key");
    assert_eq!(&content_key[..], &bytes(event_key, "plaintext_hex")[..]);

    let tampered = [
        format!("{account}x"),
        format!("{fgen}x"),
        format!("{epoch}x"),
        format!("{device}x"),
    ];
    for (index, value) in tampered.iter().enumerate() {
        let ids = EpochWrapIds {
            account_id: if index == 0 { value } else { account },
            feed_generation: if index == 1 { value } else { fgen },
            epoch: if index == 2 { value } else { epoch },
            device_id: if index == 3 { value } else { device },
        };
        assert_eq!(
            open_epoch_wrap(
                &bytes(wrap, "sk_recipient_hex"),
                &bytes(wrap, "enc_hex"),
                &bytes(wrap, "ciphertext_hex"),
                &ids,
            )
            .err(),
            Some(CryptoError::OpenFailed),
            "wrap binding {index}"
        );
    }
}

/// The §6.6 / §16 Q11 hard gate: open the cloud-sealed observer notice.
#[test]
fn rust_opens_cloud_observer_notice_zero() {
    let fixture = json(CLOUD);
    let notice = &fixture["observer_notices"][0];
    let epoch_secret = bytes(
        cloud_case(&fixture, "event_content_key"),
        "sk_recipient_hex",
    );
    let ids = &notice["ids"];
    let observer = ObserverIds {
        account_id: s(ids, "accountId"),
        feed_generation: s(ids, "feedGeneration"),
        key_epoch: s(ids, "epoch"),
        event_id: s(ids, "eventId"),
        hub_id: s(ids, "hubId"),
    };
    assert_eq!(observer.aad(), s(notice, "aad_utf8"));

    let envelope = s(notice, "envelope_utf8").as_bytes();
    assert_eq!(digest(envelope), s(notice, "digest"));
    let plain = open_observer_notice(&epoch_secret, envelope, &observer)
        .expect("Rust opens observer_notices[0]");
    assert_eq!(
        std::str::from_utf8(&plain).expect("UTF-8 plaintext"),
        s(notice, "plaintext_utf8")
    );
    let claims: Value = serde_json::from_slice(&plain).expect("plaintext JSON");
    assert_eq!(claims["type"], "psc.operator.machine_presence");
    assert_eq!(claims["kind"], "machine_unobserved");
    assert_eq!(claims["machine_id"], ids["machineId"]);
    assert_eq!(claims["hub_id"], ids["hubId"]);
}

#[test]
fn rust_reproduces_the_cloud_observer_envelope_byte_for_byte() {
    let fixture = json(CLOUD);
    let notice = &fixture["observer_notices"][0];
    let ids = &notice["ids"];
    let observer = ObserverIds {
        account_id: s(ids, "accountId"),
        feed_generation: s(ids, "feedGeneration"),
        key_epoch: s(ids, "epoch"),
        event_id: s(ids, "eventId"),
        hub_id: s(ids, "hubId"),
    };
    let sealed = seal_observer_notice_with(
        &bytes(
            cloud_case(&fixture, "event_content_key"),
            "pk_recipient_hex",
        ),
        s(notice, "plaintext_utf8").as_bytes(),
        &observer,
        &array32(notice, "content_key_hex"),
        &array12(notice, "nonce_hex"),
        &array32(notice, "ikm_ephemeral_hex"),
    )
    .expect("Rust seals the observer envelope");
    assert_eq!(
        std::str::from_utf8(&sealed.bytes).expect("UTF-8"),
        s(notice, "envelope_utf8")
    );
    assert_eq!(sealed.digest, s(notice, "digest"));
}

#[test]
fn observer_notice_refuses_any_other_row_binding() {
    let fixture = json(CLOUD);
    let notice = &fixture["observer_notices"][0];
    let epoch_secret = bytes(
        cloud_case(&fixture, "event_content_key"),
        "sk_recipient_hex",
    );
    let ids = &notice["ids"];
    let envelope = s(notice, "envelope_utf8").as_bytes();
    let (account, fgen, event, hub) = (
        s(ids, "accountId"),
        s(ids, "feedGeneration"),
        s(ids, "eventId"),
        s(ids, "hubId"),
    );
    let x = |value: &str| format!("{value}x");
    let cases = [
        (
            x(account),
            fgen.to_owned(),
            event.to_owned(),
            hub.to_owned(),
        ),
        (
            account.to_owned(),
            x(fgen),
            event.to_owned(),
            hub.to_owned(),
        ),
        (
            account.to_owned(),
            fgen.to_owned(),
            x(event),
            hub.to_owned(),
        ),
        (
            account.to_owned(),
            fgen.to_owned(),
            event.to_owned(),
            x(hub),
        ),
    ];
    for (account_id, feed_generation, event_id, hub_id) in &cases {
        let observer = ObserverIds {
            account_id,
            feed_generation,
            key_epoch: s(ids, "epoch"),
            event_id,
            hub_id,
        };
        assert_eq!(
            open_observer_notice(&epoch_secret, envelope, &observer).err(),
            Some(CryptoError::OpenFailed)
        );
    }
    let wrong_epoch = ObserverIds {
        account_id: account,
        feed_generation: fgen,
        key_epoch: "5",
        event_id: event,
        hub_id: hub,
    };
    assert!(matches!(
        open_observer_notice(&epoch_secret, envelope, &wrong_epoch),
        Err(CryptoError::EpochMismatch { .. })
    ));
}

fn event_ids(ids: &Value) -> EventIds<'_> {
    EventIds {
        account_id: s(ids, "accountId"),
        feed_generation: s(ids, "feedGeneration"),
        key_epoch: s(ids, "keyEpoch"),
        event_id: s(ids, "eventId"),
        hub_id: s(ids, "hubId"),
        project_id: s(ids, "projectId"),
        session_id: s(ids, "sessionId"),
    }
}

fn command_ids(ids: &Value) -> CommandIds<'_> {
    CommandIds {
        account_id: s(ids, "accountId"),
        machine_id: s(ids, "machineId"),
        command_id: s(ids, "commandId"),
        hub_id: s(ids, "hubId"),
        project_id: s(ids, "projectId"),
        session_id: s(ids, "sessionId"),
        operation: s(ids, "operation"),
        machine_key_id: s(ids, "machineKeyId"),
    }
}

#[test]
fn rust_opens_and_reproduces_the_js_session_event_envelope() {
    let fixture = json(ENVELOPES);
    let event = &fixture["event"];
    let ids = event_ids(&event["ids"]);
    assert_eq!(ids.aad(), s(event, "aad_utf8"));
    let derived = derive_key_pair(&bytes(event, "ikm_epoch_hex"));
    assert_eq!(&derived.secret[..], &bytes(event, "sk_epoch_hex")[..]);
    assert_eq!(&derived.public[..], &bytes(event, "pk_epoch_hex")[..]);

    let envelope = s(event, "envelope_utf8").as_bytes();
    assert_eq!(digest(envelope), s(event, "digest"));
    let plain = open_event(&bytes(event, "sk_epoch_hex"), envelope, &ids).expect("opens");
    assert_eq!(
        std::str::from_utf8(&plain).expect("UTF-8"),
        s(event, "plaintext_utf8")
    );

    let sealed = seal_event_with(
        &bytes(event, "pk_epoch_hex"),
        s(event, "plaintext_utf8").as_bytes(),
        &ids,
        &array32(event, "content_key_hex"),
        &array12(event, "nonce_hex"),
        &array32(event, "ikm_ephemeral_hex"),
    )
    .expect("seals");
    assert_eq!(
        std::str::from_utf8(&sealed.bytes).expect("UTF-8"),
        s(event, "envelope_utf8")
    );
    assert_eq!(sealed.digest, s(event, "digest"));
}

#[test]
fn session_event_refuses_every_substituted_binding_and_tamper() {
    let fixture = json(ENVELOPES);
    let event = &fixture["event"];
    let ids = event_ids(&event["ids"]);
    let secret = bytes(event, "sk_epoch_hex");
    let envelope = s(event, "envelope_utf8").as_bytes();

    let fields = [
        ids.account_id,
        ids.feed_generation,
        ids.event_id,
        ids.hub_id,
        ids.project_id,
        ids.session_id,
    ];
    for index in 0..fields.len() {
        let changed = format!("{}x", fields[index]);
        let pick = |position: usize| {
            if position == index {
                changed.as_str()
            } else {
                fields[position]
            }
        };
        let tampered = EventIds {
            account_id: pick(0),
            feed_generation: pick(1),
            key_epoch: ids.key_epoch,
            event_id: pick(2),
            hub_id: pick(3),
            project_id: pick(4),
            session_id: pick(5),
        };
        assert_eq!(
            open_event(&secret, envelope, &tampered).err(),
            Some(CryptoError::OpenFailed),
            "binding {index}"
        );
    }

    let wrong_epoch = EventIds {
        key_epoch: "8",
        ..event_ids(&event["ids"])
    };
    assert!(matches!(
        open_event(&secret, envelope, &wrong_epoch),
        Err(CryptoError::EpochMismatch { .. })
    ));

    let as_observer = s(event, "envelope_utf8").replace(
        "\"alg\":\"cas-op-event-v1\"",
        "\"alg\":\"psc-op-observer-v1\"",
    );
    assert_eq!(
        open_event(&secret, as_observer.as_bytes(), &ids).err(),
        Some(CryptoError::Unsupported)
    );

    let mut parsed: Value = serde_json::from_slice(envelope).expect("JSON");
    let mut body = b64url_decode(parsed["ct"].as_str().expect("ct"), "ct").expect("b64url");
    body[0] ^= 1;
    parsed["ct"] = Value::String(b64url_encode(&body));
    let flipped = serde_json::to_vec(&parsed).expect("JSON");
    assert_eq!(
        open_event(&secret, &flipped, &ids).err(),
        Some(CryptoError::OpenFailed)
    );

    let other = generate_key_pair();
    assert_eq!(
        open_event(&other.secret[..], envelope, &ids).err(),
        Some(CryptoError::OpenFailed)
    );
    assert_eq!(
        open_event(&secret, &[b' '; 65_537], &ids).err(),
        Some(CryptoError::TooLarge)
    );
}

#[test]
fn rust_opens_and_reproduces_the_js_command_envelope() {
    let fixture = json(ENVELOPES);
    let command = &fixture["command"];
    let ids = command_ids(&command["ids"]);
    assert_eq!(ids.aad(), s(command, "aad_utf8"));
    let derived = derive_key_pair(&bytes(command, "ikm_machine_hex"));
    assert_eq!(&derived.secret[..], &bytes(command, "sk_machine_hex")[..]);

    let envelope = s(command, "envelope_utf8").as_bytes();
    assert_eq!(digest(envelope), s(command, "digest"));
    let plain = open_command(&bytes(command, "sk_machine_hex"), envelope, &ids).expect("opens");
    assert_eq!(
        std::str::from_utf8(&plain).expect("UTF-8"),
        s(command, "plaintext_utf8")
    );

    let sealed = seal_command_with(
        &bytes(command, "pk_machine_hex"),
        s(command, "plaintext_utf8").as_bytes(),
        &ids,
        &array32(command, "ikm_ephemeral_hex"),
    )
    .expect("seals");
    assert_eq!(
        std::str::from_utf8(&sealed.bytes).expect("UTF-8"),
        s(command, "envelope_utf8")
    );
    assert_eq!(sealed.digest, s(command, "digest"));

    let other_kid = CommandIds {
        machine_key_id: "other",
        ..command_ids(&command["ids"])
    };
    assert_eq!(
        open_command(&bytes(command, "sk_machine_hex"), envelope, &other_kid).err(),
        Some(CryptoError::Malformed("kid"))
    );
    let other_command = CommandIds {
        command_id: "cmdFixture0002AbCdEfGhIjKl",
        ..command_ids(&command["ids"])
    };
    assert_eq!(
        open_command(&bytes(command, "sk_machine_hex"), envelope, &other_command).err(),
        Some(CryptoError::OpenFailed)
    );
}

#[test]
fn fresh_randomness_round_trips_and_never_repeats() {
    let fixture = json(ENVELOPES);
    let ids = event_ids(&fixture["event"]["ids"]);
    let epoch = generate_key_pair();
    let first = seal_event(&epoch.public, "hello ✦".as_bytes(), &ids).expect("seals");
    let second = seal_event(&epoch.public, "hello ✦".as_bytes(), &ids).expect("seals");
    assert_ne!(first.digest, second.digest);
    assert_eq!(
        &open_event(&epoch.secret[..], &first.bytes, &ids).expect("opens")[..],
        "hello ✦".as_bytes()
    );

    let command_ids = command_ids(&fixture["command"]["ids"]);
    let machine = generate_key_pair();
    let sealed = seal_command(&machine.public, b"ping", &command_ids).expect("seals");
    assert_eq!(
        &open_command(&machine.secret[..], &sealed.bytes, &command_ids).expect("opens")[..],
        b"ping"
    );
}

#[test]
fn malformed_keys_are_refused() {
    assert_eq!(
        public_key_of(&[0u8; 31]).err(),
        Some(CryptoError::InvalidKey)
    );
    assert_eq!(
        public_key_of(&[0u8; 32]).err(),
        Some(CryptoError::InvalidKey)
    );
    assert!(seal_with_ikm(&[4u8; 64], b"i", b"p", b"a", &[1u8; 32]).is_err());
}
