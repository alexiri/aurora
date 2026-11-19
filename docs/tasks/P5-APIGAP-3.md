# P5-APIGAP-3: Members see their key's default model; organization admins see which keys new agents get

## Goal
Owner, 2026-11-19 (D-399). Two read gaps the redesign hit (P5-POLISH-11 pass 2):
- A member's AI key row (Organization › AI models member view, P5-POLISH-9) was designed as "<key> · default
  <model>", but members can read no endpoint default: `ModelOption` has none and `/v1/secrets/{id}/models` is
  owner/admin only.
- Organization › Accounts was designed to say "New agents get this key automatically." for the organization's
  starter key, but starter grants are only in `/v1/admin/placement` (instance admins).

## Change
1. **Default model (C-1048):** where a member already lists the AI keys/models they may use (find the route the
   member view reads), each key gains `default_model: string | null`: the model their agents get on that key with
   no override (the endpoint's default after D-372's rule: the configured default when the key serves it, else the
   first chat model the key serves; null when unknown). Members learn only a model name of a key they can already
   use; no endpoint URL, no other key's data.
2. **Starter key (C-1049):** the organization's account listing (as org owners/admins read it) gains
   `starter: bool` on an AI key / account that the instance's starter grants give every new agent in this
   organization. Read-only; changing starter grants stays instance-admin only (`/v1/admin/placement`).
3. **Web:** the member key row shows "default <model>" (P5-POLISH-9 placeholder); org Accounts shows "New agents
   get this key automatically." on a starter key (P5-POLISH-7/-9 placeholders). Regenerate the API types.
4. Docs: `05`, `API_CONTRACTS` C-1048/C-1049, `14`.

## Tests
API ITs: a member sees `default_model` for a key they can use and nothing for one they cannot (403/404 or absent,
as the route's existing rules); a LiteLLM key whose configured default is not served falls back as D-372; an org
admin sees `starter: true` exactly on the starter key; a member route never exposes `starter` data it did not
before. Vitest for both lines. Targeted only (CI is the gate); `extract-schema --check`.

## Ownership
The model/key listing and organization accounts views in `services/aurora-api`, the web member AI models row and
org Accounts row. Size S. D-399, C-1048, C-1049.
