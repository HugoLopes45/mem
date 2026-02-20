-- mem: persistent memory for Claude Code sessions
-- Storage: SQLite WAL + FTS5
-- This is the canonical schema. Applied once on a fresh database (user_version 0 → 1).

-- Session tracking (created first; memories.session_id references this table)
CREATE TABLE IF NOT EXISTS sessions (
    id                    TEXT PRIMARY KEY,
    project               TEXT,
    goal                  TEXT,
    started_at            TEXT NOT NULL,
    ended_at              TEXT,
    turn_count            INTEGER NOT NULL DEFAULT 0,
    duration_secs         INTEGER NOT NULL DEFAULT 0,
    input_tokens          INTEGER NOT NULL DEFAULT 0,
    output_tokens         INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens     INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS memories (
    id              TEXT PRIMARY KEY,
    -- ON DELETE SET NULL: deleting a session leaves its memories intact (orphaned sessions
    -- are not expected in normal operation, but the FK guards against dangling references).
    session_id      TEXT REFERENCES sessions(id) ON DELETE SET NULL,
    project         TEXT,
    title           TEXT NOT NULL,
    type            TEXT NOT NULL CHECK(type IN ('auto','manual','pattern','decision')),
    content         TEXT NOT NULL,
    git_diff        TEXT,
    created_at      TEXT NOT NULL,
    access_count    INTEGER NOT NULL DEFAULT 0,
    last_accessed_at TEXT,
    status          TEXT NOT NULL DEFAULT 'active' CHECK(status IN ('active', 'cold')),
    scope           TEXT NOT NULL DEFAULT 'project' CHECK(scope IN ('project', 'global'))
);

-- FTS5 virtual table with porter stemmer for English search
CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
    title,
    content,
    content='memories',
    content_rowid='rowid',
    tokenize='porter unicode61'
);

-- Triggers to keep FTS index in sync.
-- IMPORTANT: Any direct INSERT, UPDATE, or DELETE on memories.title/content that bypasses
-- normal single-statement DML (e.g. execute_batch with multiple statements) would skip
-- these triggers and leave the FTS index out of sync. Always use single-statement DML.
CREATE TRIGGER IF NOT EXISTS memories_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, title, content)
    VALUES (new.rowid, new.title, new.content);
END;

CREATE TRIGGER IF NOT EXISTS memories_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, title, content)
    VALUES ('delete', old.rowid, old.title, old.content);
END;

CREATE TRIGGER IF NOT EXISTS memories_au AFTER UPDATE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, title, content)
    VALUES ('delete', old.rowid, old.title, old.content);
    INSERT INTO memories_fts(rowid, title, content)
    VALUES (new.rowid, new.title, new.content);
END;

-- Project-scoped queries
CREATE INDEX IF NOT EXISTS memories_project_idx   ON memories(project, created_at DESC);
CREATE INDEX IF NOT EXISTS sessions_project_idx   ON sessions(project, started_at DESC);

-- run_decay (reduces scan set for status filter), recent_memories no-project ORDER BY
CREATE INDEX IF NOT EXISTS memories_status_created_idx ON memories(status, created_at DESC);

-- recent_auto_memories WHERE type='auto'
CREATE INDEX IF NOT EXISTS memories_type_created_idx   ON memories(type, created_at DESC);

-- OR scope='global' join in search/context queries
CREATE INDEX IF NOT EXISTS memories_scope_idx          ON memories(scope);

-- Indexed MEMORY.md files for cross-project search
CREATE TABLE IF NOT EXISTS indexed_files (
    id              TEXT PRIMARY KEY,
    source_path     TEXT NOT NULL UNIQUE,
    project_path    TEXT,                   -- real path decoded from dir name (nullable)
    project_name    TEXT NOT NULL,          -- last path component, e.g. "polybot-ts"
    title           TEXT NOT NULL,          -- from # H1 header, or "MEMORY"
    content         TEXT NOT NULL,
    indexed_at      TEXT NOT NULL,
    file_mtime_secs INTEGER NOT NULL        -- Unix seconds, for O(1) change detection
);

CREATE VIRTUAL TABLE IF NOT EXISTS indexed_files_fts USING fts5(
    title, content,
    content='indexed_files', content_rowid='rowid',
    tokenize='porter unicode61'
);

CREATE TRIGGER IF NOT EXISTS indexed_files_ai AFTER INSERT ON indexed_files BEGIN
    INSERT INTO indexed_files_fts(rowid, title, content)
    VALUES (new.rowid, new.title, new.content);
END;

CREATE TRIGGER IF NOT EXISTS indexed_files_ad AFTER DELETE ON indexed_files BEGIN
    INSERT INTO indexed_files_fts(indexed_files_fts, rowid, title, content)
    VALUES ('delete', old.rowid, old.title, old.content);
END;

CREATE TRIGGER IF NOT EXISTS indexed_files_au AFTER UPDATE ON indexed_files BEGIN
    INSERT INTO indexed_files_fts(indexed_files_fts, rowid, title, content)
    VALUES ('delete', old.rowid, old.title, old.content);
    INSERT INTO indexed_files_fts(rowid, title, content)
    VALUES (new.rowid, new.title, new.content);
END;

CREATE INDEX IF NOT EXISTS indexed_files_project_name_idx ON indexed_files(project_name);
