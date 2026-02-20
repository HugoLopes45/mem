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

fn ok_text(s: impl Into<String>) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![Content::text(s.into())]))
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
        let memory_type: MemoryType = p.memory_type.into();
        let db = self.db.clone();
        let (title, content, project) = (p.title, p.content, p.project);

        let mem = tokio::task::spawn_blocking(move || {
            let db = db
                .lock()
                .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
            db.save_memory(
                &title,
                memory_type,
                &content,
                project.as_deref(),
                None,
                None,
            )
        })
        .await
        .map_err(mcp_err)?
        .map_err(mcp_err)?;

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
        let limit = (p.limit as usize).min(200);
        let db = self.db.clone();
        let (query, project) = (p.query, p.project);

        let results = tokio::task::spawn_blocking(move || {
            let db = db
                .lock()
                .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
            db.search_memories(&query, project.as_deref(), limit)
        })
        .await
        .map_err(mcp_err)?
        .map_err(mcp_err)?;

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
            let db = db
                .lock()
                .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
            db.recent_memories(Some(&project), limit)
        })
        .await
        .map_err(mcp_err)?
        .map_err(mcp_err)?;

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
            let db = db
                .lock()
                .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
            db.get_memory(&id)
        })
        .await
        .map_err(mcp_err)?
        .map_err(mcp_err)?;

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
            let db = db
                .lock()
                .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
            db.stats()
        })
        .await
        .map_err(mcp_err)?
        .map_err(mcp_err)?;

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
            let db = db
                .lock()
                .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
            db.start_session(&sid, Some(&project), goal.as_deref())
        })
        .await
        .map_err(mcp_err)?
        .map_err(mcp_err)?;

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
            let db = db
                .lock()
                .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
            db.promote_memory(&id)
        })
        .await
        .map_err(mcp_err)?
        .map_err(mcp_err)?;

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
            let db = db
                .lock()
                .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
            db.demote_memory(&id)
        })
        .await
        .map_err(mcp_err)?
        .map_err(mcp_err)?;

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
        let limit = params.0.limit as usize;
        let db = self.db.clone();

        let memories = tokio::task::spawn_blocking(move || {
            let db = db
                .lock()
                .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
            db.recent_auto_memories(limit)
        })
        .await
        .map_err(mcp_err)?
        .map_err(mcp_err)?;

        if memories.is_empty() {
            return ok_text("No auto-captured memories found. Run some sessions with the Stop hook enabled first.");
        }

        ok_text(suggest_rules(&memories, limit))
    }

    /// Session analytics: token usage, cache efficiency, top projects.
    #[tool(
        description = "Return session analytics as JSON: token counts, cache efficiency, avg turns, top projects by token usage."
    )]
    async fn mem_gain(&self) -> Result<CallToolResult, McpError> {
        let db = self.db.clone();

        let g = tokio::task::spawn_blocking(move || {
            let db = db
                .lock()
                .map_err(|e| anyhow::anyhow!("db lock poisoned: {e}"))?;
            db.gain_stats()
        })
        .await
        .map_err(mcp_err)?
        .map_err(mcp_err)?;

        let cache_efficiency = if g.total_input + g.total_cache_read > 0 {
            g.total_cache_read as f64 / (g.total_cache_read + g.total_input) as f64 * 100.0
        } else {
            0.0
        };

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
