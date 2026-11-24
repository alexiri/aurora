import { expect, type Locator, type Page } from "@playwright/test";

import { agent, id, mockApi, ws } from "./mock-api";
import type { Scheme } from "./open";

// P6-TPL-4 Part B1 (C-1116, D-458): the workspace's template gallery — **More templates…** in the New agent
// dialog — a card's details view, reached through the real UI with the mocked API, and the four-step page that
// creates an agent from a template. The README is the same deliberately hostile text the review screen uses, to
// prove the details render it as inert data too.

export const TEMPLATE_README =
  "# Schedule manager\n\nWhat it does.\n\n<img src=x onerror=alert(1)>\n\n[click](javascript:alert(1))\n\n![remote picture](https://evil.test/picture.png)\n\n[docs](./other.md)\n";

/** The agent the mocked create answers with; it is in the workspace's agents list, so its page opens. */
export const rexAgent = { ...agent, id: id(12), handle: "rex", name: "Rex", is_default: false, brief: "Triage and schedule the team's work." };

/** A library commit head; the review shows its first seven characters. */
export const LIBRARY_SHA = "abcdef1234567890abcdef1234567890abcdef12";

const library = { id: "lib1", label: "Team skills", repo: "github.com/acme/skills", path: "", ref: null, commit_sha: LIBRARY_SHA, scope: "organization" };

/** The plan's resolved library skill references (C-1142): a required one, an optional one, an unavailable one. */
const skillRefs = [
  { index: 0, kind: "library", name: "Triage", required: true, available: true, why: "It triages the queue.", library, content: { use_it_when: "The queue is long.", body: "Read the queue.", files: [] } },
  { index: 1, kind: "library", name: "Sorting", required: false, available: true, why: "", library, content: { use_it_when: "The queue needs order.", body: "Sort it newest first.", files: [] } },
  { index: 2, kind: "library", name: "Reports", required: false, available: false, why: "", library: null, unavailable_reason: "library_unreadable", content: null },
];

const planAccount = (n: number, appId: string, appName: string, name: string) => ({
  ref: { type: "connection", id: id(n) }, kind: "app", name, scope: "workspace", levels: ["read", "ask", "act"], limit: null,
  app: { app_id: appId, name: appName, custom_only: false }, health: { state: "not_used" }, agents: [], managed_by: null,
  can: { give_access: true, change_access: true, manage: true },
});

const choice = (appId: string, appName: string, accounts: unknown[], level: string) => ({
  app_id: appId, app_name: appName, available: true, accounts, level, level_note: null, custom: null, unavailable_reason: null,
});

/**
 * The offered version's plan (C-1110). Before the connect journey ran, the Linear entry has no account here; the
 * account the journey makes (C-957) appears in the refetched plan, as the server would resolve it.
 */
const plan = (linearConnected: boolean) => ({
  ...card(),
  content_sha256: "c".repeat(64),
  content: {
    name: "Schedule manager", summary: "Triage and schedule the team's work", readme: TEMPLATE_README, effort: null, min_core: null,
    agent: { name: "Daneel", may_message_first: false }, instructions: "Triage carefully.",
    inputs: [{ id: "repo", label: "Repository", help: "owner/name", kind: "text", required: true, pattern: null, max_length: 100, options: null, default: null }],
    apps: [],
    skills: [{ name: "Triage", use_it_when: "The queue is long.", body: "Read the queue. Sort it.", files: [] }],
    skill_refs: [],
    helpers: [{ path: "triage.py", size: 120, executable: true, sha256: "a".repeat(64), text: "print(1)" }],
    memories: [{ key: "tone", locked: false, content: "Be brief." }],
    automations: [
      { kind: "schedule", name: "Morning triage", prompt: "Triage the queue.", cron: "0 9 * * 1-5" },
      { kind: "webhook", name: "Alerts", instructions: "Post alerts to the room.", verification: "signed" },
    ],
    has_avatar: false,
  },
  entries: [
    { index: 0, required: true, why: "It triages pull requests.", choices: [choice("github", "GitHub", [planAccount(40, "github", "GitHub", "acme-bot")], "ask")] },
    { index: 1, required: true, why: "It files cards.", choices: [choice("linear", "Linear", linearConnected ? [planAccount(42, "linear", "Linear", "Linear · acme")] : [], "read")] },
    { index: 2, required: false, why: "It posts updates.", choices: [choice("notion", "Notion", [planAccount(43, "notion", "Notion", "Notes")], "read")] },
  ],
  skill_refs: skillRefs,
});

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

/**
 * Chooses a ChoiceCards card: the native radio is sr-only and its label intercepts the pointer at the radio's
 * click point, so the card's label is clicked — it forwards the click to the radio.
 */
export async function pickCard(root: Locator, name: string | RegExp): Promise<void> {
  await root.getByRole("radio", { name }).locator("xpath=ancestor::label[1]").click();
}

/** What the specs can see of the mocked API after a journey. */
export interface GalleryMock {
  /** The bodies of the writes, by "METHOD path". */
  writes: { key: string; body: unknown }[];
}

/**
 * Routes for the workspace's templates on top of the shared mock (later routes win; the rest falls through to
 * it): the cards of the workspace, the offered version's plan, the create (C-1111) and the account-only connect
 * journey's one request (C-957). The created agent joins the workspace's agents list, so its page opens.
 */
export async function mockWorkspaceTemplates(page: Page): Promise<GalleryMock> {
  await mockApi(page);
  const mock: GalleryMock = { writes: [] };
  let linearConnected = false;
  const created = {
    agent: rexAgent, grants: [], notes: [], skills: [],
    webhooks: [{ webhook: { id: id(70), name: "Alerts" }, url: `/v1/workspaces/${ws.id}/webhooks/${id(70)}`, secret: "whsec-9f8e7d6c5b4a3210" }],
  };
  await page.route(/\/v1\/workspaces\/[^/]+\/agent-templates/, async (route) => {
    const req = route.request();
    const p = new URL(req.url()).pathname;
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    if (req.method() === "GET" && p === `/v1/workspaces/${ws.id}/agent-templates`) return json({ items: [card(), notReadyCard] });
    if (req.method() === "GET" && p === `/v1/workspaces/${ws.id}/agent-templates/schedule-manager`) return json(plan(linearConnected));
    if (req.method() === "GET" && p === `/v1/workspaces/${ws.id}/agent-templates/incident-scribe`) return json(notReadyPlan);
    if (req.method() !== "GET") mock.writes.push({ key: `${req.method()} ${p}`, body: req.postData() ? (req.postDataJSON() ?? {}) : null });
    return route.fallback();
  });
  await page.route(/\/v1\/workspaces\/[^/]+\/agents\/from-template$/, async (route) => {
    const req = route.request();
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    if (req.method() !== "POST") return route.fallback();
    mock.writes.push({ key: "POST /v1/workspaces/{ws}/agents/from-template", body: req.postDataJSON() });
    return json(created, 201);
  });
  await page.route(/\/v1\/apps\/linear\/connect$/, async (route) => {
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    if (route.request().method() !== "POST") return route.fallback();
    linearConnected = true;
    return json({ kind: "connected", connection: { id: id(42), label: "Linear · acme" }, grants: [], missing_scopes: [] });
  });
  await page.route(new RegExp(`/v1/workspaces/${ws.id}/agents$`), async (route) => {
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    if (route.request().method() !== "GET") return route.fallback();
    return json({ items: [agent, rexAgent] });
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
