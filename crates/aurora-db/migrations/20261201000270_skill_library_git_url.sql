-- (P5-SKLIB-1, D-448, C-1126, C-1127; 03 §skill_libraries) A skill library is an https git URL
-- read by the API itself, with an optional sealed read-only token; no app connection. Additive:
-- `connection_id` and `app_id` stay (unused) until a later cleanup; nothing is dropped.

ALTER TABLE skill_libraries
    ADD COLUMN url text CHECK (url IS NULL OR char_length(url) BETWEEN 9 AND 500),
    -- The sealed `{ username, token }` (purpose `skill_library.token`, owner = the library id).
    ADD COLUMN token_ref text,
    ADD COLUMN token_updated_at timestamptz,
    ADD CONSTRAINT skill_libraries_token_ck
        CHECK ((token_ref IS NULL) = (token_updated_at IS NULL));

ALTER TABLE skill_libraries ALTER COLUMN app_id DROP NOT NULL;
ALTER TABLE skill_libraries ALTER COLUMN repo DROP NOT NULL;
-- `repo` becomes the display form `host/path` (longer than the old `owner/name`).
ALTER TABLE skill_libraries DROP CONSTRAINT skill_libraries_repo_check;
ALTER TABLE skill_libraries ADD CONSTRAINT skill_libraries_repo_check
    CHECK (repo IS NULL OR char_length(repo) BETWEEN 1 AND 500);

-- Backfill (D-448(e)): GitHub and GitLab libraries get their clone URL on the host their
-- connection used: the registered self-hosted instance (host and base path) when it has one,
-- else github.com / gitlab.com (a GitLab library whose connection is gone has no known host).
-- Where two libraries of one organization would get the same url + ref + path, only the oldest
-- does (the unique index below); the others stay without one.
UPDATE skill_libraries l
SET url = 'https://' || h.base || '/' || l.repo || '.git',
    repo = h.base || '/' || l.repo,
    updated_at = now()
FROM (
    SELECT x.id, x.base,
           row_number() OVER (PARTITION BY x.org_id, x.base, x.repo, coalesce(x.ref, ''), x.path
                              ORDER BY x.created_at, x.id) AS n
    FROM (
        SELECT s.id, s.org_id, s.repo, s.ref, s.path, s.created_at,
               CASE
                   WHEN i.id IS NOT NULL THEN i.host || coalesce(i.base_path, '')
                   WHEN s.app_id = 'github' THEN 'github.com'
                   WHEN s.app_id = 'gitlab' AND c.id IS NOT NULL THEN 'gitlab.com'
               END AS base
        FROM skill_libraries s
        LEFT JOIN app_connections c ON c.id = s.connection_id
        LEFT JOIN app_instances i ON i.id = c.instance_id
        WHERE s.app_id IN ('github', 'gitlab')
    ) x
) h
WHERE h.id = l.id
  AND h.base IS NOT NULL
  AND h.n = 1
  AND char_length('https://' || h.base || '/' || l.repo || '.git') <= 500;

-- Every other library needs an address from an admin (the one case PATCH may set `url`).
UPDATE skill_libraries
SET status = 'error', last_error = 'Enter the repository''s git address', updated_at = now()
WHERE url IS NULL;

CREATE UNIQUE INDEX skill_libraries_url_uq
    ON skill_libraries (org_id, url, coalesce(ref, ''), path) WHERE url IS NOT NULL;
