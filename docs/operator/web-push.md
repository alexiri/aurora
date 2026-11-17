# Web Push: approval notifications when Aurora is closed (phase 5, P4-011B, D-134, D-266, SEC-75, SEC-76, SEC-102)

Web Push lets a person who can decide an approval get a notification on a phone or a laptop even when no Aurora tab is open. It is **off by default** and only for approvals. What is sent is a notification that *an approval is waiting*: no title, summary, path, body or link to a decision; the person still decides in Aurora. Agent and workspace names appear only on a device whose owner turned that on.

Reference: the values are in [install-helm.md](install-helm.md) and `docs/23` (*Web Push*); the config keys are in `docs/15` (*Approval park-and-resume and Web Push*). Parked approvals (the reason Web Push matters most) are in [limits-and-approvals.md](limits-and-approvals.md#parking-long-approvals-that-do-not-hold-the-agent-phase-5-p4-011b-d-264d-267).

## Requirements

- The dashboard is served over **https** (browsers register service workers only on https or `localhost`). iOS and iPadOS additionally need the site added to the Home Screen; the dashboard shows that instruction on the device.
- The API pods can reach the browser vendors' push services on TCP 443. Default-deny egress stays in place: you name the peers in `push.networkPolicy.egressTo` (required while push is on; the chart render fails while it is empty).
- A contact for the push services: `config.push.vapid.subject` (`mailto:` or `https:`; it defaults to `server.external_url` when that is https). Startup refuses `push.enabled` with an invalid subject, an empty allow-list or an unreadable operator key, naming the key.
- The web service needs no setting: the service worker is a static file of the dashboard.

## Turning it on

```yaml
push:
  enabled: true
  networkPolicy:
    egressTo:
      - ipBlock: { cidr: 0.0.0.0/0, except: [10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16] }
```

`push.enabled` renders `config.push.enabled` and the api's egress rule. Then Account › Profile › Approval notifications shows the **Notify this device …** card to every signed-in person, and Admin › Setup shows a **Web Push** line (on or off). Nothing else changes for a deployment that keeps it off: `GET /v1/push/config` answers 404 `push_unavailable` and no outbound call is ever made.

Allowed endpoint hosts: the application's list (Google FCM, Mozilla autopush, Apple, Windows) unless you set `push.allowedEndpointHosts`. A subscription whose host is not on the list is refused at registration (422 `push_endpoint_not_allowed`) and again before every send (the device is then disabled).

## Keys: generated or operator-managed

Web Push is signed with a VAPID key (P-256).

- **Generated** (the default): the first API replica that finds no key creates one, seals it (purpose `webpush.vapid`, the `inline` secret store, so it is encrypted by the instance key and needs no external store) and stores it. Nothing to configure.
- **Operator-managed**: set `push.vapid.existingSecret` (and `push.vapid.key` when the Secret's key is not `AURORA_PUSH_VAPID_KEY`). The value is a PKCS#8 PEM or the base64url raw 32-byte scalar. It is mounted into the API pods and the migrate Job, never stored, never returned. An unreadable key refuses startup naming `AURORA_PUSH_VAPID_KEY`.

**Rotation** (generated keys only): `aurora-api push rotate-vapid [--dry-run]`. The new key becomes active, the old one is retired; browsers re-subscribe silently on their next dashboard load, so rotation keeps subscribers working. A daily job destroys a retired key when no subscription uses it, or after `PUSH_VAPID_RETIRE`; its subscriptions are then deleted and those people turn notifications on again. The command writes the critical audit row `push.vapid_rotate`. With an operator key it exits 3: rotate by changing the Secret and restarting (every device re-subscribes on its next load).

**Status**: `aurora-api push status [--json]` prints the mode (`generated`, `operator` or `disabled`), each key's id, status and dates, the subscriptions per key, per state and per push-service host. It never prints key material.

## Privacy and the shared-computer advice

- The push services see an encrypted payload and the endpoint; they cannot read the text. The payload holds ids and a few fixed fields, never request details.
- A subscription is bound to the session that registered it. Signing out removes it first (best effort); the session ending, being revoked or expiring ends delivery regardless, and a different person signing in on the same browser never inherits the device.
- **Shared or public computers:** do not turn notifications on there. If it happened, sign out (which removes the device) or remove it from Account › Profile on another device. Leave *Show agent and workspace names* off on a phone whose lock screen others can see.
- Someone with an open Aurora tab on the workspace gets no push (the in-tab notification and the page do the job); an approval nobody is watching is pushed after a short delay, then once more at half its timeout if it is parked and still pending.

## Troubleshooting by metric

`aurora_push_sent_total{result}` counts every delivery attempt; `aurora_push_subscriptions{state}` (gauge) counts devices by state; `aurora_push_duration_seconds` times the send.

| `result` | Meaning | What to do |
|---|---|---|
| `sent` | the push service accepted it (201) | nothing; the device shows it unless the browser is offline |
| `gone` | 404 or 410: the browser dropped the subscription; the row is deleted | normal churn; the person turns it on again |
| `rejected` | 400 or 413: the push service refused the message | rare; check `push status` for a stale key and the API logs |
| `forbidden` | 401 or 403: VAPID refused (wrong key, bad `sub`) | check `config.push.vapid.subject`; after an operator key change, devices must re-subscribe |
| `throttled` | 429 or 5xx: retried up to the limit | usually the push service; persistent: check egress |
| `error` | DNS, connect or TLS failed | egress NetworkPolicy (`push.networkPolicy.egressTo`), DNS, a proxy |
| `timeout` | no answer within `push.timeout_secs` | egress path; raise the timeout only after checking the network |
| `rejected_host` | the endpoint host left the allow-list; the device is disabled | restore the host in `push.allowedEndpointHosts` or let people re-subscribe |

A device that fails repeatedly is disabled and shown as "Not receiving" in the dashboard until the person turns notifications on again. People report "no notification": check the device state in Account first (Paused? Not receiving? Needs update?), then browser and OS notification settings, then the metric.

## Manual release check

Real push services are never contacted in CI, so run this by hand on a real instance before a release that touches push (about 20 minutes). Use a test person who can approve in a workspace, and a test agent with a rule that requires approval.

| Step | Chrome (desktop) | Firefox (desktop) | Android Chrome | iOS Home Screen app |
|---|---|---|---|---|
| Opt in: Account › Approval notifications › **Turn on**, allow the permission | ☐ | ☐ | ☐ | ☐ (add Aurora to the Home Screen first) |
| **Send test**: the notification appears; the list shows "Last delivery" | ☐ | ☐ | ☐ | ☐ |
| Close every Aurora tab, trigger an approval: one notification, no request details | ☐ | ☐ | ☐ | ☐ |
| Click it: Aurora opens on the Approvals page with that approval highlighted | ☐ | ☐ | ☐ | ☐ |
| Parked approval: the text adds "The agent has stopped waiting." and a reminder arrives at half time | ☐ | ☐ | ☐ | ☐ |
| Rotate the key (`aurora-api push rotate-vapid`), reload the dashboard: the device stays listed and **Send test** still arrives | ☐ | ☐ | ☐ | ☐ |
| Sign out: the device disappears from the list on another browser; the next approval sends nothing to it | ☐ | ☐ | ☐ | ☐ |
| Remove the device on another browser and trigger an approval: nothing arrives | ☐ | ☐ | ☐ | ☐ |

Record the date, the Aurora version and any failing cell in the release notes. The automated coverage is `docs/tests/security-coverage.md` (SEC-75, SEC-76, SEC-102).
