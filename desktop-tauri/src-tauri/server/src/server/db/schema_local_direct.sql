ALTER TABLE requests ADD COLUMN origin TEXT NOT NULL DEFAULT 'gateway';

CREATE INDEX IF NOT EXISTS idx_requests_origin_ts ON requests(origin, ts DESC, row_id DESC);

CREATE TABLE IF NOT EXISTS local_direct_refs (
    request_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    root_kind TEXT NOT NULL DEFAULT '',
    source_key TEXT NOT NULL UNIQUE,
    conversation_id TEXT NOT NULL DEFAULT '',
    conversation_title TEXT NOT NULL DEFAULT '',
    workspace_uri TEXT NOT NULL DEFAULT '',
    db_path TEXT NOT NULL DEFAULT '',
    transcript_path TEXT NOT NULL DEFAULT '',
    step_index INTEGER NOT NULL DEFAULT 0,
    response_id TEXT NOT NULL DEFAULT '',
    ts INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_local_direct_refs_agent_ts ON local_direct_refs(agent_id, ts DESC);
CREATE INDEX IF NOT EXISTS idx_local_direct_refs_conv ON local_direct_refs(conversation_id);

CREATE TABLE IF NOT EXISTS local_direct_files (
    file_path TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    mtime_ms INTEGER NOT NULL DEFAULT 0,
    size_bytes INTEGER NOT NULL DEFAULT 0,
    last_step_idx INTEGER NOT NULL DEFAULT -1,
    updated_at INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_local_direct_files_agent ON local_direct_files(agent_id);
