use anyhow::Result;
use rmcp::{
    ErrorData as McpError,
    handler::server::tool::ToolRouter,
    model::*,
    tool, tool_handler, tool_router,
    ServerHandler,
    handler::server::wrapper::Parameters,
};
use rmcp::schemars::JsonSchema;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::db::Db;
use crate::types::MemoryType;
use crate::auto::format_context_markdown;

fn mcp_err(msg: impl std::fmt::Display) -> McpError {
    McpError::new(rmcp::model::ErrorCode::INTERNAL_ERROR, msg.to_string(), None)
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
    /// Memory type: manual, pattern, or decision
    #[serde(default = "default_type")]
    #[schemars(default = "default_type")]
    memory_type: String,
    /// Git repo root or project path (optional — auto-detected if omitted)
    project: Option<String>,
}

fn default_type() -> String { "manual".to_string() }

#[derive(Deserialize, JsonSchema)]
struct SearchParams {
    /// Full-text search query (FTS5 with porter stemming)
    query: String,
    /// Filter by project path (optional)
    project: Option<String>,
    /// Max results (default 10)
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize { 10 }

#[derive(Deserialize, JsonSchema)]
struct ContextParams {
    /// Project path to load context for
    project: String,
    /// Number of recent memories to include (default 5)
    #[serde(default = "default_context_limit")]
    limit: usize,
}

fn default_context_limit() -> usize { 5 }

#[derive(Deserialize, JsonSchema)]
struct GetParams {
    /// Memory ID (UUID)
    id: String,
}

#[derive(Deserialize, JsonSchema)]
struct SessionStartParams {
    /// Project path
    project: String,
    /// What the session intends to accomplish (optional)
    goal: Option<String>,
    /// Session ID (from $CLAUDE_SESSION_ID)
    session_id: Option<String>,
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

    /// Save a memory manually. Use for decisions, patterns, or important findings.
    #[tool(description = "Save a memory manually. Use for important decisions, patterns, or findings you want to preserve across sessions.")]
    async fn mem_save(&self, params: Parameters<SaveParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let memory_type: MemoryType = p.memory_type.parse().map_err(|e| mcp_err(e))?;
        let db = self.db.lock().map_err(|e| mcp_err(e))?;
        let mem = db.save_memory(
            &p.title, memory_type, &p.content,
            p.project.as_deref(), None, None,
        ).map_err(|e| mcp_err(e))?;
        ok_text(format!("Saved memory: {} (id: {})", mem.title, mem.id))
    }

    /// Full-text search memories using FTS5 with porter stemming.
    #[tool(description = "Search memories using full-text search. Supports FTS5 query syntax: phrase quotes, AND/OR/NOT, prefix*.")]
    async fn mem_search(&self, params: Parameters<SearchParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let db = self.db.lock().map_err(|e| mcp_err(e))?;
        let results = db.search_memories(&p.query, p.project.as_deref(), p.limit)
            .map_err(|e| mcp_err(e))?;

        if results.is_empty() {
            return ok_text("No memories found.");
        }

        let out = results.iter().map(|m| {
            format!("**{}** ({})\n{}\n---", m.title, m.created_at.format("%Y-%m-%d"), m.content)
        }).collect::<Vec<_>>().join("\n");
        ok_text(out)
    }

    /// Get recent memories for a project — for loading context at session start.
    #[tool(description = "Get recent memories for a project. Returns last N session summaries as context.")]
    async fn mem_context(&self, params: Parameters<ContextParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let db = self.db.lock().map_err(|e| mcp_err(e))?;
        let mems = db.recent_memories(Some(&p.project), p.limit)
            .map_err(|e| mcp_err(e))?;
        ok_text(format_context_markdown(&mems))
    }

    /// Get a single memory by ID.
    #[tool(description = "Get full details of a memory by ID.")]
    async fn mem_get(&self, params: Parameters<GetParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let db = self.db.lock().map_err(|e| mcp_err(e))?;
        match db.get_memory(&p.id).map_err(|e| mcp_err(e))? {
            Some(m) => ok_text(serde_json::to_string_pretty(&m).map_err(|e| mcp_err(e))?),
            None => ok_text(format!("No memory found with id: {}", p.id)),
        }
    }

    /// Database statistics — memory count, sessions, projects tracked.
    #[tool(description = "Show database statistics: total memories, sessions, projects, and DB size.")]
    async fn mem_stats(&self) -> Result<CallToolResult, McpError> {
        let db = self.db.lock().map_err(|e| mcp_err(e))?;
        let s = db.stats().map_err(|e| mcp_err(e))?;
        ok_text(format!(
            "Memories: {}\nSessions: {}\nProjects: {}\nDB size: {} KB",
            s.memory_count, s.session_count, s.project_count,
            s.db_size_bytes / 1024,
        ))
    }

    /// Register a session start — tracks project and goal.
    #[tool(description = "Register the start of a Claude Code session. Call at session start with project and optional goal.")]
    async fn mem_session_start(&self, params: Parameters<SessionStartParams>) -> Result<CallToolResult, McpError> {
        let p = params.0;
        let session_id = p.session_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let db = self.db.lock().map_err(|e| mcp_err(e))?;
        db.start_session(&session_id, Some(&p.project), p.goal.as_deref())
            .map_err(|e| mcp_err(e))?;
        ok_text(format!("Session started: {session_id}"))
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
