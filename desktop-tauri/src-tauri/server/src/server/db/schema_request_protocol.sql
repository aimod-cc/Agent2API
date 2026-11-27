ALTER TABLE requests ADD COLUMN client_protocol TEXT NOT NULL DEFAULT '';
ALTER TABLE requests ADD COLUMN upstream_protocol TEXT NOT NULL DEFAULT '';
