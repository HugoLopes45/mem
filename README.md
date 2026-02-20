# mem — persistent memory for Claude Code

[![CI](https://github.com/HugoLopes45/mem/actions/workflows/ci.yml/badge.svg)](https://github.com/HugoLopes45/mem/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/mem.svg)](https://crates.io/crates/mem)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**`mem` gives Claude Code a persistent memory that survives session ends, context compaction, and restarts — with zero agent cooperation.**

It hooks into Claude Code's `Stop`, `PreCompact`, and `SessionStart` events to automatically capture and restore session context. No `mem_save` calls. No setup per-project. Just install and wire the hooks once.

```
$ mem search "auth middleware"
[auto] your-project: added JWT auth middleware (2026-02-19)
  Switched from session cookies to JWT. Expiry: 24h. Refresh: 7d.
  3 files changed, 142 insertions(+)
```

---

## The problem you already have

You spend 90 minutes with Claude Code. You nail the architecture, make key decisions, figure out why the database connection pool needs a 30s timeout. Session ends.

Next day, you open a new session. **Claude remembers nothing.** You spend the first 20 minutes re-explaining context — what the project does, what you decided yesterday, what you already tried. Then you hit context limit. Compaction wipes the rest.

This is not a small annoyance. It is death by a thousand re-explanations.

### Why existing tools don't fix it

Every memory tool for Claude Code has the same fatal flaw: **the agent has to do it.**

The agent must decide what's worth saving. Remember to call `mem_save`. Load memory at the next session start. Under load, at the end of a long session, after context compaction — agents don't. Humans don't either.

> **Most sessions end without any memory saved. That's not a bug in your workflow. It's the design.**

`mem` removes the agent from the equation entirely. Claude Code's own hooks fire at session boundaries — reliably, automatically, whether the agent cooperates or not. The infrastructure captures memory. You never lose context again.

```
$ mem stats
Memories: 47 active, 3 cold
Projects: 6
Sessions: 31 captured
Last: your-api — 2026-02-20 (3 files, 89 insertions)
```

### What you get back

- **Session continuity** — every new session opens with the last 3 sessions already in context
- **Compaction survival** — PreCompact hook injects recent memories *before* the window is truncated
- **Zero overhead** — no `mem_save` calls, no per-project setup, no agent discipline required
- **Full-text search** — FTS5 + porter stemming across everything ever captured, including MEMORY.md files
- **Cross-project MEMORY.md index** — `mem index` indexes all `~/.claude/projects/*/memory/MEMORY.md` files; search finds lessons across every project instantly
- **Memory decay** — Ebbinghaus-style scoring automatically archives stale memories; accessed ones stay sharp
- **Cross-project memory** — promote a pattern to global scope; it appears in every project's context
- **CLAUDE.md suggestions** — `mem suggest-rules` analyses your sessions and outputs rules ready to paste
- **Session analytics** — `mem gain` shows token usage, cache efficiency, and top projects

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/HugoLopes45/mem/main/install.sh | bash
```

One command. Installs the binary and wires all three Claude Code hooks automatically via `mem init`. No JSON editing, no manual hook setup.

No system dependencies — SQLite is statically linked.

<details>
<summary>Install from source (Rust toolchain required)</summary>

```bash
cargo install --git https://github.com/HugoLopes45/mem --locked
mem init
```

Requires Rust 1.75+.
</details>

<details>
<summary>Build from source</summary>

```bash
git clone https://github.com/HugoLopes45/mem
cd mem
cargo build --release
./target/release/mem init
```

</details>

## Quick start

**1. Install and wire**

```bash
curl -fsSL https://raw.githubusercontent.com/HugoLopes45/mem/main/install.sh | bash
```

**2. Done.** Every session is now captured automatically.

```bash
mem status   # verify hooks are installed
mem gain     # token analytics after a few sessions
```

## How it works

Three hooks. Zero agent involvement.

```
Claude Code session
  │
  ├─ SessionStart hook  ← fires before Claude sees anything
  │     → reads project MEMORY.md + last 3 session summaries from ~/.mem/mem.db
  │     → outputs {"systemMessage":"..."} — injected directly into Claude's context
  │     → Claude opens with full context, zero file boilerplate
  │
  ├─ [session runs — agent works normally]
  │     → agent can also call MCP tools explicitly (mem_search, mem_save…)
  │
  ├─ PreCompact hook  ← fires before context window is truncated
  │     → injects recent memories as {"additionalContext": "..."}
  │     → Claude Code merges this into post-compaction context
  │     → nothing is lost when the window fills up
  │
  └─ Stop hook  ← fires when session ends, with or without agent cooperation
        → parses hook stdin: cwd, session_id, transcript_path
        → captures git log (committed work) + git diff --stat
        → parses transcript for token usage and turn counts
        → writes structured memory to ~/.mem/mem.db
        → no agent call required, no agent discipline required
```

Storage: `~/.mem/mem.db` — single SQLite file, WAL mode, FTS5 full-text search with porter stemming.

## Memory lifecycle

Every memory has a **scope** (project or global) and a **status** (active or cold).

Hard deletion is also available when you want to permanently remove a memory:

```bash
mem delete <id>   # irreversible — removes the row entirely
```

For soft archival (keeps the row, excludes it from results), use `mem decay` instead.

**Scope** controls visibility:
- `project` (default) — only visible when searching within that project
- `global` — visible across all projects; use for cross-cutting rules and conventions

**Status** tracks freshness via Ebbinghaus-style decay:
- `active` — returned in search and context queries
- `cold` — archived; excluded from results but not deleted

The retention score formula used by `mem decay` is:

```
retention = (access_count + 1) / (1 + days_since_created × 0.05)
```

Memories that accumulate access events decay slower. Run `mem decay --dry-run` to preview what would be archived before committing.

**Suggest-rules** analyses auto-captured memories for recurring terms and bigrams (pure frequency — no LLM) and outputs CLAUDE.md-ready markdown you can paste directly.

## MCP server

`mem` also runs as a Model Context Protocol (MCP) server so agents can explicitly search or save memories.

Add to `~/.claude/settings.json` or a project `.mcp.json`:

```json
{
  "mcpServers": {
    "mem": {"command": "mem", "args": ["mcp"]}
  }
}
```

**10 tools:**

| Tool | Purpose |
|------|---------|
| `mem_save` | Save a memory manually (decision, pattern, finding) |
| `mem_search` | Full-text search with FTS5 + porter stemming; includes global memories |
| `mem_context` | Load last N memories for a project; includes global memories |
| `mem_get` | Fetch a memory by ID |
| `mem_stats` | Database statistics (active/cold counts, projects, DB size) |
| `mem_session_start` | Register session start with optional goal |
| `mem_promote` | Promote a memory to global scope |
| `mem_demote` | Demote a memory back to project scope |
| `mem_suggest_rules` | Suggest CLAUDE.md rules from recurring session patterns |
| `mem_gain` | Session analytics as JSON: tokens, cache efficiency, top projects |

## CLI

```bash
# Setup
mem init         # wire hooks into ~/.claude/settings.json (idempotent)
mem status       # check hook install state + DB stats

# What's been captured
mem stats
mem search "database migration"
mem search "auth" --project /path/to/project --limit 20

# Cross-project MEMORY.md index
mem index                            # index all ~/.claude/projects/*/memory/MEMORY.md files
mem index --dry-run                  # preview what would be indexed (new / updated / unchanged)
mem index --path /path/to/MEMORY.md  # index a single file

# search now queries both auto-captured memories AND indexed MEMORY.md files
mem search "biome non-null"          # returns results from any project's MEMORY.md

# Manual save
mem save \
  --title "Chose rusqlite over sqlx" \
  --content "sqlx adds 3s subprocess startup; rusqlite sync = 40ms" \
  --memory-type decision

# Memory lifecycle
mem decay --dry-run                  # preview what would be archived
mem decay --threshold 0.1            # archive low-retention memories
mem delete <id>                      # hard-delete a memory (irreversible)
mem promote <id>                     # make a memory visible across all projects
mem demote <id>                      # return a memory to project scope

# Suggest CLAUDE.md rules from session patterns
mem suggest-rules                    # analyse last 20 auto-captured memories
mem suggest-rules --limit 50         # analyse more sessions

# Session analytics
mem gain                             # token usage, cache efficiency, top projects by tokens

# Test your hook setup
echo '{"cwd":"/your/project"}' | mem auto
echo '{"cwd":"/your/project"}' | mem context --compact

# Verify DB directly
sqlite3 ~/.mem/mem.db \
  "SELECT title, type, status, scope, created_at FROM memories ORDER BY created_at DESC LIMIT 10;"
```

## Configuration

| Env var | Default | Purpose |
|---------|---------|---------|
| `MEM_DB` | `~/.mem/mem.db` | Custom database path |
| `MEM_CLAUDE_DIR` | `~/.claude/projects/` | Override Claude Code projects root (used by `mem index`; also useful for tests) |

## Architecture

```
src/
  main.rs        CLI — subcommands: init, session-start, status, mcp, save, auto,
                        context, search, stats, decay, promote, demote,
                        suggest-rules, gain, delete, index
  types.rs       Domain types: Memory, MemoryType, MemoryStatus, MemoryScope,
                        IndexedFile, IndexStats, SearchResult, HookStdin,
                        TranscriptAnalytics, GainStats, SessionStartOutput
  db.rs          SQLite layer — rusqlite, FTS5, WAL, all queries, decay logic,
                        indexed_files upsert/search/list, unified search
  auto.rs        Auto-capture — hook stdin parsing, transcript analytics, git diff,
                        find_project_memory_md, MEMORY.md scanning
  mcp.rs         MCP server — rmcp 0.16, 10 tools, stdio transport
  suggest.rs     Rule suggestion engine — pure frequency analysis, no LLM
migrations/
  001_init.sql   Canonical schema: sessions + memories + FTS5 triggers + indexes
  002_indexed_files.sql  indexed_files table + FTS5 + sync triggers
hooks/
  mem-stop.sh           Stop hook wrapper
  mem-precompact.sh     PreCompact hook wrapper
  mem-session-start.sh  SessionStart hook wrapper (outputs systemMessage JSON)
```

**Dependencies:** [`rusqlite`](https://crates.io/crates/rusqlite) (bundled SQLite + FTS5) · [`rmcp`](https://crates.io/crates/rmcp) (official Rust MCP SDK) · [`clap`](https://crates.io/crates/clap)

## Design goals

| | Manual memory tools | **mem** |
|--|---------------------|---------|
| Capture trigger | Agent must call | **Hook — fires automatically** |
| Survives compaction | No | **Yes — PreCompact hook injects context** |
| Context on start | Agent must call | **Automatic — SessionStart injects systemMessage** |
| System deps | Varies | **None — bundled SQLite binary** |
| Memory freshness | Never expires | **Ebbinghaus decay — accessed memories stay, stale ones archive** |
| Cross-project memory | No | **Yes — promote any memory to global scope** |
| Pattern extraction | Manual | **`suggest-rules` analyses sessions → CLAUDE.md rules** |
| Search | Varies | **FTS5 + porter stemmer, full history** |
| Cross-project MEMORY.md | No | **`mem index` + unified search across all projects** |
| MCP tools | Varies | **10 built-in tools** |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). PRs welcome.

## License

MIT — see [LICENSE](LICENSE).
