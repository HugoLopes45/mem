use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::db::Db;
use crate::types::{HookStdin, Memory, MemoryType, TranscriptAnalytics};

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
    /// `mem save --auto` itself triggers the Stop hook).
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

        // Guard: Stop hook fires again when `mem save --auto` itself runs as a subprocess.
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
        let content = self.build_content(&diff_text);

        let project_str = git_repo_root(&self.project)
            .unwrap_or_else(|| self.project.to_string_lossy().to_string());

        // Mark session as ended — non-fatal if it fails (session row may not exist)
        if let Some(ref sid) = self.session_id {
            if let Err(e) = db.end_session(sid) {
                eprintln!("[mem] warn: end_session failed for {sid}: {e}");
            }

            // Parse transcript analytics if available — non-fatal
            if let Some(ref tp) = self.transcript_path {
                if let Some(analytics) = parse_transcript(tp) {
                    if let Err(e) = db.update_session_analytics(sid, &analytics) {
                        eprintln!("[mem] warn: update_session_analytics failed for {sid}: {e}");
                    }
                }
            }
        }

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
        let dir = self.project.to_string_lossy();

        // Try: git log --oneline origin/HEAD..HEAD
        let log_result = Command::new("git")
            .args(["-C", &dir, "log", "--oneline", "origin/HEAD..HEAD"])
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
                        .args(["-C", &dir, "diff", "origin/HEAD", "HEAD", "--stat"])
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
            .args(["-C", &dir, "diff", "--stat", "HEAD"])
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

    fn build_content(&self, diff_stat: &Option<String>) -> String {
        let mut parts = vec![format!(
            "Project: {}\nCaptured: {}",
            self.project.display(),
            chrono::Utc::now().format("%Y-%m-%d %H:%M UTC"),
        )];

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
    // `.ok()?` is intentional: not a git repo is an expected, non-error condition
    let output = Command::new("git")
        .args([
            "-C",
            &path.to_string_lossy(),
            "rev-parse",
            "--show-toplevel",
        ])
        .output()
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Parse a JSONL transcript file and extract session analytics.
///
/// Returns `None` if the file is unreadable or contains no assistant entries.
/// Skips lines that fail JSON parsing without aborting.
pub fn parse_transcript(path: &str) -> Option<TranscriptAnalytics> {
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
        let result = parse_transcript("/tmp/nonexistent_mem_transcript_abc123.jsonl");
        assert!(result.is_none(), "missing file should return None");
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
}
