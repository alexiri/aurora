import { expect, test, type Page, type Route } from "@playwright/test";

import { ANN_DIRECT, mockChat, WITH_REDACTED } from "./support/chat-mock";
import { mockEmergencyReader } from "./support/emergency-reader";
import { mockMemories } from "./support/memory";
import { agent, org, ws } from "./support/mock-api";
import { overflow } from "./support/open";

// FIX-277: what the redirect used to hide. An emergency reader now reaches every page under `/o/<org>/w/<ws>`: what the
// grant covers is there to read, nothing offers to act, and no page asks for what the server would refuse. The mock
// answers as the API does (support/emergency-reader.ts); each test also reads the page's own network log for a 403.

const BASE = "/o/acme/w/main";
const AGENT = `${BASE}/agents/daneel`;

const SCHEDULE = {
  id: "00000000-0000-4000-8000-0000000000a1", agent_id: agent.id, name: "Morning summary", prompt: "Summarise what changed overnight.",
  cron: "0 9 * * *", timezone: "Europe/Zurich", enabled: true, consecutive_failures: 0, conversation_hidden: false, anchor: "creator",
  created_at: "2026-11-20T10:00:00Z", updated_at: "2026-11-20T10:00:00Z", created_by: { id: "u1", display_name: "Ada" }, next_runs: ["2026-11-28T08:00:00Z"],
  last_result: "answered", last_fired_at: "2026-11-20T08:00:00Z",
};
const HOOK = {
  id: "00000000-0000-4000-8000-0000000000b1", agent_id: agent.id, name: "Deploy notice", instructions: "Say what was deployed.", enabled: true, anchor: "creator",
  created_at: "2026-11-20T10:00:00Z", updated_at: "2026-11-20T10:00:00Z", url_prefix: "auw_9xLPEkbX", created_by: { id: "u1", display_name: "Ada" },
  verification: { kind: "hmac_sha256", header: "X-Signature", format: "sha256=<hex>" },
};

/** A conversation with the cards that read more than the transcript: a held approval and a failed start that has a retry. */
const EVENTS = [
  ...WITH_REDACTED,
  { seq: 6, type: "turn.queued", ts: new Date().toISOString(), payload: { turn_id: "t9", speaker_label: "Grace Hopper", door: "dashboard", text: "Run the report.", attachments: [] } },
  { seq: 7, type: "approval.pending", ts: new Date().toISOString(), payload: { turn_id: "t9", approval_id: "00000000-0000-4000-8000-0000000000d1", summary: "Merge pull request #42" } },
  { seq: 8, type: "error", ts: new Date().toISOString(), payload: { turn_id: "t9", code: "sandbox_start_failed", message: "The computer did not start." } },
  { seq: 9, type: "turn.finished", ts: new Date().toISOString(), payload: { turn_id: "t9", status: "failed", error_code: "sandbox_start_failed" } },
];

/** (FIX-280) A skill with a file, on the agent's list and on the workspace's. */
const skillWithFile = (tier: "agent" | "workspace") => ({
  id: tier === "agent" ? "00000000-0000-4000-8000-0000000000e1" : "00000000-0000-4000-8000-0000000000e2",
  org_id: org.id, agent_id: tier === "agent" ? agent.id : null, workspace_id: tier === "workspace" ? ws.id : null, scope: tier, tier,
  name: "triage-alerts", title: null, description: "Look at firing alerts.", body: "# Triage alerts\n\nRun the script.", enabled: true,
  files: [{ path: "scripts/firing.sh", size: 130, executable: true }], total_size: 40, overridden_in: null, used_by_agents: tier === "agent" ? null : 1,
  source: null, created_at: "2026-11-20T10:00:00Z", updated_at: "2026-11-20T10:00:00Z",
});

/** The reader's API over a workspace that has something on every page: conversations, memories, a schedule, a webhook. */
async function open(page: Page, path: string) {
  const answered403: string[] = [];
  page.on("response", (r) => {
    if (r.status() === 403) answered403.push(`${r.request().method()} ${new URL(r.url()).pathname}`);
  });
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  const refused = await mockEmergencyReader(page, async () => {
    await mockChat(page, { direct: ANN_DIRECT, active: [ANN_DIRECT], archived: [], events: { [ANN_DIRECT.id]: EVENTS } });
    await mockMemories(page);
    const list = (items: unknown[]) => (route: Route) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ items, next_cursor: null }) });
    await page.route(/\/v1\/agents\/[^/]+\/schedules$/, list([SCHEDULE]));
    await page.route(/\/v1\/agents\/[^/]+\/webhooks$/, list([HOOK]));
    await page.route(/\/v1\/schedules\/[^/]+\/runs(\?.*)?$/, list([]));
    await page.route(/\/v1\/webhooks\/[^/]+\/deliveries(\?.*)?$/, list([]));
    await page.route(/\/v1\/agents\/[^/]+\/skills$/, list([skillWithFile("agent")]));
    await page.route(/\/v1\/workspaces\/[^/]+\/skills$/, list([skillWithFile("workspace")]));
  });
  await page.goto(path);
  // The terminal's page has no shell, so no banner: there the "not covered" notice is what shows.
  await expect(page.getByTestId("emergency-reader-banner").or(page.getByTestId("emergency-not-covered")).first()).toBeVisible({ timeout: 20_000 });
  /** Waits for the page to go quiet, then: nothing was asked that the grant would refuse, and nothing is broken. */
  const clean = async (what = path, alerts = 0) => {
    await page.waitForLoadState("networkidle");
    // The slowest first reads start after a render or two.
    await page.waitForTimeout(750);
    expect(refused, `${what}: requests the grant would refuse`).toEqual([]);
    expect(answered403, `${what}: 403s in the network log`).toEqual([]);
    await expect(page.locator("main").getByRole("alert"), what).toHaveCount(alerts);
    expect(await overflow(page), `${what}: no sideways scroll`).toBe(0);
    expect(errors, what).toEqual([]);
  };
  return clean;
}

const none = async (page: Page, names: (string | RegExp)[]) => {
  const main = page.locator("main");
  for (const name of names) {
    await expect(main.getByRole("button", { name, exact: true }), String(name)).toHaveCount(0);
    await expect(main.getByRole("link", { name, exact: true }), String(name)).toHaveCount(0);
  }
};

for (const width of [1256, 390]) {
  test.describe(`${width} px`, () => {
    test.use({ viewport: { width, height: 900 } });

    test("Overview shows what the grant covers, with nothing to start and nothing refused", async ({ page }) => {
      const clean = await open(page, BASE);
      const agents = page.getByRole("list", { name: "Agents" });
      await expect(agents.getByRole("link", { name: agent.name, exact: true })).toBeVisible();
      await expect(page.getByRole("heading", { name: "Recent activity" })).toBeVisible();
      await expect(page.getByRole("link", { name: "View all activity" })).toBeVisible();
      await clean();
      await none(page, ["New agent", "Start from a template", "Chat", `Chat with ${agent.name}`, "Show me around", "Dismiss"]);
      await expect(page.getByRole("heading", { name: "Getting started" })).toHaveCount(0);
      await expect(page.getByRole("heading", { name: /Needs you/ })).toHaveCount(0);
      await expect(page.getByTestId("agent-summary")).toHaveCount(0);
      await expect(page.locator("main")).not.toContainText("Nothing needs you");
    });
  });
}

test("Chat opens another person's conversation to read, and creates none", async ({ page }) => {
  const clean = await open(page, `${AGENT}?tab=chat`);
  await page.getByRole("button", { name: /Grace Hopper/ }).click();
  await expect(page.getByTestId("emergency-thread-notice")).toBeVisible();
  await expect(page.locator("#chat-input")).toHaveCount(0);
  await expect(page.getByText("Merge pull request #42").first()).toBeVisible();
  // The failed turn's card is an alert of the transcript; it is the one this page may show.
  await clean(undefined, 1);
  await none(page, ["New chat", "Conversation actions", "Status", "Stop", "Retry now", "Approve", "Deny", "Always allow", /^More for /, /^Actions for /]);
  await expect(page.getByTestId("agent-state")).toHaveCount(0);
  await expect(page.getByTestId("agent-header-needs-update")).toHaveCount(0);
});

test("Automations lists schedules and webhooks with their runs, and nothing to change them", async ({ page }) => {
  const clean = await open(page, `${AGENT}?tab=automations`);
  const rows = page.getByRole("list", { name: "Automations" });
  await expect(rows.getByRole("heading", { name: /Morning summary/ })).toBeVisible();
  await expect(rows.getByRole("heading", { name: /Deploy notice/ })).toBeVisible();
  await rows.getByRole("button", { name: "Runs" }).click();
  await expect(page.getByText("No runs yet.")).toBeVisible();
  await rows.getByRole("button", { name: "Deliveries" }).click();
  await expect(page.getByText("No deliveries yet.")).toBeVisible();
  await clean();
  await none(page, ["New automation", "Run now", "Edit", "Run again", "More for Morning summary", "More for Deploy notice", "Send request", /^Chats/]);
  await expect(page.locator("main").getByRole("checkbox")).toHaveCount(0);
});

test("the old Channels address opens the same list, not a filter of chats the grant doesn't cover", async ({ page }) => {
  const clean = await open(page, `${AGENT}?tab=channels`);
  await expect(page.getByRole("list", { name: "Automations" }).getByRole("heading", { name: /Morning summary/ })).toBeVisible();
  await clean();
});

for (const [what, path, there] of [
  ["Instructions", `${AGENT}?tab=instructions`, "What Daneel should do and how."],
  ["Memory", `${AGENT}?tab=memory`, "Aurora migration project"],
  ["an agent's Skills", `${AGENT}?tab=skills`, "What it knows how to do"],
  ["the agents list", `${BASE}/agents`, "The agents in this workspace."],
  ["Activity", `${BASE}/activity`, "What your agents did"],
  ["Activity's usage", `${BASE}/activity?tab=usage`, "Activity"],
  ["the workspace's Skills", `${BASE}/skills`, "Skills every agent in"],
  ["People", `${BASE}/people`, "Emergency access"],
] as const) {
  test(`${what} reads with nothing refused`, async ({ page }) => {
    const clean = await open(page, path);
    await expect(page.locator("main")).toContainText(there);
    await clean();
  });
}

// (FIX-280) A skill's files are listed for a reader, but the grant doesn't cover them: nothing opens or asks for one.
for (const [what, path] of [
  ["an agent's", `${AGENT}?tab=skills`],
  ["the workspace's", `${BASE}/skills`],
] as const) {
  test(`${what} skill lists its file without opening it, and asks for no file`, async ({ page }) => {
    const asked: string[] = [];
    page.on("request", (r) => {
      if (new URL(r.url()).pathname.includes("/files/")) asked.push(r.url());
    });
    const clean = await open(page, path);
    await page.getByRole("list", { name: "Skills" }).getByRole("button").first().click();
    const files = page.getByRole("region", { name: "Files · 2" });
    await expect(files.getByText("firing.sh", { exact: true })).toBeVisible();
    await files.getByText("firing.sh", { exact: true }).click();
    await clean();
    expect(asked, "file requests").toEqual([]);
    await expect(files.getByText("Emergency read access doesn’t cover a skill’s files.")).toBeVisible();
    await expect(files.getByRole("button")).toHaveCount(0);
  });
}

// The rail doesn't offer these; a typed or remembered address reaches them.
for (const path of ["/settings", "/apps", "/apps?tab=apps", "/apps/github", "/apps/github/connect", "/internet-access", "/approvals", "/welcome", "/credentials", "/agents/new/t1", "/agents/daneel/terminal"]) {
  test(`${path} says the grant doesn't cover it, and asks for nothing`, async ({ page }) => {
    const clean = await open(page, `${BASE}${path}`);
    await expect(page.getByTestId("emergency-not-covered")).toContainText("Emergency read access doesn’t cover this page.");
    await clean();
    await expect(page.locator("main").getByRole("textbox")).toHaveCount(0);
    await page.getByTestId("emergency-not-covered").getByRole("link", { name: "Read the workspace’s agents" }).click();
    await page.waitForURL(`**${BASE}/agents`);
    expect(new URL(page.url()).pathname, "the reader stays in the workspace").toBe(`/o/acme/w/${ws.slug}/agents`);
  });
}
