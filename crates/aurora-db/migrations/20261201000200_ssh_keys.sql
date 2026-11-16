-- P4-SSH-1 (D-239(b), SEC-93; C-790): a person's registered SSH public keys. Owned by the person,
-- not by an org or workspace. Only `ssh-ed25519` is accepted (the CHECK mirrors the service rule);
-- `public_key` is the canonical `ssh-ed25519 <base64>` rebuilt from the parsed key (never the
-- pasted line: comment and options are dropped) and `fingerprint` the server-computed
-- `SHA256:<unpadded base64>` that `ssh-keygen -lf` prints. A row grants nothing by itself: a
-- certificate also needs the private key, so two people may register the same public key. The
-- gateway database role gets nothing.
CREATE TABLE ssh_keys (
    id           uuid PRIMARY KEY,
    user_id      uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name         text NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    algorithm    text NOT NULL CHECK (algorithm IN ('ssh-ed25519')),
    public_key   text NOT NULL,
    fingerprint  text NOT NULL,
    -- Stamped at a certificate mint (and at a session open, P4-SSH-2), at most once a minute.
    last_used_at timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, fingerprint)
);
CREATE INDEX ssh_keys_user_idx ON ssh_keys (user_id, created_at DESC, id DESC);
