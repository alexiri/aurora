-- FIX-094 (REVIEW-5 LIFE-4, SEC-25): a bootstrap token admits one supervisor connection, but a
-- claim whose response was lost (or whose `channel/hold` then failed) must not strand the boot:
-- re-presenting the token is accepted while the boot has never held a channel. `channel_held_at`
-- records the first hold of the current boot (NULL until then; reset by every new boot).

ALTER TABLE sandboxes ADD COLUMN channel_held_at timestamptz;
