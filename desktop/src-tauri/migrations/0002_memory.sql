-- P06 memory: consented, provenance-tracked local memories with FTS and embeddings.
CREATE TABLE memories (
  id TEXT PRIMARY KEY NOT NULL,
  text TEXT NOT NULL CHECK (length(text) BETWEEN 1 AND 2000),
  provenance TEXT NOT NULL CHECK (provenance IN ('User','AiProposal','Import')),
  source_ids TEXT NOT NULL DEFAULT '[]',
  consent_at TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  expires_at TEXT
);
CREATE INDEX memories_expires ON memories(expires_at) WHERE expires_at IS NOT NULL;

CREATE VIRTUAL TABLE memory_fts USING fts5(id UNINDEXED, text, tokenize = 'unicode61');

CREATE TABLE memory_embeddings (
  memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
  model TEXT NOT NULL,
  version INTEGER NOT NULL,
  dims INTEGER NOT NULL,
  vector BLOB NOT NULL,
  PRIMARY KEY (memory_id, model)
);

CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
  INSERT INTO memory_fts(id, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER memories_au AFTER UPDATE OF text ON memories BEGIN
  DELETE FROM memory_fts WHERE id = old.id;
  INSERT INTO memory_fts(id, text) VALUES (new.id, new.text);
  DELETE FROM memory_embeddings WHERE memory_id = new.id;
END;
CREATE TRIGGER memories_ad AFTER DELETE ON memories BEGIN
  DELETE FROM memory_fts WHERE id = old.id;
END;
