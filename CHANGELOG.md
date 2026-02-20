# Changelog

All notable changes to `mem` will be documented here.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- Squashed pre-release incremental migrations (002–005) into a single canonical `001_init.sql`
- `db.rs` migration chain simplified to a single version gate

### Fixed
- MCP `mem_search`: blank query now returns `INVALID_PARAMS` instead of hitting FTS5 with a malformed MATCH expression
- Mutex poison recovery message was factually wrong ("state is intact"); now accurately describes the risk
- Stale `mem save --auto` references in comments and docs updated to `mem auto`

### Tests
- Added 20 tests (65 → 85): path traversal guard, `format_duration`/`format_tokens`/`efficiency_bar`, `MemoryType` roundtrip and error messages, `UserMemoryType` invariants

## [0.2.0] — 2026-02-20

### Added
- Session analytics: `mem gain` shows total tokens, cache efficiency %, avg turns, and top projects by token usage
- `mem_gain` MCP tool — returns session analytics as JSON
- Transcript parsing: Stop hook reads JSONL transcript to extract per-session token counts, turn counts, and duration
- Memory decay: `mem decay [--threshold 0.1] [--dry-run]` marks low-retention memories cold using Ebbinghaus retention formula `(access_count + 1) / (1 + days_since_created × 0.05)`
- Namespace scoping: memories now have a `scope` field (`project` | `global`). Global memories surface in search and context for all projects.
- `mem promote <id>` — elevate a memory to global scope
- `mem demote <id>` — return a memory to project scope
- `mem suggest-rules [--limit N]` — output CLAUDE.md-ready rule suggestions from recurring patterns in auto-captured session memories (pure frequency analysis, no LLM)
- 4 new MCP tools: `mem_promote`, `mem_demote`, `mem_suggest_rules`, `mem_gain`
- `suggest.rs` — standalone rule-suggestion engine with bigram and unigram detection
- `mem stats` now reports active and cold memory counts separately and includes session analytics summary

### Changed
- `mem save --auto` renamed to `mem auto` for clarity — the subcommand is invoked by the Stop hook, not manually
- `format_duration` now emits `"1m 05s"` for sub-hour durations (was emitting `"1m"`, dropping seconds)
- Git path arguments use `OsStr` directly to avoid lossy UTF-8 conversion on non-UTF-8 filesystem paths
- `Stdio::null()` added to all git subprocesses to prevent blocking on terminal prompts in hook context
- `ParseError` is now a typed `thiserror` enum (was `anyhow::Error`) — error messages include valid value hints
- `UserMemoryType` enum at MCP boundary prevents agents from setting `type = "auto"` via the save tool
- Performance indexes added for `status`, `type`, and `scope` columns — decay, auto-memory, and scope queries are no longer full-table scans
- `stats()` query collapsed from multiple round-trips to a single aggregating SQL query
- `run_decay` TOCTOU window eliminated — count returned from `conn.changes()` instead of a second SELECT

### Fixed
- `git diff` capture was always returning an empty diff due to incorrect argument order — fixed in `auto.rs`
- `install.sh` wget invocation fixed (was producing a 0-byte binary)
- Migration atomicity: `PRAGMA user_version` is set outside the transaction so it only advances after the DDL batch succeeds
- MCP: empty title or content now returns `INVALID_PARAMS` (was reaching the database layer and returning `INTERNAL_ERROR`)
- Transcript path validation: `parse_transcript` now rejects relative paths and paths with `..` components to prevent path traversal via hook-injected `transcript_path`

### Removed
- `tui.rs` no longer depends on ratatui/crossterm; the TUI subcommand prints a stub message until a full implementation is added

## [0.1.0] — 2026-02-20

### Added
- Auto-capture session summaries at `Stop` hook via `git diff --stat HEAD`
- `PreCompact` hook output — recent memories survive context compaction (`{"additionalContext": "..."}`)
- `SessionStart` hook — writes `.mem-context.md` to project root for `@`-inclusion in `CLAUDE.md`
- SQLite storage at `~/.mem/mem.db` — WAL mode, FTS5 full-text search with porter stemming
- 6 MCP tools via `rmcp 0.16` over stdio: `mem_save`, `mem_search`, `mem_context`, `mem_get`, `mem_stats`, `mem_session_start`
- CLI subcommands: `mcp`, `save`, `auto`, `context`, `search`, `stats`
- `MEM_DB` env var for custom database path
- Infinite loop guard — `stop_hook_active=true` detection prevents recursive hook invocations
- FTS5 injection protection — user queries are phrase-quoted before passing to `MATCH`
- `UserMemoryType` enum at MCP boundary — prevents agents from setting the `auto` capture type
- Hook scripts: `mem-stop.sh`, `mem-precompact.sh`, `mem-session-start.sh`
- Zero system dependencies — SQLite statically linked via `rusqlite --bundled`

[Unreleased]: https://github.com/HugoLopes45/mem/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/HugoLopes45/mem/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/HugoLopes45/mem/releases/tag/v0.1.0
