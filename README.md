# mem — session memory for Claude Code

[![CI](https://github.com/HugoLopes45/mem/actions/workflows/ci.yml/badge.svg)](https://github.com/HugoLopes45/mem/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/mem.svg)](https://crates.io/crates/mem)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Claude forgets everything between sessions. `mem` fixes that in two steps:

1. **Teaches Claude to maintain `MEMORY.md`** — adds a rule to `~/.claude/CLAUDE.md` so Claude updates the file at the end of every session with decisions, rejections, and patterns.
2. **Injects `MEMORY.md` at session start** — wires a `SessionStart` hook so Claude opens every session with full project context already in mind.

```bash
curl -fsSL https://raw.githubusercontent.com/HugoLopes45/mem/main/install.sh | bash
```

That's it. One command, zero manual config.

---

## How it works

```
mem init
  → adds rule to ~/.claude/CLAUDE.md   (Claude writes MEMORY.md at session end)
  → wires SessionStart hook             (MEMORY.md injected at session start)

Every session:
  start  → mem session-start injects MEMORY.md into Claude's context
  end    → Claude updates MEMORY.md per the rule (decisions, rejections, patterns)
```

The file lives at your project root. Claude reads it, Claude maintains it.

---

## Commands

```bash
mem init              # setup: wire hook + add rule to CLAUDE.md
mem status            # verify: hook installed? rule present? files indexed?
mem index             # index all MEMORY.md files for search
mem search <query>    # search across all indexed MEMORY.md files
```

---

## What goes in MEMORY.md

Claude writes this automatically. Example after a few sessions:

```markdown
# myproject

- Auth: JWT, not sessions — mobile client needs stateless (2026-02-18)
- Tried Prisma, switched to raw SQL — too much magic for this schema
- Don't use `any` — Biome enforces strict types, CI will fail
- Payment webhooks must be idempotent — Stripe retries on timeout
- DB migrations: always add column nullable first, backfill, then add constraint
```

Decisions, rejections, patterns. Things Claude would otherwise ask about again.

---

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/HugoLopes45/mem/main/install.sh | bash
```

<details>
<summary>From source</summary>

```bash
cargo install --git https://github.com/HugoLopes45/mem --locked
mem init
```

Requires Rust 1.75+.
</details>

---

## Verify

```bash
mem status
```

```
Binary    : /Users/you/.cargo/bin/mem
Hook      : installed
Rule      : installed
Indexed   : 3 MEMORY.md file(s)
```

---

## Search across projects

```bash
mem index                    # index all MEMORY.md files
mem search "jwt"             # find decisions across all projects
mem search "rejected"        # find things you decided not to do
```

---

## License

MIT — see [LICENSE](LICENSE).
