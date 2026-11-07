-- I-33: scopes requested at authorize time; Connection.missing_scopes = requested - scopes (C-42).
ALTER TABLE app_connections ADD COLUMN requested_scopes text[] NOT NULL DEFAULT '{}';
