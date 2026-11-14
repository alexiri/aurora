# Retention and attachment quota

(Phase 2, RFE-010 and RFE-015.) Conversations, transcript events and attachments are kept until someone deletes them, unless an organization sets a retention window.
The instance settings below are **ceilings and defaults**: organizations choose their own values inside them, and the instance can only tighten what organizations choose.
The normative text is [docs/10 §Archive, delete, redaction, retention, export](../10-agents-conversations.md#archive-delete-redaction-retention-export-phase-2-rfe-010-d-53) and
[docs/10 §Attachments](../10-agents-conversations.md#attachments); the key table is [docs/15](../15-deployment-operations.md#configuration-reference-keys-and-defaults).

## Instance settings

| Key | Default | Range | Meaning |
|-----|---------|-------|---------|
| `retention.conversation_days_max` | unset (no ceiling) | 7 to 3650 days | The longest conversation retention window an organization may choose. |
| `attachments.days_max` | unset (no ceiling) | 7 to 3650 days | The same for attachment bytes. |
| `attachments.default_org_quota_mib` | `10240` (10 GiB) | `0` = unlimited | Attachment storage quota of an organization that has set none. |
| `attachments.org_quota_mib_max` | `51200` (50 GiB) | `0` = no ceiling | The highest quota an organization may choose. |
| `retention.request_log_days` | `30` | at least 1 | How long the gateway request log is kept (see [backups.md](backups.md)). Rows are deleted when older than this, checked daily (not only whole months). |

An out-of-range value stops the API at startup and names the key. Both quota keys are limited to 1 073 741 824 MiB.

```toml
[retention]
conversation_days_max = 365

[attachments]
default_org_quota_mib = 5120      # 5 GiB
org_quota_mib_max = 20480         # 20 GiB
days_max = 180
```

In Helm, put them under `config.retention` and `config.attachments`.

## What organizations set

An organization owner or admin sets the window and quota with `PUT /v1/orgs/{org}/retention`
(`{ conversations_days, attachments_days, attachment_quota_mib }`, `null` = none; the dashboard's organization settings does the same).
A value above the instance ceiling is refused with 422 `retention_above_instance_maximum`; an invalid value with `retention_invalid`.

## What happens

- **Conversation retention.** A daily job deletes, per organization, conversations whose last activity is older than the window, that have no active turn and no pending approval. It uses the normal delete path, so
  transcript events, attachments and harness session files go with them, and one `conversation.delete` audit row per batch records the ids with `metadata.reason = "retention"`.
  Messages already posted into a chat platform are not deleted. Direct conversations follow the same rule.
- **Attachment retention.** A daily job purges the **bytes** of attachments older than the window. The row stays, the transcript shows the file as expired, and `GET /v1/attachments/{id}/content` answers 410 `attachment_purged`.
- **Quota.** Every upload checks `used + size <= quota` first: the dashboard and API answer 422 `attachment_quota_exceeded` (`details.used_mib`, `details.quota_mib`); an agent's `send_file` gets a tool error; a connector file fetch is marked `failed`.
  The organization's owners get a notice at 80 % and 100 %.
- The attachment job also removes orphaned blobs and recomputes the usage counters.
- **Daily housekeeping** (FIX-111). A daily job removes rows that only ever grow: finished chat-platform ledger rows (inbound events, outbound operations, interactive cards, link requests) older than 7 days (a pending bot-removal operation is never removed), sessions and expired agent context credentials 7 days after they expired or were revoked, and webhook deliveries older than 30 days or beyond the newest 10 000 per webhook.

## Sizing and verification

Attachments live in Postgres (`blobs`) until an S3-compatible store exists, so they share the database disk and backups. Budget for the sum of the organization quotas in the worst case; lowering `attachments.org_quota_mib_max` bounds the sum
([docs/15 §Attachment storage](../15-deployment-operations.md#attachment-storage-phase-2-rfe-015)).

- `GET /v1/admin/storage` (instance administrator) shows blob bytes, attachment bytes per organization and the on-disk size of the large tables.
- `GET /v1/orgs/{org}/storage` (organization owner or admin) shows usage against quota and the retention settings.
- Metrics: `aurora_blob_bytes`, `aurora_org_attachment_bytes{org}`, `aurora_attachment_quota_rejections_total`.
- The two storage pages are served from a **cached snapshot** shared by all API replicas (`computed_at` in the response, "Updated …" on the page): the daily attachment sweep recomputes it, so does the first read after it is more than an hour old, and the **Refresh** button (`POST /v1/admin/storage/refresh`, `POST /v1/orgs/{org}/storage/refresh`) does on demand. An organization's used bytes and quota notice are always live. Page views are ordinary reads; only the Refresh routes are in the strict `export` rate-limit class (6 per hour per principal by default, see [limits-and-approvals.md](limits-and-approvals.md)).
