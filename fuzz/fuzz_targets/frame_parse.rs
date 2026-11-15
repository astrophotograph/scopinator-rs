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
    // Every accessor must be total for any 80 bytes.
    let _ = header.is_image();
    let _ = header.has_valid_magic();
    let _ = header.payload_kind();
    let _ = header.bytes_per_pixel();
});
