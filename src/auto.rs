use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::db::Db;
use crate::types::{
    HookStdin, IndexEntry, IndexEntryStatus, IndexStats, Memory, MemoryType, TranscriptAnalytics,
    UpsertOutcome,
};

pub struct AutoCapture {
    pub project: PathBuf,
    pub session_id: Option<String>,
    pub transcript_path: Option<String>,
}

/// Summary of git changes for a session.
struct GitChanges {
    /// The stat summary (file counts, insertions, deletions)
    diff_stat: Option<String>,
    /// Most recent commit message, if commits are ahead of origin
    latest_commit_msg: Option<String>,
}

impl AutoCapture {
    /// Parse hook stdin JSON and resolve project path.
    /// Returns `None` if `stop_hook_active=true` (prevents infinite loop when
    /// `mem auto` itself triggers the Stop hook).
    pub fn from_stdin(stdin_json: &str, override_project: Option<&Path>) -> Result<Option<Self>> {
        let hook: HookStdin = match serde_json::from_str(stdin_json) {
            Ok(h) => h,
            Err(e) => {
                // Log parse failure to stderr — Stop hooks don't block on stderr content,
                // so this is visible in `claude --debug` without affecting hook exit code.
                eprintln!("[mem] warn: failed to parse hook stdin JSON: {e}");
                HookStdin::default()
            }
        };

        // Guard: Stop hook fires again when `mem auto` itself runs as a subprocess.
        // Claude Code sets stop_hook_active=true in that inner invocation.
        if hook.stop_hook_active == Some(true) {
            return Ok(None);
        }

        let project = if let Some(p) = override_project {
            p.to_path_buf()
        } else if let Some(cwd) = hook.cwd {
            PathBuf::from(cwd)
        } else {
            eprintln!(
                "[mem] warn: no cwd in hook stdin, falling back to process working directory"
            );
            std::env::current_dir()?
        };

        Ok(Some(Self {
            project,
            session_id: hook.session_id,
            transcript_path: hook.transcript_path,
        }))
    }

    /// Capture what changed this session and write to DB.
    pub fn capture_and_save(&self, db: &Db) -> Result<Memory> {
        let changes = self.git_changes();
        let title = self.build_title(&changes);
        let diff_text = changes.diff_stat.as_deref().map(String::from);

        let project_str = git_repo_root(&self.project)
            .unwrap_or_else(|| self.project.to_string_lossy().to_string());

        // Parse transcript before building content so we can include the session summary.
        let mut session_summary: Option<String> = None;
        if let Some(ref sid) = self.session_id {
            if let Err(e) = db.end_session(sid) {
                eprintln!("[mem] warn: end_session failed for {sid}: {e}");
            }

            // Parse transcript analytics if available — non-fatal
            if let Some(ref tp) = self.transcript_path {
                if let Some(analytics) = parse_transcript(tp) {
                    session_summary = analytics.last_assistant_message.clone();
                    if let Err(e) = db.update_session_analytics(sid, &analytics) {
                        eprintln!("[mem] warn: update_session_analytics failed for {sid}: {e}");
                    }
                }
            }
        }

        let content = self.build_content(&diff_text, session_summary.as_deref());

        db.save_memory(
            &title,
            MemoryType::Auto,
            &content,
            Some(&project_str),
            self.session_id.as_deref(),
            diff_text.as_deref(),
        )
    }

    /// Gather committed work and diff stat since origin/HEAD.
    /// Falls back to `git diff --stat HEAD` when no remote is available.
    fn git_changes(&self) -> GitChanges {
        // Try: git log --oneline origin/HEAD..HEAD
        let log_result = Command::new("git")
            .arg("-C")
            .arg(&self.project)
            .args(["log", "--oneline", "origin/HEAD..HEAD"])
            .stdin(Stdio::null())
            .output();

        let log_out = match log_result {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("[mem] warn: git not found in PATH — skipping git change capture");
                return GitChanges {
                    diff_stat: None,
                    latest_commit_msg: None,
                };
            }
            Err(_) => None,
            Ok(out) => Some(out),
        };

        if let Some(out) = log_out {
            if out.status.success() {
                let log_text = String::from_utf8_lossy(&out.stdout).trim().to_string();

                // log has output → commits exist ahead of origin
                if !log_text.is_empty() {
                    // Extract most recent commit message (first line, strip hash prefix)
                    let latest_commit_msg = log_text.lines().next().map(|l| {
                        l.split_once(' ')
                            .map(|(_, msg)| msg)
                            .unwrap_or(l)
                            .to_string()
                    });

                    // Get diff stat vs origin.
                    // .ok() is safe here: git exists (the log command above succeeded).
                    let diff_stat = Command::new("git")
                        .arg("-C")
                        .arg(&self.project)
                        .args(["diff", "origin/HEAD", "HEAD", "--stat"])
                        .stdin(Stdio::null())
                        .output()
                        .ok()
                        .and_then(|o| {
                            if o.status.success() {
                                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                                if s.is_empty() {
                                    None
                                } else {
                                    Some(s)
                                }
                            } else {
                                None
                            }
                        });

                    return GitChanges {
                        diff_stat,
                        latest_commit_msg,
                    };
                }

                // No commits ahead of origin — fall through to working tree diff
            }
        }

        // Fallback: git diff --stat HEAD (working tree changes vs last commit, or offline).
        // .ok() is safe here: git exists (the log command above succeeded or exited non-zero,
        // neither of which is a NotFound error).
        let diff_stat = Command::new("git")
            .arg("-C")
            .arg(&self.project)
            .args(["diff", "--stat", "HEAD"])
            .stdin(Stdio::null())
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if s.is_empty() {
                        None
                    } else {
                        Some(s)
                    }
                } else {
                    None
                }
            });

        GitChanges {
            diff_stat,
            latest_commit_msg: None,
        }
    }

    fn build_title(&self, changes: &GitChanges) -> String {
        let project_name = self
            .project
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // Prefer the most recent commit message as the title
        if let Some(ref msg) = changes.latest_commit_msg {
            return format!("{project_name}: {msg}");
        }

        // Fall back to diff stat summary line
        if let Some(ref diff) = changes.diff_stat {
            if let Some(summary) = diff
                .lines()
                .rfind(|l| l.contains("file") && l.contains("changed"))
            {
                return format!("{project_name}: {}", summary.trim());
            }
        }

        format!("{project_name}: session ended (no git changes)")
    }

    fn build_content(&self, diff_stat: &Option<String>, session_summary: Option<&str>) -> String {
        let mut parts = vec![format!(
            "Project: {}\nCaptured: {}",
            self.project.display(),
            chrono::Utc::now().format("%Y-%m-%d %H:%M UTC"),
        )];

        if let Some(summary) = session_summary {
            parts.push(format!("## Session Summary\n{summary}"));
        }

        if let Some(diff) = diff_stat {
            parts.push(format!("## Git Changes\n```\n{diff}\n```"));
        } else {
            parts.push("## Git Changes\nNo changes detected (or not a git repo)".to_string());
        }

        parts.join("\n\n")
    }
}

/// Resolve the git repo root — for stable project identity across subdirectories.
pub fn git_repo_root(path: &Path) -> Option<String> {
    // `.ok()?` is intentional: not a git repo is an expected, non-error condition.
    // Use .arg(path) (OsStr) to avoid lossy UTF-8 conversion on non-UTF-8 paths.
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .stdin(Stdio::null())
        .output()
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Validate that a transcript path is safe to read: must be absolute and must not
/// traverse via `..` components. This prevents hook-injected paths from escaping
/// the user's filesystem context via path traversal.
fn is_safe_transcript_path(path: &str) -> bool {
    let p = std::path::Path::new(path);
    p.is_absolute() && p.components().all(|c| c != std::path::Component::ParentDir)
}

/// Parse a JSONL transcript file and extract session analytics.
///
/// Returns `None` if the file is unreadable, fails path validation, or contains
/// no assistant entries. Skips lines that fail JSON parsing without aborting.
pub fn parse_transcript(path: &str) -> Option<TranscriptAnalytics> {
    if !is_safe_transcript_path(path) {
        eprintln!("[mem] warn: transcript_path rejected (unsafe path): {path}");
        return None;
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| eprintln!("[mem] warn: could not read transcript {path}: {e}"))
        .ok()?;

    let mut turn_count: i64 = 0;
    let mut input_tokens: i64 = 0;
    let mut output_tokens: i64 = 0;
    let mut cache_read_tokens: i64 = 0;
    let mut cache_creation_tokens: i64 = 0;
    let mut first_ts: Option<i64> = None;
    let mut last_ts: Option<i64> = None;
    let mut last_assistant_message: Option<String> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let val: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if let Some(ts_str) = val.get("timestamp").and_then(|v| v.as_str()) {
            if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(ts_str) {
                let secs = ts.timestamp();
                first_ts = Some(first_ts.map_or(secs, |f: i64| f.min(secs)));
                last_ts = Some(last_ts.map_or(secs, |l: i64| l.max(secs)));
            }
        }

        if val.get("type").and_then(|v| v.as_str()) == Some("assistant") {
            turn_count += 1;

            if let Some(usage) = val.get("message").and_then(|m| m.get("usage")) {
                let get_i64 = |key: &str| usage.get(key).and_then(|v| v.as_i64()).unwrap_or(0);
                input_tokens += get_i64("input_tokens");
                output_tokens += get_i64("output_tokens");
                cache_read_tokens += get_i64("cache_read_input_tokens");
                cache_creation_tokens += get_i64("cache_creation_input_tokens");
            }

            // Extract text content from the last assistant message.
            // content is an array of blocks; concatenate all text-type blocks.
            let text = val
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n\n")
                })
                .filter(|s| !s.is_empty());

            // Overwrite on every assistant turn so we end up with the last one.
            last_assistant_message = text;
        }
    }

    if turn_count == 0 {
        return None;
    }

    let duration_secs = match (first_ts, last_ts) {
        (Some(first), Some(last)) => (last - first).max(0),
        _ => 0,
    };

    Some(TranscriptAnalytics {
        turn_count,
        duration_secs,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_creation_tokens,
        last_assistant_message,
    })
}

/// Format recent memories as markdown for context injection.
pub fn format_context_markdown(memories: &[Memory]) -> String {
    if memories.is_empty() {
        return String::from("No recent memories for this project.");
    }

    let mut out = String::from("# Recent Session Memory\n\n");
    for (i, m) in memories.iter().enumerate() {
        let ts = m.created_at.format("%Y-%m-%d %H:%M UTC");
        out.push_str(&format!(
            "## {} — {} ({})\n\n{}\n\n",
            i + 1,
            m.title,
            ts,
            m.content
        ));
    }
    out
}

/// Decode a Claude Code encoded project directory name to a real filesystem path.
///
/// Claude Code encodes project paths as `-Users-hugo-projects-myapp` (leading `-`,
/// hyphens replacing slashes). We try two strategies:
/// 1. Read `sessions-index.json` in the dir and extract `entries[0].projectPath`
/// 2. Fallback: strip the leading `-`, replace remaining `-` with `/`, and prepend `/`
pub fn decode_project_path(encoded_dir_name: &str, project_dir: &Path) -> Option<String> {
    // Strategy 1: sessions-index.json
    let index_path = project_dir.join("sessions-index.json");
    if let Ok(data) = std::fs::read_to_string(&index_path) {
        match serde_json::from_str::<serde_json::Value>(&data) {
            Ok(val) => {
                if let Some(path) = val
                    .get("entries")
                    .and_then(|e| e.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|entry| entry.get("projectPath"))
                    .and_then(|p| p.as_str())
                {
                    return Some(path.to_string());
                }
                // entries missing/empty — fall through to Strategy 2
            }
            Err(e) => {
                eprintln!(
                    "[mem] warn: malformed sessions-index.json at {}: {e}",
                    index_path.display()
                );
                // Fall through to Strategy 2
            }
        }
    }

    // Strategy 2: naive decode — encoded name starts with `-`, replace `-` with `/`
    if let Some(stripped) = encoded_dir_name.strip_prefix('-') {
        return Some(format!("/{}", stripped.replace('-', "/")));
    }

    None
}

/// Extract a title from MEMORY.md content.
/// Uses the first `# ` H1 header; falls back to the filename without extension.
pub fn extract_title(content: &str, filename: &str) -> String {
    for line in content.lines() {
        if let Some(title) = line.strip_prefix("# ") {
            let trimmed = title.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    // Fallback: filename without extension
    std::path::Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename)
        .to_string()
}

/// Read a file's modification time as Unix seconds.
/// Logs a warning and returns 0 if any step fails (metadata, mtime, or epoch conversion).
/// A return value of 0 is safe: upsert_indexed_file will always treat it as changed,
/// so the file gets re-indexed rather than silently skipped.
pub fn read_mtime_secs(path: &Path) -> i64 {
    match std::fs::metadata(path) {
        Ok(meta) => match meta.modified() {
            Ok(t) => t
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or_else(|e| {
                    eprintln!("[mem] warn: mtime before epoch for {}: {e}", path.display());
                    0
                }),
            Err(e) => {
                eprintln!("[mem] warn: cannot read mtime for {}: {e}", path.display());
                0
            }
        },
        Err(e) => {
            eprintln!("[mem] warn: cannot stat {}: {e}", path.display());
            0
        }
    }
}

/// Scan all MEMORY.md files under `~/.claude/projects/` and upsert them into the DB.
///
/// Each project's MEMORY.md is expected at `<project_dir>/memory/MEMORY.md`.
/// Uses `MEM_CLAUDE_DIR` env var as the projects root (for testing); falls back to
/// `$HOME/.claude/projects/`.
pub fn scan_and_index_memory_files(db: &Db, dry_run: bool) -> Result<IndexStats> {
    let claude_dir = if let Ok(dir) = std::env::var("MEM_CLAUDE_DIR") {
        PathBuf::from(dir)
    } else {
        dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("$HOME not set"))?
            .join(".claude")
            .join("projects")
    };

    scan_and_index_memory_files_in(db, &claude_dir, dry_run)
}

/// Inner implementation that accepts the projects root directly (testable without env vars).
pub fn scan_and_index_memory_files_in(
    db: &Db,
    claude_dir: &Path,
    dry_run: bool,
) -> Result<IndexStats> {
    let mut stats = IndexStats::default();

    let project_entries = match std::fs::read_dir(claude_dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!(
                "[mem] warn: cannot read claude projects dir {}: {e}",
                claude_dir.display()
            );
            return Ok(stats);
        }
    };

    for project_entry in project_entries {
        let project_entry = match project_entry {
            Ok(e) => e,
            Err(e) => {
                eprintln!(
                    "[mem] warn: error reading entry in {}: {e}",
                    claude_dir.display()
                );
                stats.record(IndexEntry {
                    project_name: String::new(),
                    line_count: 0,
                    status: IndexEntryStatus::Skipped,
                });
                continue;
            }
        };
        let project_dir = project_entry.path();
        if !project_dir.is_dir() {
            continue;
        }

        // Only index MEMORY.md files in the expected <project>/memory/ layout
        let memory_path = project_dir.join("memory").join("MEMORY.md");
        if !memory_path.exists() {
            continue;
        }

        let encoded_name = match project_dir.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => {
                eprintln!(
                    "[mem] warn: skipping non-UTF-8 project dir: {}",
                    project_dir.display()
                );
                stats.record(IndexEntry {
                    project_name: String::new(),
                    line_count: 0,
                    status: IndexEntryStatus::Skipped,
                });
                continue;
            }
        };

        let project_path = decode_project_path(&encoded_name, &project_dir);
        let project_name = project_path
            .as_deref()
            .and_then(|p| std::path::Path::new(p).file_name())
            .and_then(|n| n.to_str())
            .unwrap_or(&encoded_name)
            .to_string();

        let content = match std::fs::read_to_string(&memory_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[mem] warn: cannot read {}: {e}", memory_path.display());
                stats.record(IndexEntry {
                    project_name,
                    line_count: 0,
                    status: IndexEntryStatus::Skipped,
                });
                continue;
            }
        };

        let line_count = content.lines().count();
        let title = extract_title(&content, "MEMORY");
        let source_path = memory_path.to_string_lossy().to_string();

        let mtime = read_mtime_secs(&memory_path);

        // dry_run still writes to the DB so we can report accurate New/Updated/Unchanged status;
        // it differs from a live run only in that errors are swallowed rather than propagated.
        let status = if dry_run {
            match db.upsert_indexed_file(
                &source_path,
                project_path.as_deref(),
                &project_name,
                &title,
                &content,
                mtime,
            ) {
                Ok(outcome) => IndexEntryStatus::from(outcome),
                Err(_) => IndexEntryStatus::New, // conservative fallback
            }
        } else {
            let outcome: UpsertOutcome = db.upsert_indexed_file(
                &source_path,
                project_path.as_deref(),
                &project_name,
                &title,
                &content,
                mtime,
            )?;
            IndexEntryStatus::from(outcome)
        };

        stats.record(IndexEntry {
            project_name,
            line_count,
            status,
        });
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_transcript tests ─────────────────────────────────────────────────

    #[test]
    fn parse_transcript_extracts_turns_and_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.jsonl");
        let content = r#"{"type":"user","timestamp":"2026-02-20T09:00:00.000Z","message":{}}
{"type":"assistant","timestamp":"2026-02-20T09:01:00.000Z","message":{"usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":200,"cache_creation_input_tokens":10}}}
{"type":"progress","timestamp":"2026-02-20T09:01:30.000Z"}
{"type":"assistant","timestamp":"2026-02-20T09:02:00.000Z","message":{"usage":{"input_tokens":80,"output_tokens":40,"cache_read_input_tokens":150,"cache_creation_input_tokens":5}}}
"#;
        std::fs::write(&path, content).unwrap();

        let analytics = parse_transcript(path.to_str().unwrap()).expect("should parse");
        assert_eq!(analytics.turn_count, 2, "should count 2 assistant turns");
        assert_eq!(analytics.input_tokens, 180, "input tokens summed");
        assert_eq!(analytics.output_tokens, 90, "output tokens summed");
        assert_eq!(analytics.cache_read_tokens, 350, "cache read summed");
        assert_eq!(analytics.cache_creation_tokens, 15, "cache creation summed");
        // duration: 09:02:00 - 09:00:00 = 120 seconds
        assert_eq!(analytics.duration_secs, 120);
    }

    #[test]
    fn parse_transcript_returns_none_on_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent_transcript.jsonl");
        let result = parse_transcript(path.to_str().unwrap());
        assert!(result.is_none(), "missing file should return None");
    }

    // ── is_safe_transcript_path tests ─────────────────────────────────────────

    #[test]
    fn parse_transcript_rejects_relative_path() {
        let result = parse_transcript("relative/path/transcript.jsonl");
        assert!(result.is_none(), "relative path must be rejected");
    }

    #[test]
    fn parse_transcript_rejects_path_with_parent_traversal() {
        let result = parse_transcript("/tmp/../../etc/passwd");
        assert!(result.is_none(), "path with .. must be rejected");
    }

    #[test]
    fn parse_transcript_rejects_bare_parent_traversal() {
        let result = parse_transcript("../secrets.jsonl");
        assert!(result.is_none(), "relative path with .. must be rejected");
    }

    #[test]
    fn parse_transcript_accepts_normal_absolute_path_that_does_not_exist() {
        // Proves the safety guard does NOT over-block legitimate absolute paths.
        // Returns None from the missing-file branch, not the safety-rejection branch.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("safe_nonexistent.jsonl");
        let result = parse_transcript(path.to_str().unwrap());
        // None because the file doesn't exist, not because of safety rejection
        assert!(result.is_none());
    }

    #[test]
    fn parse_transcript_extracts_last_assistant_message() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.jsonl");
        // Two assistant turns; we want the LAST one's text content.
        let content = r#"{"type":"assistant","timestamp":"2026-02-20T09:01:00.000Z","message":{"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"content":[{"type":"text","text":"First turn response."}]}}
{"type":"assistant","timestamp":"2026-02-20T09:02:00.000Z","message":{"usage":{"input_tokens":20,"output_tokens":8,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"content":[{"type":"text","text":"Added JWT auth. Switched from cookies due to CORS. Expiry: 24h."}]}}
"#;
        std::fs::write(&path, content).unwrap();

        let analytics = parse_transcript(path.to_str().unwrap()).expect("should parse");
        let summary = analytics
            .last_assistant_message
            .expect("should have last assistant message");
        assert_eq!(
            summary,
            "Added JWT auth. Switched from cookies due to CORS. Expiry: 24h."
        );
    }

    #[test]
    fn parse_transcript_last_message_none_when_no_text_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.jsonl");
        // Assistant turn with tool_use only, no text block
        let content = r#"{"type":"assistant","timestamp":"2026-02-20T09:01:00.000Z","message":{"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{}}]}}
"#;
        std::fs::write(&path, content).unwrap();

        let analytics = parse_transcript(path.to_str().unwrap()).expect("should parse");
        assert!(
            analytics.last_assistant_message.is_none(),
            "no text block → None"
        );
    }

    #[test]
    fn parse_transcript_skips_non_assistant_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript.jsonl");
        let content = r#"{"type":"user","timestamp":"2026-02-20T09:00:00.000Z","message":{}}
{"type":"system","timestamp":"2026-02-20T09:00:01.000Z","message":{}}
{"type":"progress","timestamp":"2026-02-20T09:00:02.000Z"}
{"type":"assistant","timestamp":"2026-02-20T09:01:00.000Z","message":{"usage":{"input_tokens":42,"output_tokens":7,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}
"#;
        std::fs::write(&path, content).unwrap();

        let analytics = parse_transcript(path.to_str().unwrap()).expect("should parse");
        assert_eq!(analytics.turn_count, 1, "only assistant entries counted");
        assert_eq!(analytics.input_tokens, 42);
        assert_eq!(analytics.output_tokens, 7);
    }

    // ── build_title tests ──────────────────────────────────────────────────────

    #[test]
    fn build_title_uses_commit_msg_when_available() {
        let capture = AutoCapture {
            project: PathBuf::from("/tmp/myproject"),
            session_id: None,
            transcript_path: None,
        };
        let changes = GitChanges {
            diff_stat: Some("1 file changed".to_string()),
            latest_commit_msg: Some("add JWT authentication".to_string()),
        };
        let title = capture.build_title(&changes);
        assert_eq!(title, "myproject: add JWT authentication");
    }

    #[test]
    fn build_title_uses_diff_stat_when_no_commit_msg() {
        let capture = AutoCapture {
            project: PathBuf::from("/tmp/myproject"),
            session_id: None,
            transcript_path: None,
        };
        let changes = GitChanges {
            diff_stat: Some("3 files changed, 142 insertions(+), 10 deletions(-)".to_string()),
            latest_commit_msg: None,
        };
        let title = capture.build_title(&changes);
        assert_eq!(
            title,
            "myproject: 3 files changed, 142 insertions(+), 10 deletions(-)"
        );
    }

    #[test]
    fn build_title_falls_back_to_no_git_changes_when_empty() {
        let capture = AutoCapture {
            project: PathBuf::from("/tmp/myproject"),
            session_id: None,
            transcript_path: None,
        };
        let changes = GitChanges {
            diff_stat: None,
            latest_commit_msg: None,
        };
        let title = capture.build_title(&changes);
        assert_eq!(title, "myproject: session ended (no git changes)");
    }

    #[test]
    fn build_title_prefers_commit_msg_over_diff_stat() {
        let capture = AutoCapture {
            project: PathBuf::from("/tmp/myproject"),
            session_id: None,
            transcript_path: None,
        };
        let changes = GitChanges {
            diff_stat: Some("1 file changed".to_string()),
            latest_commit_msg: Some("refactor: split auth module".to_string()),
        };
        let title = capture.build_title(&changes);
        // commit message wins
        assert!(title.contains("refactor: split auth module"));
        assert!(!title.contains("1 file changed"));
    }

    #[test]
    fn build_title_uses_latest_commit_msg() {
        let capture = AutoCapture {
            project: PathBuf::from("/tmp/myproject"),
            session_id: None,
            transcript_path: None,
        };
        let changes = GitChanges {
            diff_stat: None,
            latest_commit_msg: Some("add JWT authentication middleware".to_string()),
        };
        let title = capture.build_title(&changes);
        assert_eq!(title, "myproject: add JWT authentication middleware");
    }

    #[test]
    fn git_changes_strips_hash_from_log_line() {
        // Simulate what git log --oneline produces: "abc1234 add JWT auth middleware"
        // The stripping logic: l.split_once(' ').map(|(_, msg)| msg).unwrap_or(l)
        let log_line = "abc1234 add JWT authentication middleware";
        let msg = log_line.split_once(' ').map(|(_, m)| m).unwrap_or(log_line);
        assert_eq!(msg, "add JWT authentication middleware");
        assert!(!msg.contains("abc1234"));
    }

    // ── decode_project_path tests ──────────────────────────────────────────────

    #[test]
    fn decode_uses_sessions_index() {
        let dir = tempfile::tempdir().unwrap();
        let index_content = r#"{"entries":[{"projectPath":"/Users/hugo/projects/myapp"}]}"#;
        std::fs::write(dir.path().join("sessions-index.json"), index_content).unwrap();
        let result = decode_project_path("-Users-hugo-projects-myapp", dir.path());
        assert_eq!(result.as_deref(), Some("/Users/hugo/projects/myapp"));
    }

    #[test]
    fn decode_fallback_naive() {
        let dir = tempfile::tempdir().unwrap();
        // No sessions-index.json — use naive decode
        let result = decode_project_path("-Users-hugo-projects-myapp", dir.path());
        assert_eq!(result.as_deref(), Some("/Users/hugo/projects/myapp"));
    }

    #[test]
    fn extract_title_h1() {
        let content = "# My Project Memory\n\nSome content here.";
        assert_eq!(extract_title(content, "MEMORY.md"), "My Project Memory");
    }

    #[test]
    fn extract_title_fallback_filename() {
        let content = "No header here, just text.";
        assert_eq!(extract_title(content, "MEMORY.md"), "MEMORY");
    }

    #[test]
    fn scan_skips_dirs_without_memory_md() {
        use crate::db::Db;
        let tmp = tempfile::tempdir().unwrap();
        // Create a project dir with NO memory/MEMORY.md
        std::fs::create_dir_all(tmp.path().join("some-project")).unwrap();

        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let stats = scan_and_index_memory_files_in(&db, tmp.path(), false).unwrap();

        assert_eq!(stats.new, 0);
        assert_eq!(stats.skipped, 0);
        assert!(stats.entries.is_empty());
    }

    #[test]
    fn scan_indexes_memory_md_files() {
        use crate::db::Db;
        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("-Users-hugo-projects-myapp").join("memory");
        std::fs::create_dir_all(&memory_dir).unwrap();
        std::fs::write(
            memory_dir.join("MEMORY.md"),
            "# MyApp Patterns\n\nBiome forbids non-null assertion.",
        )
        .unwrap();

        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let stats = scan_and_index_memory_files_in(&db, tmp.path(), false).unwrap();

        assert_eq!(stats.new, 1);
        assert_eq!(stats.skipped, 0);
        assert_eq!(stats.entries[0].status, IndexEntryStatus::New);

        // Verify searchable
        let results = db.search_indexed_files("biome", None, 10).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn scan_reports_unchanged_on_second_run() {
        use crate::db::Db;
        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("-proj").join("memory");
        std::fs::create_dir_all(&memory_dir).unwrap();
        let path = memory_dir.join("MEMORY.md");
        std::fs::write(&path, "# Proj\n\nsome content").unwrap();

        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        let first = scan_and_index_memory_files_in(&db, tmp.path(), false).unwrap();
        assert_eq!(first.new, 1, "first scan should index as New");

        let second = scan_and_index_memory_files_in(&db, tmp.path(), false).unwrap();
        assert_eq!(
            second.unchanged, 1,
            "second scan with same mtime should be Unchanged"
        );
        assert_eq!(second.new, 0);
        assert_eq!(second.updated, 0);
    }

    #[test]
    fn scan_reports_updated_when_mtime_changes() {
        use crate::db::Db;
        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("-proj").join("memory");
        std::fs::create_dir_all(&memory_dir).unwrap();
        let path = memory_dir.join("MEMORY.md");
        std::fs::write(&path, "# Proj\n\noriginal content").unwrap();

        let db = Db::open(std::path::Path::new(":memory:")).unwrap();
        scan_and_index_memory_files_in(&db, tmp.path(), false).unwrap();

        // Simulate mtime change by directly updating the DB record to an old mtime
        db.upsert_indexed_file(
            &path.to_string_lossy(),
            None,
            "proj",
            "Proj",
            "original content",
            0, // force mtime to 0 so next scan sees a change
        )
        .unwrap();

        let stats = scan_and_index_memory_files_in(&db, tmp.path(), false).unwrap();
        assert_eq!(
            stats.updated, 1,
            "file with changed mtime should be Updated"
        );
    }

    #[test]
    fn decode_project_path_empty_entries_falls_back_to_naive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sessions-index.json"), r#"{"entries":[]}"#).unwrap();
        // Empty entries array → fall through to Strategy 2
        let result = decode_project_path("-Users-hugo-projects-myapp", dir.path());
        assert_eq!(result.as_deref(), Some("/Users/hugo/projects/myapp"));
    }

    #[test]
    fn decode_project_path_malformed_json_falls_back_to_naive() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sessions-index.json"), b"not valid json").unwrap();
        // Malformed JSON → logs warning and falls through to Strategy 2
        let result = decode_project_path("-Users-hugo-projects-myapp", dir.path());
        assert_eq!(result.as_deref(), Some("/Users/hugo/projects/myapp"));
    }

    #[test]
    fn extract_title_h2_does_not_match() {
        // ## should NOT be treated as an H1 header
        let content = "## Section\n# Real Title\nMore content";
        assert_eq!(
            extract_title(content, "MEMORY.md"),
            "Real Title",
            "## should not match; # Real Title should"
        );
    }

    #[test]
    fn extract_title_no_space_after_hash_does_not_match() {
        // "#NoSpace" without a trailing space should fall through to filename
        let content = "#NoSpace\nSome content";
        assert_eq!(extract_title(content, "MEMORY.md"), "MEMORY");
    }
}
