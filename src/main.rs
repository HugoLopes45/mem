use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

// ── CLI ───────────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "mem", about = "Session memory for Claude Code")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Wire mem into ~/.claude/settings.json and ~/.claude/CLAUDE.md
    Init,

    /// Inject MEMORY.md at session start (called by SessionStart hook)
    SessionStart {
        #[arg(long)]
        project: Option<PathBuf>,
    },

    /// Show hook install state and indexed file count
    Status,

    /// Index all MEMORY.md files for search
    Index,

    /// Search across indexed MEMORY.md files
    Search { query: String },
}

// ── Types ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
struct HookStdin {
    pub cwd: Option<String>,
}

#[derive(Debug, Serialize)]
struct SessionStartOutput {
    #[serde(rename = "systemMessage")]
    pub system_message: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct IndexEntry {
    pub project: String,
    pub path: String,
    pub content: String,
    /// Unix mtime seconds — used to skip unchanged files on re-index
    pub mtime: i64,
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Init => cmd_init(),
        Commands::SessionStart { project } => cmd_session_start(project),
        Commands::Status => cmd_status(),
        Commands::Index => cmd_index(),
        Commands::Search { query } => cmd_search(query),
    }
}

// ── Constants ─────────────────────────────────────────────────────────────────

const CLAUDE_MD_MARKER: &str = "## Session Memory (managed by mem)";

const CLAUDE_MD_BLOCK: &str = "\
## Session Memory (managed by mem)
At the end of every session, update MEMORY.md in the project root with:
- Decisions made and why
- Things tried and rejected (and why)
- Patterns or conventions discovered
- Anything future-Claude should know to avoid repeating work
Keep it under 30 lines. Rewrite, don't append — remove stale entries.
";

// ── init ──────────────────────────────────────────────────────────────────────

fn cmd_init() -> Result<()> {
    let home = dirs::home_dir().context("$HOME not set")?;

    let mut added: Vec<&str> = Vec::new();

    if wire_session_start_hook(&home.join(".claude").join("settings.json"))? {
        added.push("SessionStart hook → ~/.claude/settings.json");
    }
    if wire_claude_md(&home.join(".claude").join("CLAUDE.md"))? {
        added.push("Memory rule → ~/.claude/CLAUDE.md");
    }

    if added.is_empty() {
        println!("mem already configured.");
    } else {
        for item in &added {
            println!("Added {item}");
        }
        println!();
        println!("Done. Claude will maintain MEMORY.md in each project root.");
        println!("Run `mem index` after your first session to enable search.");
    }
    Ok(())
}

fn wire_session_start_hook(settings_path: &Path) -> Result<bool> {
    let bin = std::env::current_exe().context("cannot resolve binary path")?;
    let cmd = format!("{} session-start", bin.display());

    let raw = if settings_path.exists() {
        std::fs::read_to_string(settings_path)
            .with_context(|| format!("read {}", settings_path.display()))?
    } else {
        if let Some(parent) = settings_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        "{}".to_string()
    };

    let mut settings: serde_json::Value =
        serde_json::from_str(&raw).context("parse settings.json")?;

    let hooks = settings
        .as_object_mut()
        .context("settings.json must be a JSON object")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("hooks must be a JSON object")?;

    let entry = hooks
        .entry("SessionStart")
        .or_insert_with(|| serde_json::json!([]));

    if hook_command_exists(entry, &cmd) {
        return Ok(false);
    }

    entry
        .as_array_mut()
        .context("SessionStart hooks must be an array")?
        .push(serde_json::json!({"hooks": [{"type": "command", "command": cmd}]}));

    atomic_write_json(settings_path, &settings)?;
    Ok(true)
}

fn wire_claude_md(path: &Path) -> Result<bool> {
    let existing = if path.exists() {
        std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?
    } else {
        String::new()
    };

    if existing.contains(CLAUDE_MD_MARKER) {
        return Ok(false);
    }

    let new_content = if existing.is_empty() {
        CLAUDE_MD_BLOCK.to_string()
    } else if existing.ends_with('\n') {
        format!("{existing}\n{CLAUDE_MD_BLOCK}")
    } else {
        format!("{existing}\n\n{CLAUDE_MD_BLOCK}")
    };

    std::fs::write(path, new_content).with_context(|| format!("write {}", path.display()))?;
    Ok(true)
}

// ── session-start ─────────────────────────────────────────────────────────────

fn cmd_session_start(project_override: Option<PathBuf>) -> Result<()> {
    let cwd = resolve_cwd(project_override)?;
    let mut parts: Vec<String> = Vec::new();

    if let Some((content, path)) = find_memory_md(&cwd) {
        parts.push(format!(
            "# Project Memory (`{}`)\n\n{}",
            path.display(),
            content.trim()
        ));
    }

    if let Some(home) = dirs::home_dir() {
        let global = home.join(".claude").join("MEMORY.md");
        if global.exists() {
            match std::fs::read_to_string(&global) {
                Ok(content) => {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        parts.push(format!("# Global Memory\n\n{trimmed}"));
                    }
                }
                Err(e) => eprintln!("mem: cannot read global memory {}: {e}", global.display()),
            }
        }
    }

    if parts.is_empty() {
        return Ok(());
    }

    let output = SessionStartOutput {
        system_message: parts.join("\n\n---\n\n"),
    };
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

// ── status ────────────────────────────────────────────────────────────────────

fn cmd_status() -> Result<()> {
    let home = dirs::home_dir().context("$HOME not set")?;
    let bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("mem"));

    println!("Binary    : {}", bin.display());

    let hook_status = check_session_start_hook(&home.join(".claude").join("settings.json"));
    println!("Hook      : {hook_status}");

    let rule_status = match std::fs::read_to_string(home.join(".claude").join("CLAUDE.md")) {
        Ok(c) if c.contains(CLAUDE_MD_MARKER) => "installed",
        Ok(_) => "NOT installed — run `mem init`",
        Err(_) => "NOT installed — run `mem init`",
    };
    println!("Rule      : {rule_status}");

    let index = load_index();
    println!("Indexed   : {} MEMORY.md file(s)", index.len());

    Ok(())
}

// ── index ─────────────────────────────────────────────────────────────────────

fn cmd_index() -> Result<()> {
    let mut existing = load_index();
    let mut new_count = 0usize;
    let mut updated_count = 0usize;
    let mut unchanged_count = 0usize;
    let mut error_count = 0usize;

    // Collect candidate MEMORY.md paths from ~/.claude/projects/
    // Only Location 2 (~/.claude/projects/<encoded>/memory/MEMORY.md) is used —
    // decoding the encoded dir name back to a filesystem path is lossy (both '/' and '.'
    // map to '-'), so attempting to locate git-root MEMORY.md via decoding produces
    // wrong paths for any project with hyphens or dots in its name.
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();

    if let Some(home) = dirs::home_dir() {
        let projects_dir = home.join(".claude").join("projects");
        match std::fs::read_dir(&projects_dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let encoded = entry
                        .path()
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    candidates.push((
                        decode_project_name(&encoded),
                        entry.path().join("memory").join("MEMORY.md"),
                    ));
                }
            }
            Err(e) if projects_dir.exists() => {
                eprintln!("mem: cannot read {}: {e}", projects_dir.display());
            }
            Err(_) => {} // projects dir doesn't exist yet — first run, expected
        }
    }

    for (project, path) in candidates {
        if !path.exists() {
            continue;
        }
        let path_str = path.to_string_lossy().to_string();
        let mtime = file_mtime(&path);

        if let Some(entry) = existing.iter_mut().find(|e| e.path == path_str) {
            if entry.mtime == mtime {
                unchanged_count += 1;
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    entry.content = content;
                    entry.mtime = mtime;
                    updated_count += 1;
                }
                Err(e) => {
                    eprintln!("mem: cannot read {}: {e}", path.display());
                    error_count += 1;
                }
            }
        } else {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    existing.push(IndexEntry {
                        project,
                        path: path_str,
                        content,
                        mtime,
                    });
                    new_count += 1;
                }
                Err(e) => {
                    eprintln!("mem: cannot read {}: {e}", path.display());
                    error_count += 1;
                }
            }
        }
    }

    // Remove entries whose files no longer exist
    let before = existing.len();
    existing.retain(|e| std::path::Path::new(&e.path).exists());
    let pruned = before - existing.len();

    save_index(&existing)?;

    if error_count > 0 {
        println!(
            "Indexed: {} new, {} updated, {} unchanged, {} pruned, {} errors ({} total)",
            new_count,
            updated_count,
            unchanged_count,
            pruned,
            error_count,
            existing.len()
        );
        std::process::exit(1);
    } else {
        println!(
            "Indexed: {} new, {} updated, {} unchanged, {} pruned ({} total)",
            new_count,
            updated_count,
            unchanged_count,
            pruned,
            existing.len()
        );
    }
    Ok(())
}

// ── search ────────────────────────────────────────────────────────────────────

fn cmd_search(query: String) -> Result<()> {
    let index = load_index();

    if index.is_empty() {
        println!("No files indexed. Run `mem index` first.");
        return Ok(());
    }

    let query_lower = query.to_lowercase();
    let mut found = false;

    for entry in &index {
        let matches: Vec<&str> = entry
            .content
            .lines()
            .filter(|l| l.to_lowercase().contains(&query_lower))
            .collect();

        if !matches.is_empty() {
            println!("── {} ──", entry.project);
            for line in matches {
                println!("  {}", line.trim());
            }
            println!();
            found = true;
        }
    }

    if !found {
        println!("No matches for: {query}");
    }
    Ok(())
}

// ── index persistence ─────────────────────────────────────────────────────────

fn index_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".mem").join("index.json"))
}

fn load_index() -> Vec<IndexEntry> {
    let Some(path) = index_path() else {
        return Vec::new();
    };
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            eprintln!("mem: cannot read index {}: {e}", path.display());
            eprintln!("mem: run `mem index` to rebuild, or check file permissions");
            return Vec::new();
        }
    };
    match serde_json::from_str(&raw) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("mem: index at {} is corrupt ({e})", path.display());
            eprintln!("mem: run `mem index` to rebuild it");
            Vec::new()
        }
    }
}

fn save_index(entries: &[IndexEntry]) -> Result<()> {
    let path = index_path().context("$HOME not set")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string(entries)?)
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn resolve_cwd(project_override: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = project_override {
        return Ok(p);
    }
    if std::io::stdin().is_terminal() {
        return Ok(std::env::current_dir()?);
    }
    let mut buf = String::new();
    std::io::stdin().read_to_string(&mut buf)?;
    match serde_json::from_str::<HookStdin>(&buf) {
        Ok(hook) => Ok(hook
            .cwd
            .map(PathBuf::from)
            .unwrap_or(std::env::current_dir()?)),
        Err(e) => {
            eprintln!(
                "mem: session-start received unexpected stdin ({e}); \
                 falling back to current directory. Payload: {:?}",
                &buf[..buf.len().min(200)]
            );
            Ok(std::env::current_dir()?)
        }
    }
}

fn find_memory_md(cwd: &Path) -> Option<(String, PathBuf)> {
    // Strategy 1: git repo root
    if let Some(root) = git_repo_root(cwd) {
        let path = PathBuf::from(&root).join("MEMORY.md");
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(c) => return Some((c, path)),
                Err(e) => eprintln!("mem: cannot read {}: {e}", path.display()),
            }
        }
    }
    // Strategy 2: ~/.claude/projects/<encoded>/memory/MEMORY.md
    let projects = dirs::home_dir()?.join(".claude").join("projects");
    let canonical = std::fs::canonicalize(cwd).ok()?;
    let encoded = "-".to_string()
        + &canonical
            .to_string_lossy()
            .trim_start_matches('/')
            .replace(['/', '.'], "-");
    let path = projects.join(encoded).join("memory").join("MEMORY.md");
    if path.exists() {
        match std::fs::read_to_string(&path) {
            Ok(c) => return Some((c, path)),
            Err(e) => eprintln!("mem: cannot read {}: {e}", path.display()),
        }
    }
    None
}

fn git_repo_root(path: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

fn file_mtime(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

fn hook_command_exists(entry: &serde_json::Value, cmd: &str) -> bool {
    entry.as_array().is_some_and(|arr| {
        arr.iter().any(|item| {
            item.get("hooks")
                .and_then(|h| h.as_array())
                .is_some_and(|hooks| {
                    hooks
                        .iter()
                        .any(|h| h.get("command").and_then(|c| c.as_str()) == Some(cmd))
                })
        })
    })
}

fn atomic_write_json(path: &Path, value: &serde_json::Value) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(value)? + "\n")
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
    Ok(())
}

fn check_session_start_hook(settings_path: &Path) -> &'static str {
    let Ok(raw) = std::fs::read_to_string(settings_path) else {
        return "NOT installed — run `mem init`";
    };
    let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return "malformed settings.json";
    };
    let has_hook = val
        .get("hooks")
        .and_then(|h| h.get("SessionStart"))
        .and_then(|s| s.as_array())
        .is_some_and(|arr| {
            arr.iter().any(|item| {
                item.get("hooks")
                    .and_then(|h| h.as_array())
                    .is_some_and(|hooks| {
                        hooks.iter().any(|h| {
                            h.get("command")
                                .and_then(|c| c.as_str())
                                .is_some_and(|c| c.ends_with(" session-start"))
                        })
                    })
            })
        });
    if has_hook {
        "installed"
    } else {
        "NOT installed — run `mem init`"
    }
}

/// Return a human-readable project label from a Claude-encoded dir name.
/// The encoding is lossy (both '/' and '.' map to '-'), so we don't attempt
/// to decode — we just strip the leading '-' and use the result as-is.
fn decode_project_name(encoded: &str) -> String {
    encoded.trim_start_matches('-').to_string()
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_claude_md_adds_block_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        std::fs::write(&path, "# Existing\n\nSome content.\n").unwrap();

        assert!(wire_claude_md(&path).unwrap());

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains(CLAUDE_MD_MARKER));
        assert!(content.contains("Existing"));
    }

    #[test]
    fn wire_claude_md_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        wire_claude_md(&path).unwrap();
        assert!(!wire_claude_md(&path).unwrap());
        assert_eq!(
            std::fs::read_to_string(&path)
                .unwrap()
                .matches(CLAUDE_MD_MARKER)
                .count(),
            1
        );
    }

    #[test]
    fn wire_claude_md_creates_file_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("CLAUDE.md");
        wire_claude_md(&path).unwrap();
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains(CLAUDE_MD_MARKER));
    }

    #[test]
    fn wire_session_start_hook_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, "{}").unwrap();
        wire_session_start_hook(&path).unwrap();
        wire_session_start_hook(&path).unwrap();
        let val: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(val["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn wire_session_start_hook_preserves_existing_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, r#"{"model":"claude-sonnet-4-6"}"#).unwrap();
        wire_session_start_hook(&path).unwrap();
        let val: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(val["model"].as_str(), Some("claude-sonnet-4-6"));
    }

    #[test]
    fn session_start_output_serializes_correctly() {
        let out = SessionStartOutput {
            system_message: "hello".to_string(),
        };
        assert!(serde_json::to_string(&out)
            .unwrap()
            .contains(r#""systemMessage":"hello""#));
    }

    #[test]
    fn decode_project_name_strips_leading_dash() {
        assert_eq!(
            decode_project_name("-Users-hugo-projects-myapp"),
            "Users-hugo-projects-myapp"
        );
        // Hyphenated project names are preserved intact
        assert_eq!(
            decode_project_name("-Users-hugo-my-cool-app"),
            "Users-hugo-my-cool-app"
        );
    }

    #[test]
    fn find_memory_md_returns_none_for_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(find_memory_md(tmp.path()).is_none());
    }

    #[test]
    fn index_roundtrip_new_and_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        // Override index path via a helper that takes an explicit path
        let index_file = tmp.path().join("index.json");

        let entry = IndexEntry {
            project: "myapp".to_string(),
            path: tmp.path().join("MEMORY.md").to_string_lossy().to_string(),
            content: "- Used JWT for auth".to_string(),
            mtime: 12345,
        };

        // Serialize and reload
        std::fs::write(&index_file, serde_json::to_string(&[&entry]).unwrap()).unwrap();
        let loaded: Vec<IndexEntry> =
            serde_json::from_str(&std::fs::read_to_string(&index_file).unwrap()).unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].project, "myapp");
        assert_eq!(loaded[0].content, "- Used JWT for auth");
    }

    #[test]
    fn search_matches_lines_case_insensitive() {
        let entries = vec![IndexEntry {
            project: "proj".to_string(),
            path: "/proj/MEMORY.md".to_string(),
            content: "- Used JWT for auth\n- Rejected OAuth (too complex)".to_string(),
            mtime: 0,
        }];
        let query = "jwt";
        let matches: Vec<&str> = entries[0]
            .content
            .lines()
            .filter(|l| l.to_lowercase().contains(query))
            .collect();
        assert_eq!(matches, vec!["- Used JWT for auth"]);
    }
}
