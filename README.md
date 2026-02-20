# mem

**Persistent memory for Claude Code.** Auto-captures session summaries at session end via hooks — no agent cooperation required.

```
Session ends → git diff captured → SQLite entry written → next session loads context automatically
```

## The problem

Claude Code agents forget everything when a session ends. Most memory solutions (engram, mem_save) require the agent to remember to save — three failure points, most sessions end without any memory saved.

`mem` uses Claude Code hooks that fire reliably at session boundaries. Zero agent involvement.

## Install

```bash
cargo install --git https://github.com/YOUR_ORG/mem
```

Or clone and build:

```bash
git clone https://github.com/YOUR_ORG/mem
cd mem
cargo install --path .
```

**No system dependencies.** SQLite is statically linked (`rusqlite --bundled`). Works on macOS and Linux.

## How it works

```
Claude Code session
  │
  ├─ SessionStart hook
  │     → writes .mem-context.md (last 3 session summaries)
  │     → @-include in project CLAUDE.md for auto-injection
  │
  ├─ [session runs — agent can also call MCP tools explicitly]
  │
  ├─ PreCompact hook
  │     → outputs {"additionalContext": "..."} JSON
  │     → recent memories survive context window compaction
  │
  └─ Stop hook
        → reads git diff --stat, session metadata from stdin
        → writes structured summary to ~/.mem/mem.db
        → no agent involvement needed
```

## Hook configuration

Copy the hook scripts from `hooks/` to a stable location, then add to `~/.claude/settings.json`:

```json
{
  "hooks": {
    "Stop": [{
      "hooks": [{
        "type": "command",
        "command": "/path/to/hooks/mem-stop.sh"
      }]
    }],
    "PreCompact": [{
      "matcher": "auto",
      "hooks": [{
        "type": "command",
        "command": "/path/to/hooks/mem-precompact.sh"
      }]
    }],
    "SessionStart": [{
      "hooks": [{
        "type": "command",
        "command": "/path/to/hooks/mem-session-start.sh"
      }]
    }]
  }
}
```

## MCP server

For explicit agent memory control, `mem` also runs as an MCP server. Add to `~/.claude/settings.json`:

```json
{
  "mcpServers": {
    "mem": {
      "command": "mem",
      "args": ["mcp"]
    }
  }
}
```

**Available tools:** `mem_save`, `mem_search`, `mem_context`, `mem_get`, `mem_stats`, `mem_session_start`

## CLI reference

```bash
# Check what's been captured
mem stats
mem search "auth middleware"
mem search "database schema" --project /path/to/project --limit 20

# Manual save
mem save \
  --title "Switched from sqlx to rusqlite" \
  --content "sqlx adds 3s to subprocess startup; rusqlite sync is 40ms" \
  --memory-type decision \
  --project /path/to/project

# Simulate hooks (for testing)
echo '{"cwd":"/your/project","session_id":"abc123"}' | mem save --auto
echo '{"cwd":"/your/project"}' | mem context --compact

# Interactive TUI (coming in v0.2)
mem tui
```

## Project context injection

At SessionStart, `mem-session-start.sh` writes `.mem-context.md` to your project root containing the last 3 session summaries. Reference it from your project `CLAUDE.md`:

```markdown
@.mem-context.md
```

Add to your project `.gitignore`:

```
.mem-context.md
```

## Storage

| Setting | Default | Override |
|---------|---------|----------|
| Database | `~/.mem/mem.db` | `MEM_DB=/custom/path.db` |
| Format | SQLite WAL + FTS5 | — |
| Search | Porter stemmer | — |

## Verification

```bash
# Build
cargo build --release && ./target/release/mem stats

# Simulate Stop hook
echo '{"cwd":"/tmp/test-project"}' | ./target/release/mem save --auto

# Check DB
sqlite3 ~/.mem/mem.db \
  "SELECT title, type, created_at FROM memories ORDER BY created_at DESC LIMIT 5;"

# Simulate PreCompact (must output valid JSON)
echo '{"cwd":"/tmp/test-project"}' | ./target/release/mem context --compact

# MCP handshake
printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}\n' \
  | ./target/release/mem mcp
```

## Architecture

```
src/
  main.rs        # clap CLI: mcp | save | context | search | stats | tui
  types.rs       # Memory, MemoryType, HookStdin, CompactContextOutput
  db.rs          # rusqlite: open, save, search (FTS5), recent, stats
  auto.rs        # Auto-capture: parse hook stdin + git diff --stat
  mcp.rs         # rmcp 0.16: 6 MCP tools over stdio
  tui.rs         # ratatui interactive search (v0.2)
migrations/
  001_init.sql   # memories + FTS5 + triggers + sessions
hooks/
  mem-stop.sh           # Stop hook wrapper
  mem-precompact.sh     # PreCompact hook wrapper
  mem-session-start.sh  # SessionStart hook wrapper
```

**Key dependencies:**
- [`rusqlite`](https://crates.io/crates/rusqlite) — sync SQLite, bundled, FTS5
- [`rmcp`](https://crates.io/crates/rmcp) — official Rust MCP SDK
- [`clap`](https://crates.io/crates/clap) — CLI
- [`ratatui`](https://crates.io/crates/ratatui) — TUI (v0.2)

## vs. engram

| | engram | mem |
|--|--------|-----|
| Memory capture | Agent must call save | **Automatic via Stop hook** |
| Context on compaction | Agent must call | **Automatic via PreCompact hook** |
| Context on start | Agent must call | **Automatic via SessionStart hook** |
| System deps | None | None (bundled SQLite) |
| Install | brew / binary | `cargo install` |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT — see [LICENSE](LICENSE).
