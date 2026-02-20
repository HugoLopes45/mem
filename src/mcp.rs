use anyhow::Result;
use rmcp::schemars::JsonSchema;
use rmcp::{
    handler::server::tool::ToolRouter, handler::server::wrapper::Parameters, model::*, tool,
    tool_handler, tool_router, ErrorData as McpError, ServerHandler,
};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::auto::format_context_markdown;
use crate::db::Db;
use crate::suggest::suggest_rules;
use crate::types::{MemoryType, UserMemoryType};

fn mcp_err(msg: impl std::fmt::Display) -> McpError {
    McpError::new(
        rmcp::model::ErrorCode::INTERNAL_ERROR,
        msg.to_string(),
        None,
    )
}

fn mcp_invalid_params(msg: impl std::fmt::Display) -> McpError {
    McpError::new(
        rmcp::model::ErrorCode::INVALID_PARAMS,
        msg.to_string(),
        None,
    )
}

fn ok_text(s: impl Into<String>) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![Content::text(s.into())]))
}

/// Acquire the DB mutex, failing fast with an MCP error if the mutex is poisoned.
///
/// Mutex poison means a thread panicked while holding the lock — the rusqlite
/// `Connection` inside may be in a partially-written state. Continuing with
/// `into_inner()` would risk returning corrupt query results, so we return an
/// error instead and let the caller surface it to the MCP client.
fn lock_db(db: &Mutex<Db>) -> Result<std::sync::MutexGuard<'_, Db>, McpError> {
    db.lock().map_err(|_| {
        mcp_err("db mutex poisoned — server is in an inconsistent state, restart required")
    })
}

#[derive(Clone)]
pub struct MemServer {
    db: Arc<Mutex<Db>>,
    tool_router: ToolRouter<Self>,
}

// ── Input schemas ─────────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
struct SaveParams {
    /// Short title for this memory
    title: String,
    /// Memory content — what happened, decisions made, patterns observed
    content: String,
    /// Memory type: manual (default), pattern, or decision
    #[serde(default)]
    memory_type: UserMemoryType,
    /// Git repo root or project path (optional — auto-detected if omitted)
    project: Option<String>,
}

impl rmcp::schemars::JsonSchema for UserMemoryType {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "UserMemoryType".into()
    }
    fn json_schema(gen: &mut rmcp::schemars::SchemaGenerator) -> rmcp::schemars::Schema {
        let _ = gen;
        rmcp::schemars::json_schema!({
            "type": "string",
            "enum": ["manual", "pattern", "decision"],
            "default": "manual"
        })
    }
}

#[derive(Deserialize, JsonSchema)]
struct SearchParams {
    /// Full-text search query (FTS5 with porter stemming)
    query: String,
    /// Filter by project path (optional). Also includes global memories.
    project: Option<String>,
    /// Max results — capped at 200 (default 10)
    #[serde(default = "default_limit")]
    limit: u32,
}

fn default_limit() -> u32 {
    10
}

#[derive(Deserialize, JsonSchema)]
struct ContextParams {
    /// Project path to load context for
    project: String,
    /// Number of recent memories to include (default 5, max 50)
    #[serde(default = "default_context_limit")]
    limit: u32,
}

fn default_context_limit() -> u32 {
    5
}

#[derive(Deserialize, JsonSchema)]
struct GetParams {
    /// Memory ID (UUID)
    id: String,
}

#[derive(Deserialize, JsonSchema)]
struct SessionStartParams {
    /// Project path
    project: String,
    /// What this session intends to accomplish (optional)
    goal: Option<String>,
    /// Session ID (use $CLAUDE_SESSION_ID from env)
    session_id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct PromoteDemoteParams {
    /// Memory ID (UUID)
    id: String,
}

#[derive(Deserialize, JsonSchema)]
struct SuggestRulesParams {
    /// Number of recent auto-captured memories to analyse (default 20)
    #[serde(default = "default_suggest_limit")]
    limit: u32,
}

fn default_suggest_limit() -> u32 {
    20
}

// ── Tool implementations ──────────────────────────────────────────────────────

#[tool_router]
impl MemServer {
    pub fn new(db: Db) -> Self {
        Self {
            db: Arc::new(Mutex::new(db)),
            tool_router: Self::tool_router(),
        }
    }

    /// Save a memory manually. Use for important decisions, patterns, or findings.
    #[tool(
        description = "Save a memory manually. Use for important decisions, patterns, or findings you want to preserve across sessions."
    )]
    async fn mem_save(&self, params: Parameters<SaveParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        if p.title.trim().is_empty() {
            return Err(mcp_invalid_params(
                "title is required and must not be blank",
            ));
        }
        if p.content.trim().is_empty() {
            return Err(mcp_invalid_params(
                "content is required and must not be blank",
            ));
        }
        let memory_type: MemoryType = p.memory_type.into();
        let db = self.db.clone();
        let (title, content, project) = (p.title, p.content, p.project);

        let mem = tokio::task::spawn_blocking(move || {
            let db = lock_db(&db)?;
            db.save_memory(
                &title,
                memory_type,
                &content,
                project.as_deref(),
                None,
                None,
            )
            .map_err(mcp_err)
        })
        .await
        .map_err(mcp_err)??;

        ok_text(format!(
            "Saved memory: {} (id: {}, scope: {})",
            mem.title, mem.id, mem.scope
        ))
    }

    /// Full-text search memories using FTS5 with porter stemming.
    #[tool(
        description = "Search memories using full-text search. Input is treated as a phrase search — no FTS5 syntax required. Results ordered by relevance. Includes global memories when project is specified."
    )]
    async fn mem_search(
        &self,
        params: Parameters<SearchParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = params.0;
        if p.query.trim().is_empty() {
            return Err(mcp_invalid_params("query must not be blank"));
        }
        let limit = (p.limit as usize).min(200);
        let db = self.db.clone();
        let (query, project) = (p.query, p.project);

        let results = tokio::task::spawn_blocking(move || {
            let db = lock_db(&db)?;
            db.search_memories(&query, project.as_deref(), limit)
                .map_err(mcp_err)
        })
        .await
        .map_err(mcp_err)??;

        if results.is_empty() {
            return ok_text("No memories found.");
        }

        let out = results
            .iter()
            .map(|m| {
                format!(
                    "**{}** ({}) [scope: {}]\n{}\n---",
                    m.title,
                    m.created_at.format("%Y-%m-%d"),
                    m.scope,
                    m.content
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        ok_text(out)
    }

    /// Get recent memories for a project — for loading context at session start.
    #[tool(
        description = "Get recent memories for a project. Returns last N session summaries as context. Includes global memories in addition to project-scoped ones. Use at session start to restore prior context."
    )]
    async fn mem_context(
        &self,
        params: Parameters<ContextParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let limit = (p.limit as usize).min(50);
        let db = self.db.clone();
        let project = p.project;

        let mems = tokio::task::spawn_blocking(move || {
            let db = lock_db(&db)?;
            db.recent_memories(Some(&project), limit).map_err(mcp_err)
        })
        .await
        .map_err(mcp_err)??;

        ok_text(format_context_markdown(&mems))
    }

    /// Get a single memory by ID.
    #[tool(description = "Get full details of a memory by its ID. Response includes scope field.")]
    async fn mem_get(&self, params: Parameters<GetParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let db = self.db.clone();
        let id = p.id;
        let id_display = id.clone();

        let mem = tokio::task::spawn_blocking(move || {
            let db = lock_db(&db)?;
            db.get_memory(&id).map_err(mcp_err)
        })
        .await
        .map_err(mcp_err)??;

        match mem {
            Some(m) => ok_text(serde_json::to_string_pretty(&m).map_err(mcp_err)?),
            None => ok_text(format!("No memory found with id: {id_display}")),
        }
    }

    /// Database statistics — memory count, sessions, projects, active/cold counts, DB size.
    #[tool(
        description = "Show database statistics: total memories, active vs cold counts, sessions, projects tracked, and DB size on disk."
    )]
    async fn mem_stats(&self) -> Result<CallToolResult, McpError> {
        let db = self.db.clone();

        let s = tokio::task::spawn_blocking(move || {
            let db = lock_db(&db)?;
            db.stats().map_err(mcp_err)
        })
        .await
        .map_err(mcp_err)??;

        ok_text(format!(
            "Memories: {} ({} active, {} cold)\nSessions: {}\nProjects: {}\nDB size: {} KB",
            s.memory_count,
            s.active_count,
            s.cold_count,
            s.session_count,
            s.project_count,
            s.db_size_bytes / 1024,
        ))
    }

    /// Register a session start — tracks project and optional goal.
    #[tool(
        description = "Register the start of a Claude Code session. Records project and goal for context. Use $CLAUDE_SESSION_ID for session_id."
    )]
    async fn mem_session_start(
        &self,
        params: Parameters<SessionStartParams>,
    ) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let session_id = p
            .session_id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let db = self.db.clone();
        let (project, goal, sid) = (p.project, p.goal, session_id.clone());

        tokio::task::spawn_blocking(move || {
            let db = lock_db(&db)?;
            db.start_session(&sid, Some(&project), goal.as_deref())
                .map_err(mcp_err)
        })
        .await
        .map_err(mcp_err)??;

        ok_text(format!("Session started: {session_id}"))
    }

    /// Promote a memory to global scope — makes it visible across all projects.
    #[tool(
        description = "Promote a memory to global scope. Global memories appear in search and context for all projects, not just the project they were captured in."
    )]
    async fn mem_promote(
        &self,
        params: Parameters<PromoteDemoteParams>,
    ) -> Result<CallToolResult, McpError> {
        let id = params.0.id;
        let db = self.db.clone();
        let id_display = id.clone();

        let changed = tokio::task::spawn_blocking(move || {
            let db = lock_db(&db)?;
            db.promote_memory(&id).map_err(mcp_err)
        })
        .await
        .map_err(mcp_err)??;

        if changed {
            ok_text(format!("Memory {id_display} promoted to global scope."))
        } else {
            ok_text(format!("No memory found with id: {id_display}"))
        }
    }

    /// Demote a memory back to project scope.
    #[tool(
        description = "Demote a memory from global back to project scope. It will no longer appear cross-project."
    )]
    async fn mem_demote(
        &self,
        params: Parameters<PromoteDemoteParams>,
    ) -> Result<CallToolResult, McpError> {
        let id = params.0.id;
        let db = self.db.clone();
        let id_display = id.clone();

        let changed = tokio::task::spawn_blocking(move || {
            let db = lock_db(&db)?;
            db.demote_memory(&id).map_err(mcp_err)
        })
        .await
        .map_err(mcp_err)??;

        if changed {
            ok_text(format!("Memory {id_display} demoted to project scope."))
        } else {
            ok_text(format!("No memory found with id: {id_display}"))
        }
    }

    /// Suggest CLAUDE.md rules from recurring patterns in auto-captured memories.
    #[tool(
        description = "Analyse recent auto-captured memories for recurring terms/phrases and suggest CLAUDE.md-ready rules. Uses pure frequency analysis — no LLM calls."
    )]
    async fn mem_suggest_rules(
        &self,
        params: Parameters<SuggestRulesParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = (params.0.limit as usize).min(500);
        let db = self.db.clone();

        let memories = tokio::task::spawn_blocking(move || {
            let db = lock_db(&db)?;
            db.recent_auto_memories(limit).map_err(mcp_err)
        })
        .await
        .map_err(mcp_err)??;

        if memories.is_empty() {
            return ok_text("No auto-captured memories found. Run some sessions with the Stop hook enabled first.");
        }

        ok_text(suggest_rules(&memories))
    }

    /// Session analytics: token usage, cache efficiency, top projects.
    #[tool(
        description = "Return session analytics as JSON: token counts, cache efficiency, avg turns, top projects by token usage."
    )]
    async fn mem_gain(&self) -> Result<CallToolResult, McpError> {
        let db = self.db.clone();

        let g = tokio::task::spawn_blocking(move || {
            let db = lock_db(&db)?;
            db.gain_stats().map_err(mcp_err)
        })
        .await
        .map_err(mcp_err)??;

        let cache_efficiency = g.cache_efficiency_pct();

        let top_projects: Vec<serde_json::Value> = g
            .top_projects
            .iter()
            .map(|r| {
                serde_json::json!({
                    "project": r.project,
                    "sessions": r.sessions,
                    "total_tokens": r.total_tokens,
                })
            })
            .collect();

        let out = serde_json::json!({
            "session_count": g.session_count,
            "total_secs": g.total_secs,
            "total_input_tokens": g.total_input,
            "total_output_tokens": g.total_output,
            "total_cache_read_tokens": g.total_cache_read,
            "total_cache_creation_tokens": g.total_cache_creation,
            "cache_efficiency_pct": (cache_efficiency * 10.0).round() / 10.0,
            "avg_turns_per_session": (g.avg_turns * 10.0).round() / 10.0,
            "avg_session_duration_secs": (g.avg_secs * 10.0).round() / 10.0,
            "top_projects": top_projects,
        });

        ok_text(serde_json::to_string_pretty(&out).map_err(mcp_err)?)
    }
}

#[tool_handler]
impl ServerHandler for MemServer {}

pub async fn run_mcp_server(db_path: PathBuf) -> Result<()> {
    use rmcp::ServiceExt;

    let db = Db::open(&db_path)?;
    let server = MemServer::new(db);
    let transport = rmcp::transport::io::stdio();
    server.serve(transport).await?.waiting().await?;
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_server() -> MemServer {
        let db = Db::open(std::path::Path::new(":memory:")).expect("in-memory DB");
        MemServer::new(db)
    }

    fn save_params(title: &str, content: &str) -> Parameters<SaveParams> {
        Parameters(SaveParams {
            title: title.to_string(),
            content: content.to_string(),
            memory_type: UserMemoryType::Manual,
            project: None,
        })
    }

    /// Extract the text string from the first content block of a `CallToolResult`.
    fn result_text(r: &CallToolResult) -> &str {
        r.content[0]
            .as_text()
            .expect("expected text content")
            .text
            .as_str()
    }

    // ── mem_save ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn mem_save_rejects_blank_title() {
        let s = test_server();
        let err = s
            .mem_save(Parameters(SaveParams {
                title: "  ".to_string(),
                content: "content".to_string(),
                memory_type: UserMemoryType::Manual,
                project: None,
            }))
            .await
            .unwrap_err();
        assert!(err.message.contains("title"));
    }

    #[tokio::test]
    async fn mem_save_rejects_blank_content() {
        let s = test_server();
        let err = s
            .mem_save(Parameters(SaveParams {
                title: "title".to_string(),
                content: "".to_string(),
                memory_type: UserMemoryType::Manual,
                project: None,
            }))
            .await
            .unwrap_err();
        assert!(err.message.contains("content"));
    }

    #[tokio::test]
    async fn mem_save_success_returns_id() {
        let s = test_server();
        let result = s
            .mem_save(save_params("My title", "Some content"))
            .await
            .unwrap();
        let text = result_text(&result);
        assert!(text.contains("My title"));
        assert!(text.contains("id:"));
    }

    // ── mem_search ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn mem_search_rejects_blank_query() {
        let s = test_server();
        let err = s
            .mem_search(Parameters(SearchParams {
                query: "  ".to_string(),
                project: None,
                limit: 10,
            }))
            .await
            .unwrap_err();
        assert!(err.message.contains("blank"));
    }

    #[tokio::test]
    async fn mem_search_limit_capped_at_200() {
        let s = test_server();
        // Save one memory so search has something to match
        s.mem_save(save_params("cap test", "content for cap test"))
            .await
            .unwrap();
        // limit=9999 should be capped — just verify it doesn't error
        let result = s
            .mem_search(Parameters(SearchParams {
                query: "cap test".to_string(),
                project: None,
                limit: 9999,
            }))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn mem_search_returns_no_memories_message() {
        let s = test_server();
        let result = s
            .mem_search(Parameters(SearchParams {
                query: "xyzzy_not_found".to_string(),
                project: None,
                limit: 10,
            }))
            .await
            .unwrap();
        assert_eq!(result_text(&result), "No memories found.");
    }

    #[tokio::test]
    async fn mem_search_finds_saved_memory() {
        let s = test_server();
        s.mem_save(save_params("JWT auth", "Decided to use JWT tokens"))
            .await
            .unwrap();
        let result = s
            .mem_search(Parameters(SearchParams {
                query: "JWT".to_string(),
                project: None,
                limit: 10,
            }))
            .await
            .unwrap();
        assert!(result_text(&result).contains("JWT auth"));
    }

    // ── mem_context ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn mem_context_limit_capped_at_50() {
        let s = test_server();
        let result = s
            .mem_context(Parameters(ContextParams {
                project: "/test".to_string(),
                limit: 9999,
            }))
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn mem_context_returns_empty_when_no_memories() {
        let s = test_server();
        let result = s
            .mem_context(Parameters(ContextParams {
                project: "/nonexistent-project".to_string(),
                limit: 5,
            }))
            .await
            .unwrap();
        // format_context_markdown returns a non-empty header even with no memories
        assert!(!result_text(&result).is_empty());
    }

    // ── mem_get ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn mem_get_returns_not_found_for_unknown_id() {
        let s = test_server();
        let result = s
            .mem_get(Parameters(GetParams {
                id: "no-such-id".to_string(),
            }))
            .await
            .unwrap();
        assert!(result_text(&result).contains("No memory found"));
    }

    #[tokio::test]
    async fn mem_get_returns_memory_by_id() {
        let s = test_server();
        let saved = s
            .mem_save(save_params("Get test", "content"))
            .await
            .unwrap();
        // Extract id from "Saved memory: Get test (id: <uuid>, scope: ...)"
        let id = result_text(&saved)
            .split("id: ")
            .nth(1)
            .unwrap()
            .split(',')
            .next()
            .unwrap()
            .trim()
            .to_string();

        let result = s.mem_get(Parameters(GetParams { id })).await.unwrap();
        assert!(result_text(&result).contains("Get test"));
    }

    // ── mem_stats ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn mem_stats_returns_counts() {
        let s = test_server();
        s.mem_save(save_params("stats title", "stats content"))
            .await
            .unwrap();
        let result = s.mem_stats().await.unwrap();
        let text = result_text(&result);
        assert!(text.contains("Memories:"));
        assert!(text.contains("Sessions:"));
        assert!(text.contains("DB size:"));
    }

    // ── mem_session_start ────────────────────────────────────────────────────

    #[tokio::test]
    async fn mem_session_start_with_explicit_id() {
        let s = test_server();
        let result = s
            .mem_session_start(Parameters(SessionStartParams {
                project: "/my/proj".to_string(),
                goal: Some("test goal".to_string()),
                session_id: Some("explicit-session-id".to_string()),
            }))
            .await
            .unwrap();
        assert!(result_text(&result).contains("explicit-session-id"));
    }

    #[tokio::test]
    async fn mem_session_start_generates_id_when_omitted() {
        let s = test_server();
        let result = s
            .mem_session_start(Parameters(SessionStartParams {
                project: "/my/proj".to_string(),
                goal: None,
                session_id: None,
            }))
            .await
            .unwrap();
        let text = result_text(&result);
        // UUID should be 36 chars; confirm a session ID was generated
        assert!(text.contains("Session started:"));
        let sid = text.trim_start_matches("Session started: ").trim();
        assert_eq!(sid.len(), 36, "auto-generated session ID should be a UUID");
    }

    // ── mem_promote / mem_demote ──────────────────────────────────────────────

    #[tokio::test]
    async fn mem_promote_unknown_id_returns_not_found() {
        let s = test_server();
        let result = s
            .mem_promote(Parameters(PromoteDemoteParams {
                id: "unknown".to_string(),
            }))
            .await
            .unwrap();
        assert!(result_text(&result).contains("No memory found"));
    }

    #[tokio::test]
    async fn mem_promote_then_demote_roundtrip() {
        let s = test_server();
        let saved = s
            .mem_save(save_params("promote me", "content"))
            .await
            .unwrap();
        let id = result_text(&saved)
            .split("id: ")
            .nth(1)
            .unwrap()
            .split(',')
            .next()
            .unwrap()
            .trim()
            .to_string();

        let promote = s
            .mem_promote(Parameters(PromoteDemoteParams { id: id.clone() }))
            .await
            .unwrap();
        assert!(result_text(&promote).contains("promoted to global"));

        let demote = s
            .mem_demote(Parameters(PromoteDemoteParams { id }))
            .await
            .unwrap();
        assert!(result_text(&demote).contains("demoted to project"));
    }

    // ── mem_suggest_rules ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn mem_suggest_rules_no_memories_returns_guidance() {
        let s = test_server();
        let result = s
            .mem_suggest_rules(Parameters(SuggestRulesParams { limit: 20 }))
            .await
            .unwrap();
        assert!(result_text(&result).contains("No auto-captured memories"));
    }

    #[tokio::test]
    async fn mem_suggest_rules_limit_capped_at_500() {
        let s = test_server();
        // Should not error even with huge limit
        let result = s
            .mem_suggest_rules(Parameters(SuggestRulesParams { limit: 99999 }))
            .await;
        assert!(result.is_ok());
    }

    // ── mem_gain ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn mem_gain_returns_valid_json() {
        let s = test_server();
        let result = s.mem_gain().await.unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(result_text(&result)).expect("gain should be JSON");
        assert!(parsed.get("session_count").is_some());
        assert!(parsed.get("cache_efficiency_pct").is_some());
        assert!(parsed.get("top_projects").is_some());
    }
}
