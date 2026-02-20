#!/usr/bin/env bash
# SessionStart hook — outputs {"systemMessage": "..."} JSON on stdout.
# Must always exit 0.

output=$(cat | mem session-start) || output=''
[ -z "$output" ] && output='{"systemMessage":""}'
echo "$output"
