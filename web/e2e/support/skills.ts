import type { Page, Route } from "@playwright/test";

import { id } from "./mock-api";

// (P5-SKILLS-3) The Skills pages' routes, stateful, over the shared mock (call `mockApi` first). The data is shaped like
// the review's (docs/design/skills-memory/seed.py), as the mockups skill-1 … skill-4 show it.

const now = new Date().toISOString();
const SHA = "e4bb5d9e4bb5d9e4bb5d9e4bb5d9e4bb5d9e4bb5";
const PENDING = "89abcdef0123456789abcdef0123456789abcdef";

export interface MockSkill {
  id: string;
  org_id: string;
  agent_id: string | null;
  workspace_id: string | null;
  scope: string;
  tier: "agent" | "workspace" | "org";
  name: string;
  title: string | null;
  description: string;
  body: string;
  enabled: boolean;
  files: { path: string; size: number; executable: boolean }[];
  total_size: number;
  overridden_in: null;
  used_by_agents: number | null;
  source: Record<string, unknown> | null;
  created_at: string;
  updated_at: string;
}

const contents = new Map<string, string>();

function make(n: number, tier: MockSkill["tier"], name: string, description: string, body: string, over: Partial<MockSkill> = {}, files: Record<string, { text: string; executable?: boolean }> = {}): MockSkill {
  const skillId = id(800 + n);
  for (const [path, f] of Object.entries(files)) contents.set(`${skillId}/${path}`, f.text);
  return {
    id: skillId,
    org_id: id(1),
    agent_id: tier === "agent" ? id(11) : null,
    workspace_id: tier === "workspace" ? id(2) : null,
    scope: tier === "org" ? "organization" : tier,
    tier,
    name,
    title: null,
    description,
    body,
    enabled: true,
    files: Object.entries(files).map(([path, f]) => ({ path, size: f.text.length, executable: !!f.executable })),
    total_size: body.length,
    overridden_in: null,
    used_by_agents: tier === "agent" ? null : 0,
    source: null,
    created_at: now,
    updated_at: now,
    ...over,
  };
}

const library = (label: string, path: string, pending = false) => ({
  kind: "library", library_id: id(700), label, repo: "github.com/anthropics/skills", path, commit_sha: SHA, pinned_at: now, detached: false, update_pending: pending,
});

function skills(): MockSkill[] {
  contents.clear();
  return [
    make(1, "agent", "weekly-status-report", "Write Bob’s Friday status report from this week’s Jira and GitHub activity.", "# Weekly status report\n\nGroup the work into Shipped, In progress and Blocked.\n\nTemplate: `template.md`.", { title: "Weekly status report" }, { "template.md": { text: "## Shipped\n- \n\n## Blocked\n- \n" } }),
    make(2, "agent", "triage-alerts", "Look at firing alerts, group them by service and suggest who should look.", "# Triage alerts\n\nRun `scripts/firing.sh` to list firing alerts as JSON, then:\n\n- group by `service` label;\n- for each group say the likely cause in one line;\n- suggest an owner from the memory `platform-team`.\n\n**Never silence an alert yourself.**", {}, {
      "scripts/firing.sh": { text: "#!/usr/bin/env bash\n# List firing alerts as JSON\ncurl -s \"$ALERTMANAGER_URL/api/v2/alerts?active=true\" \\\n  | jq '[.[] | {service: .labels.service, alert: .labels.alertname}]'\n", executable: true },
    }),
    make(3, "agent", "summarize-pull-requests", "Summarize a GitHub pull request for a reviewer in five bullets.", "# Summarize pull requests\n\nRead the pull request with the GitHub tools, then write\nexactly five bullets for a reviewer:\n- what changed and why;\n- the riskiest file;\n- tests added or missing;\n- anything that needs a migration;\n- one question to ask the author.\n\nUse the layout in **template.md**.", { title: "Summarize pull requests" }, { "template.md": { text: "## Summary\n1. …  2. …  3. …  4. …  5. …\n" } }),
    make(4, "agent", "draft-email", "Draft a short email in Bob’s tone.", "# Draft an email\n\nShort, friendly. Never send.", { enabled: false }),
    make(5, "agent", "release-notes", "Write release notes the way Bob likes them.", "# Release notes (Bob's version)\n\nOne paragraph summary, then a bullet per user-visible change.\nSkip internal refactors."),
    make(6, "agent", "pdf", "Read, merge, split and fill in PDF files.", "## PDF processing\n\nUse this skill whenever a task involves a `.pdf`: reading text, merging or splitting pages, filling forms.\n\n- Prefer text extraction before OCR.\n- Never overwrite the original file.", { source: library("Anthropic skills", "pdf", true) }, {
      "forms.md": { text: "# Forms\n" }, "scripts/merge.py": { text: "print('merge')\n", executable: true },
    }),
    make(11, "workspace", "release-notes", "Write release notes for a tagged release.", "# Release notes\n\nCollect merged PRs since the previous tag. Link each PR.", { used_by_agents: 3 }),
    make(12, "workspace", "meeting-minutes", "Turn a meeting transcript into minutes with decisions and actions.", "# Meeting minutes\n\nSections: Attendees, Decisions, Actions.", { used_by_agents: 4 }),
    make(13, "workspace", "incident-postmortem", "Draft a blameless postmortem from an incident channel.", "# Postmortem\n\nTimeline, impact, root cause.", { used_by_agents: 2 }),
    make(21, "org", "acme-style-guide", "Write anything that leaves the team in Acme’s style.", "# Acme style guide\n\nPlain words, short sentences.", { used_by_agents: 14 }),
    make(22, "org", "security-review-checklist", "Run Acme’s security checklist on a change before it ships.", "# Security review checklist\n\n1. Secrets never in code.", { used_by_agents: 14 }),
    make(23, "org", "onboarding-new-hire", "Prepare a first-week plan for a new team member.", "# Onboarding new hire\n\nPrepare a first-week plan for a new team member:\n\n1. Ask for the start date, team and manager.\n2. Book the day-one accounts and hardware checklist.\n3. List three people to meet, with why.\n4. End with a Friday check-in question.", { used_by_agents: 14 }),
    make(24, "org", "internal-comms", "Write status reports, updates and newsletters in Acme’s house formats.", "# Internal comms\n\nHouse formats.", { used_by_agents: 6, source: library("Acme team recipes", "internal-comms") }),
  ];
}

export interface SkillsMock {
  /** Every write, as "METHOD path". */
  writes: { key: string; body: string }[];
}

/** The Skills routes of the agent Daneel, the workspace and the organization. */
export async function mockSkills(page: Page): Promise<SkillsMock> {
  const all = skills();
  const off = new Set<string>();
  const mock: SkillsMock = { writes: [] };
  const own = () => all.filter((s) => s.tier === "agent");
  const effective = () => {
    const names = [...new Set(all.map((s) => s.name))];
    const items: unknown[] = [];
    const shadowed: unknown[] = [];
    for (const name of names) {
      const same = all.filter((s) => s.name === name);
      const winner = same.find((s) => s.enabled && !off.has(s.id));
      const usage = (s: MockSkill) =>
        s.name === "triage-alerts" ? { week: 3, last: { at: now, conversation: { id: id(900), title: "Alert storm 26 Nov" } } } : { week: 0, last: null };
      if (winner) {
        const lower = same.filter((s) => s !== winner);
        items.push({ skill: winner, tier: winner.tier, enabled: true, off_for_agent: false, usage: usage(winner), shadows: lower.map((s) => ({ id: s.id, tier: s.tier })) });
      }
      for (const s of same.filter((x) => x !== winner)) {
        const reason = !s.enabled ? "disabled" : off.has(s.id) ? "off_for_agent" : "replaced";
        shadowed.push({ skill: s, tier: s.tier, enabled: s.enabled, off_for_agent: off.has(s.id), reason, usage: usage(s), shadowed_by: winner ? { id: winner.id, tier: winner.tier } : null });
      }
    }
    return { items, shadowed };
  };

  await page.route("**/v1/**", async (route: Route) => {
    const req = route.request();
    const p = decodeURIComponent(new URL(req.url()).pathname);
    const method = req.method();
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (method !== "GET") mock.writes.push({ key: `${method} ${p}`, body: req.postData() ?? "" });
    const list = (items: MockSkill[]) => json({ items, next_cursor: null });
    if (p === `/v1/agents/${id(11)}/skills/effective`) return json(effective());
    if (p === `/v1/agents/${id(11)}/skills`) {
      if (method === "POST") {
        const b = req.postDataJSON() as { name: string; title?: string; description: string; body: string; enabled?: boolean };
        const made = make(60 + mock.writes.length, "agent", b.name, b.description, b.body, { title: b.title ?? null, enabled: b.enabled ?? true });
        all.push(made);
        return json(made, 201);
      }
      return list(own());
    }
    const switched = /\/v1\/agents\/[^/]+\/skills\/([^/]+)\/off$/.exec(p);
    if (switched) {
      if (method === "PUT") off.add(switched[1]!);
      else off.delete(switched[1]!);
      return route.fulfill({ status: 204 });
    }
    if (p === `/v1/workspaces/${id(2)}/skills` && method === "GET") return list(all.filter((s) => s.tier === "workspace"));
    if (p === `/v1/orgs/${id(1)}/skills` && method === "GET") return list(all.filter((s) => s.tier === "org"));
    if (p.endsWith("/skill-library-updates")) return json({ items: [] });
    if (p === `/v1/orgs/${id(1)}/skill-libraries` && method === "GET") {
      return json({ items: [{ id: id(700), scope: "org", org_id: id(1), label: "Anthropic skills", status: "active" }], next_cursor: null });
    }
    const file = /^\/v1\/skills\/([^/]+)\/files\/(.+)$/.exec(p);
    if (file) {
      const skill = all.find((s) => s.id === file[1]);
      if (!skill) return json({ error: { code: "not_found", message: "nf" } }, 404);
      const key = `${file[1]}/${file[2]}`;
      if (method === "GET") return route.fulfill({ status: 200, contentType: "application/octet-stream", body: contents.get(key) ?? "" });
      if (method === "PUT") {
        const text = req.postData() ?? "";
        contents.set(key, text);
        const executable = new URL(req.url()).searchParams.get("executable") === "true";
        skill.files = [...skill.files.filter((f) => f.path !== file[2]), { path: file[2]!, size: text.length, executable }];
        return json(skill);
      }
      skill.files = skill.files.filter((f) => f.path !== file[2]);
      return route.fulfill({ status: 204 });
    }
    const info = /^\/v1\/skills\/([^/]+)\/library$/.exec(p);
    if (info) {
      return json({
        library: { id: id(700), label: "Anthropic skills", repo: "github.com/anthropics/skills", path: "pdf" }, commit_sha: SHA, pinned_at: now, pinned_by: id(99), detached: false,
        update: {
          id: id(720), kind: "changed", commit_sha: PENDING, created_at: now, content_sha256: "b".repeat(64), over_budget: false, reason: null,
          diff: {
            description: { old: "Read PDF files", new: "Read PDF files", changed: false }, body: { old: "a\n", new: "b\n", changed: true },
            files: [{ path: "forms.md", change: "changed", old: "a", new: "b", binary: false }, { path: "scripts/merge.py", change: "changed", old: "a", new: "b", binary: false }],
          },
        },
      });
    }
    const one = /^\/v1\/skills\/([^/]+)$/.exec(p);
    if (one) {
      const skill = all.find((s) => s.id === one[1]);
      if (!skill) return json({ error: { code: "not_found", message: "nf" } }, 404);
      if (method === "PATCH") {
        Object.assign(skill, req.postDataJSON(), { updated_at: new Date().toISOString() });
        return json(skill);
      }
      if (method === "DELETE") {
        all.splice(all.indexOf(skill), 1);
        return route.fulfill({ status: 204 });
      }
      return json(skill);
    }
    return route.fallback();
  });
  return mock;
}
