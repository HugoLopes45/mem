# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
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
