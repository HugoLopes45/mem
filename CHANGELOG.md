# Changelog

All notable changes to `mem` will be documented here.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `mem init` — atomically patches `~/.claude/settings.json` with all three hooks (SessionStart, Stop, PreCompact auto+manual). Idempotent. Resolves binary path dynamically via `current_exe()`. Called automatically by `install.sh` — zero manual JSON editing.
- `mem session-start` — SessionStart hook handler. Outputs `{"systemMessage":"..."}` JSON containing project MEMORY.md + global `~/.claude/MEMORY.md` + last 3 DB captures. Falls back to empty string on any error; always exits 0.
- `mem status` — shows binary path, hook install state, DB statistics (memories, sessions, projects, DB size, last capture time), and cache efficiency.
- `find_project_memory_md()` in `auto.rs` — two-strategy MEMORY.md lookup: git repo root first, then `~/.claude/projects/<encoded>/memory/MEMORY.md`.
- `last_capture_time()` DB method — returns the timestamp of the most recent auto-captured memory.
- `SessionStartOutput` type in `types.rs` — `{"systemMessage":"..."}` protocol struct.
- `install.sh` calls `mem init` automatically after binary install — one curl command fully wires everything.
- 20 new tests: `build_title` with session_summary (priority 1), truncation, first-line extraction; `find_project_memory_md` (absent, non-git); `cmd_init` (adds hooks, idempotent, preserves existing keys); `check_mem_hooks_present`; `last_capture_time`.

### Changed
- `build_title` priority: session summary (Claude's last message, first line, 100-char truncation) → commit message → diff stat → fallback. Previously: commit message was priority 1.
- `capture_and_save` now parses transcript **before** building title so the session summary is available for `build_title`.
- `cmd_context --compact` now prepends `# Project Memory\n\n{MEMORY.md content}` before recent captures.
- `cmd_init` extracted into `apply_hooks_to_settings(&Path)` (SRP) — `cmd_init` resolves path and prints; the settings-patching logic is reusable and directly testable.
- `map_or(false, ...)` → `is_some_and(...)` throughout (idiomatic Rust 1.70+).
- `hooks/mem-session-start.sh` simplified: pipes stdin → `mem session-start` → stdout JSON. No longer writes `.mem-context.md`.

### Removed
- `.mem-context.md` file-write pattern — replaced by `systemMessage` JSON protocol (native Claude Code API, no extra file or `@`-include needed).
- Dead `MEM_BIN` env var from configuration table — hook scripts now use the wired binary path from `mem init`.

- `mem index` — scan all `~/.claude/projects/*/memory/MEMORY.md` files and index them into a new `indexed_files` FTS5 table. Supports `--dry-run` (preview with accurate new/updated/unchanged status) and `--path <file>` (single-file mode).
- `mem search` now queries both auto-captured memories **and** indexed MEMORY.md files in a single unified result set, interleaved by relevance. Results are labelled `[MEMORY.md: <project>]` to distinguish source.
- `--project` filter on `mem search` now applies to both memories and indexed files (previously only filtered memories).
- `MEM_CLAUDE_DIR` env var — override the Claude Code projects root used by `mem index` (default: `~/.claude/projects/`). Useful for CI and tests.
- Migration `002_indexed_files.sql` — `indexed_files` table with FTS5 virtual table, three sync triggers (insert/delete/update), and a `project_name` index. Applied via table-existence check rather than `user_version` to handle databases created before the schema squash.
- `read_mtime_secs()` helper in `auto.rs` — replaces the silent `.ok().and_then()` mtime chain; now logs a warning at each failure point (stat, modified(), epoch conversion) so the `Unchanged` optimisation is never silently broken.
- `IndexStats::record()` — enforces counter/entries consistency at every mutation site; `skipped` entries are counted but excluded from the display list.
- 12 new tests (118 → 130 total): project filter on indexed-file search, FTS trigger correctness after update, `search_unified` edge cases (only memories, only files, limit enforcement), `list_indexed_files` ordering, scan Unchanged/Updated paths, `decode_project_path` edge cases (empty entries, malformed JSON), `extract_title` negative cases (H2, no space after `#`).

### Changed
- `search_indexed_files` signature gains a `project: Option<&str>` parameter for `--project` scoping.
- `search_unified` now queries each source for up to `limit` results (was `limit/2`) so neither source is starved when the other returns few matches.
- `decode_project_path` now logs a warning (`[mem] warn: malformed sessions-index.json`) when the JSON file exists but fails to parse, before falling back to the naive hyphen-decode strategy.
- Non-UTF-8 project directory names are now logged and skipped (previously stored an empty string as `project_name`).
- `DirEntry` iteration errors in `scan_and_index_memory_files_in` now increment `stats.skipped` and emit a warning (previously silently continued).
- `file_mtime` column renamed to `file_mtime_secs` in the DB and `IndexedFile` struct — unit (Unix seconds) is now explicit in both name and doc comment.
- Migration gate uses `sqlite_master` table-existence check and propagates query errors with `?` instead of `.unwrap_or(0)`.
- Doc comment for `decode_project_path` Strategy 2 corrected: was "prepend nothing"; now accurately describes "strip leading `-`, replace remaining `-` with `/`, prepend `/`".

## [0.3.0] — 2026-02-20

### Added
- `mem delete <id>` — hard-delete a memory by ID (irreversible; use `mem decay` for soft archival)
- 20 new MCP handler tests covering all 10 tools: validation, limit capping, session ID generation, save/search/get/stats/context/promote/demote/suggest_rules/gain
- `cargo audit` step in CI — blocks on CVE findings before build

### Changed
- `lock_db()` helper in `mcp.rs` replaces 10× copy-pasted mutex guard pattern
- Mutex poison now returns an MCP error instead of continuing with a potentially corrupt `rusqlite::Connection`
- `track_access_batch` in `db.rs` uses a transaction + per-ID loop instead of dynamic SQL string building
- `split_tokens()` extracted as a shared helper in `suggest.rs`; `tokenize()` delegates to it
- Squashed pre-release incremental migrations (002–005) into a single canonical `001_init.sql`
- `db.rs` migration chain simplified to a single version gate

### Fixed
- `suggest.rs` stop-word list had `"2026"` hardcoded — replaced with `is_year_token()` covering 2000–2099; no manual updates needed
- MCP `mem_search`: blank query now returns `INVALID_PARAMS` instead of hitting FTS5 with a malformed MATCH expression
- Stale `mem save --auto` references in comments updated to `mem auto`

### Tests
- Added 20 MCP handler tests (85 → 104 total): limit capping, empty-query rejection, session ID fallback, roundtrip promote/demote

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

[Unreleased]: https://github.com/HugoLopes45/mem/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/HugoLopes45/mem/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/HugoLopes45/mem/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/HugoLopes45/mem/releases/tag/v0.1.0
