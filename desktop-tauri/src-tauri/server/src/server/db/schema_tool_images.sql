CREATE TABLE IF NOT EXISTS tool_image_jobs (
  id TEXT PRIMARY KEY,
  owner TEXT NOT NULL,
  request_id TEXT NOT NULL,
  request_hash TEXT NOT NULL,
  operation TEXT NOT NULL,
  provider TEXT NOT NULL,
  model TEXT NOT NULL,
  account_id TEXT NOT NULL DEFAULT '',
  state TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  deadline_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  reserved_bytes INTEGER NOT NULL DEFAULT 0,
  result TEXT,
  error TEXT,
  UNIQUE(owner, request_id)
);
CREATE INDEX IF NOT EXISTS idx_tool_image_jobs_state ON tool_image_jobs(state, deadline_at);
CREATE INDEX IF NOT EXISTS idx_tool_image_jobs_owner ON tool_image_jobs(owner, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_tool_image_jobs_expiry ON tool_image_jobs(expires_at);

CREATE TABLE IF NOT EXISTS tool_image_files (
  id TEXT PRIMARY KEY,
  job_id TEXT NOT NULL REFERENCES tool_image_jobs(id) ON DELETE CASCADE,
  mime TEXT NOT NULL,
  width INTEGER NOT NULL,
  height INTEGER NOT NULL,
  bytes BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tool_image_files_job ON tool_image_files(job_id);
