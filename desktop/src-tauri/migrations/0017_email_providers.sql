-- Extend provider kinds while preserving every legacy account/status/config row.
-- No child table references provider_config; all reference connector_accounts.
CREATE TABLE provider_config_email (
  account_id TEXT PRIMARY KEY NOT NULL REFERENCES connector_accounts(id) ON DELETE CASCADE,
  provider TEXT NOT NULL CHECK (provider IN ('hiz_imap','teams_graph','imap_email','google_mail','microsoft_mail')),
  config TEXT NOT NULL CHECK (length(config) <= 8000),
  secret_saved INTEGER NOT NULL DEFAULT 0 CHECK (secret_saved IN (0,1)),
  authenticated_at TEXT,
  last_read_ok_at TEXT,
  last_attempt_at TEXT,
  last_success_at TEXT,
  last_outcome TEXT CHECK (last_outcome IS NULL OR length(last_outcome) <= 300),
  updated_at TEXT NOT NULL
);
INSERT INTO provider_config_email
  (account_id,provider,config,secret_saved,authenticated_at,last_read_ok_at,last_attempt_at,last_success_at,last_outcome,updated_at)
SELECT account_id,provider,config,secret_saved,authenticated_at,last_read_ok_at,last_attempt_at,last_success_at,last_outcome,updated_at
FROM provider_config;
DROP TABLE provider_config;
ALTER TABLE provider_config_email RENAME TO provider_config;
