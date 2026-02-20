#!/usr/bin/env bash
# SessionStart hook — outputs {"systemMessage": "..."} JSON on stdout.
# Called automatically by the SessionStart hook wired via `mem init`.
# Must always exit 0.
set -euo pipefail

MEM_BIN="${MEM_BIN:-mem}"

output=$(cat | "$MEM_BIN" session-start 2>/dev/null) || output=''
[ -z "$output" ] && output='{"systemMessage":""}'
echo "$output"
