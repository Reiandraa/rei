-- 0006_notifications: opt-in categories, quiet hours, receipts (dedupe), worker leases, briefings.
CREATE TABLE notification_prefs (
  category TEXT PRIMARY KEY NOT NULL
    CHECK (category IN ('morning_brief','deadline','weekly_recap','inbox','teams')),
  enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0,1)),
  updated_at TEXT NOT NULL
);

CREATE TABLE notification_config (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  quiet_enabled INTEGER NOT NULL DEFAULT 0 CHECK (quiet_enabled IN (0,1)),
  quiet_start TEXT NOT NULL DEFAULT '22:00',
  quiet_end TEXT NOT NULL DEFAULT '07:00',
  tz TEXT NOT NULL DEFAULT 'UTC',
  catch_up_window_min INTEGER NOT NULL DEFAULT 720 CHECK (catch_up_window_min > 0),
  updated_at TEXT NOT NULL
);

CREATE TABLE notification_receipts (
  id TEXT PRIMARY KEY NOT NULL,
  dedupe_key TEXT NOT NULL UNIQUE,
  category TEXT NOT NULL,
  title TEXT NOT NULL CHECK (length(title) <= 500),
  body TEXT NOT NULL DEFAULT '' CHECK (length(body) <= 4000),
  due_at TEXT NOT NULL,
  created_at TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'Pending'
    CHECK (status IN ('Pending','Delivered','Missed','PermissionDenied','Dismissed')),
  snoozed_until TEXT,
  delivered_at TEXT,
  delivered_by_device TEXT
);
CREATE INDEX notification_receipts_pending ON notification_receipts(status, due_at);

CREATE TABLE worker_leases (
  name TEXT PRIMARY KEY NOT NULL,
  device_id TEXT NOT NULL,
  epoch INTEGER NOT NULL DEFAULT 1,
  acquired_at TEXT NOT NULL,
  expires_at TEXT NOT NULL
);

CREATE TABLE briefings (
  id TEXT PRIMARY KEY NOT NULL,
  local_date TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL CHECK (kind IN ('morning','weekly')),
  body TEXT NOT NULL,
  generated_at TEXT NOT NULL,
  generated_by_device TEXT NOT NULL,
  source TEXT NOT NULL DEFAULT 'deterministic'
);
