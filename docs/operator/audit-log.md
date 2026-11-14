# Instance audit log

(FIX-125, C-638.) Every security-relevant action writes an audit row (SEC-32). Organization and workspace
admins read the rows of their own organization on its audit page. Actions that belong to no organization,
such as managing connector-host tokens ([plugins-and-connectors.md](plugins-and-connectors.md#connector-host-tokens)),
trusted keys or instance settings, have `org_id` empty: **instance administrators** read those in the dashboard at
**Admin → Audit** (`/admin/audit`).
The normative text is `docs/04 §Audit` and the route contract is C-638 in [API_CONTRACTS](../API_CONTRACTS.md);
the dashboard page is described in `docs/14 §Admin`.

## What the page shows

- **Scope.** *Instance* (the default) lists the rows without an organization. *All organizations* lists every row and adds
  the organization column; choosing one organization in the filter implies *All organizations*.
- **Filters.** Action prefix (for example `admin.connector_host_token`), actor, *security-critical only*, and
  from/to dates (`since` inclusive, `until` exclusive). The list is paged by a keyset cursor, newest first.
- **Security-critical rows** are written in the same transaction as the change; if the audit write fails, the
  action fails with 503 `audit_unavailable` and changes nothing (`docs/17`, SEC-32).

## API

`GET /v1/admin/audit` (instance admin, session): `scope=instance|all`, `org`, `actor_id`, `action`, `critical`, `since`, `until`
and the cursor. An unknown `scope` is 400 `invalid_request`. The per-organization and per-workspace audit routes are
unchanged. The upgrade adds indexes on `(created_at desc, id desc)`, so the query stays cheap on a large table; no
configuration is needed.
