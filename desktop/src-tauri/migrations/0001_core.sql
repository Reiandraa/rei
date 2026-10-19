-- 0001_core: foundation schema. Common columns follow docs/INTERFACES.md §3.
CREATE TABLE app_meta (
  key TEXT PRIMARY KEY NOT NULL,
  value TEXT NOT NULL
);

CREATE TABLE settings (
  key TEXT PRIMARY KEY NOT NULL,
  value_json TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  updated_at TEXT NOT NULL
);

CREATE TABLE projects (
  id TEXT PRIMARY KEY NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  origin_device_id TEXT NOT NULL,
  deleted_at TEXT,
  name TEXT NOT NULL CHECK (length(name) <= 500)
);

CREATE TABLE tags (
  id TEXT PRIMARY KEY NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  origin_device_id TEXT NOT NULL,
  deleted_at TEXT,
  name TEXT NOT NULL UNIQUE
);

CREATE TABLE recurrence_rules (
  id TEXT PRIMARY KEY NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  origin_device_id TEXT NOT NULL,
  deleted_at TEXT,
  series_id TEXT NOT NULL UNIQUE,
  rrule TEXT NOT NULL,
  tz TEXT NOT NULL,
  dtstart_local TEXT NOT NULL
);

CREATE TABLE tasks (
  id TEXT PRIMARY KEY NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  origin_device_id TEXT NOT NULL,
  deleted_at TEXT,
  title TEXT NOT NULL CHECK (length(title) <= 500),
  notes TEXT NOT NULL DEFAULT '' CHECK (length(notes) <= 65536),
  project_id TEXT REFERENCES projects(id),
  status TEXT NOT NULL DEFAULT 'todo' CHECK (status IN ('todo','doing','done','cancelled')),
  priority INTEGER NOT NULL DEFAULT 0 CHECK (priority BETWEEN 0 AND 3),
  energy TEXT,
  duration_min INTEGER CHECK (duration_min IS NULL OR duration_min > 0),
  due_date TEXT,            -- LocalDate, all-day semantics
  due_at_utc TEXT,          -- UtcInstant, paired with due_tz
  due_tz TEXT,
  recurrence_rule_id TEXT REFERENCES recurrence_rules(id),
  reminder_offsets_json TEXT NOT NULL DEFAULT '[]',
  provenance_json TEXT NOT NULL DEFAULT '{"kind":"User"}',
  CHECK (NOT (due_date IS NOT NULL AND due_at_utc IS NOT NULL)),
  CHECK ((due_at_utc IS NULL) = (due_tz IS NULL))
);

CREATE TABLE task_tags (
  task_id TEXT NOT NULL REFERENCES tasks(id),
  tag_id TEXT NOT NULL REFERENCES tags(id),
  PRIMARY KEY (task_id, tag_id)
);

CREATE TABLE occurrences (
  id TEXT PRIMARY KEY NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  origin_device_id TEXT NOT NULL,
  deleted_at TEXT,
  series_id TEXT NOT NULL REFERENCES recurrence_rules(series_id),
  original_local TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'Normal' CHECK (state IN ('Normal','Done','Skipped','Moved')),
  override_json TEXT,
  UNIQUE (series_id, original_local)
);

CREATE TABLE events (
  id TEXT PRIMARY KEY NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  origin_device_id TEXT NOT NULL,
  deleted_at TEXT,
  title TEXT NOT NULL CHECK (length(title) <= 500),
  provider TEXT,
  account_id TEXT,
  provider_object_id TEXT,
  start_value TEXT NOT NULL,
  end_value TEXT NOT NULL,
  all_day INTEGER NOT NULL DEFAULT 0 CHECK (all_day IN (0,1)),
  tz TEXT NOT NULL,
  source_version TEXT,
  local_override_json TEXT,
  read_only INTEGER NOT NULL DEFAULT 0 CHECK (read_only IN (0,1))
);
CREATE UNIQUE INDEX events_provider_key
  ON events(provider, account_id, provider_object_id) WHERE provider IS NOT NULL;

CREATE TABLE proposals (
  id TEXT PRIMARY KEY NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  origin_device_id TEXT NOT NULL,
  deleted_at TEXT,
  action TEXT NOT NULL,
  target_id TEXT,
  expected_target_revision INTEGER,
  source_revisions_json TEXT NOT NULL DEFAULT '[]',
  payload_json TEXT NOT NULL,
  payload_hash TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'Pending'
    CHECK (status IN ('Pending','Approved','Rejected','Expired','Executed','Failed')),
  confidence REAL NOT NULL DEFAULT 0,
  rationale TEXT NOT NULL DEFAULT '',
  irreversible INTEGER NOT NULL DEFAULT 0 CHECK (irreversible IN (0,1))
);

CREATE TABLE approvals (
  id TEXT PRIMARY KEY NOT NULL,
  proposal_id TEXT NOT NULL REFERENCES proposals(id),
  payload_hash TEXT NOT NULL,
  approved_at TEXT NOT NULL,
  actor_device_id TEXT NOT NULL,
  execution_id TEXT NOT NULL UNIQUE
);

CREATE TABLE outbox (
  op_id TEXT PRIMARY KEY NOT NULL,
  entity_type TEXT NOT NULL,
  entity_id TEXT NOT NULL,
  base_revision INTEGER NOT NULL,
  field_mask TEXT NOT NULL DEFAULT '*',
  ciphertext BLOB,
  epoch INTEGER NOT NULL DEFAULT 0,
  logical_seq INTEGER NOT NULL,
  status TEXT NOT NULL DEFAULT 'Pending'
    CHECK (status IN ('Pending','Sent','Acked','Quarantined'))
);

CREATE TABLE audit_events (
  id TEXT PRIMARY KEY NOT NULL,
  at TEXT NOT NULL,
  actor TEXT NOT NULL CHECK (actor IN ('User','Ai','Sync','Worker')),
  action TEXT NOT NULL,
  target_type TEXT NOT NULL,
  target_id TEXT,
  outcome TEXT NOT NULL,
  detail_redacted TEXT NOT NULL DEFAULT ''
);
