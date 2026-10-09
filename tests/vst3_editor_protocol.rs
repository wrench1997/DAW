//! Additive editor wire-contract regressions. These do not open a native window.
#![cfg(feature = "vst3")]

use vst3_host::process_isolation::{HostCommand, HostResponse};
use vst3_host::{IsolatedEditorCommand, IsolatedEditorOwner, IsolatedEditorState};

#[test]
fn legacy_editor_wire_values_are_unchanged() {
    assert_eq!(
        serde_json::to_string(&HostCommand::CreateGui).unwrap(),
        "\"CreateGui\""
    );
    assert_eq!(
        serde_json::to_string(&HostCommand::CloseGui).unwrap(),
        "\"CloseGui\""
    );
    assert!(matches!(
        serde_json::from_str::<HostCommand>("\"CreateGui\"").unwrap(),
        HostCommand::CreateGui
    ));
}

#[test]
fn isolated_editor_query_has_explicit_typed_wire_value() {
    let command = HostCommand::Editor {
        command: IsolatedEditorCommand::Query,
    };
    assert_eq!(
        serde_json::to_value(command).unwrap(),
        serde_json::json!({"Editor":{"command":"Query"}})
    );
}

#[test]
fn isolated_editor_owner_round_trips_without_pointer_casts() {
    let request = IsolatedEditorCommand::Open {
        owner: Some(IsolatedEditorOwner {
            window: 0x1234_5678_9abc_def0,
            process_id: 42,
        }),
    };
    let json = serde_json::to_string(&request).unwrap();
    assert_eq!(
        serde_json::from_str::<IsolatedEditorCommand>(&json).unwrap(),
        request
    );
    assert!(json.len() < 128);
}

#[test]
fn isolated_editor_state_is_reported_by_helper_not_inferred_from_request() {
    for open in [false, true] {
        let state = IsolatedEditorState {
            supported: true,
            has_editor: true,
            open,
            width: if open { 560 } else { 0 },
            height: if open { 400 } else { 0 },
            generation: 2,
        };
        let response = HostResponse::EditorState { state };
        let json = serde_json::to_string(&response).unwrap();
        let decoded: HostResponse = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded, HostResponse::EditorState { state: actual } if actual == state));
        assert!(json.len() < 256);
    }
}

#[test]
fn isolated_editor_options_reject_unknown_fields_and_invalid_types() {
    for json in [
        r#"{"Open":{"owner":null,"embed":true}}"#,
        r#"{"Open":{"owner":{"window":123,"process_id":42,"foreign":true}}}"#,
        r#"{"Open":{"owner":{"window":-1,"process_id":42}}}"#,
        r#"{"Open":{"owner":{"window":123,"process_id":"42"}}}"#,
        r#"{"Resize":{"width":100,"height":100}}"#,
    ] {
        assert!(
            serde_json::from_str::<IsolatedEditorCommand>(json).is_err(),
            "accepted {json}"
        );
    }
}
