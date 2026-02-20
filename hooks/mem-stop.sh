#!/usr/bin/env bash
# Stop hook — auto-captures session memory to SQLite.
# Reads hook stdin JSON (contains cwd, session_id, stop_hook_active).
# Called by Claude Code at session end.
set -euo pipefail

MEM_BIN="${MEM_BIN:-mem}"

# Pass stdin through to mem — it reads and parses the hook JSON
exec "$MEM_BIN" save --auto 2>/dev/null || true
