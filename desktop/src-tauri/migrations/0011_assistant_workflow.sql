-- Preserve older results and decisions; allow a bounded one-time upgrade of
-- routine/no-result messages when the academic importance policy changes.
ALTER TABLE source_analyses ADD COLUMN policy_version INTEGER NOT NULL DEFAULT 1;
