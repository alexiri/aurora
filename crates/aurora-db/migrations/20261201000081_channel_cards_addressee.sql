-- FIX-112: a card DMed to one person records who it was sent to. `space_ref` is overwritten with the
-- real DM channel once a click reveals it, which used to drop the addressee and with it the
-- ownership check of DM menus. Cards whose space was already learned keep NULL and fail closed.
ALTER TABLE channel_cards ADD COLUMN addressee_ref text;
UPDATE channel_cards SET addressee_ref = substr(space_ref, 6) WHERE left(space_ref, 5) = 'user:';
