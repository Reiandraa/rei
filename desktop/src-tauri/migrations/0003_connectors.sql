-- Connectors (P02/P03/P04): accounts, durable cursors and untrusted source messages.
CREATE TABLE connector_accounts (
  id TEXT PRIMARY KEY NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('email','calendar','teams_chat','teams_channel','manual')),
  provider TEXT NOT NULL,
  display_name TEXT NOT NULL CHECK (length(display_name) <= 200),
  state TEXT NOT NULL DEFAULT 'not_configured' CHECK (state IN
    ('not_configured','consent_needed','testing','connected','degraded','expired','permission_denied')),
  evidence TEXT NOT NULL DEFAULT 'none' CHECK (evidence IN ('none','fixture','live')),
  last_error TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  CHECK (state <> 'connected' OR evidence = 'live')
);

CREATE TABLE source_cursors (
  account_id TEXT NOT NULL REFERENCES connector_accounts(id) ON DELETE CASCADE,
  stream TEXT NOT NULL,
  cursor TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (account_id, stream)
);

CREATE TABLE source_messages (
  id TEXT PRIMARY KEY NOT NULL,
  account_id TEXT NOT NULL REFERENCES connector_accounts(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('email','teams_chat','teams_channel','manual')),
  provider_object_id TEXT NOT NULL,
  container_id TEXT,
  parent_id TEXT,
  sender TEXT,
  subject TEXT,
  body_text TEXT NOT NULL,
  received_at TEXT NOT NULL,
  provider_modified_at TEXT,
  label TEXT NOT NULL,
  untrusted INTEGER NOT NULL DEFAULT 1 CHECK (untrusted = 1),
  deleted_at TEXT,
  ingested_at TEXT NOT NULL,
  UNIQUE (account_id, kind, provider_object_id)
);
CREATE INDEX source_messages_recent ON source_messages(account_id, received_at);
