# mem

Persistent memory for Claude Code. Auto-captures session summaries to SQLite at session end. Zero agent cooperation required.

## Install

```bash
cargo install --path .
# or, once published:
cargo install mem
```

Requires: Rust 1.75+. No system deps — SQLite is statically linked.

## What it does

```
Claude Code session
  │
  ├─ SessionStart hook → writes .mem-context.md (last 3 sessions)
  │     → @-include in project CLAUDE.md for auto-injection
  │
  ├─ [session runs — agent optionally calls MCP tools]
  │
  ├─ PreCompact hook → outputs {"additionalContext": "..."} JSON
  │     → recent memories survive context compaction
  │
  └─ Stop hook → mem save --auto
        → reads git diff --stat, session metadata
        → writes structured summary to ~/.mem/mem.db
```

## Hook configuration

Paste into `~/.claude/settings.json` (merge with existing `hooks`):

```json
{
  "hooks": {
    "Stop": [{
      "hooks": [{
        "type": "command",
        "command": "/path/to/mem/hooks/mem-stop.sh"
      }]
    }],
    "PreCompact": [{
      "matcher": "auto",
      "hooks": [{
        "type": "command",
        "command": "/path/to/mem/hooks/mem-precompact.sh"
      }]
    }],
    "SessionStart": [{
      "hooks": [{
        "type": "command",
        "command": "/path/to/mem/hooks/mem-session-start.sh"
      }]
    }]
  }
}
```

Replace `/path/to/mem/hooks/` with the actual path, or install `mem` globally (`cargo install`) and use the scripts directly.

## MCP server

Add to `~/.claude/settings.json` or `.mcp.json`:

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

**6 MCP tools**: `mem_save`, `mem_search`, `mem_context`, `mem_get`, `mem_stats`, `mem_session_start`

## CLI

```bash
# Search memories
mem search "auth middleware"
mem search "database schema" --project /path/to/project

# View stats
mem stats

# Manual save
mem save --title "Key decision" --content "We chose X because Y" --memory-type decision

# Simulate Stop hook (for testing)
echo '{"cwd":"/your/project","session_id":"abc123"}' | mem save --auto

# Simulate PreCompact hook
echo '{"cwd":"/your/project"}' | mem context --compact
```

## Storage

- Database: `~/.mem/mem.db` (SQLite, WAL mode)
- Override: `MEM_DB=/custom/path.db mem stats`
- FTS5 with porter stemmer for English search

## Project context injection

At SessionStart, `mem-session-start.sh` writes `.mem-context.md` to your project root.
Add to your project `CLAUDE.md`:

```
@.mem-context.md
```

Add to `.gitignore`:
```
.mem-context.md
```

## Verification

```bash
# Build and test
cargo build --release
./target/release/mem stats

# Simulate Stop hook
echo '{"cwd":"/tmp/test"}' | ./target/release/mem save --auto

# Check DB
sqlite3 ~/.mem/mem.db "SELECT title, type, created_at FROM memories ORDER BY created_at DESC LIMIT 5;"

# Simulate PreCompact
echo '{"cwd":"/tmp/test"}' | ./target/release/mem context --compact
# Expected: {"additionalContext": "..."}

# MCP smoke test
printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}\n' | ./target/release/mem mcp
```
