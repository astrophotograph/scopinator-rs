#![no_main]

use libfuzzer_sys::fuzz_target;
use scopinator_seestar::protocol::json_rpc;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else { return };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(s) else { return };
    let _ = json_rpc::is_event(&value);
    let _ = json_rpc::is_response(&value);
    let _ = json_rpc::method_name(&value);
    let _ = json_rpc::event_name(&value);
    let _ = json_rpc::json_rpc_id(&value);
    let _ = json_rpc::response_code(&value);
    let _ = json_rpc::error_message(&value);
});
