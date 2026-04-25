#![no_main]

use libfuzzer_sys::fuzz_target;
use scopinator_seestar::protocol::frame::{FrameHeader, HEADER_SIZE};

fuzz_target!(|data: &[u8]| {
    if data.len() < HEADER_SIZE {
        return;
    }
    let mut buf = [0u8; HEADER_SIZE];
    buf.copy_from_slice(&data[..HEADER_SIZE]);
    let header = FrameHeader::parse(&buf);
    let _ = header.is_image();
});
