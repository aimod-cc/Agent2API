-- 定时任务类插件、任务实例与执行记录（schema v33）
CREATE TABLE IF NOT EXISTS task_plugins (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  version TEXT NOT NULL DEFAULT '0.1.0',
  source TEXT NOT NULL DEFAULT 'local',
  signed INTEGER NOT NULL DEFAULT 0,
  enabled INTEGER NOT NULL DEFAULT 1,
  icon TEXT NOT NULL DEFAULT '',
  default_cron TEXT NOT NULL DEFAULT '0 9 * * *',
  default_timezone TEXT NOT NULL DEFAULT 'Asia/Shanghai',
  default_timeout INTEGER NOT NULL DEFAULT 30,
  domains_json TEXT NOT NULL DEFAULT '[]',
  declares_notify INTEGER NOT NULL DEFAULT 0,
  notify_allowed INTEGER NOT NULL DEFAULT 1,
  fields_json TEXT NOT NULL DEFAULT '[]',
  actions_json TEXT NOT NULL DEFAULT '[]',
  status_fields_json TEXT NOT NULL DEFAULT '[]',
  script_body TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS task_jobs (
  id TEXT PRIMARY KEY,
  plugin_id TEXT NOT NULL,
  name TEXT NOT NULL,
  cron_expression TEXT NOT NULL,
  timezone TEXT NOT NULL DEFAULT 'Asia/Shanghai',
  timeout_seconds INTEGER NOT NULL DEFAULT 30,
  proxy_id TEXT NOT NULL DEFAULT '',
  enabled INTEGER NOT NULL DEFAULT 1,
  values_json TEXT NOT NULL DEFAULT '{}',
  secrets_enc TEXT NOT NULL DEFAULT '',
  storage_enc TEXT NOT NULL DEFAULT '',
  status_summary_json TEXT NOT NULL DEFAULT '{}',
  next_run_at INTEGER,
  last_run_at INTEGER,
  last_status TEXT,
  last_duration_ms INTEGER,
  last_error TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_task_jobs_due
  ON task_jobs(enabled, next_run_at);

CREATE INDEX IF NOT EXISTS idx_task_jobs_plugin
  ON task_jobs(plugin_id);

CREATE TABLE IF NOT EXISTS task_job_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  job_id TEXT NOT NULL,
  trigger_type TEXT NOT NULL,
  trigger_label TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL,
  started_at INTEGER NOT NULL,
  finished_at INTEGER,
  duration_ms INTEGER,
  output TEXT,
  error TEXT,
  FOREIGN KEY (job_id) REFERENCES task_jobs(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_task_job_runs_job
  ON task_job_runs(job_id, id DESC);
