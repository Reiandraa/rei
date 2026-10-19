-- P11 sync (docs/ARCHITECTURE.md sync section). Outbox ops are sealed with
-- ChaCha20-Poly1305 under the current key epoch; plaintext never leaves the device.
ALTER TABLE outbox ADD COLUMN nonce BLOB;
ALTER TABLE outbox ADD COLUMN origin_device_id TEXT;

CREATE TABLE sync_devices (
  device_id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL CHECK (length(name) <= 100),
  paired_at TEXT NOT NULL,
  revoked_at TEXT
);

-- Idempotency ledger for remote ops (duplicates/replays are skipped).
CREATE TABLE sync_applied (
  op_id TEXT PRIMARY KEY NOT NULL,
  origin_device_id TEXT NOT NULL,
  relay_seq INTEGER NOT NULL,
  applied_at TEXT NOT NULL,
  outcome TEXT NOT NULL CHECK (outcome IN ('applied','stale','conflict_local','conflict_remote','quarantined'))
);

CREATE TABLE sync_cursor (
  relay_id TEXT PRIMARY KEY NOT NULL,
  last_seq INTEGER NOT NULL
);

CREATE TABLE sync_conflicts (
  id TEXT PRIMARY KEY NOT NULL,
  entity_type TEXT NOT NULL,
  entity_id TEXT NOT NULL,
  local_json TEXT NOT NULL,
  remote_json TEXT NOT NULL,
  winner TEXT NOT NULL CHECK (winner IN ('local','remote')),
  reason TEXT NOT NULL,
  created_at TEXT NOT NULL,
  resolved_at TEXT
);
