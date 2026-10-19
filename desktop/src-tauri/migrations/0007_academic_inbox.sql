-- Academic inbox (docs/ACADEMIC-INBOX-NEXT.md): local analysis records and cited candidates.
-- Rows reference sources by ID + content hash. Only the literal excerpt the owner reviews is
-- copied. These tables are never written to the sync outbox.
CREATE TABLE source_analyses (
  id TEXT PRIMARY KEY NOT NULL,
  source_id TEXT NOT NULL,
  source_hash TEXT NOT NULL,
  model TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('candidates','routine','nothing_found','invalid_output')),
  detail TEXT NOT NULL DEFAULT '' CHECK (length(detail) <= 500),
  truncated_input INTEGER NOT NULL DEFAULT 0 CHECK (truncated_input IN (0,1)),
  analysed_at TEXT NOT NULL
);
CREATE INDEX source_analyses_by_source ON source_analyses(source_id, analysed_at);

CREATE TABLE source_candidates (
  id TEXT PRIMARY KEY NOT NULL,
  source_id TEXT NOT NULL,
  source_hash TEXT NOT NULL,
  analysis_id TEXT NOT NULL REFERENCES source_analyses(id),
  dedupe_key TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL CHECK (kind IN ('task','deadline','update')),
  title TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 200),
  due_date TEXT,
  date_text TEXT CHECK (date_text IS NULL OR length(date_text) <= 200),
  date_status TEXT NOT NULL CHECK (date_status IN ('none','explicit','ambiguous')),
  excerpt TEXT NOT NULL CHECK (length(excerpt) BETWEEN 1 AND 400),
  confidence REAL NOT NULL CHECK (confidence BETWEEN 0 AND 1),
  model TEXT NOT NULL,
  extracted_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  payload_hash TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','approved','rejected','stale')),
  task_id TEXT UNIQUE REFERENCES tasks(id),
  decided_at TEXT,
  CHECK (status <> 'approved' OR task_id IS NOT NULL),
  CHECK (date_status <> 'explicit' OR due_date IS NOT NULL)
);
CREATE INDEX source_candidates_by_source ON source_candidates(source_id);
CREATE INDEX source_candidates_by_status ON source_candidates(status);
