-- Extensions required by docs/03-data-model.md.
-- citext: case-insensitive email columns. UUIDs are app-generated (v7), so no uuid-ossp.
CREATE EXTENSION IF NOT EXISTS citext;
