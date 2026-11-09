# CommandResponse: `method` is optional in Rust, required in Python

Source: Forge task "Add differential response parsing tests" (Scopinator, testing sweep).
Status: unadjudicated. This report describes both behaviours and gives no verdict.

## Python (pyscopinator)

`commands/common.py::CommandResponse` declares `method: str` (required), alongside
`id, jsonrpc, Timestamp, code, error, result`. A response without `method` fails
validation. (Per the task spec; the Python file was not re-read when writing this report.)

## Rust (scopinator-seestar)

`crates/scopinator-seestar/src/response/mod.rs`: `CommandResponse.method` is
`Option<String>`. A response lacking `method`, or with `"method": null`, parses with
`method == None`. `CommandError.method` is likewise `Option<String>`.
Only `id` is required; `jsonrpc` and `code` default (`""`, `0`).

## Example

`{"id": 1, "jsonrpc": "2.0", "code": 0}`: Python rejects, Rust accepts as success.

## Observed on real data

Every telescope-side JSON-RPC response in `conformance/sessions/*/control.jsonl`
carries `method`, so the two ports agree on the corpus.

## Pinned by (in `response/mod.rs`)

- `diff_response_without_method_parses_in_rust`
- `diff_response_with_null_method_parses_in_rust`
- `diff_methodless_error_response_into_result`
- `diff_corpus_all_sessions_responses_parse`
