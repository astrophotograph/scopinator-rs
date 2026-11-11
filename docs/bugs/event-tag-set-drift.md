# Event tag set drift: Rust models `Internal`, which Python's EventTypes union deliberately excludes

Filed from Forge task "Port pyscopinator event parsing cases to Rust" (Scopinator, testing sweep).
Status: **open, not adjudicated.** This report describes current behaviour on both sides and changes
no production code.

## Behaviour today (Rust, scopinator-seestar)

`SeestarEvent` (`crates/scopinator-seestar/src/event/mod.rs`) deserializes 33 distinct `Event` wire
tags, each to its own variant; the set is reported by `known_event_names()`. Python's `EventTypes`
discriminated union (`pyscopinator`, `src/scopinator/seestar/events/__init__.py:385`) has 32 arms.
The sets agree on 32 tags; the one Rust-only member is:

- **`Internal`** — `SeestarEvent::Internal(serde_json::Value)`. Python *has* an `InternalEvent`
  model, but it is deliberately kept **out** of the `EventTypes` union: the oracle's
  `TestInternalEvent::test_internal_not_in_discriminated_union` pins that
  `parse({"Event": "Internal", ...})` raises `ValidationError` there (it is a client-side model,
  not a wire event). In the Rust port the same message parses to a variant, so an internal-only
  event appearing on the control port is accepted — and broadcast to subscribers as
  `SeestarEvent::Internal` — rather than rejected at the parse boundary.

The remaining 32 tags correspond one-to-one with the Python arms, including Python's `ThreePPAEvent`
(the Rust variant `ThreePPA` is serde-renamed to the wire tag `"3PPA"`; the oracle's per-event
isinstance tests do not exercise it, so the Python wire tag is presumed to match). No Python union
arm is missing from the Rust enum.

## Which side is right

Unknown — plausibly deliberate on both sides. The Rust port's comment groups `Internal` with
"less common events — capture as raw JSON for now", and accepting internal events
forward-compatibly is defensible; Python's stricter exclusion keeps client-internal traffic out of
the wire union, and `InternalEvent` is used internally there without a wire counterpart. Because
the port's contract is to mirror the oracle, the membership difference is pinned here rather than
reconciled; nothing is changed on either side. Whoever adjudicates should decide whether
`{"Event": "Internal", ...}` on the wire is a protocol violation (drop it / route it to `Unknown`,
matching Python) or a modelled internal channel (keep it, and document it as Rust-only surface).

## Pinned by

- `known_event_names_lists_the_full_tag_set` (`crates/scopinator-seestar/src/event/mod.rs`): the
  full 33-name list, so any addition, removal or renaming in `known_event_names()` fails loudly.
- `every_known_event_name_parses_to_its_variant` (`crates/scopinator-seestar/src/event/mod.rs`):
  every reported name parses to its own non-`Unknown` variant.
- `internal_event_parses_to_variant` (`crates/scopinator-seestar/src/event/mod.rs`):
  `{"Event": "Internal", ...}` parses in Rust where the oracle raises.

These tests characterize current behaviour and will fail, on purpose, if either side of the drift
is reconciled.
