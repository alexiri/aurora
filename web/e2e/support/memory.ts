import type { Page, Route } from "@playwright/test";

import { id } from "./mock-api";

// The Memory page's routes, in memory, over the shared mock (call `mockApi` or `mockWithTerminal` first). The data is
// shaped like the review's (docs/design/skills-memory/seed.py), as the mockups mem-1 and mem-3 show it.

const daysAgo = (n: number) => new Date(Date.now() - n * 86_400_000).toISOString();
const bob = { id: id(99), display_name: "Ada Lovelace" };
const byMe = { kind: "person", person: bob };

export interface MockMemory {
  key: string;
  title: string | null;
  description: string | null;
  content: string;
  index_line: string;
  revision: number;
  created_at: string;
  updated_at: string;
  locked: boolean;
  locked_at: string | null;
  locked_by: typeof bob | null;
  added_by: Record<string, unknown>;
  changed_by: Record<string, unknown>;
  last_used: Record<string, unknown> | null;
}

export function memory(over: Partial<MockMemory> & { key: string }, age = 2): MockMemory {
  return {
    title: null,
    description: null,
    content: "Text.",
    index_line: over.key,
    revision: 1,
    created_at: daysAgo(age),
    updated_at: daysAgo(age),
    locked: false,
    locked_at: null,
    locked_by: null,
    added_by: byMe,
    changed_by: byMe,
    last_used: null,
    ...over,
  };
}

export const AURORA = [
  "# Aurora migration",
  "",
  "**Goal:** move the three internal agents from the old bot to Aurora by 15 December.",
  "",
  "## Status (updated weekly)",
  "- [x] Pilot workspace set up",
  "- [x] Instructions for Daneel migrated",
  "- [ ] Move the on-call agent",
  "- [ ] Move the reporting agent",
  "- [ ] Switch the old bot off",
  "",
  "## Decisions",
  "- Cut over on Monday mornings, never on a Friday.",
  "- The Jira project keys memory stays locked until the move is done.",
].join("\n");

const ONCALL = [
  "# On-call rotation",
  "",
  "Rotation changes every Monday at 09:00.",
  "",
  "- This week: Carol (primary), Dana (secondary).",
  "- Next week: Eli (primary), Bob (secondary).",
  "- Page through the Opsgenie team `platform-oncall`; never page outside 07:00–22:00 for P3/P4.",
  "- Holiday swap: Dana covers Carol over Christmas.",
].join("\n");

export function reviewMemories(): MockMemory[] {
  const m = (key: string, title: string, description: string, age: number, over: Partial<MockMemory> = {}) =>
    memory({ key, title, description, ...over }, age);
  return [
    m("aurora-migration", "Aurora migration project", "Status, decisions and next steps", 2, {
      content: AURORA,
      revision: 3,
      updated_at: daysAgo(0),
      last_used: { at: daysAgo(1), conversation: { id: "c1", title: "Weekly report" } },
    }),
    memory(
      {
        key: "carol-drinks-green-tea",
        content: "Carol drinks green tea, no sugar.",
        updated_at: daysAgo(1),
        added_by: { kind: "agent", agent: { id: id(11), name: "Daneel" }, conversation: { id: "c2", title: "Release prep" } },
      },
      0,
    ),
    m("acme-glossary", "Acme glossary", "Internal words and what they mean", 2),
    m("how-bob-likes-to-work", "How Bob likes to work", "Tone, format and hours", 2),
    m("q4-budget", "Q4 budget", "Remaining budget and approvals needed", 3, { locked: true, locked_at: daysAgo(3), locked_by: bob }),
    m("grafana-dashboards", "Grafana dashboards", "Which dashboard answers which question", 8),
    m("incident-12-nov", "Incident 12 Nov: API outage", "What happened and what we changed", 15),
    m("jira-project-keys", "Jira project keys", "Where to file what", 16, { locked: true, locked_at: daysAgo(16), locked_by: bob }),
    m("on-call-rotation", "On-call rotation", "Who is on call and how to reach them", 22, { content: ONCALL, revision: 2 }),
    m("the-platform-team", "The platform team", "Who does what", 23),
    m("vendor-contacts", "Vendor contacts", "Who to call at our suppliers", 24),
    m("weekly-report-format", "Weekly report format", "What Bob's Friday report contains", 28),
  ];
}

/** Serves `items` (changed by the page's own requests) for any agent. */
export async function mockMemories(page: Page, items: MockMemory[] = reviewMemories()) {
  const json = (route: Route, status: number, body: unknown) =>
    route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
  await page.route(/\/v1\/agents\/[^/]+\/memories(\/[^?]*)?(\?.*)?$/, async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const rest = url.pathname.replace(/^.*\/memories\/?/, "").split("/").filter(Boolean);
    const method = request.method();
    const found = items.find((m) => m.key === rest[0]);
    if (rest.length === 0 && method === "GET") return json(route, 200, { items, next_cursor: null });
    if (rest.length === 0 && method === "POST") {
      const body = request.postDataJSON() as Partial<MockMemory> & { key: string };
      if (items.some((m) => m.key === body.key)) {
        return json(route, 409, { error: { code: "memory_exists", message: "A memory with this key already exists." } });
      }
      const made = memory(body, 0);
      items.push(made);
      return json(route, 201, made);
    }
    if (rest[0] === "search") {
      const q = (url.searchParams.get("q") ?? "").toLowerCase();
      const hits = items.flatMap((m) => {
        const text = m.content.replace(/^#.*$/gm, "").replace(/[`*]|^- (\[.\] )?/gm, "").replace(/\s+/g, " ").trim();
        const at = text.toLowerCase().indexOf(q);
        if (at < 0) return [];
        const snippet = text.slice(at, at + 130);
        const inTitle = (m.title ?? m.key).toLowerCase().indexOf(q);
        return [
          {
            key: m.key,
            title: m.title,
            locked: m.locked,
            score: 1,
            snippet,
            highlights: [[0, q.length]],
            title_highlights: inTitle >= 0 ? [[inTitle, inTitle + q.length]] : [],
          },
        ];
      });
      return json(route, 200, { items: hits });
    }
    if (!found) return json(route, 404, { error: { code: "not_found", message: "Not found." } });
    if (rest[1] === "locked" && method === "PUT") {
      found.locked = (request.postDataJSON() as { locked: boolean }).locked;
      found.locked_at = found.locked ? new Date().toISOString() : null;
      found.locked_by = found.locked ? bob : null;
      return json(route, 200, found);
    }
    if (rest[1] === "revisions") return json(route, 200, { items: [], next_cursor: null });
    if (rest.length === 1 && method === "PUT") {
      Object.assign(found, request.postDataJSON(), { revision: found.revision + 1, updated_at: new Date().toISOString() });
      return json(route, 200, found);
    }
    return route.fallback();
  });
}
