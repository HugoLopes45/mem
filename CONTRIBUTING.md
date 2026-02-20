# Contributing to mem

## Development setup

```bash
git clone https://github.com/HugoLopes45/mem
cd mem
git config core.hooksPath .githooks   # enforces fmt + clippy on commit
cargo build
cargo test
```

## Running locally

```bash
# Dev build (fast)
cargo build

# Test Stop hook simulation
echo '{"cwd":"/tmp/test"}' | ./target/debug/mem auto

# Test PreCompact simulation
echo '{"cwd":"/tmp/test"}' | ./target/debug/mem context --compact

# Test search
./target/debug/mem search "your query"
```

## Project structure

```
src/
  main.rs      CLI entry point — add new subcommands here
  types.rs     Domain types — keep this small and stable
  db.rs        All database logic — queries, schema, FTS5
  auto.rs      Auto-capture logic — hook stdin parsing, transcript analytics, git diff
  mcp.rs       MCP server — add new tools here
  suggest.rs   Rule suggestion engine
migrations/
  001_init.sql Canonical schema — edit this file to change the schema
hooks/
  *.sh         Shell wrappers for Claude Code hook events
```

## Adding a new MCP tool

1. Define input struct with `#[derive(Deserialize, JsonSchema)]` in `mcp.rs`
2. Add `async fn your_tool(&self, params: Parameters<YourParams>)` inside the `#[tool_router]` impl
3. Annotate with `#[tool(description = "...")]`

The `#[tool_handler]` macro on `ServerHandler` picks it up automatically.

## Database schema changes

`mem` uses a single canonical schema file (`migrations/001_init.sql`) applied once on a fresh database. All DDL uses `IF NOT EXISTS` so the batch is safe to re-run.

`db.rs` gates the apply on `PRAGMA user_version < 1` — the version is advanced to 1 after the first successful apply and never re-applied.

**To change the schema:**

1. Edit `migrations/001_init.sql` directly — add your columns, indexes, or tables
2. Bump `PRAGMA user_version` in `db.rs` if you need to apply an `ALTER TABLE` to existing databases:

```rust
// db.rs — add after the existing version < 1 block:
if version < 2 {
    conn.execute_batch("ALTER TABLE memories ADD COLUMN your_column TEXT;")?;
    conn.execute_batch("PRAGMA user_version = 2;")?;
}
```

For pre-release changes where no existing database needs upgrading (development-only), simply edit `001_init.sql` directly without adding a version block.

## Testing

```bash
cargo test
```

Integration tests simulate hook stdin:

```bash
echo '{"cwd":"/tmp/test","session_id":"test-123","stop_hook_active":false}' \
  | MEM_DB=/tmp/test-mem.db ./target/debug/mem auto
```

Coverage targets:
- 80%+ overall
- 100% for security-sensitive paths (e.g. `is_safe_transcript_path`)

## Pull requests

- Keep PRs focused — one feature or fix per PR
- Write a failing test before implementing new behaviour
- Run `cargo clippy` before submitting
- Update README if you add a CLI flag or MCP tool
- Update CHANGELOG under `[Unreleased]`

## Reporting bugs

Open an issue with:
- OS and Rust version (`rustc --version`)
- Steps to reproduce
- Expected vs actual output
