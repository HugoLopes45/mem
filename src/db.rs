use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection};
use std::path::Path;
use uuid::Uuid;

use crate::types::{
    DbStats, GainStats, IndexedFile, Memory, MemoryScope, MemoryStatus, MemoryType, ProjectGainRow,
    SearchResult, TranscriptAnalytics, UpsertOutcome,
};

const SCHEMA: &str = include_str!("../migrations/001_init.sql");

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
        let conn = Connection::open(path).with_context(|| format!("open db {}", path.display()))?;

        // WAL mode for concurrent readers + single writer.
        // busy_timeout=5000 prevents silent SQLITE_BUSY drops when a reader holds a WAL lock.
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA synchronous=NORMAL; PRAGMA busy_timeout=5000;",
        )?;

        // Apply schema on first open. All DDL uses IF NOT EXISTS — safe to re-run.
        // user_version gates the apply so the batch is skipped on every subsequent open.
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            conn.execute_batch(SCHEMA).context("schema init failed")?;
            conn.execute_batch("PRAGMA user_version = 1;")?;
        }

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
        anyhow::ensure!(
            !content.trim().is_empty(),
            "memory content must not be empty"
        );

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
            access_count: 0,
            last_accessed_at: None,
            status: MemoryStatus::Active,
            scope: MemoryScope::Project,
        })
    }

    pub fn get_memory(&self, id: &str) -> Result<Option<Memory>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, project, title, type, content, git_diff, created_at,
                    access_count, last_accessed_at, status, scope
             FROM memories WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            let mem = row_to_memory(row)?;
            // Best-effort access tracking — do not fail the get if UPDATE fails
            let now = Utc::now().to_rfc3339();
            if let Err(e) = self.conn.execute(
                "UPDATE memories SET access_count = access_count + 1, last_accessed_at = ?1
                 WHERE id = ?2",
                params![now, id],
            ) {
                eprintln!("[mem] warn: access tracking failed for {id}: {e}");
            }
            Ok(Some(mem))
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

        let mems = if let Some(proj) = project {
            // With --project: include project-scoped AND global memories
            let mut stmt = self.conn.prepare(
                "SELECT m.id, m.session_id, m.project, m.title, m.type, m.content, m.git_diff,
                        m.created_at, m.access_count, m.last_accessed_at, m.status, m.scope
                 FROM memories m
                 JOIN memories_fts fts ON m.rowid = fts.rowid
                 WHERE memories_fts MATCH ?1
                   AND (m.project = ?2 OR m.scope = 'global')
                   AND m.status = 'active'
                 ORDER BY rank
                 LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![safe_query, proj, limit_i64], row_to_memory)?;
            rows.map(|r| r.map_err(Into::into))
                .collect::<Result<Vec<_>>>()?
        } else {
            // Without --project: return all (project + global) active memories
            let mut stmt = self.conn.prepare(
                "SELECT m.id, m.session_id, m.project, m.title, m.type, m.content, m.git_diff,
                        m.created_at, m.access_count, m.last_accessed_at, m.status, m.scope
                 FROM memories m
                 JOIN memories_fts fts ON m.rowid = fts.rowid
                 WHERE memories_fts MATCH ?1
                   AND m.status = 'active'
                 ORDER BY rank
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![safe_query, limit_i64], row_to_memory)?;
            rows.map(|r| r.map_err(Into::into))
                .collect::<Result<Vec<_>>>()?
        };

        let ids: Vec<String> = mems.iter().map(|m| m.id.clone()).collect();
        self.track_access_batch(&ids);

        Ok(mems)
    }

    pub fn recent_memories(&self, project: Option<&str>, limit: usize) -> Result<Vec<Memory>> {
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);

        let mems = if let Some(proj) = project {
            // Include project-scoped AND global active memories
            let mut stmt = self.conn.prepare(
                "SELECT id, session_id, project, title, type, content, git_diff, created_at,
                        access_count, last_accessed_at, status, scope
                 FROM memories
                 WHERE (project = ?1 OR scope = 'global') AND status = 'active'
                 ORDER BY created_at DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![proj, limit_i64], row_to_memory)?;
            rows.map(|r| r.map_err(Into::into))
                .collect::<Result<Vec<_>>>()?
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT id, session_id, project, title, type, content, git_diff, created_at,
                        access_count, last_accessed_at, status, scope
                 FROM memories WHERE status = 'active'
                 ORDER BY created_at DESC LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit_i64], row_to_memory)?;
            rows.map(|r| r.map_err(Into::into))
                .collect::<Result<Vec<_>>>()?
        };

        let ids: Vec<String> = mems.iter().map(|m| m.id.clone()).collect();
        self.track_access_batch(&ids);

        Ok(mems)
    }

    fn track_access_batch(&self, ids: &[String]) {
        if ids.is_empty() {
            return;
        }
        let now = chrono::Utc::now().to_rfc3339();
        // Use a transaction + per-ID update instead of dynamic IN(?,?,?) SQL.
        // This avoids string-building, stays within rusqlite's safe param API,
        // and is equally fast for typical batch sizes (< 200 rows).
        let result = (|| -> Result<()> {
            let tx = self.conn.unchecked_transaction()?;
            for id in ids {
                tx.execute(
                    "UPDATE memories SET access_count = access_count + 1, last_accessed_at = ?1 WHERE id = ?2",
                    params![now, id],
                )?;
            }
            tx.commit()?;
            Ok(())
        })();
        // In `mem mcp` mode, stderr is observed by the MCP client (stdio transport).
        // The "[mem]" prefix lets clients filter these warnings. Access tracking
        // failures are non-fatal — they do not affect query results.
        if let Err(e) = result {
            eprintln!("[mem] warn: batch access tracking failed: {e}");
        }
    }

    /// Hard-delete a memory by ID. Returns true if a row was deleted.
    pub fn delete_memory(&self, id: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM memories WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    /// Run Ebbinghaus decay: mark memories below retention threshold as 'cold'.
    /// Returns the count of memories marked cold.
    /// If dry_run=true, returns what would be marked without making changes.
    pub fn run_decay(&self, threshold: f64, dry_run: bool) -> Result<u64> {
        let now = Utc::now();
        let now_str = now.to_rfc3339();

        // Compute retention_score = (access_count + 1) / (1 + days_since_created * 0.05)
        // We compute in SQL using julianday arithmetic.
        if dry_run {
            // COUNT only — no write, no TOCTOU window
            let count: u64 = self
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM memories
                     WHERE status = 'active'
                       AND (CAST(access_count AS REAL) + 1.0)
                           / (1.0 + (julianday(?1) - julianday(created_at)) * 0.05) < ?2",
                    params![now_str, threshold],
                    |r| r.get::<_, i64>(0),
                )
                .map(|n| n.max(0) as u64)?;
            return Ok(count);
        }

        // Live path: single UPDATE then read changes() — eliminates TOCTOU between
        // the COUNT and UPDATE that the previous two-query approach had.
        self.conn.execute(
            "UPDATE memories SET status = 'cold'
             WHERE status = 'active'
               AND (CAST(access_count AS REAL) + 1.0)
                   / (1.0 + (julianday(?1) - julianday(created_at)) * 0.05) < ?2",
            params![now_str, threshold],
        )?;
        Ok(self.conn.changes())
    }

    /// Set a memory's scope to 'global'.
    pub fn promote_memory(&self, id: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE memories SET scope = 'global' WHERE id = ?1",
            params![id],
        )?;
        Ok(n > 0)
    }

    /// Set a memory's scope back to 'project'.
    pub fn demote_memory(&self, id: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE memories SET scope = 'project' WHERE id = ?1",
            params![id],
        )?;
        Ok(n > 0)
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

    /// Write transcript analytics to a session row. No-op if session not found.
    pub fn update_session_analytics(
        &self,
        id: &str,
        analytics: &TranscriptAnalytics,
    ) -> Result<()> {
        let rows = self.conn.execute(
            "UPDATE sessions SET
                 turn_count = ?1,
                 duration_secs = ?2,
                 input_tokens = ?3,
                 output_tokens = ?4,
                 cache_read_tokens = ?5,
                 cache_creation_tokens = ?6
             WHERE id = ?7",
            params![
                analytics.turn_count,
                analytics.duration_secs,
                analytics.input_tokens,
                analytics.output_tokens,
                analytics.cache_read_tokens,
                analytics.cache_creation_tokens,
                id
            ],
        )?;
        if rows == 0 {
            eprintln!("[mem] warn: session_id {id} not found — analytics not stored");
        }
        Ok(())
    }

    // ── Stats ─────────────────────────────────────────────────────────────────

    pub fn stats(&self) -> Result<DbStats> {
        // Single aggregating query — avoids 4 separate full scans (N+1 pattern).
        // COALESCE handles the NULL that SUM returns on an empty table.
        let (memory_count, active_count, cold_count, project_count): (u64, u64, u64, u64) =
            self.conn.query_row(
                "SELECT COUNT(*),
                        COALESCE(SUM(CASE WHEN status='active' THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(CASE WHEN status='cold' THEN 1 ELSE 0 END), 0),
                        COUNT(DISTINCT CASE WHEN project IS NOT NULL THEN project END)
                 FROM memories",
                [],
                |r| {
                    let mc: i64 = r.get(0)?;
                    let ac: i64 = r.get(1)?;
                    let cc: i64 = r.get(2)?;
                    let pc: i64 = r.get(3)?;
                    Ok((
                        mc.max(0) as u64,
                        ac.max(0) as u64,
                        cc.max(0) as u64,
                        pc.max(0) as u64,
                    ))
                },
            )?;

        let session_count: u64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get::<_, i64>(0))
            .map(|n| n.max(0) as u64)?;

        // Propagate PRAGMA errors rather than silently reporting "0 KB"
        let db_size_bytes: u64 = self
            .conn
            .query_row(
                "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
                [],
                |r| r.get::<_, i64>(0),
            )
            .context("failed to read DB page size from PRAGMA")
            .map(|n| n.max(0) as u64)?;

        Ok(DbStats {
            memory_count,
            session_count,
            project_count,
            db_size_bytes,
            active_count,
            cold_count,
        })
    }

    /// Aggregate token/time analytics across all sessions.
    pub fn gain_stats(&self) -> Result<GainStats> {
        let aggregate = self.conn.query_row(
            "SELECT
                 COUNT(*) as session_count,
                 COALESCE(SUM(duration_secs), 0) as total_secs,
                 COALESCE(SUM(input_tokens), 0) as total_input,
                 COALESCE(SUM(output_tokens), 0) as total_output,
                 COALESCE(SUM(cache_read_tokens), 0) as total_cache_read,
                 COALESCE(SUM(cache_creation_tokens), 0) as total_cache_creation,
                 COALESCE(AVG(CAST(turn_count AS REAL)), 0.0) as avg_turns,
                 COALESCE(AVG(CAST(duration_secs AS REAL)), 0.0) as avg_secs
             FROM sessions",
            [],
            |r| {
                Ok(GainStats {
                    session_count: r.get(0)?,
                    total_secs: r.get(1)?,
                    total_input: r.get(2)?,
                    total_output: r.get(3)?,
                    total_cache_read: r.get(4)?,
                    total_cache_creation: r.get(5)?,
                    avg_turns: r.get(6)?,
                    avg_secs: r.get(7)?,
                    top_projects: Vec::new(),
                })
            },
        )?;

        let mut stmt = self.conn.prepare(
            "SELECT project, COUNT(*) as sessions,
                    SUM(input_tokens + output_tokens + cache_read_tokens) as total_tokens
             FROM sessions
             WHERE project IS NOT NULL
             GROUP BY project
             ORDER BY total_tokens DESC
             LIMIT 5",
        )?;
        let top_projects = stmt
            .query_map([], |r| {
                Ok(ProjectGainRow {
                    project: r.get(0)?,
                    sessions: r.get(1)?,
                    total_tokens: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
                })
            })?
            .map(|r| r.map_err(Into::into))
            .collect::<Result<Vec<_>>>()?;

        Ok(GainStats {
            top_projects,
            ..aggregate
        })
    }

    // ── suggest-rules helpers ─────────────────────────────────────────────────

    /// Load the last N auto-captured memories for pattern analysis.
    pub fn recent_auto_memories(&self, limit: usize) -> Result<Vec<Memory>> {
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, project, title, type, content, git_diff, created_at,
                    access_count, last_accessed_at, status, scope
             FROM memories WHERE type = 'auto'
             ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit_i64], row_to_memory)?;
        rows.map(|r| r.map_err(Into::into)).collect()
    }

    // ── Indexed files ─────────────────────────────────────────────────────────

    pub fn upsert_indexed_file(
        &self,
        source_path: &str,
        project_path: Option<&str>,
        project_name: &str,
        title: &str,
        content: &str,
        mtime: i64,
    ) -> Result<UpsertOutcome> {
        let existing = self.conn.query_row(
            "SELECT id, file_mtime_secs FROM indexed_files WHERE source_path = ?1",
            params![source_path],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        );

        match existing {
            Ok((_id, existing_mtime)) if existing_mtime == mtime => {
                return Ok(UpsertOutcome::Unchanged);
            }
            Ok(_) => {
                // mtime changed — fall through to upsert
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                // New row — fall through to upsert
            }
            Err(e) => return Err(e.into()),
        }

        let is_new = matches!(existing, Err(rusqlite::Error::QueryReturnedNoRows));
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        self.conn.execute(
            "INSERT OR REPLACE INTO indexed_files
                 (id, source_path, project_path, project_name, title, content, indexed_at, file_mtime_secs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, source_path, project_path, project_name, title, content, now, mtime],
        )?;

        if is_new {
            Ok(UpsertOutcome::New)
        } else {
            Ok(UpsertOutcome::Updated)
        }
    }

    pub fn search_indexed_files(
        &self,
        query: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<IndexedFile>> {
        let safe_query = format!("\"{}\"", query.replace('"', "\"\""));
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);

        if let Some(proj) = project {
            let mut stmt = self.conn.prepare(
                "SELECT f.id, f.source_path, f.project_path, f.project_name, f.title,
                        f.content, f.indexed_at, f.file_mtime_secs
                 FROM indexed_files f
                 JOIN indexed_files_fts fts ON f.rowid = fts.rowid
                 WHERE indexed_files_fts MATCH ?1
                   AND (f.project_path = ?2 OR f.project_name = ?2)
                 ORDER BY rank
                 LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![safe_query, proj, limit_i64], row_to_indexed_file)?;
            rows.map(|r| r.map_err(Into::into)).collect()
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT f.id, f.source_path, f.project_path, f.project_name, f.title,
                        f.content, f.indexed_at, f.file_mtime_secs
                 FROM indexed_files f
                 JOIN indexed_files_fts fts ON f.rowid = fts.rowid
                 WHERE indexed_files_fts MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![safe_query, limit_i64], row_to_indexed_file)?;
            rows.map(|r| r.map_err(Into::into)).collect()
        }
    }

    #[cfg(test)]
    pub fn list_indexed_files(&self) -> Result<Vec<IndexedFile>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, source_path, project_path, project_name, title, content, indexed_at, file_mtime_secs
             FROM indexed_files
             ORDER BY project_name ASC",
        )?;
        let rows = stmt.query_map([], row_to_indexed_file)?;
        rows.map(|r| r.map_err(Into::into)).collect()
    }

    pub fn search_unified(
        &self,
        query: &str,
        project: Option<&str>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        // Query each source for up to `limit` so neither is starved when the other
        // returns few matches. The interleave + truncate caps the combined total.
        let memories = self.search_memories(query, project, limit)?;
        let files = self.search_indexed_files(query, project, limit)?;

        let mut results: Vec<SearchResult> = Vec::with_capacity(limit);
        let mut mi = memories.into_iter();
        let mut fi = files.into_iter();
        loop {
            match (mi.next(), fi.next()) {
                (None, None) => break,
                (Some(m), None) => results.push(SearchResult::Memory(m)),
                (None, Some(f)) => results.push(SearchResult::IndexedFile(f)),
                (Some(m), Some(f)) => {
                    results.push(SearchResult::Memory(m));
                    results.push(SearchResult::IndexedFile(f));
                }
            }
            if results.len() >= limit {
                break;
            }
        }
        results.truncate(limit);
        Ok(results)
    }
}

// ── Row helpers ───────────────────────────────────────────────────────────────

fn row_to_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<Memory> {
    let type_str: String = row.get("type")?;
    let created_at_str: String = row.get("created_at")?;

    let memory_type = type_str.parse::<MemoryType>().map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, e.into())
    })?;

    let created_at = created_at_str.parse().map_err(|e: chrono::ParseError| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;

    Ok(Memory {
        id: row.get("id")?,
        session_id: row.get("session_id")?,
        project: row.get("project")?,
        title: row.get("title")?,
        memory_type,
        content: row.get("content")?,
        git_diff: row.get("git_diff")?,
        created_at,
        access_count: {
            let ac: i64 = row.get("access_count")?;
            // The schema enforces NOT NULL DEFAULT 0, so negative values are
            // impossible. Values exceeding u32::MAX (~4 billion accesses) are
            // implausible in practice; saturate to 0 rather than propagating an error.
            u32::try_from(ac).unwrap_or(0)
        },
        last_accessed_at: {
            let la: Option<String> = row.get("last_accessed_at")?;
            la.map(|s| {
                s.parse::<chrono::DateTime<chrono::Utc>>().map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })
            })
            .transpose()?
        },
        status: {
            let s: String = row.get("status")?;
            s.parse::<MemoryStatus>().map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, e.into())
            })?
        },
        scope: {
            let s: String = row.get("scope")?;
            s.parse::<MemoryScope>().map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, e.into())
            })?
        },
    })
}

fn row_to_indexed_file(row: &rusqlite::Row<'_>) -> rusqlite::Result<IndexedFile> {
    let indexed_at_str: String = row.get("indexed_at")?;
    let indexed_at = indexed_at_str
        .parse::<chrono::DateTime<chrono::Utc>>()
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;
    Ok(IndexedFile {
        id: row.get("id")?,
        source_path: row.get("source_path")?,
        project_path: row.get("project_path")?,
        project_name: row.get("project_name")?,
        title: row.get("title")?,
        content: row.get("content")?,
        indexed_at,
        file_mtime_secs: row.get("file_mtime_secs")?,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn in_memory_db() -> Db {
        Db::open(std::path::Path::new(":memory:")).expect("in-memory DB")
    }

    #[test]
    fn save_and_retrieve_memory() {
        let db = in_memory_db();
        let mem = db
            .save_memory(
                "Test title",
                MemoryType::Manual,
                "Test content",
                Some("/proj"),
                None,
                None,
            )
            .unwrap();
        assert_eq!(mem.title, "Test title");
        assert_eq!(mem.memory_type, MemoryType::Manual);
        assert_eq!(mem.project.as_deref(), Some("/proj"));

        let got = db.get_memory(&mem.id).unwrap().expect("should exist");
        assert_eq!(got.id, mem.id);
        assert_eq!(got.content, "Test content");
    }

    #[test]
    fn save_rejects_empty_title() {
        let db = in_memory_db();
        let err = db
            .save_memory("", MemoryType::Manual, "content", None, None, None)
            .unwrap_err();
        assert!(err.to_string().contains("title"));
    }

    #[test]
    fn save_rejects_empty_content() {
        let db = in_memory_db();
        let err = db
            .save_memory("title", MemoryType::Manual, "  ", None, None, None)
            .unwrap_err();
        assert!(err.to_string().contains("content"));
    }

    #[test]
    fn fts5_search_finds_saved_memory() {
        let db = in_memory_db();
        db.save_memory(
            "JWT auth decision",
            MemoryType::Decision,
            "Chose JWT over sessions",
            Some("/proj"),
            None,
            None,
        )
        .unwrap();
        db.save_memory(
            "Database schema",
            MemoryType::Pattern,
            "Uses UUID primary keys",
            Some("/proj"),
            None,
            None,
        )
        .unwrap();

        let results = db.search_memories("JWT", Some("/proj"), 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "JWT auth decision");
    }

    #[test]
    fn fts5_search_handles_injection_input() {
        let db = in_memory_db();
        db.save_memory("title", MemoryType::Manual, "content", None, None, None)
            .unwrap();

        // These would crash if passed raw to FTS5 MATCH — must not panic
        for bad_query in &["AND OR", "\"unterminated", "NEAR(a b)", "*prefix"] {
            let res = db.search_memories(bad_query, None, 10);
            assert!(
                res.is_ok(),
                "query {bad_query:?} should not error: {:?}",
                res.err()
            );
        }
    }

    #[test]
    fn recent_memories_scoped_by_project() {
        let db = in_memory_db();
        db.save_memory(
            "proj-a memory",
            MemoryType::Auto,
            "content a",
            Some("/proj-a"),
            None,
            None,
        )
        .unwrap();
        db.save_memory(
            "proj-b memory",
            MemoryType::Auto,
            "content b",
            Some("/proj-b"),
            None,
            None,
        )
        .unwrap();

        let a = db.recent_memories(Some("/proj-a"), 10).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].title, "proj-a memory");
    }

    #[test]
    fn stats_returns_correct_counts() {
        let db = in_memory_db();
        db.save_memory("t1", MemoryType::Manual, "c1", Some("/p1"), None, None)
            .unwrap();
        db.save_memory("t2", MemoryType::Manual, "c2", Some("/p2"), None, None)
            .unwrap();

        let s = db.stats().unwrap();
        assert_eq!(s.memory_count, 2);
        assert_eq!(s.project_count, 2);
        assert!(s.db_size_bytes > 0);
    }

    #[test]
    fn session_lifecycle() {
        let db = in_memory_db();
        db.start_session("sess-1", Some("/proj"), Some("add auth"))
            .unwrap();
        db.end_session("sess-1").unwrap();

        let s = db.stats().unwrap();
        assert_eq!(s.session_count, 1);
    }

    #[test]
    fn stop_hook_active_guard_in_auto_capture() {
        use crate::auto::AutoCapture;
        let result =
            AutoCapture::from_stdin(r#"{"cwd":"/proj","stop_hook_active":true}"#, None).unwrap();
        assert!(result.is_none(), "stop_hook_active=true must return None");
    }

    #[test]
    fn compact_context_output_is_valid_json_with_correct_key() {
        use crate::types::CompactContextOutput;
        let out = CompactContextOutput {
            additional_context: "some memory".to_string(),
        };
        let json = serde_json::to_string(&out).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(
            parsed.get("additionalContext").is_some(),
            "must have additionalContext key"
        );
        assert!(
            parsed.get("additional_context").is_none(),
            "must NOT have snake_case key"
        );
    }

    #[test]
    fn get_memory_returns_none_for_missing_id() {
        let db = in_memory_db();
        let result = db.get_memory("nonexistent-id").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn search_returns_empty_for_no_matches() {
        let db = in_memory_db();
        db.save_memory(
            "title",
            MemoryType::Manual,
            "content about databases",
            None,
            None,
            None,
        )
        .unwrap();
        let results = db
            .search_memories("completelymissingword", None, 10)
            .unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_limit_is_respected() {
        let db = in_memory_db();
        for i in 0..5 {
            db.save_memory(
                &format!("title {i}"),
                MemoryType::Manual,
                "rust async tokio content",
                None,
                None,
                None,
            )
            .unwrap();
        }
        let results = db.search_memories("rust", None, 3).unwrap();
        assert!(results.len() <= 3);
    }

    #[test]
    fn recent_memories_respects_limit() {
        let db = in_memory_db();
        for i in 0..5 {
            db.save_memory(
                &format!("memory {i}"),
                MemoryType::Manual,
                "content",
                None,
                None,
                None,
            )
            .unwrap();
        }
        let results = db.recent_memories(None, 2).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn recent_memories_returns_all_when_no_project_filter() {
        let db = in_memory_db();
        db.save_memory("a", MemoryType::Auto, "ca", Some("/a"), None, None)
            .unwrap();
        db.save_memory("b", MemoryType::Auto, "cb", Some("/b"), None, None)
            .unwrap();
        db.save_memory("c", MemoryType::Auto, "cc", None, None, None)
            .unwrap();

        let all = db.recent_memories(None, 10).unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn save_memory_with_git_diff_roundtrips() {
        let db = in_memory_db();
        let diff = "1 file changed, 10 insertions(+)";
        let mem = db
            .save_memory("title", MemoryType::Auto, "content", None, None, Some(diff))
            .unwrap();
        let got = db.get_memory(&mem.id).unwrap().unwrap();
        assert_eq!(got.git_diff.as_deref(), Some(diff));
    }

    #[test]
    fn save_memory_whitespace_only_title_is_rejected() {
        let db = in_memory_db();
        let err = db
            .save_memory("   \t  ", MemoryType::Manual, "content", None, None, None)
            .unwrap_err();
        assert!(err.to_string().contains("title"));
    }

    #[test]
    fn duplicate_session_start_is_idempotent() {
        let db = in_memory_db();
        db.start_session("sess-1", Some("/proj"), Some("goal"))
            .unwrap();
        // Second INSERT OR IGNORE must not fail
        db.start_session("sess-1", Some("/proj"), Some("goal"))
            .unwrap();
        let s = db.stats().unwrap();
        assert_eq!(s.session_count, 1);
    }

    #[test]
    fn end_session_twice_is_idempotent() {
        let db = in_memory_db();
        db.start_session("sess-2", None, None).unwrap();
        db.end_session("sess-2").unwrap();
        // Second call must not error — UPDATE WHERE ended_at IS NULL matches nothing
        db.end_session("sess-2").unwrap();
        let s = db.stats().unwrap();
        assert_eq!(s.session_count, 1);
    }

    #[test]
    fn search_across_projects_without_filter() {
        let db = in_memory_db();
        db.save_memory(
            "auth flow",
            MemoryType::Decision,
            "JWT chosen",
            Some("/a"),
            None,
            None,
        )
        .unwrap();
        db.save_memory(
            "auth cache",
            MemoryType::Decision,
            "Redis auth cache",
            Some("/b"),
            None,
            None,
        )
        .unwrap();

        let results = db.search_memories("auth", None, 10).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn fts5_porter_stemming_finds_variants() {
        let db = in_memory_db();
        db.save_memory(
            "testing strategy",
            MemoryType::Manual,
            "We write tests to validate correctness",
            None,
            None,
            None,
        )
        .unwrap();

        // "testing" and "tested" should both match via porter stemmer
        let r1 = db.search_memories("testing", None, 10).unwrap();
        let r2 = db.search_memories("tested", None, 10).unwrap();
        assert!(!r1.is_empty(), "porter stemmer should match 'testing'");
        assert!(!r2.is_empty(), "porter stemmer should match 'tested'");
    }

    // ── Migration versioning tests ────────────────────────────────────────────

    #[test]
    fn in_memory_db_has_user_version_1() {
        let db = in_memory_db();
        let version: i64 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1, "in-memory DB must be at schema version 1");
    }

    #[test]
    fn memories_table_has_new_columns() {
        let db = in_memory_db();
        // Inserting a memory and reading it back confirms the schema has the new columns
        let mem = db
            .save_memory("title", MemoryType::Manual, "content", None, None, None)
            .unwrap();
        assert_eq!(mem.access_count, 0);
        assert_eq!(mem.status, MemoryStatus::Active);
        assert_eq!(mem.scope, MemoryScope::Project);
        assert!(mem.last_accessed_at.is_none());
    }

    // ── Access tracking tests ─────────────────────────────────────────────────

    #[test]
    fn get_memory_increments_access_count() {
        let db = in_memory_db();
        let mem = db
            .save_memory("title", MemoryType::Manual, "content", None, None, None)
            .unwrap();
        assert_eq!(mem.access_count, 0);

        // get_memory returns the pre-update value (snapshot before tracking fires)
        db.get_memory(&mem.id).unwrap();

        // Read the updated value directly
        let updated: i64 = db
            .conn
            .query_row(
                "SELECT access_count FROM memories WHERE id = ?1",
                params![mem.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(updated, 1, "access_count should be 1 after one get");
    }

    #[test]
    fn search_memories_increments_access_count() {
        let db = in_memory_db();
        let mem = db
            .save_memory(
                "rust async runtime",
                MemoryType::Manual,
                "tokio is used for async",
                None,
                None,
                None,
            )
            .unwrap();

        db.search_memories("rust", None, 10).unwrap();

        let updated: i64 = db
            .conn
            .query_row(
                "SELECT access_count FROM memories WHERE id = ?1",
                params![mem.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(updated, 1);
    }

    #[test]
    fn recent_memories_increments_access_count() {
        let db = in_memory_db();
        let mem = db
            .save_memory("title", MemoryType::Manual, "content", None, None, None)
            .unwrap();

        db.recent_memories(None, 10).unwrap();

        let updated: i64 = db
            .conn
            .query_row(
                "SELECT access_count FROM memories WHERE id = ?1",
                params![mem.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(updated, 1);
    }

    // ── Decay tests ───────────────────────────────────────────────────────────

    #[test]
    fn decay_marks_never_accessed_memory_cold() {
        let db = in_memory_db();
        // A memory with access_count=0 and created_at = now:
        // retention = (0+1)/(1 + 0*0.05) = 1.0/1.0 = 1.0
        // Only becomes cold with high threshold. Use threshold 2.0 to force cold.
        db.save_memory("old memory", MemoryType::Auto, "content", None, None, None)
            .unwrap();

        let count = db.run_decay(2.0, false).unwrap();
        assert_eq!(count, 1, "one memory should be marked cold");

        let s = db.stats().unwrap();
        assert_eq!(s.cold_count, 1);
        assert_eq!(s.active_count, 0);
    }

    #[test]
    fn decay_dry_run_does_not_change_status() {
        let db = in_memory_db();
        db.save_memory("title", MemoryType::Auto, "content", None, None, None)
            .unwrap();

        let count = db.run_decay(2.0, true).unwrap();
        assert_eq!(count, 1, "dry-run should report 1 would be marked cold");

        let s = db.stats().unwrap();
        assert_eq!(s.cold_count, 0, "dry-run must not change status");
        assert_eq!(s.active_count, 1);
    }

    #[test]
    fn decay_excludes_already_cold_memories() {
        let db = in_memory_db();
        db.save_memory("m1", MemoryType::Auto, "content", None, None, None)
            .unwrap();

        // Mark cold once
        db.run_decay(2.0, false).unwrap();
        // Run again — should report 0 (already cold, filtered by WHERE status='active')
        let count = db.run_decay(2.0, false).unwrap();
        assert_eq!(count, 0, "already-cold memories should not be re-counted");
    }

    #[test]
    fn recent_memories_excludes_cold() {
        let db = in_memory_db();
        db.save_memory(
            "active mem",
            MemoryType::Auto,
            "active content",
            None,
            None,
            None,
        )
        .unwrap();
        db.save_memory(
            "cold mem",
            MemoryType::Auto,
            "cold content",
            None,
            None,
            None,
        )
        .unwrap();

        // Mark all cold then re-save one active
        db.run_decay(2.0, false).unwrap();
        db.save_memory(
            "new active",
            MemoryType::Auto,
            "new content",
            None,
            None,
            None,
        )
        .unwrap();

        let recents = db.recent_memories(None, 10).unwrap();
        assert_eq!(recents.len(), 1);
        assert_eq!(recents[0].title, "new active");
    }

    #[test]
    fn stats_reports_active_and_cold_counts() {
        let db = in_memory_db();
        db.save_memory("m1", MemoryType::Auto, "c1", None, None, None)
            .unwrap();
        db.save_memory("m2", MemoryType::Auto, "c2", None, None, None)
            .unwrap();
        db.save_memory("m3", MemoryType::Auto, "c3", None, None, None)
            .unwrap();

        db.run_decay(2.0, false).unwrap(); // marks all 3 cold

        let s = db.stats().unwrap();
        assert_eq!(s.cold_count, 3);
        assert_eq!(s.active_count, 0);
        assert_eq!(s.memory_count, 3);
    }

    // ── Promote / demote tests ────────────────────────────────────────────────

    #[test]
    fn promote_sets_scope_global() {
        let db = in_memory_db();
        let mem = db
            .save_memory(
                "title",
                MemoryType::Manual,
                "content",
                Some("/proj"),
                None,
                None,
            )
            .unwrap();
        assert_eq!(mem.scope, MemoryScope::Project);

        let changed = db.promote_memory(&mem.id).unwrap();
        assert!(changed);

        let scope: String = db
            .conn
            .query_row(
                "SELECT scope FROM memories WHERE id = ?1",
                params![mem.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(scope, "global");
    }

    #[test]
    fn demote_sets_scope_project() {
        let db = in_memory_db();
        let mem = db
            .save_memory(
                "title",
                MemoryType::Manual,
                "content",
                Some("/proj"),
                None,
                None,
            )
            .unwrap();

        db.promote_memory(&mem.id).unwrap();
        let changed = db.demote_memory(&mem.id).unwrap();
        assert!(changed);

        let scope: String = db
            .conn
            .query_row(
                "SELECT scope FROM memories WHERE id = ?1",
                params![mem.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(scope, "project");
    }

    #[test]
    fn promote_returns_false_for_missing_id() {
        let db = in_memory_db();
        let changed = db.promote_memory("nonexistent-id").unwrap();
        assert!(!changed);
    }

    // ── Scope-aware search / context tests ───────────────────────────────────

    #[test]
    fn search_with_project_includes_global_memories() {
        let db = in_memory_db();
        // Save a memory in /proj-a
        let _m1 = db
            .save_memory(
                "project auth",
                MemoryType::Decision,
                "JWT for proj-a",
                Some("/proj-a"),
                None,
                None,
            )
            .unwrap();
        // Save a memory in /proj-b and promote to global
        let m2 = db
            .save_memory(
                "global auth pattern",
                MemoryType::Pattern,
                "JWT refresh tokens global",
                Some("/proj-b"),
                None,
                None,
            )
            .unwrap();
        db.promote_memory(&m2.id).unwrap();

        // Search scoped to /proj-a — should get both proj-a and global
        let results = db.search_memories("auth", Some("/proj-a"), 10).unwrap();
        assert_eq!(results.len(), 2, "should find proj-a and global memories");
    }

    #[test]
    fn recent_with_project_includes_global_memories() {
        let db = in_memory_db();
        db.save_memory(
            "proj-a memory",
            MemoryType::Auto,
            "content a",
            Some("/proj-a"),
            None,
            None,
        )
        .unwrap();
        let m2 = db
            .save_memory(
                "global memory",
                MemoryType::Auto,
                "global content",
                Some("/proj-b"),
                None,
                None,
            )
            .unwrap();
        db.promote_memory(&m2.id).unwrap();

        let recents = db.recent_memories(Some("/proj-a"), 10).unwrap();
        assert_eq!(
            recents.len(),
            2,
            "project + global memories should be included"
        );
    }

    // ── suggest-rules helper test ─────────────────────────────────────────────

    #[test]
    fn recent_auto_memories_filters_by_type() {
        let db = in_memory_db();
        db.save_memory(
            "auto mem",
            MemoryType::Auto,
            "auto content",
            None,
            None,
            None,
        )
        .unwrap();
        db.save_memory(
            "manual mem",
            MemoryType::Manual,
            "manual content",
            None,
            None,
            None,
        )
        .unwrap();

        let autos = db.recent_auto_memories(10).unwrap();
        assert_eq!(autos.len(), 1);
        assert_eq!(autos[0].title, "auto mem");
    }

    // ── Decay formula tests ──────────────────────────────────────────────────

    #[test]
    fn decay_formula_ages_old_memory_correctly() {
        let db = in_memory_db();
        let m = db
            .save_memory(
                "old title",
                MemoryType::Auto,
                "old content",
                None,
                None,
                None,
            )
            .unwrap();
        // Simulate 40-day-old memory: score = (0+1)/(1+40*0.05) = 1/3 ≈ 0.33
        let forty_days_ago = (chrono::Utc::now() - chrono::Duration::days(40)).to_rfc3339();
        db.conn
            .execute(
                "UPDATE memories SET created_at = ?1 WHERE id = ?2",
                rusqlite::params![forty_days_ago, m.id],
            )
            .unwrap();
        // Should decay at threshold=0.4 (score 0.33 < 0.4)
        assert_eq!(
            db.run_decay(0.4, false).unwrap(),
            1,
            "40-day-old memory should be cold at 0.4"
        );
        // Should survive at threshold=0.3 (score 0.33 > 0.3) — need fresh DB
        let db2 = in_memory_db();
        let m2 = db2
            .save_memory("old2", MemoryType::Auto, "content", None, None, None)
            .unwrap();
        db2.conn
            .execute(
                "UPDATE memories SET created_at = ?1 WHERE id = ?2",
                rusqlite::params![forty_days_ago, m2.id],
            )
            .unwrap();
        assert_eq!(
            db2.run_decay(0.3, false).unwrap(),
            0,
            "40-day-old memory should survive at 0.3"
        );
    }

    #[test]
    fn decay_formula_survives_high_access_count() {
        let db = in_memory_db();
        let m = db
            .save_memory("accessed", MemoryType::Auto, "content", None, None, None)
            .unwrap();
        // Simulate 10 accesses: score = (10+1)/(1+0*0.05) = 11.0
        db.conn
            .execute(
                "UPDATE memories SET access_count = 10 WHERE id = ?1",
                rusqlite::params![m.id],
            )
            .unwrap();
        // Should survive threshold=1.5 (score 11.0 >> 1.5)
        assert_eq!(
            db.run_decay(1.5, false).unwrap(),
            0,
            "heavily accessed memory should not decay"
        );
    }

    // ── gain_stats tests ──────────────────────────────────────────────────────

    #[test]
    fn gain_stats_returns_zero_for_empty_db() {
        let db = in_memory_db();
        let g = db.gain_stats().unwrap();
        assert_eq!(g.session_count, 0, "no sessions in empty db");
        assert_eq!(g.total_input, 0);
        assert_eq!(g.total_output, 0);
        assert_eq!(g.total_cache_read, 0);
        assert_eq!(g.total_cache_creation, 0);
        assert_eq!(g.total_secs, 0);
        assert!(g.top_projects.is_empty());
    }

    // ── Migration idempotency test ───────────────────────────────────────────

    #[test]
    fn migration_is_idempotent_on_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mem.db");
        // First open runs migrations
        Db::open(&path).expect("first open must succeed");
        // Second open must not error — migration guard prevents re-running ALTER TABLE
        let db2 = Db::open(&path).expect("second open must succeed");
        let version: i64 = db2
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 1, "user_version should be 1 on reopen");
    }

    // ── Cold global memory excluded from scoped search ───────────────────────

    #[test]
    fn search_excludes_cold_global_memories() {
        let db = in_memory_db();
        let m = db
            .save_memory(
                "global cold memory",
                MemoryType::Pattern,
                "global content cold",
                Some("/other-project"),
                None,
                None,
            )
            .unwrap();
        db.promote_memory(&m.id).unwrap();
        // Force decay (use threshold=2.0 to mark as cold regardless of age)
        let count = db.run_decay(2.0, false).unwrap();
        assert_eq!(count, 1, "memory should be marked cold");
        // Should not appear in search for /proj-a
        let results = db.search_memories("global", Some("/proj-a"), 10).unwrap();
        assert!(
            results.is_empty(),
            "cold global memory must not appear in search"
        );
    }

    // ── Demote missing ID ────────────────────────────────────────────────────

    #[test]
    fn demote_returns_false_for_missing_id() {
        let db = in_memory_db();
        let result = db.demote_memory("nonexistent-id-xyz").unwrap();
        assert!(!result, "demote should return false for nonexistent ID");
    }

    // ── Session analytics write-read round-trip ───────────────────────────────

    #[test]
    fn update_session_analytics_persists_and_gain_stats_reads_back() {
        use crate::types::TranscriptAnalytics;

        let db = in_memory_db();
        db.start_session("sess-analytics", Some("/proj-a"), None)
            .unwrap();

        let analytics = TranscriptAnalytics {
            turn_count: 7,
            duration_secs: 300,
            input_tokens: 1000,
            output_tokens: 500,
            cache_read_tokens: 200,
            cache_creation_tokens: 50,
            last_assistant_message: None,
        };
        db.update_session_analytics("sess-analytics", &analytics)
            .unwrap();

        let g = db.gain_stats().unwrap();
        assert_eq!(g.session_count, 1);
        assert_eq!(g.total_input, 1000);
        assert_eq!(g.total_output, 500);
        assert_eq!(g.total_cache_read, 200);
        assert_eq!(g.total_cache_creation, 50);
        assert_eq!(g.total_secs, 300);
        assert_eq!(g.avg_turns, 7.0);
    }

    #[test]
    fn gain_stats_aggregates_multiple_sessions() {
        use crate::types::TranscriptAnalytics;

        let db = in_memory_db();
        for (id, proj) in [("s1", "/a"), ("s2", "/b"), ("s3", "/a")] {
            db.start_session(id, Some(proj), None).unwrap();
            db.update_session_analytics(
                id,
                &TranscriptAnalytics {
                    turn_count: 4,
                    duration_secs: 120,
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_read_tokens: 30,
                    cache_creation_tokens: 10,
                    last_assistant_message: None,
                },
            )
            .unwrap();
        }

        let g = db.gain_stats().unwrap();
        assert_eq!(g.session_count, 3);
        assert_eq!(g.total_input, 300);
        assert_eq!(g.total_output, 150);
        assert_eq!(g.total_cache_read, 90);
        assert_eq!(g.total_secs, 360);
        assert_eq!(g.avg_turns, 4.0);
        // top_projects: /a has 2 sessions, /b has 1
        assert!(!g.top_projects.is_empty());
        assert_eq!(g.top_projects[0].project, "/a");
        assert_eq!(g.top_projects[0].sessions, 2);
    }

    #[test]
    fn update_session_analytics_missing_session_id_is_no_op() {
        use crate::types::TranscriptAnalytics;

        let db = in_memory_db();
        // No session created — should complete without error (warning goes to stderr)
        let result = db.update_session_analytics(
            "nonexistent-session",
            &TranscriptAnalytics {
                turn_count: 3,
                duration_secs: 60,
                input_tokens: 100,
                output_tokens: 50,
                cache_read_tokens: 20,
                cache_creation_tokens: 5,
                last_assistant_message: None,
            },
        );
        assert!(result.is_ok(), "missing session_id must not return Err");
        // No sessions exist, so gain_stats shows 0
        let g = db.gain_stats().unwrap();
        assert_eq!(g.session_count, 0);
    }

    // ── indexed_files tests ───────────────────────────────────────────────────

    #[test]
    fn upsert_indexed_file_new() {
        let db = in_memory_db();
        let outcome = db
            .upsert_indexed_file(
                "/home/user/.claude/projects/foo/memory/MEMORY.md",
                Some("/home/user/projects/foo"),
                "foo",
                "Foo MEMORY",
                "some content",
                1000,
            )
            .unwrap();
        assert_eq!(outcome, UpsertOutcome::New);
    }

    #[test]
    fn upsert_indexed_file_unchanged() {
        let db = in_memory_db();
        db.upsert_indexed_file("/path/MEMORY.md", None, "proj", "Title", "content", 1000)
            .unwrap();
        let outcome = db
            .upsert_indexed_file("/path/MEMORY.md", None, "proj", "Title", "content", 1000)
            .unwrap();
        assert_eq!(outcome, UpsertOutcome::Unchanged);
    }

    #[test]
    fn upsert_indexed_file_updated() {
        let db = in_memory_db();
        db.upsert_indexed_file(
            "/path/MEMORY.md",
            None,
            "proj",
            "Title",
            "old content",
            1000,
        )
        .unwrap();
        let outcome = db
            .upsert_indexed_file(
                "/path/MEMORY.md",
                None,
                "proj",
                "Title",
                "new content",
                2000,
            )
            .unwrap();
        assert_eq!(outcome, UpsertOutcome::Updated);
    }

    #[test]
    fn search_indexed_files_finds_content() {
        let db = in_memory_db();
        db.upsert_indexed_file(
            "/path/MEMORY.md",
            None,
            "polybot-ts",
            "Polybot Memory",
            "Biome forbids non-null assertion",
            1000,
        )
        .unwrap();
        let results = db.search_indexed_files("biome", None, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].project_name, "polybot-ts");
    }

    #[test]
    fn search_indexed_files_injection_safety() {
        let db = in_memory_db();
        db.upsert_indexed_file("/path/MEMORY.md", None, "proj", "Title", "content", 1000)
            .unwrap();
        for bad in &["AND OR", "\"unterminated", "NEAR(a b)"] {
            let res = db.search_indexed_files(bad, None, 10);
            assert!(res.is_ok(), "query {bad:?} should not error");
        }
    }

    #[test]
    fn search_indexed_files_project_filter() {
        let db = in_memory_db();
        db.upsert_indexed_file(
            "/a/MEMORY.md",
            Some("/proj-a"),
            "proj-a",
            "A Patterns",
            "rust async tokio",
            1000,
        )
        .unwrap();
        db.upsert_indexed_file(
            "/b/MEMORY.md",
            Some("/proj-b"),
            "proj-b",
            "B Patterns",
            "rust async tokio",
            1000,
        )
        .unwrap();

        // With project filter — only proj-a
        let results = db
            .search_indexed_files("tokio", Some("/proj-a"), 10)
            .unwrap();
        assert_eq!(results.len(), 1, "project filter should limit to proj-a");
        assert_eq!(results[0].project_name, "proj-a");

        // Without project filter — both
        let all = db.search_indexed_files("tokio", None, 10).unwrap();
        assert_eq!(all.len(), 2, "no filter should return both projects");
    }

    #[test]
    fn search_indexed_files_update_refreshes_fts_index() {
        let db = in_memory_db();
        db.upsert_indexed_file(
            "/path/MEMORY.md",
            None,
            "proj",
            "Title",
            "old unique term xyzzy",
            1000,
        )
        .unwrap();
        // Update with new content
        db.upsert_indexed_file(
            "/path/MEMORY.md",
            None,
            "proj",
            "Title",
            "new unique term frobble",
            2000,
        )
        .unwrap();

        // Old term should no longer match
        let old = db.search_indexed_files("xyzzy", None, 10).unwrap();
        assert!(
            old.is_empty(),
            "old term should be removed from FTS after update"
        );

        // New term should match
        let new = db.search_indexed_files("frobble", None, 10).unwrap();
        assert_eq!(new.len(), 1, "new term should be searchable after update");
    }

    #[test]
    fn search_unified_returns_both_sources() {
        let db = in_memory_db();
        db.save_memory(
            "rust async",
            MemoryType::Manual,
            "tokio runtime content",
            None,
            None,
            None,
        )
        .unwrap();
        db.upsert_indexed_file(
            "/path/MEMORY.md",
            None,
            "myproj",
            "Rust Patterns",
            "tokio async runtime patterns",
            1000,
        )
        .unwrap();
        let results = db.search_unified("tokio", None, 10).unwrap();
        let has_memory = results.iter().any(|r| matches!(r, SearchResult::Memory(_)));
        let has_file = results
            .iter()
            .any(|r| matches!(r, SearchResult::IndexedFile(_)));
        assert!(has_memory, "unified search should include memories");
        assert!(has_file, "unified search should include indexed files");
    }

    #[test]
    fn search_unified_only_memories_match() {
        let db = in_memory_db();
        db.save_memory(
            "jwt auth",
            MemoryType::Manual,
            "jwt token content unique",
            None,
            None,
            None,
        )
        .unwrap();
        // No indexed files — unified search should still return memories up to limit
        let results = db.search_unified("jwt", None, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], SearchResult::Memory(_)));
    }

    #[test]
    fn search_unified_only_files_match() {
        let db = in_memory_db();
        db.upsert_indexed_file(
            "/path/MEMORY.md",
            None,
            "proj",
            "Title",
            "biome linter unique term",
            1000,
        )
        .unwrap();
        // No memories — unified search should still return indexed files up to limit
        let results = db.search_unified("biome", None, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], SearchResult::IndexedFile(_)));
    }

    #[test]
    fn search_unified_respects_limit() {
        let db = in_memory_db();
        for i in 0..5 {
            db.save_memory(
                &format!("rust memory {i}"),
                MemoryType::Manual,
                "rust async tokio content",
                None,
                None,
                None,
            )
            .unwrap();
            db.upsert_indexed_file(
                &format!("/path/{i}/MEMORY.md"),
                None,
                &format!("proj{i}"),
                "Rust Patterns",
                "rust async tokio patterns",
                1000 + i as i64,
            )
            .unwrap();
        }
        let results = db.search_unified("tokio", None, 3).unwrap();
        assert_eq!(results.len(), 3, "unified search must respect limit");
    }

    #[test]
    fn list_indexed_files_returns_ordered_by_project() {
        let db = in_memory_db();
        db.upsert_indexed_file(
            "/z/MEMORY.md",
            None,
            "zoo",
            "Zoo Memory",
            "zoo content",
            1000,
        )
        .unwrap();
        db.upsert_indexed_file(
            "/a/MEMORY.md",
            None,
            "apple",
            "Apple Memory",
            "apple content",
            1000,
        )
        .unwrap();
        db.upsert_indexed_file(
            "/m/MEMORY.md",
            None,
            "mango",
            "Mango Memory",
            "mango content",
            1000,
        )
        .unwrap();

        let files = db.list_indexed_files().unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].project_name, "apple");
        assert_eq!(files[1].project_name, "mango");
        assert_eq!(files[2].project_name, "zoo");
    }
}
