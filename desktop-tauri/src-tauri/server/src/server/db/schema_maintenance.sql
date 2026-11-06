CREATE TABLE IF NOT EXISTS maintenance_runs (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    source TEXT NOT NULL,
    retry_of TEXT,
    started_at INTEGER NOT NULL,
    heartbeat_at INTEGER NOT NULL,
    finished_at INTEGER,
    duration_ms INTEGER,
    status TEXT NOT NULL,
    refreshed INTEGER NOT NULL DEFAULT 0,
    skipped INTEGER NOT NULL DEFAULT 0,
    failed INTEGER NOT NULL DEFAULT 0,
    total INTEGER NOT NULL DEFAULT 0,
    error TEXT,
    metadata TEXT
);
CREATE INDEX IF NOT EXISTS idx_maintenance_runs_time ON maintenance_runs(started_at DESC, id DESC);
CREATE INDEX IF NOT EXISTS idx_maintenance_runs_task ON maintenance_runs(task_id, started_at DESC);
CREATE TABLE IF NOT EXISTS maintenance_items (
    run_id TEXT NOT NULL REFERENCES maintenance_runs(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    provider TEXT NOT NULL,
    account_id TEXT,
    account_name TEXT,
    status TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (run_id, position)
);
CREATE INDEX IF NOT EXISTS idx_maintenance_items_account ON maintenance_items(account_id, run_id);
