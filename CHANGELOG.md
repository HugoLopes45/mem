# Changelog

All notable changes to `mem` will be documented here.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
