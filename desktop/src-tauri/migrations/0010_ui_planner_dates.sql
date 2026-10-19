-- Contextual deadlines and work plans (docs/UI-PLANNER-DATES-NEXT.md). Additive only:
-- existing rows, decisions and payload hashes stay valid (resolver_version 1 = legacy).
ALTER TABLE source_candidates ADD COLUMN due_time TEXT CHECK (due_time IS NULL OR length(due_time) = 5);
ALTER TABLE source_candidates ADD COLUMN date_basis TEXT CHECK (date_basis IS NULL OR date_basis IN ('written','relative'));
ALTER TABLE source_candidates ADD COLUMN date_anchor TEXT;
ALTER TABLE source_candidates ADD COLUMN date_zone TEXT;
ALTER TABLE source_candidates ADD COLUMN date_note TEXT NOT NULL DEFAULT '' CHECK (length(date_note) <= 200);
ALTER TABLE source_candidates ADD COLUMN resolver_version INTEGER NOT NULL DEFAULT 1;
-- Explicit owner zone for interpreting relative wording; source content cannot change it.
ALTER TABLE auto_analysis_settings ADD COLUMN date_zone TEXT NOT NULL DEFAULT 'Europe/Berlin';
-- Work plan (when the owner intends to work on it), separate from the hard deadline.
ALTER TABLE tasks ADD COLUMN plan_date TEXT;
ALTER TABLE tasks ADD COLUMN plan_time TEXT CHECK (plan_time IS NULL OR length(plan_time) = 5);
ALTER TABLE tasks ADD COLUMN plan_duration_min INTEGER CHECK (plan_duration_min IS NULL OR plan_duration_min BETWEEN 5 AND 1440);
ALTER TABLE tasks ADD COLUMN plan_tz TEXT;
CREATE INDEX tasks_by_plan_date ON tasks(plan_date) WHERE plan_date IS NOT NULL AND deleted_at IS NULL;
-- Bounded Undo: prior state of the latest complete/delete/plan/unplan per task revision.
CREATE TABLE task_undo (
  id TEXT PRIMARY KEY NOT NULL,
  task_id TEXT NOT NULL REFERENCES tasks(id),
  action TEXT NOT NULL CHECK (action IN ('complete','delete','plan','unplan')),
  prior_json TEXT NOT NULL CHECK (length(prior_json) <= 2000),
  after_revision INTEGER NOT NULL,
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  used_at TEXT
);
CREATE INDEX task_undo_by_task ON task_undo(task_id, after_revision);
