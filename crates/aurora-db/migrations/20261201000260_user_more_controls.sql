-- P5-REFINE-3 (C-1056, D-406): the person's "Show more controls" switch. It only changes what the
-- dashboard shows; the server never uses it to authorize anything.
ALTER TABLE user_onboarding ADD COLUMN more_controls boolean NOT NULL DEFAULT false;
