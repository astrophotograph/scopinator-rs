
## VCS

This project uses **jj** (Jujutsu), not git.

- Commit: `jj describe -m "message"` then `jj new`
- Push: `jj bookmark set main -r @- && jj git push`
- Log: `jj log`

## Task Management

Task specs and feature specs live in the **Forge** notebook in Nous. When working on a task:

- Use `mcp__nous__get_page` to read the task spec from Forge (e.g., "Task: FUSE Mount — Read-Only")
- To check task status and dependencies, use the **targeted query tools** (NOT `get_database`, which is too large):
  - `mcp__nous__task_summary` — cheapest: task counts by project/status/feature
  - `mcp__nous__query_tasks` — filtered queries with compact rows (by project, feature, status, phase, priority, blocked state)
  - `mcp__nous__get_feature_tasks` — tasks for a project/feature in dependency-resolved execution order
- Update task status via `mcp__nous__update_database_rows` in the Project Tasks database (not internal task tools)
- Feature pages in Forge contain the full context: data model, API contracts, edge cases, test plans

Do NOT use `mcp__nous__get_database` on the Project Tasks database — it returns too much data. Use the targeted query tools above.

Do NOT create ad-hoc task tracking internally — all task state lives in Forge.

## Conventions

- **Error handling**: `thiserror` for library crate errors, `anyhow` for CLI
- **Logging**: `tracing` for structured logging, `tracing-subscriber` for output
- **Async**: `tokio` runtime, `async-trait` for async trait definitions
- **Types**: strong newtypes (BlobId, not String), exhaustive enums
- **Testing**: unit tests in-module, integration tests in `tests/`
