-- Rebuildable, local-only advisory labels. Facts/model/zone/date are hashed per day.
-- Persisting the cache avoids reclassifying unchanged dates after a restart.
CREATE TABLE calendar_label_cache (
  cache_key TEXT PRIMARY KEY NOT NULL,
  label TEXT NOT NULL CHECK (label IN ('School','Work','Holiday','Personal','Mixed'))
);
