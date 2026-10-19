-- 0005_study_life: P07 study, P08 life/admin, P09 projects/music, dialog-selected file grants.
-- Common columns follow docs/INTERFACES.md §3. Money is integer minor units + ISO 4217 currency.
CREATE TABLE file_grants (
  id TEXT PRIMARY KEY NOT NULL, revision INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL, origin_device_id TEXT NOT NULL, deleted_at TEXT,
  path TEXT NOT NULL,                -- canonical path chosen through the OS dialog
  purpose TEXT NOT NULL CHECK (purpose IN ('document','link','import','export')),
  revoked_at TEXT
);
CREATE UNIQUE INDEX file_grants_live ON file_grants(path, purpose) WHERE revoked_at IS NULL AND deleted_at IS NULL;
CREATE TABLE courses (
  id TEXT PRIMARY KEY NOT NULL, revision INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL, origin_device_id TEXT NOT NULL, deleted_at TEXT,
  name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
  code TEXT NOT NULL DEFAULT '', term TEXT NOT NULL DEFAULT ''
);
CREATE TABLE study_items (
  id TEXT PRIMARY KEY NOT NULL, revision INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL, origin_device_id TEXT NOT NULL, deleted_at TEXT,
  course_id TEXT NOT NULL REFERENCES courses(id),
  kind TEXT NOT NULL CHECK (kind IN ('assignment','exam','homework','note')),
  title TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 500),
  due_date TEXT,                       -- only user-entered or source-quoted; never guessed
  status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','done','cancelled')),
  body TEXT NOT NULL DEFAULT '' CHECK (length(body) <= 65536)
);
CREATE TABLE study_sessions (
  id TEXT PRIMARY KEY NOT NULL, revision INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL, origin_device_id TEXT NOT NULL, deleted_at TEXT,
  course_id TEXT REFERENCES courses(id),
  topic TEXT NOT NULL CHECK (length(topic) BETWEEN 1 AND 500),
  minutes INTEGER NOT NULL CHECK (minutes BETWEEN 1 AND 1440),
  day TEXT NOT NULL
);
CREATE TABLE language_logs (
  id TEXT PRIMARY KEY NOT NULL, revision INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL, origin_device_id TEXT NOT NULL, deleted_at TEXT,
  language TEXT NOT NULL CHECK (length(language) BETWEEN 1 AND 60),
  activity TEXT NOT NULL CHECK (length(activity) BETWEEN 1 AND 200),
  minutes INTEGER NOT NULL CHECK (minutes BETWEEN 1 AND 1440),
  day TEXT NOT NULL, notes TEXT NOT NULL DEFAULT '' CHECK (length(notes) <= 8192)
);
-- HTB progress is bookkeeping for authorized labs; the app never runs offensive tools.
CREATE TABLE htb_progress (
  id TEXT PRIMARY KEY NOT NULL, revision INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL, origin_device_id TEXT NOT NULL, deleted_at TEXT,
  name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
  kind TEXT NOT NULL CHECK (kind IN ('machine','challenge','module','track')),
  status TEXT NOT NULL DEFAULT 'planned' CHECK (status IN ('planned','in_progress','completed')),
  notes TEXT NOT NULL DEFAULT '' CHECK (length(notes) <= 8192)
);
CREATE TABLE anki_cards (
  id TEXT PRIMARY KEY NOT NULL, revision INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL, origin_device_id TEXT NOT NULL, deleted_at TEXT,
  deck TEXT NOT NULL CHECK (length(deck) BETWEEN 1 AND 200),
  front TEXT NOT NULL CHECK (length(front) BETWEEN 1 AND 8192),
  back TEXT NOT NULL CHECK (length(back) <= 8192),
  tags TEXT NOT NULL DEFAULT ''
);
CREATE UNIQUE INDEX anki_cards_key ON anki_cards(deck, front) WHERE deleted_at IS NULL;

CREATE TABLE expenses (
  id TEXT PRIMARY KEY NOT NULL, revision INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL, origin_device_id TEXT NOT NULL, deleted_at TEXT,
  amount_minor INTEGER NOT NULL CHECK (amount_minor > 0),
  currency TEXT NOT NULL CHECK (length(currency) = 3 AND currency = upper(currency)),
  category TEXT NOT NULL CHECK (length(category) BETWEEN 1 AND 100),
  spent_on TEXT NOT NULL, note TEXT NOT NULL DEFAULT '' CHECK (length(note) <= 2048),
  document_grant_id TEXT REFERENCES file_grants(id)
);
CREATE TABLE budgets (
  id TEXT PRIMARY KEY NOT NULL, revision INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL, origin_device_id TEXT NOT NULL, deleted_at TEXT,
  category TEXT NOT NULL CHECK (length(category) BETWEEN 1 AND 100),
  currency TEXT NOT NULL CHECK (length(currency) = 3 AND currency = upper(currency)),
  month TEXT NOT NULL CHECK (month GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]'),
  limit_minor INTEGER NOT NULL CHECK (limit_minor >= 0)
);
CREATE UNIQUE INDEX budgets_key ON budgets(category, currency, month) WHERE deleted_at IS NULL;
CREATE TABLE income (
  id TEXT PRIMARY KEY NOT NULL, revision INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL, origin_device_id TEXT NOT NULL, deleted_at TEXT,
  amount_minor INTEGER NOT NULL CHECK (amount_minor > 0),
  currency TEXT NOT NULL CHECK (length(currency) = 3 AND currency = upper(currency)),
  source TEXT NOT NULL CHECK (length(source) BETWEEN 1 AND 200),
  received_on TEXT NOT NULL
);
CREATE TABLE shifts (
  id TEXT PRIMARY KEY NOT NULL, revision INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL, origin_device_id TEXT NOT NULL, deleted_at TEXT,
  employer TEXT NOT NULL CHECK (length(employer) BETWEEN 1 AND 200),
  shift_date TEXT NOT NULL,
  worked_min INTEGER NOT NULL CHECK (worked_min BETWEEN 1 AND 1440),
  break_min INTEGER NOT NULL DEFAULT 0 CHECK (break_min >= 0),
  rate_minor_per_hour INTEGER NOT NULL CHECK (rate_minor_per_hour >= 0),
  currency TEXT NOT NULL CHECK (length(currency) = 3 AND currency = upper(currency))
);
CREATE TABLE admin_cases (
  id TEXT PRIMARY KEY NOT NULL, revision INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL, origin_device_id TEXT NOT NULL, deleted_at TEXT,
  title TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 500),
  kind TEXT NOT NULL CHECK (kind IN ('rent','fintiba','immigration','insurance','rundfunkbeitrag','other')),
  due_date TEXT,
  date_source TEXT CHECK (date_source IN ('user','source')),
  source_ref TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','done','cancelled')),
  notes TEXT NOT NULL DEFAULT '' CHECK (length(notes) <= 65536),
  document_grant_id TEXT REFERENCES file_grants(id),
  CHECK ((due_date IS NULL) = (date_source IS NULL))   -- every deadline names where it came from
);

CREATE TABLE milestones (
  id TEXT PRIMARY KEY NOT NULL, revision INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL, origin_device_id TEXT NOT NULL, deleted_at TEXT,
  project_id TEXT NOT NULL REFERENCES projects(id),
  title TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 500),
  state TEXT NOT NULL DEFAULT 'backlog' CHECK (state IN ('backlog','active','done')),
  due_date TEXT, notes TEXT NOT NULL DEFAULT '' CHECK (length(notes) <= 65536)
);
CREATE TABLE practice_sessions (
  id TEXT PRIMARY KEY NOT NULL, revision INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL, origin_device_id TEXT NOT NULL, deleted_at TEXT,
  kind TEXT NOT NULL CHECK (kind IN ('music','coding','htb')),
  title TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 200),
  state TEXT NOT NULL CHECK (state IN ('running','paused','stopped')),
  accumulated_s INTEGER NOT NULL DEFAULT 0 CHECK (accumulated_s >= 0),
  resumed_at TEXT,                     -- set only while running; survives app restart
  stopped_at TEXT,
  CHECK ((state = 'running') = (resumed_at IS NOT NULL))
);
-- At most one live (running/paused) timer: a concurrent start is a conflict, not a second timer.
CREATE UNIQUE INDEX practice_one_live ON practice_sessions((1)) WHERE state IN ('running','paused') AND deleted_at IS NULL;
CREATE TABLE file_links (
  id TEXT PRIMARY KEY NOT NULL, revision INTEGER NOT NULL DEFAULT 1, created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL, origin_device_id TEXT NOT NULL, deleted_at TEXT,
  owner_type TEXT NOT NULL CHECK (owner_type IN ('project','milestone','practice_session','course','study_item')),
  owner_id TEXT NOT NULL,
  grant_id TEXT NOT NULL REFERENCES file_grants(id),
  label TEXT NOT NULL DEFAULT '' CHECK (length(label) <= 200)
);
