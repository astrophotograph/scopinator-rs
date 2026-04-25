#![no_main]

use libfuzzer_sys::fuzz_target;
use scopinator_seestar::protocol::discovery::parse_discovery_response;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

fuzz_target!(|data: &[u8]| {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) else {
        return;
    };
    let src = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 1), 4720));
    let _ = parse_discovery_response(&value, src);
});
