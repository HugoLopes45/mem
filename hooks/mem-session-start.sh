#!/usr/bin/env bash
# SessionStart hook — writes recent memories to .mem-context.md in project root.
# Include this file via @.mem-context.md in your project CLAUDE.md.
set -euo pipefail

MEM_BIN="${MEM_BIN:-mem}"

# Read cwd from hook stdin JSON
INPUT=$(cat)
CWD=$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null || true)

if [ -z "$CWD" ]; then
    CWD="$(pwd)"
fi

# Write context file to project root
"$MEM_BIN" context --out "$CWD/.mem-context.md" 2>/dev/null || true
