-- Preserve existing series; an empty map retains their single DTSTART wall time.
ALTER TABLE recurrence_rules ADD COLUMN weekday_times_json TEXT NOT NULL DEFAULT '{}'
  CHECK (json_valid(weekday_times_json) AND json_type(weekday_times_json) = 'object');
