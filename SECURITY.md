# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.2.x   | ✅ |
| 0.1.x   | ✅ |

## Reporting a Vulnerability

Please **do not** open a public GitHub issue for security vulnerabilities.

Report privately via [GitHub Security Advisories](https://github.com/HugoLopes45/mem/security/advisories/new).

Include:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

You'll receive a response within 48 hours. If confirmed, we'll coordinate a fix and disclosure timeline with you.

## Scope

`mem` stores session summaries locally in `~/.mem/mem.db`. It:
- Does **not** send data to any remote service
- Does **not** require network access (except `cargo install`)
- Runs as hook subprocesses with your user's permissions
- Executes `git` as a subprocess — no arbitrary command execution

The MCP server (`mem mcp`) communicates only over stdio with the local Claude Code process.

## Known attack surface

- **Hook-injected `transcript_path`**: the Stop hook receives a `transcript_path` from Claude Code via stdin JSON. `mem` validates this path is absolute and contains no `..` components before reading it. Relative paths and path traversal attempts are rejected and logged to stderr.
- **FTS5 query injection**: user-supplied search queries are phrase-quoted before being passed to SQLite's `MATCH` operator, preventing FTS5 operator injection.
- **MCP input validation**: `mem_save` and `mem_search` validate that title, content, and query are non-blank before reaching the database layer, returning `INVALID_PARAMS` on bad input.
