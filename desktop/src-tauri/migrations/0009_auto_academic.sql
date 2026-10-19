-- Automatic local academic analysis (docs/AUTO-ACADEMIC-NEXT.md). Opt-in settings and a
-- durable queue bound to the source content hash. No message text is stored here; these
-- tables are never written to the sync outbox.
CREATE TABLE auto_analysis_settings (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0,1)),
  paused INTEGER NOT NULL DEFAULT 0 CHECK (paused IN (0,1)),
  model TEXT NOT NULL,
  -- idle | working | offline | model_missing | error (worker health, not message content)
  worker_state TEXT NOT NULL DEFAULT 'idle',
  worker_detail TEXT NOT NULL DEFAULT '' CHECK (length(worker_detail) <= 300),
  backoff_until TEXT,
  backoff_failures INTEGER NOT NULL DEFAULT 0,
  updated_at TEXT NOT NULL
);

CREATE TABLE analysis_jobs (
  id TEXT PRIMARY KEY NOT NULL,
  source_id TEXT NOT NULL,
  source_hash TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('queued','running','done','failed','cancelled','superseded')),
  attempts INTEGER NOT NULL DEFAULT 0,
  next_attempt_at TEXT NOT NULL,
  last_error TEXT NOT NULL DEFAULT '' CHECK (length(last_error) <= 300),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE (source_id, source_hash)
);
CREATE INDEX analysis_jobs_by_status ON analysis_jobs(status, next_attempt_at);
