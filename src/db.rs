use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection};
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
        let conn = Connection::open(path).with_context(|| format!("open db {}", path.display()))?;

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
        let memory_count: u64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get::<_, i64>(0))
            .map(|n| n.max(0) as u64)?;

        let session_count: u64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get::<_, i64>(0))
            .map(|n| n.max(0) as u64)?;

        let project_count: u64 = self
            .conn
            .query_row(
                "SELECT COUNT(DISTINCT project) FROM memories WHERE project IS NOT NULL",
                [],
                |r| r.get::<_, i64>(0),
            )
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
        })
    }
}

// ── Row helpers ───────────────────────────────────────────────────────────────

fn row_to_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<Memory> {
    let type_str: String = row.get(4)?;
    let created_at_str: String = row.get(7)?;

    let memory_type = type_str.parse::<MemoryType>().map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, e.into())
    })?;

    let created_at = created_at_str.parse().map_err(|e: chrono::ParseError| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e))
    })?;

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
}
