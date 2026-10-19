-- Owner-approved local schedule amendments. Original RRULE/occurrence IDs stay stable.
CREATE TABLE schedule_adjustments (
  id TEXT PRIMARY KEY NOT NULL,
  candidate_id TEXT NOT NULL UNIQUE REFERENCES source_candidates(id),
  task_id TEXT NOT NULL REFERENCES tasks(id),
  series_id TEXT NOT NULL,
  request_hash TEXT NOT NULL,
  source_order INTEGER NOT NULL,
  from_date TEXT NOT NULL,
  weekday INTEGER NOT NULL CHECK (weekday BETWEEN 0 AND 6),
  scope TEXT NOT NULL CHECK (scope IN ('this','following')),
  new_time TEXT,
  location TEXT,
  created_at TEXT NOT NULL,
  active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0,1)),
  CHECK (new_time IS NOT NULL OR location IS NOT NULL)
);
CREATE INDEX schedule_adjustments_by_task ON schedule_adjustments(task_id, active, from_date);
