CREATE TABLE IF NOT EXISTS model_usage_recency (
  model TEXT PRIMARY KEY COLLATE NOCASE,
  last_used_at INTEGER NOT NULL
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS model_usage_recency_backfill (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  before_row_id INTEGER NOT NULL
);
INSERT OR IGNORE INTO model_usage_recency_backfill (id, before_row_id)
SELECT 1, COALESCE(MAX(row_id), 0) FROM requests;

-- 跟随请求事务增量记账；不读取历史明细，也不因流式进度反复写入同一个时间。
CREATE TRIGGER IF NOT EXISTS requests_model_recency_insert
AFTER INSERT ON requests
WHEN NEW.ts > 0 AND NEW.provider <> ''
  AND (NEW.is_test = 0 OR NEW.client IN ('assistant', 'assistant_images'))
BEGIN
  INSERT INTO model_usage_recency (model, last_used_at)
  SELECT LOWER(TRIM(name)), NEW.ts
  FROM (SELECT NEW.model AS name UNION SELECT NEW.upstream_model)
  WHERE TRIM(name) <> ''
  ON CONFLICT(model) DO UPDATE SET last_used_at = excluded.last_used_at
  WHERE excluded.last_used_at > model_usage_recency.last_used_at;
END;

CREATE TRIGGER IF NOT EXISTS requests_model_recency_update
AFTER UPDATE OF model, upstream_model, provider, ts, is_test, client ON requests
WHEN NEW.ts > 0 AND NEW.provider <> ''
  AND (NEW.is_test = 0 OR NEW.client IN ('assistant', 'assistant_images'))
  AND (NEW.model <> OLD.model OR NEW.upstream_model <> OLD.upstream_model
    OR NEW.provider <> OLD.provider OR NEW.ts <> OLD.ts
    OR NEW.is_test <> OLD.is_test OR NEW.client <> OLD.client)
BEGIN
  INSERT INTO model_usage_recency (model, last_used_at)
  SELECT LOWER(TRIM(name)), NEW.ts
  FROM (SELECT NEW.model AS name UNION SELECT NEW.upstream_model)
  WHERE TRIM(name) <> ''
  ON CONFLICT(model) DO UPDATE SET last_used_at = excluded.last_used_at
  WHERE excluded.last_used_at > model_usage_recency.last_used_at;
END;
