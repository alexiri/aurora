#!/usr/bin/env python3
"""Seed realistic memories and skills through the Aurora API.
usage: python3 -I seed.py <base> <bob_key> <alice_org_key> <agent_id> <ws_id> <org_id>"""
import json, sys, urllib.request, urllib.error

BASE, BOB, ALICE, AGENT, WS, ORG = sys.argv[1:7]

def call(method, path, key, body=None, raw=None, ctype="application/json"):
    data = raw if raw is not None else (json.dumps(body).encode() if body is not None else None)
    req = urllib.request.Request(BASE + path, data=data, method=method,
                                 headers={"Authorization": f"Bearer {key}", "Content-Type": ctype})
    try:
        with urllib.request.urlopen(req) as r:
            t = r.read().decode() or "{}"
            return r.status, (json.loads(t) if t.strip().startswith(("{", "[")) else t)
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()[:300]

MEMORIES = [
 ("bob-working-style", "How Bob likes to work", "Tone, format and hours",
  "Bob prefers short answers with the conclusion first, then the detail.\n\n- Use bullet points, not long paragraphs.\n- Times in Zurich time (Europe/Zurich).\n- No meetings before 09:00 or on Friday afternoons.\n- When unsure, ask one question instead of guessing.", False),
 ("platform-team", "The platform team", "Who does what",
  "| Person | Role | Ask them about |\n|---|---|---|\n| Bob Builder | Platform lead | Roadmap, priorities |\n| Carol Member | SRE | Kubernetes, on-call |\n| Dana Ortiz | Backend | Billing service, Postgres |\n| Eli Park | Frontend | Dashboard, design system |\n| Farah Khan | Security | Access reviews, incidents |", False),
 ("oncall-rotation", "On-call rotation", "Who is on call and how to reach them",
  "Rotation changes every Monday at 09:00.\n\n- This week: Carol (primary), Dana (secondary).\n- Next week: Eli (primary), Bob (secondary).\n- Page through Opsgenie team `platform-oncall`; never page outside 07:00–22:00 for P3/P4.", False),
 ("aurora-migration", "Aurora migration project", "Status, decisions and next steps",
  "# Aurora migration\n\n**Goal:** move the three internal agents from the old bot to Aurora by 15 December.\n\n## Status (updated weekly)\n- [x] Pilot workspace set up\n- [x] GitHub and Jira connected\n- [ ] Grafana dashboards linked\n- [ ] Retire the old bot\n\n## Decisions\n1. Agents run on Aurora, not on laptops.\n2. Every agent asks before changing anything in Jira.\n\n## Risks\n- The Grafana service account still needs read access to the `prod` folder.", False),
 ("acme-glossary", "Acme glossary", "Internal words and what they mean",
  "- **PRB** — a problem record, opened after any P1 incident.\n- **Golden path** — the supported way to ship a service (template + CI + dashboards).\n- **Blue room** — the shared on-call channel in Mattermost.\n- **Freeze week** — the week before a release, no infra changes.", False),
 ("grafana-dashboards", "Grafana dashboards", "Which dashboard answers which question",
  "- *API latency*: `platform/api-overview` — p50/p95 by route.\n- *Kubernetes capacity*: `infra/cluster-capacity` — CPU and memory headroom per node pool.\n- *Billing errors*: `billing/errors` — 5xx by endpoint, owned by Dana.", False),
 ("weekly-report-format", "Weekly report format", "What Bob's Friday report contains",
  "Three sections, in this order: **Shipped**, **In progress**, **Blocked**. One line per item with the Jira key. End with one sentence on next week's focus.", False),
 ("incident-2026-11-12", "Incident 12 Nov: API outage", "What happened and what we changed",
  "**What happened:** a config push removed the API's readiness probe; 23 minutes of 5xx.\n\n**Fix:** probe restored, config pushes now need a second reviewer.\n\n**Follow-ups:** PLAT-412 (alert on missing probes), PLAT-413 (runbook update).", False),
 ("vendor-contacts", "Vendor contacts", "Who to call at our suppliers",
  "- Hosting: Northwind Cloud, support portal, contract #NW-22841.\n- Monitoring: Grafana Labs, account manager Sam Lee.\n- Paging: Opsgenie, admin is Farah.", False),
 ("budget-q4", "Q4 budget", "Remaining budget and approvals needed",
  "Remaining: CHF 18,400 of 60,000.\n\nAnything over CHF 2,000 needs Bob's approval; over CHF 10,000 also the director's.", True),
 ("jira-projects", "Jira project keys", "Where to file what",
  "- PLAT — platform work\n- OPS — operational requests\n- SEC — security findings (private)\n- BILL — billing service", True),
 ("carol-tea", None, None, "Carol drinks green tea, no sugar.", False),
]

SKILLS_AGENT = [
 ("weekly-status-report", "Write Bob's Friday status report from this week's Jira and GitHub activity.",
  "# Weekly status report\n\nUse this when Bob asks for the weekly report.\n\n1. List the Jira issues in PLAT moved to Done since Monday.\n2. List merged GitHub pull requests in `acme/platform`.\n3. Group them into Shipped / In progress / Blocked (see memory `weekly-report-format`).\n4. Keep it under 20 lines; link every item.\n\nTemplate: `template.md`.", True,
  {"template.md": "## Shipped\n- \n\n## In progress\n- \n\n## Blocked\n- \n\n**Next week:** "}),
 ("triage-alerts", "Look at firing alerts, group them by service and suggest who should look.",
  "# Triage alerts\n\nRun `scripts/firing.sh` to list firing alerts as JSON, then:\n- group by `service` label;\n- for each group say the likely cause in one line;\n- suggest an owner from the memory `platform-team`.\n\nNever silence an alert yourself.", True,
  {"scripts/firing.sh": "#!/bin/sh\n# List firing alerts (needs the Grafana app).\ncurl -s \"$GRAFANA_URL/api/alertmanager/grafana/api/v2/alerts?active=true\"\n"}),
 ("summarize-pr", "Summarize a GitHub pull request for a reviewer in five bullets.",
  "# Summarize a pull request\n\nRead the PR description, the diff stats and the review comments. Write five bullets: what changes, why, risk, tests, open questions.", True, {}),
 ("draft-email", "Draft a short email in Bob's tone.",
  "# Draft an email\n\nShort, friendly, no exclamation marks. Sign as Bob. Always show the draft; never send.", False, {}),
 ("release-notes", "Write release notes the way Bob likes them (overrides the workspace skill).",
  "# Release notes (Bob's version)\n\nOne paragraph summary, then a bullet per user-visible change. Skip internal refactors.", True, {}),
]
SKILLS_WS = [
 ("release-notes", "Write release notes for a tagged release from the merged pull requests.",
  "# Release notes\n\nCollect merged PRs since the previous tag. Group: Features, Fixes, Internal. Link each PR.", True, {}),
 ("meeting-minutes", "Turn a meeting transcript into minutes with decisions and actions.",
  "# Meeting minutes\n\nSections: Attendees, Decisions, Actions (owner + due date), Open questions.", True, {}),
 ("incident-postmortem", "Draft a blameless postmortem from an incident channel.",
  "# Postmortem\n\nTimeline (UTC), impact, root cause, what went well, what to improve, follow-up tickets.", True, {}),
]
SKILLS_ORG = [
 ("acme-style-guide", "Acme's writing style for anything that leaves the team.",
  "# Acme style guide\n\n- Plain words, short sentences.\n- Dates as 12 Nov 2026.\n- Never promise dates to customers without Product.", True, {}),
 ("security-review-checklist", "Run Acme's security checklist on a change before it ships.",
  "# Security review checklist\n\n1. Secrets never in code or logs.\n2. New endpoints have authorization tests.\n3. Dependencies pinned and scanned.\n4. Data retention stated.", True, {}),
 ("onboarding-new-hire", "Prepare a first-week plan for a new team member.",
  "# Onboarding\n\nDay 1 accounts, day 2 golden path walkthrough, day 3 shadow on-call, day 4–5 first small change.", True, {}),
]

def main():
    out = {}
    for key, title, desc, content, locked in MEMORIES:
        body = {"key": key, "content": content}
        if title: body["title"] = title
        if desc: body["description"] = desc
        if locked: body["locked"] = True
        out["mem:" + key] = call("POST", f"/v1/agents/{AGENT}/memories", BOB, body)[0]
    # a few revisions
    for key, extra in [("aurora-migration", "\n- [x] Grafana dashboards linked"), ("oncall-rotation", "\n\nHoliday swap: Dana covers 24–26 Dec.")]:
        st, m = call("GET", f"/v1/agents/{AGENT}/memories/{key}", BOB)
        if st == 200:
            call("PUT", f"/v1/agents/{AGENT}/memories/{key}", BOB, {"title": m.get("title"), "description": m.get("description"), "content": m["content"] + extra})
            out["rev:" + key] = 200
    def mk(path, key, items):
        for name, desc, body, enabled, files in items:
            st, s = call("POST", path, key, {"name": name, "description": desc, "body": body, "enabled": enabled})
            out[f"skill:{path}:{name}"] = st
            if st in (200, 201) and isinstance(s, dict):
                sid = s.get("id") or s.get("skill", {}).get("id")
                for fp, content in files.items():
                    out[f"file:{name}:{fp}"] = call("PUT", f"/v1/skills/{sid}/files/{fp}", key, raw=content.encode(), ctype="application/octet-stream")[0]
    mk(f"/v1/agents/{AGENT}/skills", BOB, SKILLS_AGENT)
    mk(f"/v1/workspaces/{WS}/skills", BOB, SKILLS_WS)
    mk(f"/v1/orgs/{ORG}/skills", ALICE, SKILLS_ORG)
    print(json.dumps(out, indent=1))

main()
