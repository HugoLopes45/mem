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
echo '{"cwd":"/tmp/test"}' | ./target/debug/mem save --auto

# Test PreCompact simulation
echo '{"cwd":"/tmp/test"}' | ./target/debug/mem context --compact

# Test search
./target/debug/mem search "your query"
```

## Project structure

```
src/
  main.rs      CLI entry point — add new subcommands here
  types.rs     Shared types — keep this small and stable
  db.rs        All database logic — queries, migrations, FTS5
  auto.rs      Auto-capture logic — hook stdin parsing, git diff
  mcp.rs       MCP server — add new tools here
  tui.rs       Interactive TUI — ratatui (v0.2 scope)
migrations/
  001_init.sql Schema — add new migrations as 002_, 003_, etc.
hooks/
  *.sh         Shell wrappers for Claude Code hook events
```

## Adding a new MCP tool

1. Define input struct with `#[derive(Deserialize, JsonSchema)]` in `mcp.rs`
2. Add `async fn your_tool(&self, params: Parameters<YourParams>)` inside the `#[tool_router]` impl
3. Annotate with `#[tool(description = "...")]`

The `#[tool_handler]` macro on `ServerHandler` picks it up automatically.

## Database changes

Add a new migration file: `migrations/002_your_change.sql`

Then load it in `db.rs` alongside the existing `include_str!`:

```rust
conn.execute_batch(include_str!("../migrations/002_your_change.sql"))?;
```

## Testing

```bash
cargo test
```

Integration tests simulate hook stdin:

```bash
echo '{"cwd":"/tmp/test","session_id":"test-123","stop_hook_active":false}' \
  | MEM_DB=/tmp/test-mem.db cargo run -- save --auto
```

## Pull requests

- Keep PRs focused — one feature or fix per PR
- Include a test for new behavior
- Run `cargo clippy` before submitting
- Update README if you add a CLI flag or MCP tool

## Reporting bugs

Open an issue with:
- OS and Rust version (`rustc --version`)
- Steps to reproduce
- Expected vs actual output
