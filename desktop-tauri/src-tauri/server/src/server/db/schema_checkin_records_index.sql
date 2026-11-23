CREATE INDEX IF NOT EXISTS idx_checkin_records_ts_rowid
  ON checkin_records(ts DESC, row_id DESC);
