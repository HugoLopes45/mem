-- Migration 002: Ebbinghaus decay + namespace scope
-- Applied only when PRAGMA user_version < 2

BEGIN;

ALTER TABLE memories ADD COLUMN access_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE memories ADD COLUMN last_accessed_at TEXT;
ALTER TABLE memories ADD COLUMN status TEXT NOT NULL DEFAULT 'active'
    CHECK(status IN ('active', 'cold'));
ALTER TABLE memories ADD COLUMN scope TEXT NOT NULL DEFAULT 'project'
    CHECK(scope IN ('project', 'global'));

COMMIT;

-- PRAGMA user_version does not participate in SQLite transactions; set it
-- separately after the DDL batch succeeds so atomicity is preserved.
PRAGMA user_version = 2;
