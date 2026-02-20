use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{Connection, params};
use std::path::Path;
use uuid::Uuid;

use crate::types::{DbStats, Memory, MemoryType};

const MIGRATION: &str = include_str!("../migrations/001_init.sql");

pub struct Db {
    conn: Connection,
}

impl Db {
    // `&Path` is idiomatic — accepts both `&Path` and `&PathBuf` via deref
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create db dir {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("open db {}", path.display()))?;

        // WAL mode for concurrent readers + single writer
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL;",
        )?;

        // Run migrations (idempotent — all statements use IF NOT EXISTS)
        conn.execute_batch(MIGRATION)?;

        Ok(Self { conn })
    }

    // ── Memories ──────────────────────────────────────────────────────────────

    pub fn save_memory(
        &self,
        title: &str,
        memory_type: MemoryType,
        content: &str,
        project: Option<&str>,
        session_id: Option<&str>,
        git_diff: Option<&str>,
    ) -> Result<Memory> {
        anyhow::ensure!(!title.trim().is_empty(), "memory title must not be empty");
        anyhow::ensure!(!content.trim().is_empty(), "memory content must not be empty");

        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let created_at = now.to_rfc3339();
        let type_str = memory_type.to_string();

        self.conn.execute(
            "INSERT INTO memories (id, session_id, project, title, type, content, git_diff, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, session_id, project, title, type_str, content, git_diff, created_at],
        )?;

        Ok(Memory {
            id,
            session_id: session_id.map(String::from),
            project: project.map(String::from),
            title: title.to_string(),
            memory_type,
            content: content.to_string(),
            git_diff: git_diff.map(String::from),
            created_at: now,
        })
    }

    pub fn get_memory(&self, id: &str) -> Result<Option<Memory>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, project, title, type, content, git_diff, created_at
             FROM memories WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row_to_memory(row)?))
        } else {
            Ok(None)
        }
    }

    pub fn search_memories(
        &self,
        query: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Memory>> {
        // Wrap in double-quotes to treat the query as a phrase, disabling raw FTS5
        // operator injection (AND/OR/NEAR/column filters). Users can still search
        // effectively; explicit FTS5 syntax is available via the CLI's own escaping.
        let safe_query = format!("\"{}\"", query.replace('"', "\"\""));
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);

        if let Some(proj) = project {
            let mut stmt = self.conn.prepare(
                "SELECT m.id, m.session_id, m.project, m.title, m.type, m.content, m.git_diff, m.created_at
                 FROM memories m
                 JOIN memories_fts fts ON m.rowid = fts.rowid
                 WHERE memories_fts MATCH ?1 AND m.project = ?2
                 ORDER BY rank
                 LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![safe_query, proj, limit_i64], row_to_memory)?;
            rows.map(|r| r.map_err(Into::into)).collect()
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT m.id, m.session_id, m.project, m.title, m.type, m.content, m.git_diff, m.created_at
                 FROM memories m
                 JOIN memories_fts fts ON m.rowid = fts.rowid
                 WHERE memories_fts MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![safe_query, limit_i64], row_to_memory)?;
            rows.map(|r| r.map_err(Into::into)).collect()
        }
    }

    pub fn recent_memories(&self, project: Option<&str>, limit: usize) -> Result<Vec<Memory>> {
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);

        if let Some(proj) = project {
            let mut stmt = self.conn.prepare(
                "SELECT id, session_id, project, title, type, content, git_diff, created_at
                 FROM memories WHERE project = ?1
                 ORDER BY created_at DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![proj, limit_i64], row_to_memory)?;
            rows.map(|r| r.map_err(Into::into)).collect()
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT id, session_id, project, title, type, content, git_diff, created_at
                 FROM memories ORDER BY created_at DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit_i64], row_to_memory)?;
            rows.map(|r| r.map_err(Into::into)).collect()
        }
    }

    // ── Sessions ──────────────────────────────────────────────────────────────

    pub fn start_session(&self, id: &str, project: Option<&str>, goal: Option<&str>) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT OR IGNORE INTO sessions (id, project, goal, started_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, project, goal, now],
        )?;
        Ok(())
    }

    pub fn end_session(&self, id: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE sessions SET ended_at = ?1 WHERE id = ?2 AND ended_at IS NULL",
            params![now, id],
        )?;
        Ok(())
    }

    // ── Stats ─────────────────────────────────────────────────────────────────

    pub fn stats(&self) -> Result<DbStats> {
        let memory_count: u64 = self.conn.query_row(
            "SELECT COUNT(*) FROM memories", [], |r| r.get::<_, i64>(0),
        ).map(|n| n.max(0) as u64)?;

        let session_count: u64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sessions", [], |r| r.get::<_, i64>(0),
        ).map(|n| n.max(0) as u64)?;

        let project_count: u64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT project) FROM memories WHERE project IS NOT NULL",
            [], |r| r.get::<_, i64>(0),
        ).map(|n| n.max(0) as u64)?;

        // Propagate PRAGMA errors rather than silently reporting "0 KB"
        let db_size_bytes: u64 = self.conn.query_row(
            "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
            [], |r| r.get::<_, i64>(0),
        ).context("failed to read DB page size from PRAGMA")
            .map(|n| n.max(0) as u64)?;

        Ok(DbStats { memory_count, session_count, project_count, db_size_bytes })
    }
}

// ── Row helpers ───────────────────────────────────────────────────────────────

fn row_to_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<Memory> {
    let type_str: String = row.get(4)?;
    let created_at_str: String = row.get(7)?;

    let memory_type = type_str.parse::<MemoryType>()
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, e.into()))?;

    let created_at = created_at_str.parse()
        .map_err(|e: chrono::ParseError| rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e)))?;

    Ok(Memory {
        id: row.get(0)?,
        session_id: row.get(1)?,
        project: row.get(2)?,
        title: row.get(3)?,
        memory_type,
        content: row.get(5)?,
        git_diff: row.get(6)?,
        created_at,
    })
}
