ALTER TABLE account_usage_records ADD COLUMN query_config TEXT NOT NULL DEFAULT '';
ALTER TABLE account_usage_records ADD COLUMN query_request_id TEXT NOT NULL DEFAULT '';
