-- P4-SSH-2 (D-239(h); C-796): the SSH keep-awake clock and its per-boot cap clock. Written by the
-- runner's keep-awake call (`kind: "ssh"`) while an SSH channel carries bytes, at most one stamp per
-- grace; read by the keep-awake predicate. `ssh_awake_since` is reset at every start claim (the same
-- shape as the terminal and files columns). The SSH wake demand reuses `terminal_wake_at` and the
-- terminal's wake budget.
ALTER TABLE sandboxes
    ADD COLUMN ssh_active_until timestamptz,
    ADD COLUMN ssh_awake_since  timestamptz;
