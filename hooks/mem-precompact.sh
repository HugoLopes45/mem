#!/usr/bin/env bash
# PreCompact hook — injects recent memories into post-compaction context.
# Must output: {"additionalContext": "..."} on stdout.
# Reads hook stdin JSON (contains cwd).
set -euo pipefail

MEM_BIN="${MEM_BIN:-mem}"

# mem context reads stdin hook JSON for cwd, outputs additionalContext JSON
"$MEM_BIN" context --compact 2>/dev/null || echo '{"additionalContext":""}'
