#!/usr/bin/env bash
# SessionStart hook — writes recent memories to .mem-context.md in project root.
# Include via @.mem-context.md in your project CLAUDE.md.
# Must always exit 0.
set -euo pipefail

MEM_BIN="${MEM_BIN:-mem}"
MEM_LOG="${MEM_LOG:-}"

# Read cwd from hook stdin JSON
INPUT=$(cat)
CWD=$(echo "$INPUT" | jq -r '.cwd // empty' 2>/dev/null || true)

if [ -z "$CWD" ]; then
    CWD="$(pwd)"
fi

# Pass the hook JSON via stdin AND explicit --project so mem gets the right scope
# even if the binary reads stdin for cwd detection.
echo "$INPUT" | "$MEM_BIN" context --project "$CWD" --out "$CWD/.mem-context.md" 2>/dev/null || {
    [ -n "$MEM_LOG" ] && echo "[mem] warn: session-start context write failed for $CWD" >> "$MEM_LOG"
    true
}
