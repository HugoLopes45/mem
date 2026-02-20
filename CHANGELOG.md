# Changelog

All notable changes to `mem` will be documented here.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] — 2026-02-20

### Added
- Memory decay: `mem decay [--threshold 0.1] [--dry-run]` marks low-retention memories cold using Ebbinghaus retention formula `(access_count + 1) / (1 + days_since_created × 0.05)`
- Namespace scoping: memories now have a `scope` field (`project` | `global`). Global memories surface in search and context for all projects.
- `mem promote <id>` — elevate a memory to global scope
- `mem demote <id>` — return a memory to project scope
- `mem suggest-rules [--limit N]` — output CLAUDE.md-ready rule suggestions from recurring patterns in auto-captured session memories (pure frequency analysis, no LLM)
- 3 new MCP tools: `mem_promote`, `mem_demote`, `mem_suggest_rules`
- `suggest.rs` — standalone rule-suggestion engine with bigram and unigram detection
- Migration `002_decay_scope.sql` — adds `access_count`, `last_accessed_at`, `status`, `scope` columns to the `memories` table
- `mem stats` now reports active and cold memory counts separately

### Fixed
- `git diff` capture was always returning an empty diff due to incorrect argument order — fixed in `auto.rs`
- `install.sh` wget invocation fixed (was producing a 0-byte binary)
- Migration atomicity: `PRAGMA user_version` is now set outside the transaction so it only advances after the DDL batch succeeds

### Removed
- `tui.rs` no longer depends on ratatui/crossterm; the TUI subcommand prints a stub message until a full implementation is added

## [0.1.0] — 2026-02-20

### Added
- Auto-capture session summaries at `Stop` hook via `git diff --stat HEAD`
- `PreCompact` hook output — recent memories survive context compaction (`{"additionalContext": "..."}`)
- `SessionStart` hook — writes `.mem-context.md` to project root for `@`-inclusion in `CLAUDE.md`
- SQLite storage at `~/.mem/mem.db` — WAL mode, FTS5 full-text search with porter stemming
- 6 MCP tools via `rmcp 0.16` over stdio: `mem_save`, `mem_search`, `mem_context`, `mem_get`, `mem_stats`, `mem_session_start`
- CLI subcommands: `mcp`, `save`, `context`, `search`, `stats`, `tui` (stub)
- `MEM_DB` env var for custom database path
- `MEM_LOG` env var for hook debug logging
- `infinite loop guard` — `stop_hook_active=true` detection prevents recursive hook invocations
- FTS5 injection protection — user queries are phrase-quoted before passing to `MATCH`
- `UserMemoryType` enum at MCP boundary — prevents agents from setting the `auto` capture type
- Hook scripts: `mem-stop.sh`, `mem-precompact.sh`, `mem-session-start.sh`
- Zero system dependencies — SQLite statically linked via `rusqlite --bundled`

[Unreleased]: https://github.com/HugoLopes45/mem/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/HugoLopes45/mem/releases/tag/v0.1.0
