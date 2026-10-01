//! End-to-end serde checks against literal wire JSON.
//!
//! Unit tests inside the crate build values in Rust and serialise them; these
//! tests go the other direction — they parse the exact JSON shapes the ACP v1
//! contract promises to prove the on-the-wire format is right, independent of
//! our Rust field names.

use serde_json::{json, Value};
use srud_protocol::acp::{Meta, ToolCall};
use srud_protocol::srud::meta::SrudMeta;
use srud_protocol::srud::methods::{ForkSessionRequest, SteerRequest};
use srud_protocol::srud::MaybeSessionUpdate;

#[test]
fn tool_call_parses_the_contract_shape() {
    // ACP wire conventions: camelCase keys, snake_case discriminants, `_meta`
    // reserved for extensions. This is a real upstream type, so parsing it
    // proves our re-export matches the spec shape.
    let wire = json!({
        "toolCallId": "call_01",
        "title": "Read config",
        "kind": "read",
        "status": "in_progress",
        "rawInput": { "path": "/etc/app/config.toml" },
        "_meta": { "srud": { "turnId": "turn_01", "stepIndex": 2 } }
    });

    let call: ToolCall = serde_json::from_value(wire.clone()).expect("parse ToolCall");
    assert_eq!(&*call.tool_call_id.0, "call_01");
    assert_eq!(call.title, "Read config");
    assert_eq!(
        call.raw_input,
        Some(json!({ "path": "/etc/app/config.toml" }))
    );

    // SrudAgent reads its private data back out of the standard `_meta`.
    let meta = call.meta.as_ref().expect("_meta present");
    let srud = SrudMeta::from_meta(meta).expect("_meta.srud parses");
    assert_eq!(srud.turn_id.as_deref(), Some("turn_01"));
    assert_eq!(srud.step_index, Some(2));

    // Round-trip: re-serialising keeps the `_meta.srud` payload intact.
    let out = serde_json::to_value(&call).unwrap();
    assert_eq!(
        out["_meta"]["srud"],
        json!({ "turnId": "turn_01", "stepIndex": 2 })
    );
}

#[test]
fn srud_meta_round_trips_through_a_wire_value() {
    let srud = SrudMeta {
        turn_id: Some("t".into()),
        interrupted: Some(true),
        turn_end_reason: Some("blocked".into()),
        ..Default::default()
    };
    let meta = srud.to_meta().unwrap();

    // The wrapper is a plain JSON object on the wire.
    let value = serde_json::to_value(&meta).unwrap();
    assert_eq!(
        value,
        json!({ "srud": { "turnId": "t", "interrupted": true, "turnEndReason": "blocked" } })
    );

    let parsed = SrudMeta::from_meta(&serde_json::from_value::<Meta>(value).unwrap()).unwrap();
    assert_eq!(parsed, srud);
}

#[test]
fn extension_requests_use_camel_case_session_keys() {
    // `_srud/unstable/*` types are ours, but must follow the same key
    // convention as the standard methods they sit beside.
    let fork: ForkSessionRequest =
        serde_json::from_value(json!({ "sessionId": "s", "title": "alt" })).unwrap();
    assert_eq!(&*fork.session_id.0, "s");
    let fork_value: Value = serde_json::to_value(&fork).unwrap();
    assert_eq!(fork_value["sessionId"], json!("s"));
    assert!(fork_value.get("session_id").is_none());

    let steer: SteerRequest = serde_json::from_value(json!({
        "sessionId": "s",
        "prompt": [{ "type": "text", "text": "go slower" }]
    }))
    .unwrap();
    assert_eq!(steer.prompt.len(), 1);
}

#[test]
fn unknown_session_update_variant_survives_relay() {
    // ACP's compatibility rule: a future `sessionUpdate` variant must parse,
    // be recognisable as unknown, and re-serialise byte-for-byte so a bridge
    // can relay it untouched to a newer peer.
    let wire = json!({
        "sessionUpdate": "subagent_progress",
        "agentId": "a-7",
        "unknownField": [1, 2, 3]
    });
    let parsed: MaybeSessionUpdate = serde_json::from_value(wire.clone()).unwrap();
    assert!(parsed.is_unknown());
    assert_eq!(serde_json::to_value(&parsed).unwrap(), wire);
}
