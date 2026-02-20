mod auto;
mod db;
mod mcp;
mod suggest;
mod tui;
mod types;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::{IsTerminal, Read};
use std::path::PathBuf;

use auto::{format_context_markdown, AutoCapture};
use db::Db;
use types::{CompactContextOutput, MemoryType};

fn default_db_path() -> PathBuf {
    match dirs::home_dir() {
        Some(home) => home.join(".mem").join("mem.db"),
        None => {
            eprintln!("[mem] warn: $HOME not set — using /tmp/.mem/mem.db (data will not persist across reboots)");
            PathBuf::from("/tmp").join(".mem").join("mem.db")
        }
    }
}

#[derive(Parser)]
#[command(name = "mem", about = "Persistent memory for Claude Code sessions")]
struct Cli {
    /// Path to the SQLite database
    #[arg(long, env = "MEM_DB", default_value_os_t = default_db_path())]
    db: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run as MCP server (stdio transport)
    Mcp,

    /// Auto-capture session memory from hook stdin (Stop hook)
    Save {
        /// Read hook stdin and auto-capture (Stop hook mode)
        #[arg(long)]
        auto: bool,

        /// Project path override
        #[arg(long)]
        project: Option<PathBuf>,

        /// Save manually with title + content
        #[arg(long)]
        title: Option<String>,

        /// Memory content (for manual save)
        #[arg(long)]
        content: Option<String>,

        /// Memory type: manual | pattern | decision
        #[arg(long, default_value = "manual")]
        memory_type: String,
    },

    /// Output recent memories for context injection
    Context {
        /// Project path
        #[arg(long)]
        project: Option<PathBuf>,

        /// Number of recent memories to include
        #[arg(long, default_value_t = 3)]
        limit: usize,

        /// Output as PreCompact additionalContext JSON
        #[arg(long)]
        compact: bool,

        /// Write output to file instead of stdout
        #[arg(long)]
        out: Option<PathBuf>,
    },

    /// Full-text search memories
    Search {
        /// Search query
        query: String,

        /// Filter by project
        #[arg(long)]
        project: Option<String>,

        /// Max results
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },

    /// Show database statistics
    Stats,

    /// Mark low-retention memories as cold (Ebbinghaus decay)
    Decay {
        /// Retention score threshold — memories below this are marked cold
        #[arg(long, default_value_t = 0.1)]
        threshold: f64,

        /// Show what would be marked without making changes
        #[arg(long)]
        dry_run: bool,
    },

    /// Promote a memory to global scope (visible across all projects)
    Promote {
        /// Memory ID to promote
        id: String,
    },

    /// Demote a memory back to project scope
    Demote {
        /// Memory ID to demote
        id: String,
    },

    /// Suggest CLAUDE.md rules from recurring patterns in auto-captured memories
    SuggestRules {
        /// Number of recent auto-captured memories to analyse
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },

    /// Interactive search TUI (not yet implemented)
    Tui,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let db_path = cli.db;

    match cli.command {
        Commands::Mcp => tokio::runtime::Runtime::new()?.block_on(mcp::run_mcp_server(db_path)),
        Commands::Save {
            auto,
            project,
            title,
            content,
            memory_type,
        } => {
            if auto {
                cmd_save_auto(db_path, project)
            } else {
                cmd_save_manual(db_path, title, content, memory_type, project)
            }
        }
        Commands::Context {
            project,
            limit,
            compact,
            out,
        } => cmd_context(db_path, project, limit, compact, out),
        Commands::Search {
            query,
            project,
            limit,
        } => cmd_search(db_path, query, project, limit),
        Commands::Stats => cmd_stats(db_path),
        Commands::Decay { threshold, dry_run } => cmd_decay(db_path, threshold, dry_run),
        Commands::Promote { id } => cmd_promote(db_path, id),
        Commands::Demote { id } => cmd_demote(db_path, id),
        Commands::SuggestRules { limit } => cmd_suggest_rules(db_path, limit),
        Commands::Tui => {
            println!("TUI not yet implemented. Use `mem search <query>` for now.");
            Ok(())
        }
    }
}

// ── Command implementations ───────────────────────────────────────────────────

fn cmd_save_auto(db_path: PathBuf, project_override: Option<PathBuf>) -> Result<()> {
    let mut stdin_buf = String::new();
    std::io::stdin().read_to_string(&mut stdin_buf)?;

    let Some(capture) = AutoCapture::from_stdin(&stdin_buf, project_override.as_deref())? else {
        // stop_hook_active=true — bail to avoid infinite loop
        return Ok(());
    };

    let db = Db::open(&db_path)?;
    let _mem = capture.capture_and_save(&db)?;
    Ok(())
}

fn cmd_save_manual(
    db_path: PathBuf,
    title: Option<String>,
    content: Option<String>,
    memory_type: String,
    project: Option<PathBuf>,
) -> Result<()> {
    let title = title.context("--title required for manual save")?;
    let content = content.context("--content required for manual save")?;
    let mt: MemoryType = memory_type.parse()?;
    let project_str = project
        .as_deref()
        .and_then(|p| auto::git_repo_root(p).or_else(|| p.to_str().map(String::from)));

    let db = Db::open(&db_path)?;
    let mem = db.save_memory(&title, mt, &content, project_str.as_deref(), None, None)?;
    println!("Saved: {} (id: {})", mem.title, mem.id);
    Ok(())
}

fn cmd_context(
    db_path: PathBuf,
    project: Option<PathBuf>,
    limit: usize,
    compact: bool,
    out: Option<PathBuf>,
) -> Result<()> {
    let project_str = match project {
        Some(p) => auto::git_repo_root(&p).or_else(|| p.to_str().map(String::from)),
        None => {
            // In hook context, cwd is provided via stdin JSON.
            // IsTerminal check prevents blocking on stdin in interactive use.
            if std::io::stdin().is_terminal() {
                None
            } else {
                let mut buf = String::new();
                std::io::stdin()
                    .read_to_string(&mut buf)
                    .context("reading hook stdin for cwd")?;
                match serde_json::from_str::<types::HookStdin>(&buf) {
                    Ok(hook) => hook.cwd.as_deref().and_then(|cwd| {
                        auto::git_repo_root(std::path::Path::new(cwd))
                            .or_else(|| Some(cwd.to_string()))
                    }),
                    Err(e) => {
                        eprintln!("[mem] warn: failed to parse hook stdin JSON in context: {e}");
                        None
                    }
                }
            }
        }
    };

    let db = Db::open(&db_path)?;
    let mems = db.recent_memories(project_str.as_deref(), limit)?;
    let markdown = format_context_markdown(&mems);

    if compact {
        let output = CompactContextOutput {
            additional_context: markdown,
        };
        println!("{}", serde_json::to_string(&output)?);
    } else if let Some(path) = out {
        std::fs::write(&path, &markdown)
            .with_context(|| format!("write context to {}", path.display()))?;
    } else {
        print!("{markdown}");
    }
    Ok(())
}

fn cmd_search(
    db_path: PathBuf,
    query: String,
    project: Option<String>,
    limit: usize,
) -> Result<()> {
    let db = Db::open(&db_path)?;
    let results = db.search_memories(&query, project.as_deref(), limit)?;

    if results.is_empty() {
        println!("No memories found for: {query}");
        return Ok(());
    }

    for m in &results {
        println!(
            "[{}] {} ({}) [{}]\n  {}\n",
            m.memory_type,
            m.title,
            m.created_at.format("%Y-%m-%d"),
            m.scope,
            m.content.lines().next().unwrap_or(""),
        );
    }
    Ok(())
}

fn cmd_stats(db_path: PathBuf) -> Result<()> {
    if !db_path.exists() {
        println!("No memory database yet. Run a session with the Stop hook configured.");
        return Ok(());
    }
    let db = Db::open(&db_path)?;
    let s = db.stats()?;
    println!(
        "Memories : {} ({} active, {} cold)",
        s.memory_count, s.active_count, s.cold_count
    );
    println!("Sessions : {}", s.session_count);
    println!("Projects : {}", s.project_count);
    println!("DB size  : {} KB", s.db_size_bytes / 1024);
    println!("DB path  : {}", db_path.display());
    Ok(())
}

fn cmd_decay(db_path: PathBuf, threshold: f64, dry_run: bool) -> Result<()> {
    let db = Db::open(&db_path)?;
    let count = db.run_decay(threshold, dry_run)?;

    if dry_run {
        println!(
            "{count} memories would be marked cold (threshold: {threshold:.2}) [dry-run — no changes made]"
        );
    } else {
        println!("{count} memories marked cold (threshold: {threshold:.2})");
    }
    Ok(())
}

fn cmd_promote(db_path: PathBuf, id: String) -> Result<()> {
    let db = Db::open(&db_path)?;
    if db.promote_memory(&id)? {
        println!("Memory {id} promoted to global scope.");
        Ok(())
    } else {
        anyhow::bail!("No memory found with id: {id}")
    }
}

fn cmd_demote(db_path: PathBuf, id: String) -> Result<()> {
    let db = Db::open(&db_path)?;
    if db.demote_memory(&id)? {
        println!("Memory {id} demoted to project scope.");
        Ok(())
    } else {
        anyhow::bail!("No memory found with id: {id}")
    }
}

fn cmd_suggest_rules(db_path: PathBuf, limit: usize) -> Result<()> {
    let db = Db::open(&db_path)?;
    let memories = db.recent_auto_memories(limit)?;

    if memories.is_empty() {
        println!("No auto-captured memories found. Run some sessions with the Stop hook first.");
        return Ok(());
    }

    let output = suggest::suggest_rules(&memories, limit);
    print!("{output}");
    Ok(())
}
