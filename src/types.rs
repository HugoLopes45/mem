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
    /// Captured automatically by the Stop hook — not user-settable via MCP
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
            other => Err(anyhow::anyhow!(
                "unknown memory type: '{other}'. Valid values: manual, pattern, decision"
            )),
        }
    }
}

/// A `MemoryType` that only accepts user-settable values (not `auto`).
/// Used in MCP `SaveParams` to prevent agents from setting the auto-capture type.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum UserMemoryType {
    Manual,
    Pattern,
    Decision,
}

impl Default for UserMemoryType {
    fn default() -> Self { UserMemoryType::Manual }
}

impl From<UserMemoryType> for MemoryType {
    fn from(u: UserMemoryType) -> Self {
        match u {
            UserMemoryType::Manual => MemoryType::Manual,
            UserMemoryType::Pattern => MemoryType::Pattern,
            UserMemoryType::Decision => MemoryType::Decision,
        }
    }
}

#[allow(dead_code)]
pub struct Session {
    pub id: String,
    pub project: Option<String>,
    pub goal: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub turn_count: i64,
}

/// Output from `mem context --compact` — matches Claude Code PreCompact hook protocol.
// Not serialized beyond this specific use — do not add fields.
#[derive(Debug, Serialize)]
pub struct CompactContextOutput {
    #[serde(rename = "additionalContext")]
    pub additional_context: String,
}

/// Common fields from Claude Code hook stdin JSON.
// Uses Default so malformed stdin falls back gracefully rather than hard-failing.
// See auto.rs for why that tradeoff is intentional.
#[derive(Debug, Deserialize, Default)]
pub struct HookStdin {
    pub cwd: Option<String>,
    pub session_id: Option<String>,
    pub stop_hook_active: Option<bool>,
}

// Not serialized — formatted manually in cmd_stats. Adding Serialize here would
// risk accidentally exposing this internal type through a future API.
#[derive(Debug)]
pub struct DbStats {
    pub memory_count: u64,
    pub session_count: u64,
    pub project_count: u64,
    pub db_size_bytes: u64,
}
