CREATE TABLE IF NOT EXISTS coding_submissions (
    scope TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    local_id TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    state TEXT NOT NULL,
    turn_id TEXT,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (scope, thread_id, local_id)
);
CREATE INDEX IF NOT EXISTS idx_coding_submissions_thread ON coding_submissions(scope, thread_id, updated_at);

CREATE TABLE IF NOT EXISTS coding_thread_settings (
    scope TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    sandbox TEXT NOT NULL,
    PRIMARY KEY (scope, thread_id)
);
