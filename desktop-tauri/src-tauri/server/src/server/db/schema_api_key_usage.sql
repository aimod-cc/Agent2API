CREATE TABLE IF NOT EXISTS api_key_usage_events (
  row_id INTEGER PRIMARY KEY AUTOINCREMENT,
  id TEXT NOT NULL UNIQUE,
  key_id TEXT NOT NULL,
  ts INTEGER NOT NULL,
  tokens INTEGER NOT NULL DEFAULT 0,
  requests INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS idx_api_key_usage_events_key_ts ON api_key_usage_events(key_id, ts);
