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

// ---- the libraries of skill-5 -------------------------------------------------------------------

const LIBRARIES = [
  { id: id(700), scope: "org", org_id: id(1), label: "Anthropic skills", status: "active", skills_offered: 17 },
  { id: id(701), scope: "org", org_id: id(1), label: "Acme team recipes", status: "active", skills_offered: 5 },
  { id: id(702), scope: "org", org_id: id(1), label: "Platform tools", status: "active", skills_offered: 9 },
];

const offered = (name: string, description: string, files: number, kb: number, over: Record<string, unknown> = {}) => ({
  path: name, name, description, valid: true, reason: null, size: kb * 1024, files, added: [], blocked: null, ...over,
});

/** Anthropic skills as the mockup lists it, with the eight that can't be added. `pdfId` is the agent's own copy. */
const libraryCatalogue = (pdfId: string) => ({
  commit_sha: SHA,
  truncated: false,
  next_cursor: null,
  budget: { bytes: 1_048_576, used: 6144 },
  items: [
    offered("algorithmic-art", "Create generative art with p5.js, seeded randomness and interactive parameters.", 4, 19),
    offered("brand-guidelines", "Apply Anthropic’s brand colors and typography to any artifact.", 2, 2),
    offered("doc-coauthoring", "A structured workflow for co-authoring documentation, specs and proposals.", 1, 15),
    offered("frontend-design", "Distinctive, intentional visual design when building a new UI.", 2, 9),
    offered("internal-comms", "Formats for status reports, updates, newsletters, FAQs and incident reports.", 6, 2),
    offered("pdf", "Read, merge, split and fill in PDF files.", 3, 6, { added: [{ tier: "agent", id: pdfId }], blocked: { reason: "name_taken", message: "This agent already has a skill named pdf." } }),
    offered("skill-creator", "Guide for creating and improving skills.", 4, 11),
    offered("release-notes", "Write release notes.", 1, 1, { blocked: { reason: "name_taken", message: "This agent already has a skill named release-notes." } }),
    offered("canvas-design", "Posters and static art.", 80, 2048, { blocked: { reason: "budget", message: "Too large for the room left for library skills here (6144 of 1048576 characters used)." } }),
    ...["docx", "pptx", "xlsx", "mcp-builder", "slack-gif-creator", "web-artifacts-builder"].map((name) =>
      offered(name, "", 1, 0, { valid: false, reason: "the skill has more than 50 files", blocked: { reason: "invalid", message: "the skill has more than 50 files" } }),
    ),
  ],
});

const brandGuidelines = {
  commit_sha: SHA, path: "brand-guidelines", name: "brand-guidelines", valid: true, reason: null,
  description: "Apply Anthropic’s brand colors and typography to any artifact.",
  body: "## Colors\n\n- Dark `#141413`, light `#faf9f5`\n- Accents: orange, blue and green\n\n## Typography\n\nHeadings in Poppins, body in Lora, with Arial and Georgia as fallbacks.\n\n![a pixel](/v1/me)\n",
  size: 2252, content_sha256: "a".repeat(64),
  files: [{ path: "LICENSE.txt", size: 307, executable: false, binary: false, content: "MIT\n", content_b64: null }],
};

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
        const b = req.postDataJSON() as { name: string; title?: string; description: string; body: string; enabled?: boolean; from_library?: { path: string } };
        const made = b.from_library
          ? make(60 + mock.writes.length, "agent", b.from_library.path, brandGuidelines.description, brandGuidelines.body, { source: library("Anthropic skills", b.from_library.path) })
          : make(60 + mock.writes.length, "agent", b.name, b.description, b.body, { title: b.title ?? null, enabled: b.enabled ?? true });
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
    if (p === `/v1/orgs/${id(1)}/skill-libraries` && method === "GET") return json({ items: LIBRARIES, next_cursor: null });
    if (p === `/v1/skill-libraries/${id(700)}/skills`) return json(libraryCatalogue(all.find((s) => s.tier === "agent" && s.name === "pdf")!.id));
    if (p === `/v1/skill-libraries/${id(700)}/skills/brand-guidelines`) return json(brandGuidelines);
    if (/^\/v1\/skill-libraries\/[^/]+\/skills\/.+/.test(p)) return json({ ...brandGuidelines, path: p.split("/").pop(), name: p.split("/").pop(), files: [] });
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
