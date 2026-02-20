# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/en/1.0.0/) · [Semantic Versioning](https://semver.org/spec/v2.0.0.html)

## [0.5.0] — 2026-02-20

Complete rewrite. Dropped SQLite, MCP server, auto-capture, and 11 commands.

### What mem does now

1. `mem init` — wires `SessionStart` hook + adds memory rule to `~/.claude/CLAUDE.md`
2. `mem session-start` — injects `MEMORY.md` at session start (called by hook)
3. `mem index` — scans `~/.claude/projects/*/memory/MEMORY.md`, stores to `~/.mem/index.json`
4. `mem search <query>` — greps the index
5. `mem status` — hook installed? rule present? files indexed?

### Why

The previous design captured git diffs and Claude's last message automatically.
The content was noise. The only thing that actually helps Claude is a curated `MEMORY.md`
with decisions, rejections, and patterns — written by the agent that has context, not by a
hook that only sees file stats.

`mem init` now teaches Claude to maintain `MEMORY.md` itself via a rule in `CLAUDE.md`.
`mem session-start` injects it. That's the whole product.

### Removed

- SQLite database (`~/.mem/mem.db`)
- MCP server (`mem mcp`, 10 MCP tools)
- Stop hook / auto-capture (`mem auto`)
- PreCompact hook (`mem context --compact`)
- `mem save`, `mem decay`, `mem promote`, `mem demote`, `mem delete`
- `mem suggest-rules`, `mem gain`, `mem stats`
- All migrations (`migrations/`)
- Dead source files: `src/auto.rs`, `src/db.rs`, `src/mcp.rs`, `src/suggest.rs`, `src/types.rs`

### Fixed

- `load_index` distinguishes file-not-found (silent) from corrupt JSON / I/O error (logged)
- `resolve_cwd` logs unexpected stdin content before falling back to cwd
- `find_memory_md` read failures surface to stderr instead of silently returning `None`
- Global `MEMORY.md` read errors now logged
- `cmd_index` tracks error count, shows it in summary, exits 1 on errors
- `cmd_index` prunes stale entries (deleted files no longer accumulate in index)
- `find_memory_md` Strategy 2 encoding now includes `.replace('.', "-")` to match Claude Code
- `decode_project_name` no longer attempts lossy reverse-decode of encoded dir names
- Location 1 in `cmd_index` removed — lossy decode produced wrong paths for hyphenated projects

### Binary size

623 KB (was ~4 MB with bundled SQLite + MCP SDK)

---

## [0.4.0] — 2026-02-19

- `mem init` wires all three hooks atomically
- `mem session-start` outputs `{"systemMessage":"..."}` protocol
- `install.sh` calls `mem init` — zero manual steps

## [0.3.0] — 2026-02-18

- `mem delete <id>` — hard-delete a memory
- MCP handler tests (all 10 tools)
- `cargo audit` in CI

## [0.2.0] — 2026-02-17

- FTS5 full-text search
- `mem gain` — session analytics
- `mem suggest-rules` — CLAUDE.md rule suggestions from session patterns

## [0.1.0] — 2026-02-15

- Initial release: Stop hook auto-capture, SQLite storage, MCP server
