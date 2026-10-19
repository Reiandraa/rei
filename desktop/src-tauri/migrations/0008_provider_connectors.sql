-- Provider connectors (docs/PROVIDER-CONNECTORS-NEXT.md). Non-secret configuration,
-- owner-selected sources, IMAP identity mapping and fetch run status. Secrets (HIZ
-- password, Teams refresh token) live only in the OS vault; access tokens only in memory.
CREATE TABLE provider_config (
  account_id TEXT PRIMARY KEY NOT NULL REFERENCES connector_accounts(id) ON DELETE CASCADE,
  provider TEXT NOT NULL CHECK (provider IN ('hiz_imap','teams_graph')),
  -- JSON: HIZ {username, initial_count}; Teams {client_id, tenant}. Never a secret.
  config TEXT NOT NULL CHECK (length(config) <= 2000),
  -- 'saved' only means saved settings; authentication/read status are separate columns.
  secret_saved INTEGER NOT NULL DEFAULT 0 CHECK (secret_saved IN (0,1)),
  authenticated_at TEXT,
  last_read_ok_at TEXT,
  last_attempt_at TEXT,
  last_success_at TEXT,
  last_outcome TEXT CHECK (last_outcome IS NULL OR length(last_outcome) <= 300),
  updated_at TEXT NOT NULL
);

-- Explicit allow-list: HIZ folders, Teams chats and Teams channels. Discovery inserts
-- rows with selected = 0; only owner selection enables fetching.
CREATE TABLE provider_sources (
  account_id TEXT NOT NULL REFERENCES connector_accounts(id) ON DELETE CASCADE,
  source_kind TEXT NOT NULL CHECK (source_kind IN ('folder','chat','channel')),
  source_ref TEXT NOT NULL CHECK (length(source_ref) BETWEEN 1 AND 900),
  display_name TEXT NOT NULL CHECK (length(display_name) <= 300),
  selected INTEGER NOT NULL DEFAULT 0 CHECK (selected IN (0,1)),
  discovered_at TEXT NOT NULL,
  PRIMARY KEY (account_id, source_kind, source_ref)
);

-- IMAP identity: (folder, UIDVALIDITY, UID) -> stored message. Lets deletions and
-- UIDVALIDITY resets be reconciled without confusing old and new UIDs.
CREATE TABLE imap_uids (
  account_id TEXT NOT NULL REFERENCES connector_accounts(id) ON DELETE CASCADE,
  folder TEXT NOT NULL,
  uidvalidity INTEGER NOT NULL,
  uid INTEGER NOT NULL,
  provider_object_id TEXT NOT NULL,
  PRIMARY KEY (account_id, folder, uidvalidity, uid)
);

CREATE TABLE provider_polling (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0,1)),
  interval_minutes INTEGER NOT NULL DEFAULT 30 CHECK (interval_minutes BETWEEN 15 AND 1440),
  updated_at TEXT NOT NULL
);
