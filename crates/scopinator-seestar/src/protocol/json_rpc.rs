use serde_json::Value;

/// Default control port.
pub const CONTROL_PORT: u16 = 4700;

/// Default imaging port.
pub const IMAGING_PORT: u16 = 4800;

/// Default discovery port (UDP).
pub const DISCOVERY_PORT: u16 = 4720;

/// Maximum line size for JSON messages (1 MB).
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// Starting ID for command correlation (high offset to avoid collision
/// with hardcoded IDs like 21/23 used on the imaging port).
pub const INITIAL_COMMAND_ID: u64 = 100_000;

/// Check if a JSON message is an async event (has "Event" key).
pub fn is_event(msg: &Value) -> bool {
    msg.get("Event").is_some()
}

/// Check if a JSON message is a command response (has "id" + "code" or "result").
pub fn is_response(msg: &Value) -> bool {
    msg.get("id").is_some() && (msg.get("code").is_some() || msg.get("result").is_some())
}

/// Extract the method name from a JSON message.
pub fn method_name(msg: &Value) -> Option<&str> {
    msg.get("method").and_then(|v| v.as_str())
}

/// Extract the event name from a JSON message.
pub fn event_name(msg: &Value) -> Option<&str> {
    msg.get("Event").and_then(|v| v.as_str())
}

/// Extract the `id` as a u64 (returns None for non-integer IDs).
pub fn json_rpc_id(msg: &Value) -> Option<u64> {
    msg.get("id").and_then(|v| v.as_u64())
}

/// Extract the response code from a JSON message.
pub fn response_code(msg: &Value) -> Option<i64> {
    msg.get("code").and_then(|v| v.as_i64())
}

/// Extract the error message from a JSON message.
pub fn error_message(msg: &Value) -> Option<&str> {
    msg.get("error").and_then(|v| v.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_event() {
        let msg: Value = serde_json::from_str(r#"{"Event": "PiStatus", "temp": 35.0}"#).unwrap();
        assert!(is_event(&msg));
        assert!(!is_response(&msg));
        assert_eq!(event_name(&msg), Some("PiStatus"));
    }

    #[test]
    fn classify_response() {
        let msg: Value = serde_json::from_str(r#"{"id": 42, "code": 0, "result": null}"#).unwrap();
        assert!(!is_event(&msg));
        assert!(is_response(&msg));
        assert_eq!(json_rpc_id(&msg), Some(42));
        assert_eq!(response_code(&msg), Some(0));
    }

    #[test]
    fn classify_command() {
        let msg: Value =
            serde_json::from_str(r#"{"id": 100, "method": "get_device_state"}"#).unwrap();
        assert!(!is_event(&msg));
        assert!(!is_response(&msg));
        assert_eq!(method_name(&msg), Some("get_device_state"));
    }

    mod prop {
        use super::*;
        use proptest::prelude::*;

        fn arbitrary_scalar() -> impl Strategy<Value = Value> {
            prop_oneof![
                Just(Value::Null),
                any::<bool>().prop_map(Value::Bool),
                any::<i64>().prop_map(|n| Value::Number(n.into())),
                "[ -~]{0,32}".prop_map(Value::String),
            ]
        }

        // JSON object whose keys are drawn from the protocol's semantically
        // meaningful set plus some noise.
        fn arbitrary_object() -> impl Strategy<Value = Value> {
            let key = prop_oneof![
                Just("Event".to_string()),
                Just("id".to_string()),
                Just("code".to_string()),
                Just("result".to_string()),
                Just("method".to_string()),
                Just("error".to_string()),
                "[a-z_]{1,8}",
            ];
            proptest::collection::vec((key, arbitrary_scalar()), 0..6).prop_map(|entries| {
                let map: serde_json::Map<String, Value> = entries.into_iter().collect();
                Value::Object(map)
            })
        }

        proptest! {
            #[test]
            fn classifiers_match_key_presence(v in arbitrary_object()) {
                prop_assert_eq!(is_event(&v), v.get("Event").is_some());

                let expected = v.get("id").is_some()
                    && (v.get("code").is_some() || v.get("result").is_some());
                prop_assert_eq!(is_response(&v), expected);
            }

            #[test]
            fn extractors_match_typed_lookups(v in arbitrary_object()) {
                prop_assert_eq!(method_name(&v), v.get("method").and_then(|x| x.as_str()));
                prop_assert_eq!(event_name(&v), v.get("Event").and_then(|x| x.as_str()));
                prop_assert_eq!(json_rpc_id(&v), v.get("id").and_then(|x| x.as_u64()));
                prop_assert_eq!(response_code(&v), v.get("code").and_then(|x| x.as_i64()));
                prop_assert_eq!(error_message(&v), v.get("error").and_then(|x| x.as_str()));
            }

            #[test]
            fn classifiers_never_panic_on_scalars(v in arbitrary_scalar()) {
                let _ = is_event(&v);
                let _ = is_response(&v);
                let _ = method_name(&v);
                let _ = event_name(&v);
                let _ = json_rpc_id(&v);
                let _ = response_code(&v);
                let _ = error_message(&v);
            }
        }
    }
}
