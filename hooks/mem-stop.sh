#!/usr/bin/env bash
# Stop hook — auto-captures session memory to SQLite.
# Reads hook stdin JSON (contains cwd, session_id, stop_hook_active).
# Called by Claude Code at session end. Must always exit 0.
set -euo pipefail

MEM_BIN="${MEM_BIN:-mem}"
MEM_LOG="${MEM_LOG:-}"  # set to a file path to enable debug logging

# Note: do NOT use `exec` here — it replaces the shell, making `|| true` dead code
# if the binary is missing. Call the binary directly so the fallback applies.
"$MEM_BIN" auto 2>/dev/null || {
    [ -n "$MEM_LOG" ] && echo "[mem] warn: auto failed (exit $?)" >> "$MEM_LOG"
    true
}
