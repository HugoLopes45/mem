use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub session_id: Option<String>,
    pub project: Option<String>,
    pub title: String,
    #[serde(rename = "type")]
    pub memory_type: MemoryType,
    pub content: String,
    pub git_diff: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryType {
    Auto,
    Manual,
    Pattern,
    Decision,
}

impl std::fmt::Display for MemoryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryType::Auto => write!(f, "auto"),
            MemoryType::Manual => write!(f, "manual"),
            MemoryType::Pattern => write!(f, "pattern"),
            MemoryType::Decision => write!(f, "decision"),
        }
    }
}

impl std::str::FromStr for MemoryType {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "auto" => Ok(MemoryType::Auto),
            "manual" => Ok(MemoryType::Manual),
            "pattern" => Ok(MemoryType::Pattern),
            "decision" => Ok(MemoryType::Decision),
            other => Err(anyhow::anyhow!("unknown memory type: {other}")),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Session {
    pub id: String,
    pub project: Option<String>,
    pub goal: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub turn_count: i64,
}

/// Output from `mem context --compact` — Claude Code PreCompact hook format
#[derive(Debug, Serialize)]
pub struct CompactContextOutput {
    #[serde(rename = "additionalContext")]
    pub additional_context: String,
}

/// Hook stdin JSON — common fields across hook types
#[derive(Debug, Deserialize, Default)]
pub struct HookStdin {
    pub cwd: Option<String>,
    pub session_id: Option<String>,
    pub stop_hook_active: Option<bool>,
}

#[derive(Debug)]
pub struct DbStats {
    pub memory_count: i64,
    pub session_count: i64,
    pub project_count: i64,
    pub db_size_bytes: u64,
}
