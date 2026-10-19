-- Local-only cited summaries. No task/approval/source or cloud outbox mutations.
CREATE TABLE assistant_digest_state (
 id INTEGER PRIMARY KEY CHECK(id=1), enabled INTEGER NOT NULL DEFAULT 0,
 state TEXT NOT NULL DEFAULT 'idle', updated_at TEXT, last_error TEXT NOT NULL DEFAULT '', retry_at TEXT,
 attempt_key TEXT NOT NULL DEFAULT '', attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts>=0)
);
INSERT INTO assistant_digest_state(id) VALUES(1);
CREATE TABLE assistant_message_summaries (
 source_id TEXT NOT NULL REFERENCES source_messages(id) ON DELETE CASCADE, cache_key TEXT NOT NULL,
 importance TEXT NOT NULL CHECK(importance IN ('important','routine','uncertain','failed')),
 points_json TEXT NOT NULL, partial INTEGER NOT NULL CHECK(partial IN(0,1)),
 detail TEXT NOT NULL DEFAULT '', updated_at TEXT NOT NULL,
 PRIMARY KEY(source_id,cache_key)
);
