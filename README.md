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
- **Full-text search** — FTS5 + porter stemming across everything ever captured
- **Memory decay** — Ebbinghaus-style scoring automatically archives stale memories; accessed ones stay sharp
- **Cross-project memory** — promote a pattern to global scope; it appears in every project's context
- **CLAUDE.md suggestions** — `mem suggest-rules` analyses your sessions and outputs rules ready to paste

## Install

```bash
cargo install --git https://github.com/HugoLopes45/mem
```

No system dependencies. SQLite is statically linked — `cargo install` is all you need.

<details>
<summary>Build from source</summary>

```bash
git clone https://github.com/HugoLopes45/mem
cd mem
cargo build --release
# binary at: ./target/release/mem
```

Requires Rust 1.75+.
</details>

## Quick start

**1. Install `mem`**

```bash
cargo install --git https://github.com/HugoLopes45/mem
```

**2. Copy hook scripts to a stable location**

```bash
cp /path/to/mem/hooks/* ~/.claude/hooks/
chmod +x ~/.claude/hooks/mem-*.sh
```

**3. Add to `~/.claude/settings.json`**

```json
{
  "hooks": {
    "Stop": [{
      "hooks": [{"type": "command", "command": "~/.claude/hooks/mem-stop.sh"}]
    }],
    "PreCompact": [{
      "matcher": "auto",
      "hooks": [{"type": "command", "command": "~/.claude/hooks/mem-precompact.sh"}]
    }],
    "SessionStart": [{
      "hooks": [{"type": "command", "command": "~/.claude/hooks/mem-session-start.sh"}]
    }]
  }
}
```

**4. Done.** Every session end is now captured automatically.

## How it works

Three hooks. Zero agent involvement.

```
Claude Code session
  │
  ├─ SessionStart hook  ← fires before Claude sees anything
  │     → reads last 3 session summaries from ~/.mem/mem.db
  │     → writes .mem-context.md to project root
  │     → @-included in CLAUDE.md → Claude opens with full context
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
        → writes structured memory to ~/.mem/mem.db
        → no agent call required, no agent discipline required
```

Storage: `~/.mem/mem.db` — single SQLite file, WAL mode, FTS5 full-text search with porter stemming.

## Memory lifecycle

Every memory has a **scope** (project or global) and a **status** (active or cold).

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

**9 tools:**

| Tool | Purpose |
|------|---------|
| `mem_save` | Save a memory manually (decision, pattern, finding) |
| `mem_search` | Full-text search with FTS5 query syntax; includes global memories |
| `mem_context` | Load last N memories for a project; includes global memories |
| `mem_get` | Fetch a memory by ID |
| `mem_stats` | Database statistics (active/cold counts, projects, DB size) |
| `mem_session_start` | Register session start with optional goal |
| `mem_promote` | Promote a memory to global scope |
| `mem_demote` | Demote a memory back to project scope |
| `mem_suggest_rules` | Suggest CLAUDE.md rules from recurring session patterns |

## CLI

```bash
# What's been captured
mem stats
mem search "database migration"
mem search "auth" --project /path/to/project --limit 20

# Manual save
mem save \
  --title "Chose rusqlite over sqlx" \
  --content "sqlx adds 3s subprocess startup; rusqlite sync = 40ms" \
  --memory-type decision

# Memory lifecycle
mem decay --dry-run                  # preview what would be archived
mem decay --threshold 0.1            # archive low-retention memories
mem promote <id>                     # make a memory visible across all projects
mem demote <id>                      # return a memory to project scope

# Suggest CLAUDE.md rules from session patterns
mem suggest-rules                    # analyse last 20 auto-captured memories
mem suggest-rules --limit 50         # analyse more sessions

# Test your hook setup
echo '{"cwd":"/your/project"}' | mem save --auto
echo '{"cwd":"/your/project"}' | mem context --compact

# Verify DB directly
sqlite3 ~/.mem/mem.db \
  "SELECT title, type, status, scope, created_at FROM memories ORDER BY created_at DESC LIMIT 10;"
```

## Project context injection

`mem-session-start.sh` writes `.mem-context.md` to your project root at each session start. Reference it from your project `CLAUDE.md`:

```markdown
@.mem-context.md
```

Add to `.gitignore`:

```
.mem-context.md
```

Now every new Claude Code session opens with the last 3 session summaries already in context.

## Configuration

| Env var | Default | Purpose |
|---------|---------|---------|
| `MEM_DB` | `~/.mem/mem.db` | Custom database path |
| `MEM_BIN` | `mem` | Custom binary path (for hook scripts) |

## Architecture

```
src/
  main.rs        CLI — subcommands: mcp, save, context, search, stats, decay, promote, demote, suggest-rules, tui
  types.rs       Shared types: Memory, MemoryType, MemoryStatus, MemoryScope, HookStdin, CompactContextOutput
  db.rs          SQLite layer — rusqlite, FTS5, WAL, all queries, decay logic
  auto.rs        Auto-capture — hook stdin parsing, git diff, title generation
  mcp.rs         MCP server — rmcp 0.16, 9 tools, stdio transport
  suggest.rs     Rule suggestion engine — pure frequency analysis, no LLM
  tui.rs         Interactive TUI (not yet implemented)
migrations/
  001_init.sql   Schema: memories + FTS5 triggers + sessions
  002_decay_scope.sql  Adds access_count, last_accessed_at, status, scope columns
hooks/
  mem-stop.sh           Stop hook wrapper
  mem-precompact.sh     PreCompact hook wrapper
  mem-session-start.sh  SessionStart hook wrapper
```

**Dependencies:** [`rusqlite`](https://crates.io/crates/rusqlite) (bundled SQLite + FTS5) · [`rmcp`](https://crates.io/crates/rmcp) (official Rust MCP SDK) · [`clap`](https://crates.io/crates/clap)

## Design goals

| | Manual memory tools | **mem** |
|--|---------------------|---------|
| Capture trigger | Agent must call | **Hook — fires automatically** |
| Survives compaction | No | **Yes — PreCompact hook injects context** |
| Context on start | Agent must call | **Automatic — SessionStart writes it** |
| System deps | Varies | **None — bundled SQLite binary** |
| Memory freshness | Never expires | **Ebbinghaus decay — accessed memories stay, stale ones archive** |
| Cross-project memory | No | **Yes — promote any memory to global scope** |
| Pattern extraction | Manual | **`suggest-rules` analyses sessions → CLAUDE.md rules** |
| Search | Varies | **FTS5 + porter stemmer, full history** |
| MCP tools | Varies | **9 built-in tools** |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). PRs welcome.

## License

MIT — see [LICENSE](LICENSE).
