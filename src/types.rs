use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Typed parse error for memory type/status/scope string conversion.
/// Provides descriptive messages without pulling in anyhow for this domain type.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("unknown memory type: '{0}'. Valid values: manual, pattern, decision")]
    Type(String),
    #[error("unknown memory status: '{0}'")]
    Status(String),
    #[error("unknown memory scope: '{0}'")]
    Scope(String),
}

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
    pub access_count: u32,
    pub last_accessed_at: Option<DateTime<Utc>>,
    pub status: MemoryStatus,
    pub scope: MemoryScope,
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
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, ParseError> {
        match s {
            "auto" => Ok(MemoryType::Auto),
            "manual" => Ok(MemoryType::Manual),
            "pattern" => Ok(MemoryType::Pattern),
            "decision" => Ok(MemoryType::Decision),
            other => Err(ParseError::Type(other.to_string())),
        }
    }
}

/// A `MemoryType` that only accepts user-settable values (not `auto`).
/// Used in MCP `SaveParams` to prevent agents from setting the auto-capture type.
#[derive(Default, Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum UserMemoryType {
    #[default]
    Manual,
    Pattern,
    Decision,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryStatus {
    Active,
    Cold,
}

impl std::fmt::Display for MemoryStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryStatus::Active => write!(f, "active"),
            MemoryStatus::Cold => write!(f, "cold"),
        }
    }
}

impl std::str::FromStr for MemoryStatus {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, ParseError> {
        match s {
            "active" => Ok(MemoryStatus::Active),
            "cold" => Ok(MemoryStatus::Cold),
            other => Err(ParseError::Status(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryScope {
    Project,
    Global,
}

impl std::fmt::Display for MemoryScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryScope::Project => write!(f, "project"),
            MemoryScope::Global => write!(f, "global"),
        }
    }
}

impl std::str::FromStr for MemoryScope {
    type Err = ParseError;
    fn from_str(s: &str) -> Result<Self, ParseError> {
        match s {
            "project" => Ok(MemoryScope::Project),
            "global" => Ok(MemoryScope::Global),
            other => Err(ParseError::Scope(other.to_string())),
        }
    }
}

/// Output from `mem context --compact` — matches Claude Code PreCompact hook protocol.
// Not serialized beyond this specific use — do not add fields.
#[derive(Debug, Serialize)]
pub struct CompactContextOutput {
    #[serde(rename = "additionalContext")]
    pub additional_context: String,
}

/// Output from `mem session-start` — matches Claude Code SessionStart hook protocol.
#[derive(Debug, Serialize)]
pub struct SessionStartOutput {
    #[serde(rename = "systemMessage")]
    pub system_message: String,
}

/// Common fields from Claude Code hook stdin JSON.
// Uses Default so malformed stdin falls back gracefully rather than hard-failing.
// See auto.rs for why that tradeoff is intentional.
#[derive(Debug, Deserialize, Default)]
pub struct HookStdin {
    pub cwd: Option<String>,
    pub session_id: Option<String>,
    pub stop_hook_active: Option<bool>,
    /// Path to the JSONL transcript file — provided by Claude Code Stop hook.
    pub transcript_path: Option<String>,
}

/// Analytics extracted from a session transcript JSONL file.
// i64 matches rusqlite's native INTEGER decoding; all values are non-negative by construction.
#[derive(Debug, Clone, Default)]
pub struct TranscriptAnalytics {
    pub turn_count: i64,
    pub duration_secs: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    /// Text content of the last assistant message — used as session summary.
    pub last_assistant_message: Option<String>,
}

/// Aggregate statistics across all sessions with analytics data.
#[derive(Debug, Default)]
pub struct GainStats {
    pub session_count: i64,
    pub total_secs: i64,
    pub total_input: i64,
    pub total_output: i64,
    pub total_cache_read: i64,
    pub total_cache_creation: i64,
    pub avg_turns: f64,
    pub avg_secs: f64,
    pub top_projects: Vec<ProjectGainRow>,
}

impl GainStats {
    /// Percentage of total input-side tokens served from cache (0.0–100.0).
    pub fn cache_efficiency_pct(&self) -> f64 {
        let denominator = self.total_cache_read + self.total_input;
        if denominator == 0 {
            return 0.0;
        }
        self.total_cache_read as f64 / denominator as f64 * 100.0
    }
}

/// One row from the top-projects-by-tokens query.
#[derive(Debug)]
pub struct ProjectGainRow {
    pub project: String,
    pub sessions: i64,
    /// Sum of input + output + cache_read tokens. Excludes cache_creation.
    pub total_tokens: i64,
}

// Not serialized — formatted manually in cmd_stats. Adding Serialize here would
// risk accidentally exposing this internal type through a future API.
#[derive(Debug)]
pub struct DbStats {
    pub memory_count: u64,
    pub session_count: u64,
    pub project_count: u64,
    pub db_size_bytes: u64,
    pub active_count: u64,
    pub cold_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedFile {
    pub id: String,
    pub source_path: String,
    pub project_path: Option<String>,
    pub project_name: String,
    pub title: String,
    pub content: String,
    pub indexed_at: DateTime<Utc>,
    /// Unix timestamp in seconds — matches SQLite INTEGER storage.
    pub file_mtime_secs: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UpsertOutcome {
    New,
    Updated,
    Unchanged,
}

#[derive(Debug, Default)]
pub struct IndexStats {
    pub new: usize,
    pub updated: usize,
    pub unchanged: usize,
    /// Skipped entries are counted but not added to `entries` (no log row for unreadable files).
    pub skipped: usize,
    pub entries: Vec<IndexEntry>,
}

impl IndexStats {
    /// Record a processed entry, keeping counters and entries in sync.
    /// Skipped entries increment the counter only — they are not pushed to `entries`.
    pub fn record(&mut self, entry: IndexEntry) {
        match entry.status {
            IndexEntryStatus::New => self.new += 1,
            IndexEntryStatus::Updated => self.updated += 1,
            IndexEntryStatus::Unchanged => self.unchanged += 1,
            IndexEntryStatus::Skipped => {
                self.skipped += 1;
                return;
            }
        }
        self.entries.push(entry);
    }
}

#[derive(Debug)]
pub struct IndexEntry {
    pub project_name: String,
    pub line_count: usize,
    pub status: IndexEntryStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum IndexEntryStatus {
    New,
    Updated,
    Unchanged,
    Skipped,
}

impl From<UpsertOutcome> for IndexEntryStatus {
    fn from(o: UpsertOutcome) -> Self {
        match o {
            UpsertOutcome::New => Self::New,
            UpsertOutcome::Updated => Self::Updated,
            UpsertOutcome::Unchanged => Self::Unchanged,
        }
    }
}

#[derive(Debug)]
pub enum SearchResult {
    Memory(Memory),
    IndexedFile(IndexedFile),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_type_from_str_roundtrip() {
        assert_eq!("auto".parse::<MemoryType>().unwrap(), MemoryType::Auto);
        assert_eq!("manual".parse::<MemoryType>().unwrap(), MemoryType::Manual);
        assert_eq!(
            "pattern".parse::<MemoryType>().unwrap(),
            MemoryType::Pattern
        );
        assert_eq!(
            "decision".parse::<MemoryType>().unwrap(),
            MemoryType::Decision
        );
        assert!("Manual".parse::<MemoryType>().is_err()); // case-sensitive
        assert!("unknown".parse::<MemoryType>().is_err());
    }

    #[test]
    fn memory_type_display_roundtrip() {
        for (variant, expected) in [
            (MemoryType::Auto, "auto"),
            (MemoryType::Manual, "manual"),
            (MemoryType::Pattern, "pattern"),
            (MemoryType::Decision, "decision"),
        ] {
            assert_eq!(variant.to_string(), expected);
        }
    }

    #[test]
    fn memory_type_error_message_includes_valid_values() {
        let err = "bogus".parse::<MemoryType>().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("manual"),
            "error should list valid types: {msg}"
        );
        assert!(
            msg.contains("pattern"),
            "error should list valid types: {msg}"
        );
        assert!(
            msg.contains("decision"),
            "error should list valid types: {msg}"
        );
    }

    #[test]
    fn memory_type_error_message_includes_bad_value() {
        let err = "bogus_type".parse::<MemoryType>().unwrap_err();
        assert!(
            err.to_string().contains("bogus_type"),
            "error should echo the invalid value"
        );
    }

    #[test]
    fn user_memory_type_does_not_include_auto() {
        // UserMemoryType exists specifically to exclude MemoryType::Auto from MCP/CLI.
        // Verify that `auto` cannot deserialize into it.
        let result = serde_json::from_str::<UserMemoryType>("\"auto\"");
        assert!(
            result.is_err(),
            "UserMemoryType must not accept 'auto': {result:?}"
        );
    }

    #[test]
    fn user_memory_type_default_is_manual() {
        assert_eq!(UserMemoryType::default(), UserMemoryType::Manual);
    }

    #[test]
    fn memory_status_from_str_roundtrip() {
        assert_eq!(
            "active".parse::<MemoryStatus>().unwrap(),
            MemoryStatus::Active
        );
        assert_eq!("cold".parse::<MemoryStatus>().unwrap(), MemoryStatus::Cold);
        assert!("Active".parse::<MemoryStatus>().is_err()); // case-sensitive
        assert!("unknown".parse::<MemoryStatus>().is_err());
    }

    #[test]
    fn memory_scope_from_str_roundtrip() {
        assert_eq!(
            "project".parse::<MemoryScope>().unwrap(),
            MemoryScope::Project
        );
        assert!("Global".parse::<MemoryScope>().is_err()); // case-sensitive
        assert!("unknown".parse::<MemoryScope>().is_err());
    }
}
