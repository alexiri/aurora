-- FIX-138: the web terminal's shell choice. The enumerated value (`bash` | `zsh`, never a path)
-- travels API → grant → runner → supervisor; the grant carries it so the runner learns it from
-- the redemption answer, never from the browser. Existing (short-lived) rows read as bash.
ALTER TABLE terminal_stream_grants
    ADD COLUMN shell text NOT NULL DEFAULT 'bash' CHECK (shell IN ('bash', 'zsh'));
