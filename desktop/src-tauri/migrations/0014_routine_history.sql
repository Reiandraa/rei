-- Preserve the old schedule before an owner-selected effective date.
CREATE TABLE routine_schedule_history (
  id TEXT PRIMARY KEY NOT NULL,
  task_id TEXT NOT NULL REFERENCES tasks(id),
  through_date TEXT NOT NULL,
  recurrence_json TEXT NOT NULL,
  UNIQUE(task_id, through_date)
);
