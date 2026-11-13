-- FIX-097 (REVIEW-5 PLAT-5): a personal workspace stays personal after its user is deleted.
-- `personal_owner_user_id` is ON DELETE SET NULL, so it cannot be the marker: placement choices
-- listed "workspaces where it is NULL", which made a departed person's workspace (with its
-- conversations and memory) selectable as a shared workspace. `personal` is the sticky marker.

ALTER TABLE workspaces ADD COLUMN personal boolean NOT NULL DEFAULT false;

UPDATE workspaces SET personal = true WHERE personal_owner_user_id IS NOT NULL;
