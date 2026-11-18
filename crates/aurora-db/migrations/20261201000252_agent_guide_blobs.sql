-- (phase 5, P5-GUIDE-1, C-642) Agent guide files are stored like every package blob, keyed by the
-- sha256 of their bytes: the kind vocabulary gains 'agent_guide'. Forward-only; no data change.
ALTER TABLE plugin_blobs DROP CONSTRAINT plugin_blobs_kind_check;
ALTER TABLE plugin_blobs ADD CONSTRAINT plugin_blobs_kind_check
    CHECK (kind IN ('hooks_wasm', 'icon', 'icon_dark', 'setup_guide', 'archive', 'agent_guide'));
