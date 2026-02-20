mod auto;
mod db;
mod mcp;
mod tui;
mod types;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::Read;
use std::path::PathBuf;

use auto::{AutoCapture, format_context_markdown};
use db::Db;
use types::{CompactContextOutput, MemoryType};

fn default_db_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".mem")
        .join("mem.db")
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
        /// Search query (FTS5 syntax)
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

    /// Interactive search TUI
    Tui,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let db_path = cli.db;

    match cli.command {
        Commands::Mcp => {
            // MCP server is async — spin up tokio runtime
            tokio::runtime::Runtime::new()?
                .block_on(mcp::run_mcp_server(db_path))
        }

        Commands::Save { auto, project, title, content, memory_type } => {
            if auto {
                cmd_save_auto(db_path, project)
            } else {
                cmd_save_manual(db_path, title, content, memory_type, project)
            }
        }

        Commands::Context { project, limit, compact, out } => {
            cmd_context(db_path, project, limit, compact, out)
        }

        Commands::Search { query, project, limit } => {
            cmd_search(db_path, query, project, limit)
        }

        Commands::Stats => cmd_stats(db_path),

        Commands::Tui => tui::run_tui(db_path),
    }
}

// ── Command implementations ───────────────────────────────────────────────────

fn cmd_save_auto(db_path: PathBuf, project_override: Option<PathBuf>) -> Result<()> {
    let mut stdin_buf = String::new();
    std::io::stdin().read_to_string(&mut stdin_buf)?;

    let capture = AutoCapture::from_stdin(
        &stdin_buf,
        project_override.as_deref(),
    )?;

    let Some(capture) = capture else {
        // stop_hook_active=true — bail silently to avoid infinite loop
        return Ok(());
    };

    let db = Db::open(&db_path)?;
    let mem = capture.capture_and_save(&db)?;
    eprintln!("[mem] saved: {} ({})", mem.title, mem.id);
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
    let project_str = project.as_deref()
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
    // If no project given, try reading from stdin (hook payload)
    let project_str = match project {
        Some(p) => auto::git_repo_root(&p)
            .or_else(|| p.to_str().map(String::from)),
        None => {
            let mut buf = String::new();
            // Non-blocking stdin check — hooks pipe JSON to us
            if atty::is(atty::Stream::Stdin) {
                None
            } else {
                std::io::stdin().read_to_string(&mut buf).ok();
                let hook: types::HookStdin = serde_json::from_str(&buf).unwrap_or_default();
                hook.cwd.as_deref()
                    .and_then(|cwd| auto::git_repo_root(std::path::Path::new(cwd))
                        .or_else(|| Some(cwd.to_string())))
            }
        }
    };

    let db = Db::open(&db_path)?;
    let mems = db.recent_memories(project_str.as_deref(), limit)?;
    let markdown = format_context_markdown(&mems);

    if compact {
        // PreCompact hook format
        let output = CompactContextOutput { additional_context: markdown };
        println!("{}", serde_json::to_string(&output)?);
    } else if let Some(path) = out {
        std::fs::write(&path, &markdown)
            .with_context(|| format!("write context to {}", path.display()))?;
        eprintln!("[mem] context written to {}", path.display());
    } else {
        print!("{markdown}");
    }
    Ok(())
}

fn cmd_search(db_path: PathBuf, query: String, project: Option<String>, limit: usize) -> Result<()> {
    let db = Db::open(&db_path)?;
    let results = db.search_memories(&query, project.as_deref(), limit)?;

    if results.is_empty() {
        println!("No memories found for: {query}");
        return Ok(());
    }

    for m in &results {
        println!(
            "[{}] {} ({})\n  {}\n",
            m.memory_type,
            m.title,
            m.created_at.format("%Y-%m-%d"),
            m.content.lines().next().unwrap_or(""),
        );
    }
    Ok(())
}

fn cmd_stats(db_path: PathBuf) -> Result<()> {
    // If DB doesn't exist yet, show friendly message
    if !db_path.exists() {
        println!("No memory database yet. Run a session with Stop hook configured.");
        return Ok(());
    }
    let db = Db::open(&db_path)?;
    let s = db.stats()?;
    println!("Memories : {}", s.memory_count);
    println!("Sessions : {}", s.session_count);
    println!("Projects : {}", s.project_count);
    println!("DB size  : {} KB", s.db_size_bytes / 1024);
    println!("DB path  : {}", db_path.display());
    Ok(())
}
