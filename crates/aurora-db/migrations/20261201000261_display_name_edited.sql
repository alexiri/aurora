-- P5-REFINE-5 (D-407, C-1064): people are named by the sign-in's `name` claim until they edit it.
--
-- `display_name_edited_at` is set when the person changes their own name (`PATCH /v1/me`) or a
-- directory writes it (SCIM `displayName`); while it is NULL every OIDC sign-in refreshes the name
-- from the token. Backfill: a name that is not the local part of the e-mail looks chosen and is
-- kept (`display_name_edited_at = created_at`); a name equal to the local part (the bootstrap admin
-- "alice") stays NULL so the next sign-in corrects it. Case-insensitive.
ALTER TABLE users ADD COLUMN display_name_edited_at timestamptz NULL;

UPDATE users
   SET display_name_edited_at = created_at
 WHERE lower(display_name) <> lower(split_part(email::text, '@', 1));
