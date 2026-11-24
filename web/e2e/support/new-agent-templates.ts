import { expect, type Page } from "@playwright/test";

import { mockApi, ws } from "./mock-api";
import type { Scheme } from "./open";

// P6-TPL-4 Part B1 (C-1116, D-458): the workspace's template gallery — **More templates…** in the New agent
// dialog — and a card's details view, reached through the real UI with the mocked API. The README is the same
// deliberately hostile text the review screen uses, to prove the details render it as inert data too.

export const TEMPLATE_README =
  "# Schedule manager\n\nWhat it does.\n\n<img src=x onerror=alert(1)>\n\n[click](javascript:alert(1))\n\n![remote picture](https://evil.test/picture.png)\n\n[docs](./other.md)\n";

const card = (over: Record<string, unknown> = {}) => ({
  template_id: "schedule-manager", name: "Schedule manager", summary: "Triage and schedule the team's work",
  readme: TEMPLATE_README, version: "1.0.0", has_avatar: false, has_helpers: true,
  apps: [{ app_ids: ["github"], required: true, available: true, skills: [] }],
  skill_refs: [],
  automations: [{ kind: "schedule", name: "Morning triage" }, { kind: "webhook", name: "Alerts" }],
  ready: true,
  ...over,
});

// A second card whose required app is not available here: the card and its details say what is missing and
// **Use this template** stays disabled (D-470).
const notReadyCard = card({
  template_id: "incident-scribe", name: "Incident scribe", summary: "Write the incident write-up while it happens",
  readme: "# Incident scribe\n\nWrites the incident write-up.\n", version: "1.1.0", has_helpers: false, ready: false,
  apps: [{ app_ids: ["github"], required: true, available: false, skills: [] }],
  automations: [],
});

const notReadyPlan = {
  ...notReadyCard, content_sha256: "d".repeat(64),
  content: {
    name: "Incident scribe", summary: "Write the incident write-up while it happens",
    readme: "# Incident scribe\n\nWrites the incident write-up.\n", effort: null, min_core: null,
    agent: { name: "Scribe", may_message_first: false }, instructions: "Write it up.", inputs: [], apps: [], skills: [], skill_refs: [],
    helpers: [], memories: [], automations: [], has_avatar: false,
  },
  entries: [],
  skill_refs: [],
};

const plan = {
  ...card(),
  content_sha256: "c".repeat(64),
  content: {
    name: "Schedule manager", summary: "Triage and schedule the team's work", readme: TEMPLATE_README, effort: null, min_core: null,
    agent: { name: "Daneel", may_message_first: false }, instructions: "Triage carefully.", inputs: [], apps: [], skills: [], skill_refs: [],
    helpers: [{ path: "triage.py", size: 120, executable: true, sha256: "a".repeat(64), text: "print(1)" }],
    memories: [], automations: [], has_avatar: false,
  },
  entries: [],
  skill_refs: [],
};

/** What the specs can see of the mocked API after a journey. */
export interface GalleryMock {
  /** The bodies of the writes, by "METHOD path". */
  writes: { key: string; body: unknown }[];
}

/**
 * Routes for the workspace's templates on top of the shared mock (later routes win; the rest falls through to
 * it): the cards of the workspace, the offered version's plan (the details' helper files read it).
 */
export async function mockWorkspaceTemplates(page: Page): Promise<GalleryMock> {
  await mockApi(page);
  const mock: GalleryMock = { writes: [] };
  await page.route(/\/v1\/workspaces\/[^/]+\/agent-templates/, async (route) => {
    const req = route.request();
    const p = new URL(req.url()).pathname;
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    if (req.method() === "GET" && p === `/v1/workspaces/${ws.id}/agent-templates`) return json({ items: [card(), notReadyCard] });
    if (req.method() === "GET" && p === `/v1/workspaces/${ws.id}/agent-templates/schedule-manager`) return json(plan);
    if (req.method() === "GET" && p === `/v1/workspaces/${ws.id}/agent-templates/incident-scribe`) return json(notReadyPlan);
    if (req.method() !== "GET") mock.writes.push({ key: `${req.method()} ${p}`, body: req.postData() ? (req.postDataJSON() ?? {}) : null });
    return route.fallback();
  });
  return mock;
}

/** Opens the workspace's Agents page, the New agent dialog and its gallery, with the OS colour scheme emulated. */
export async function openGallery(page: Page, scheme: Scheme): Promise<GalleryMock> {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => {
    if (m.type() === "error" && !/^Failed to load resource/.test(m.text())) errors.push(m.text());
  });
  const mock = await mockWorkspaceTemplates(page);
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto("/o/acme/w/main/agents");
  await page.locator("h1, h2").first().waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  expect(errors, "the agents page threw").toEqual([]);
  await page.getByRole("button", { name: "New agent" }).click();
  await page.getByRole("button", { name: "More templates…" }).click();
  await page.getByRole("dialog", { name: "More templates" }).waitFor();
  return mock;
}
