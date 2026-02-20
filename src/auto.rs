use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::db::Db;
use crate::types::{HookStdin, Memory, MemoryType};

pub struct AutoCapture {
    pub project: PathBuf,
    pub session_id: Option<String>,
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
        }))
    }

    /// Capture what changed this session and write to DB.
    pub fn capture_and_save(&self, db: &Db) -> Result<Memory> {
        let git_diff = self.git_diff_stat();
        let title = self.build_title(&git_diff);
        let content = self.build_content(&git_diff);

        let project_str = git_repo_root(&self.project)
            .unwrap_or_else(|| self.project.to_string_lossy().to_string());

        // Mark session as ended — non-fatal if it fails (session row may not exist)
        if let Some(ref sid) = self.session_id {
            if let Err(e) = db.end_session(sid) {
                eprintln!("[mem] warn: end_session failed for {sid}: {e}");
            }
        }

        db.save_memory(
            &title,
            MemoryType::Auto,
            &content,
            Some(&project_str),
            self.session_id.as_deref(),
            git_diff.as_deref(),
        )
    }

    fn git_diff_stat(&self) -> Option<String> {
        // `.ok()?` is intentional: no git repo is an expected, non-error condition
        let output = Command::new("git")
            .args([
                "-C",
                &self.project.to_string_lossy(),
                "diff",
                "--stat",
                "HEAD",
            ])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    fn build_title(&self, git_diff: &Option<String>) -> String {
        let project_name = self
            .project
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        if let Some(diff) = git_diff {
            // Extract the summary line: "3 files changed, 142 insertions(+)"
            if let Some(summary) = diff
                .lines()
                .rfind(|l| l.contains("file") && l.contains("changed"))
            {
                return format!("{project_name}: {}", summary.trim());
            }
        }
        format!("{project_name}: session ended (no git changes)")
    }

    fn build_content(&self, git_diff: &Option<String>) -> String {
        let mut parts = vec![format!(
            "Project: {}\nCaptured: {}",
            self.project.display(),
            chrono::Utc::now().format("%Y-%m-%d %H:%M UTC"),
        )];

        if let Some(diff) = git_diff {
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
