#![no_main]

//! Event deserialization is the highest-volume parse path at runtime: every
//! event line the telescope sends goes through `SeestarEvent`. It must never
//! panic, whatever the bytes. Seeds: `fuzz/seeds/event_parse` (real event
//! lines from the sanitized corpus), e.g.
//! `cargo +nightly fuzz run event_parse fuzz/corpus/event_parse fuzz/seeds/event_parse`.

use libfuzzer_sys::fuzz_target;
use scopinator_seestar::event::SeestarEvent;

fuzz_target!(|data: &[u8]| {
    // Straight from bytes, as a stricter caller might.
    let _ = serde_json::from_slice::<SeestarEvent>(data);

    // And the way the control reader does it: decode to a value first.
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(data) else {
        return;
    };
    if let Ok(event) = SeestarEvent::from_value(value) {
        // The accessors must be total too.
        let _ = event.name();
        let _ = event.is_unknown();
    }
});
