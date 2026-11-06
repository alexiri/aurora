-- Spec fix S-04 (docs/90-decisions.md): the CLI device flow names the minted
-- key "CLI on <hostname>"; the hostname is supplied at start and must survive
-- across API replicas until the key is minted on consume.
ALTER TABLE cli_logins ADD COLUMN hostname text CHECK (hostname IS NULL OR char_length(hostname) <= 80);
