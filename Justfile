# Run with `just <recipe>` (https://github.com/casey/just).

set shell := ["bash", "-cu"]

# Default: list recipes.
default:
    @just --list

# --- Fast checks (run on every commit; mirror CI fast tier). ---

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

test:
    cargo test --workspace --all-features

# Run fmt-check + clippy + test in one shot. What CI runs.
ci: fmt-check lint test

# --- Session corpus / conformance / cross-language parity. ---

# Analyze the raw captures (firmware/command/event matrix + drift detection).
# Defaults to ../seestar-proxy; pass a dir to override.
analyze dir="":
    python3 tools/analyze_sessions.py {{dir}}

# List secrets in a capture directory (must be empty for the committed corpus).
analyze-pii dir="conformance/sessions":
    python3 tools/analyze_sessions.py --pii {{dir}}

# Sanitize a raw seestar-proxy capture into the committable corpus.
# `just sanitize ../seestar-proxy/session_XXXX conformance/sessions/s50_fw670_x`
sanitize src dst:
    python3 tools/sanitize_session.py {{src}} {{dst}}

# Replay the corpus through scopinator-rs (session + IO/concurrency tests).
replay:
    cargo test -p scopinator-seestar --test session_replay --test control_integration

# Cross-language parity: replay the corpus through scopinator-rs AND pyscopinator
# and diff the normalized reports. Needs cargo + pyscopinator (uv).
parity:
    python3 conformance/parity/compare.py --run

# --- Coverage (cargo-llvm-cov). ---
# Install once: `cargo install cargo-llvm-cov` and `rustup component add llvm-tools-preview`.

cov:
    cargo llvm-cov --workspace --all-features --summary-only

cov-html:
    cargo llvm-cov --workspace --all-features --html
    @echo "open target/llvm-cov/html/index.html"

cov-lcov:
    cargo llvm-cov --workspace --all-features --lcov --output-path lcov.info

# --- Fuzzing (cargo-fuzz). Requires nightly toolchain. ---
# Install once: `cargo install cargo-fuzz`.

fuzz-list:
    cd fuzz && cargo +nightly fuzz list

# `just fuzz-run frame_parse` — runs until ctrl-c.
fuzz-run target:
    cd fuzz && cargo +nightly fuzz run {{target}}

# `just fuzz-smoke frame_parse` — 60s smoke run, like CI.
fuzz-smoke target seconds="60":
    cd fuzz && cargo +nightly fuzz run {{target}} -- -max_total_time={{seconds}}

# Reproduce a crash from a corpus file: `just fuzz-repro frame_parse fuzz/artifacts/frame_parse/crash-abc`.
fuzz-repro target input:
    cd fuzz && cargo +nightly fuzz run {{target}} {{input}}
