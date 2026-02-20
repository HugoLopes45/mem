-- Migration 004: Missing performance indexes
-- Applied only when PRAGMA user_version < 4

BEGIN;

-- Covers: run_decay WHERE status='active', recent_memories no-project path
CREATE INDEX IF NOT EXISTS memories_status_created_idx
    ON memories(status, created_at DESC);

-- Covers: recent_auto_memories WHERE type='auto'
CREATE INDEX IF NOT EXISTS memories_type_created_idx
    ON memories(type, created_at DESC);

-- Covers: OR scope='global' join in search/context queries
CREATE INDEX IF NOT EXISTS memories_scope_idx ON memories(scope);

COMMIT;
