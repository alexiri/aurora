-- P4-NARR (D-279, C-846): per-agent opt-in for live narration cards on chat platforms.
-- Off by default; existing agents read `false`. Takes the free number 000231 of the block
-- the coordinator reserved (docs/03 §Planned changes).

ALTER TABLE agents
    ADD COLUMN narration_cards boolean NOT NULL DEFAULT false;
