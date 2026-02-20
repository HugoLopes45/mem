-- Migration 003: Session analytics columns
-- Applied only when PRAGMA user_version < 3
-- Note: turn_count already exists from 001_init.sql — not added here.

BEGIN;
ALTER TABLE sessions ADD COLUMN duration_secs INTEGER;
ALTER TABLE sessions ADD COLUMN input_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN output_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN cache_read_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN cache_creation_tokens INTEGER NOT NULL DEFAULT 0;
COMMIT;

-- PRAGMA user_version does not participate in SQLite transactions; set it
-- separately after the DDL batch succeeds so atomicity is preserved.
PRAGMA user_version = 3;
