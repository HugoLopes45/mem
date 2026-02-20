#!/usr/bin/env bash
# PreCompact hook — injects recent memories into post-compaction context.
# Must output: {"additionalContext": "..."} on stdout. Must always exit 0.
# Reads hook stdin JSON (contains cwd) — passed through to mem via stdin inheritance.
set -euo pipefail

MEM_BIN="${MEM_BIN:-mem}"
MEM_LOG="${MEM_LOG:-}"

# mem context --compact reads stdin for cwd and outputs additionalContext JSON.
# Stdin is inherited from the hook caller — no explicit piping needed.
# Capture output explicitly: zero-exit with empty stdout is also a failure mode
# for the PreCompact protocol (requires non-empty JSON on stdout).
output=$("$MEM_BIN" context --compact 2>/dev/null) || output=''
if [ -z "$output" ]; then
    [ -n "$MEM_LOG" ] && echo "[mem] warn: context --compact produced no output" >> "$MEM_LOG"
    output='{"additionalContext":""}'
fi
echo "$output"
