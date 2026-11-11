# Undeserializable response is warn-and-dropped; caller eats the full response timeout

Filed from Forge task "Characterize silent drop paths and write bug report" (Scopinator, testing sweep).
Status: **open, not adjudicated.** This report describes current behaviour and what a caller would
want instead; it changes no production code.

## Behaviour today (Rust, scopinator-seestar)

A control-port line that parses as JSON and satisfies `json_rpc::is_response` (has `id` plus `code`
or `result`) is routed to `handle_response` (`crates/scopinator-seestar/src/connection/control.rs`).
There it must deserialize as `CommandResponse` (`crates/scopinator-seestar/src/response/mod.rs`).
When deserialization fails, the message is warn-and-dropped:

```rust
let response: CommandResponse = match serde_json::from_value(msg.clone()) {
    Ok(r) => r,
    Err(e) => {
        warn!("failed to deserialize response: {e}");
        return;
    }
};
```

This fires even when the message's `id` matches a **pending** request. Example wire line (fails
because `CommandResponse.code` is `i32` but the body carries a string):

```json
{"jsonrpc":"2.0","id":100000,"method":"get_view_state","code":"garbage"}
```

Consequences for the waiting caller:

- The pending registry entry is never taken, so the caller's oneshot never resolves and
  `SeestarClient::send_command` blocks until its response timeout (`DEFAULT_RESPONSE_TIMEOUT`,
  30 s by default) and returns `SeestarError::Timeout`. A scope that mangled one response is
  indistinguishable from a scope that never answered at all.
- The registry entry stays registered (only a response with that id or the disconnect flush removes
  it), so the request slot leaks until the next disconnect.
- The connection itself stays healthy: the reader continues processing subsequent lines, and
  event subscribers see no disruption.

## What a caller would want instead

Fail fast and informatively: a response-shaped message carrying the id of a pending request that
cannot be parsed should be delivered to that request's caller as an error — the `serde_json::Error`
and ideally the raw line — so `send_command` returns promptly instead of burning the full timeout.
A dedicated `SeestarError` variant naming the deserialization failure would let callers tell
"the scope answered garbage" apart from "the scope went silent". Callers that prefer to wait out
the timeout could still do so by treating the error as retryable; the point is that today they
cannot make that choice.

## The Unknown-event observability gap (noted alongside)

`SeestarEvent` (`crates/scopinator-seestar/src/event/mod.rs`) uses `#[serde(other)] Unknown` as its
forward-compatible catch-all. Serde's `other` requires a unit variant, so an event whose `Event`
tag is not modeled loses both the tag and the payload at deserialization: subscribers receive
`SeestarEvent::Unknown` and cannot tell which event fired, let alone what it carried — two
different unmodeled events are indistinguishable. The only signal is a `warn!` in the connection
reader (`handle_event`, control.rs) that logs the raw event name at runtime; consumers of the
broadcast stream have no programmatic access to it, so firmware drift (new event types) is
invisible without log scraping. A caller would want at minimum the raw tag, and ideally the raw
JSON payload, preserved on the `Unknown` variant.

## Pinned by

- `malformed_response_for_pending_id_is_dropped_and_caller_times_out`
  (`crates/scopinator-seestar/tests/control_integration.rs`, paused tokio time): pushes a
  response-shaped, undeserializable body for a pending request id via `FakeSeestar::send_raw_line`
  and asserts the caller gets `SeestarError::Timeout` after the full configured timeout rather
  than a prompt correlation error, while the event stream keeps flowing.
- `unknown_event_broadcast_as_unknown_and_stream_continues`
  (`crates/scopinator-seestar/tests/control_integration.rs`): an unmodeled event tag reaches
  subscribers as `SeestarEvent::Unknown`, and the broadcast stream continues with the next event.
- `unknown_event_drops_tag_and_payload` (`crates/scopinator-seestar/src/event/mod.rs`): the wire
  tag and payload are unrecoverable from the deserialized value; distinct unknown tags are
  indistinguishable.

These tests characterize current behaviour and will fail, on purpose, if the drop paths are fixed.
