-- mem: persistent memory for Claude Code sessions
-- Storage: SQLite WAL + FTS5

-- Session tracking (created first; memories.session_id references this table)
CREATE TABLE IF NOT EXISTS sessions (
    id         TEXT PRIMARY KEY,
    project    TEXT,
    goal       TEXT,
    started_at TEXT NOT NULL,
    ended_at   TEXT,
    turn_count INTEGER DEFAULT 0
);

CREATE TABLE IF NOT EXISTS memories (
    id         TEXT PRIMARY KEY,
    -- ON DELETE SET NULL: deleting a session leaves its memories intact (orphaned sessions
    -- are not expected in normal operation, but the FK guards against dangling references).
    session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL,
    project    TEXT,
    title      TEXT NOT NULL,
    type       TEXT NOT NULL CHECK(type IN ('auto','manual','pattern','decision')),
    content    TEXT NOT NULL,
    git_diff   TEXT,
    created_at TEXT NOT NULL
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
-- IMPORTANT: Any direct UPDATE to memories.title or memories.content in db.rs
-- must go through these triggers — bypassing them (e.g. raw execute without UPDATE)
-- would leave the FTS index out of sync. Always use the trigger-covered UPDATE path.
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

-- Index for project-scoped queries
CREATE INDEX IF NOT EXISTS memories_project_idx ON memories(project, created_at DESC);
CREATE INDEX IF NOT EXISTS sessions_project_idx ON sessions(project, started_at DESC);
