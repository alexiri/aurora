-- FIX-107 (SEC-15): `plugin_packages.signer_key_id` holds one spelling per key — the canonical
-- standard-base64 (padded) encoding of the 32-byte public key. Older rows may carry the URL-safe
-- or unpadded spelling the lenient parser used to accept; revocation and `enable` compare the
-- column to `trusted_keys.public_key` as strings, so such a package stayed active after its key
-- was revoked. `builtin` and `unsigned` are not keys and stay as they are.
UPDATE plugin_packages
SET signer_key_id = translate(rtrim(signer_key_id, '='), '-_', '+/') || '='
WHERE signer_key_id NOT IN ('builtin', 'unsigned')
  AND length(rtrim(signer_key_id, '=')) = 43
  AND signer_key_id <> translate(rtrim(signer_key_id, '='), '-_', '+/') || '=';
