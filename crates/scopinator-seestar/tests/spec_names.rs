//! Every method and event name this crate models must be known to the
//! protocol specification (zmerlynn/seestar-spec), or be listed below with
//! the evidence for it.
//!
//! This is the guard against inventing names: `goto_target`,
//! `get_annotated_result` and `get_stack_setting` were all modeled here once,
//! and none of them exists on the device (103 "method not found" on an S50,
//! firmware 6.70). See `docs/protocol/hardware-observations.md`.
//!
//! The names come from `conformance/spec/seestar-spec-names.json`, a
//! names-only extract of the spec catalog (regenerate with
//! `tools/vendor_spec_names.py`). The spec describes firmware 9.31 and 9.75,
//! so a name that exists only on older firmware belongs in the allowlist
//! with a hardware or corpus citation.

use scopinator_seestar::command::{ImagingCommand, command_method_names};
use scopinator_seestar::event::known_event_names;
use serde_json::Value;

/// Names that are not in the spec catalog, with the evidence for keeping them.
const METHODS_OUTSIDE_SPEC: &[(&str, &str)] = &[];

const EVENTS_OUTSIDE_SPEC: &[(&str, &str)] = &[];

fn spec() -> Value {
    serde_json::from_str(include_str!(
        "../../../conformance/spec/seestar-spec-names.json"
    ))
    .expect("spec names file is valid JSON")
}

fn check(kind: &str, ours: &[&str], spec_names: &Value, allow: &[(&str, &str)]) {
    let known = spec_names.as_object().expect("names object");
    let missing: Vec<&str> = ours
        .iter()
        .copied()
        .filter(|name| !known.contains_key(*name))
        .filter(|name| !allow.iter().any(|(allowed, _)| allowed == name))
        .collect();
    assert!(
        missing.is_empty(),
        "{kind} names in neither the spec catalog nor the allowlist: {missing:?}\n\
         Check the name against the spec, or add it to the allowlist with a \
         hardware observation."
    );
    // An allowlist entry that the spec now has, or that we no longer model,
    // is stale and should be removed.
    for (name, _) in allow {
        assert!(
            ours.contains(name) && !known.contains_key(*name),
            "stale allowlist entry for {kind} `{name}`"
        );
    }
}

#[test]
fn control_methods_exist_in_the_spec() {
    let spec = spec();
    check(
        "method",
        command_method_names(),
        &spec["methods"],
        METHODS_OUTSIDE_SPEC,
    );
}

#[test]
fn image_socket_methods_exist_in_the_spec() {
    let spec = spec();
    let methods = [
        ImagingCommand::BeginStreaming,
        ImagingCommand::StopStreaming,
        ImagingCommand::TestConnection,
        ImagingCommand::GetStackedImage,
        ImagingCommand::GetCurrentImage {
            compress: None,
            raw: false,
        },
        ImagingCommand::GetPhotometryImage,
    ]
    .map(|cmd| cmd.method());
    check("image-socket method", &methods, &spec["methods"], &[]);
}

#[test]
fn events_exist_in_the_spec() {
    let spec = spec();
    check(
        "event",
        known_event_names(),
        &spec["events"],
        EVENTS_OUTSIDE_SPEC,
    );
}

#[test]
fn the_guard_rejects_an_invented_name() {
    let spec = spec();
    let result = std::panic::catch_unwind(|| {
        check("method", &["goto_target"], &spec["methods"], &[]);
    });
    assert!(result.is_err(), "goto_target must not pass the guard");
}
