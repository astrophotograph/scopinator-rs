---
name: scopinator-rs project context
description: Rust port of pyscopinator telescope control library, starting with Seestar support
type: project
---

scopinator-rs is a pure Rust implementation of the pyscopinator Python telescope control library.

**Why:** User wants a Rust telescope control library that currently targets Seestar smart telescopes but is architected for future telescope backends (Alpaca, INDI).

**How to apply:**
- Primary reference: ~/Projects/erewhon/pyscopinator (Python, has full command/event models, V2 abstraction layer, sequencer, CLI)
- Secondary reference: ~/Projects/erewhon/seestar-proxy (Rust, battle-tested connection/framing/reconnect logic, no typed models)
- Use pyscopinator for protocol semantics, command/event types, and API design
- Use seestar-proxy for Rust-specific patterns: tokio channels, frame fan-out with Arc<Vec<u8>>, ID remapping, reconnect flushing
