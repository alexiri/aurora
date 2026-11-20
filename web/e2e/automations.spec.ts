import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { openThemed, overflow, type Scheme } from "./support/open";
import { mockWithTerminal } from "./support/terminal";

// (P5-REFINE-14, D-405) The Automations page against the mocked API: create a schedule by preset, Run now and open the run's own
// thread, Pause and Undo, create a webhook (address and secret shown once, How to call it, Copy), rotate with the dialog, the
// deliveries table. Both themes at 1280 and 390 px, no horizontal scroll, axe with no serious violation. Screenshots go to
// /tmp/aurora-screens/p5-refine-14/ (outside the repo).

const SHOTS = "/tmp/aurora-screens/p5-refine-14";
const PAGE = "/o/acme/w/main/agents/daneel?tab=automations";

const SCHEDULE = {
  id: "00000000-0000-4000-8000-0000000000a1", agent_id: "00000000-0000-4000-8000-0000000000c1", name: "Morning summary", prompt: "Summarise what changed overnight.",
  cron: "0 9 * * *", timezone: "Europe/Zurich", enabled: true, consecutive_failures: 0, conversation_hidden: false, anchor: "creator",
  created_at: "2026-11-20T10:00:00Z", updated_at: "2026-11-20T10:00:00Z", created_by: { id: "u1", display_name: "Ada" }, next_runs: ["2026-11-21T08:00:00Z"], last_result: "answered",
  last_fired_at: "2026-11-20T08:00:00Z",
};
const HOOK = {
  id: "00000000-0000-4000-8000-0000000000b1", agent_id: "00000000-0000-4000-8000-0000000000c1", name: "Deploy notice", instructions: "Say what was deployed.", enabled: true, anchor: "creator",
  created_at: "2026-11-20T10:00:00Z", updated_at: "2026-11-20T10:00:00Z", url_prefix: "https://aurora.example/hooks/whk_ab12", created_by: { id: "u1", display_name: "Ada" },
  verification: { kind: "hmac_sha256", header: "X-Signature", format: "sha256=<hex>" },
};
const RUN = (id: string, over: Record<string, unknown> = {}) => ({
  id, schedule_id: SCHEDULE.id, fired_at: "2026-11-20T08:00:00Z", scheduled_for: "2026-11-20T08:00:00Z", created_at: "2026-11-20T08:00:00Z", outcome: "fired", result: "answered", reason: null, conversation_id: `conv-${id}`, ...over,
});

interface State {
  schedules: unknown[];
  hooks: unknown[];
  patches: unknown[];
}

async function mockAutomations(page: Page, state: State) {
  await mockWithTerminal(page, { output: new Uint8Array(), resizes: [] });
  const reply = (body: unknown, status = 200) => ({ status, contentType: "application/json", body: JSON.stringify(body) });
  await page.route("**/v1/agents/*/presences", (route) => route.fulfill(reply({ items: [], next_cursor: null })));
  await page.route("**/v1/agents/*/presence-options", (route) => route.fulfill(reply({ items: [] })));
  await page.route("**/v1/agents/*/schedules", async (route) => {
    if (route.request().method() === "POST") {
      state.schedules = [SCHEDULE];
      return route.fulfill(reply(SCHEDULE, 201));
    }
    return route.fulfill(reply({ items: state.schedules, next_cursor: null }));
  });
  await page.route("**/v1/agents/*/schedules/preview", (route) =>
    route.fulfill(reply({ next: ["2026-11-21T08:00:00Z", "2026-11-22T08:00:00Z", "2026-11-23T08:00:00Z"], interval_secs: 86400, too_frequent: false, min_interval_secs: 300 })),
  );
  await page.route("**/v1/schedules/*/run-now", (route) => route.fulfill(reply(RUN("r-now", { result: "running", conversation_id: "conv-run-1" }), 201)));
  await page.route("**/v1/schedules/*/runs**", (route) =>
    route.fulfill(reply({ items: [RUN("r1"), RUN("r2", { outcome: "gated_quiet", result: "skipped_by_gate", conversation_id: null })], next_cursor: null })),
  );
  await page.route(/\/v1\/schedules\/[^/]+$/, async (route) => {
    const body = route.request().postDataJSON() as { enabled?: boolean } | null;
    state.patches.push(body);
    return route.fulfill(reply({ ...SCHEDULE, enabled: body?.enabled ?? true }));
  });
  await page.route("**/v1/agents/*/webhooks", async (route) => {
    if (route.request().method() === "POST") {
      state.hooks = [HOOK];
      return route.fulfill(reply({ url: "https://aurora.example/hooks/whk_ab12_FULLADDRESS", secret: "gen3rated5ecretValue0123456789abcd", webhook: HOOK }, 201));
    }
    return route.fulfill(reply({ items: state.hooks, next_cursor: null }));
  });
  await page.route("**/v1/webhooks/*/rotate", (route) => route.fulfill(reply({ url: "https://aurora.example/hooks/whk_ROTATED_ADDRESS", webhook: HOOK })));
  await page.route("**/v1/webhooks/*/deliveries**", (route) =>
    route.fulfill(
      reply({
        items: [
          { id: "d1", received_at: "2026-11-20T08:00:00Z", size: 31, status: "accepted", result: "answered", conversation_id: "conv-d1", duplicate_of: null },
          { id: "d2", received_at: "2026-11-20T07:00:00Z", size: 31, status: "rejected_signature", result: "refused_signature", conversation_id: null, duplicate_of: null },
        ],
        next_cursor: null,
      }),
    ),
  );
}

async function serious(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).slice(0, 4).join(" | ")}`);
}
async function check(page: Page) {
  expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
  expect(await serious(page)).toEqual([]);
}

for (const scheme of ["light", "dark"] as const satisfies readonly Scheme[]) {
  for (const width of [1280, 390]) {
    test.describe(`Automations (${width} px, ${scheme})`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("the empty page, then a schedule by preset with the next runs", async ({ page }) => {
        const errors: string[] = [];
        page.on("pageerror", (e) => errors.push(e.message));
        const state: State = { schedules: [], hooks: [], patches: [] };
        await mockAutomations(page, state);
        await openThemed(page, PAGE, scheme);
        await expect(page.getByRole("heading", { name: "No automations yet" })).toBeVisible();
        await check(page);
        await page.screenshot({ path: `${SHOTS}/empty-${scheme}-${width}.png` });
        await page.getByRole("button", { name: "New automation" }).first().click();
        await page.getByRole("button", { name: /On a schedule/ }).click();
        const form = page.getByRole("form", { name: "New schedule" });
        await expect(form.getByRole("radio", { name: "Every day" })).toBeChecked();
        await expect(form.getByRole("radio", { name: /^Every minute/ })).toHaveCount(0);
        await form.getByLabel("Name").fill("Morning summary");
        await form.getByLabel("Prompt").fill("Summarise what changed overnight.");
        await expect(form.getByTestId("schedule-summary")).toContainText("Every day at 09:00");
        await expect(form.getByTestId("schedule-summary")).toContainText("Schedules run at most every 5 minutes.");
        await check(page);
        await page.screenshot({ path: `${SHOTS}/schedule-form-${scheme}-${width}.png` });
        await form.getByRole("button", { name: "Create schedule" }).click();
        await expect(page.getByText("Morning summary")).toBeVisible();
        expect(errors).toEqual([]);
      });

      test("Run now opens the run's own thread; Pause has an Undo", async ({ page }) => {
        const state: State = { schedules: [SCHEDULE], hooks: [], patches: [] };
        await mockAutomations(page, state);
        await openThemed(page, PAGE, scheme);
        await expect(page.getByText("Every day at 09:00 · Zurich")).toBeVisible();
        await check(page);
        await page.getByRole("button", { name: "Run now" }).click();
        const started = page.getByTestId("run-started");
        await expect(started).toContainText("Started · Open the conversation");
        // (schedules-3) The same words as a toast at every width; its link opens the run's own thread.
        const toast = page.getByTestId("undo-toast");
        await expect(toast).toContainText("Started · Open the conversation");
        await page.screenshot({ path: `${SHOTS}/run-now-${scheme}-${width}.png` });
        await toast.getByRole("link", { name: "Open the conversation" }).click();
        await expect(page).toHaveURL(/tab=chat/);
        await expect(page).toHaveURL(/c=conv-run-1/);
        await page.goto(PAGE);
        await page.getByRole("button", { name: "More for Morning summary" }).click();
        await page.getByRole("menuitem", { name: "Pause" }).click();
        await expect(page.getByText("Paused Morning summary")).toBeVisible();
        await page.getByRole("button", { name: "Undo" }).click();
        await expect.poll(() => state.patches).toEqual([{ enabled: false }, { enabled: true }]);
      });

      test("Runs: a table in words with links to each run's thread", async ({ page }) => {
        await mockAutomations(page, { schedules: [SCHEDULE], hooks: [], patches: [] });
        await openThemed(page, PAGE, scheme);
        // Below 640 px Runs stays on the row; above it is in the menu.
        if (width <= 640) await page.getByRole("button", { name: "Runs", exact: true }).click();
        else {
          await page.getByRole("button", { name: "More for Morning summary" }).click();
          await page.getByRole("menuitem", { name: "Runs" }).click();
        }
        const table = page.getByRole("table", { name: "Runs" });
        await expect(table).toContainText("Answered");
        await expect(table).toContainText("Skipped by the gate · nothing to report");
        // (schedules-3) The link reads like the thread's own name: "<Schedule name> · <time>".
        await expect(table.getByRole("link", { name: /^Morning summary · \d{1,2} \w{3}( \d{4})? \d{2}:\d{2}$/ }).first()).toHaveAttribute("href", /c=conv-r1/);
        await check(page);
        await page.screenshot({ path: `${SHOTS}/runs-${scheme}-${width}.png` });
      });

      test("a webhook: signed by default, address and secret shown once, How to call it, Copy, Rotate, Deliveries", async ({ page, context }) => {
        await context.grantPermissions(["clipboard-read", "clipboard-write"]).catch(() => undefined);
        const state: State = { schedules: [], hooks: [], patches: [] };
        await mockAutomations(page, state);
        await openThemed(page, PAGE, scheme);
        await page.getByRole("button", { name: "New automation" }).first().click();
        await page.getByRole("button", { name: /When something calls it/ }).click();
        const form = page.getByRole("form", { name: "New webhook" });
        await expect(form.getByRole("radio", { name: /Signed/ })).toBeChecked();
        await expect(form.getByText("Only senders that know the secret can wake Dani.")).toBeVisible();
        await form.getByLabel("Name").fill("Deploy notice");
        await form.getByLabel("Instructions").fill("Say what was deployed.");
        await form.getByRole("button", { name: "Generate" }).click();
        await expect(form.getByLabel("Signing secret")).toHaveValue(/^[A-Za-z0-9]{32}$/);
        await check(page);
        await page.screenshot({ path: `${SHOTS}/webhook-form-${scheme}-${width}.png` });
        await form.getByRole("button", { name: "Create webhook" }).click();
        const dialog = page.getByRole("dialog");
        await expect(dialog.getByTestId("secret-key").first()).toHaveText("https://aurora.example/hooks/whk_ab12_FULLADDRESS");
        await expect(dialog.getByRole("heading", { name: "How to call it" })).toBeVisible();
        const snippet = dialog.getByTestId("howto-snippet");
        await expect(snippet).toContainText("X-Signature: sha256=");
        await expect(snippet).not.toContainText("gen3rated5ecret");
        await check(page);
        await page.screenshot({ path: `${SHOTS}/webhook-created-${scheme}-${width}.png` });
        await dialog.getByRole("button", { name: "Copy the curl example" }).click();
        await expect.poll(() => page.evaluate(() => navigator.clipboard.readText()).catch(() => "")).toContain("openssl dgst -sha256");
        await dialog.getByRole("button", { name: "I have saved it" }).click();
        await dialog.getByRole("button", { name: "Close anyway" }).click();
        await expect(page.getByText("Deploy notice")).toBeVisible();
        expect(await page.content()).not.toContain("FULLADDRESS");

        await page.getByRole("button", { name: "More for Deploy notice" }).click();
        await page.getByRole("menuitem", { name: "Rotate URL…" }).click();
        const rotate = page.getByRole("dialog");
        await expect(rotate).toContainText("The old URL stops working now. Update every sender that uses it. The new URL is shown once.");
        await page.screenshot({ path: `${SHOTS}/webhook-rotate-${scheme}-${width}.png` });
        await rotate.getByRole("button", { name: "Cancel" }).click();
        await expect(rotate).toBeHidden();

        if (width <= 640) await page.getByRole("button", { name: "Deliveries", exact: true }).click();
        else {
          await page.getByRole("button", { name: "More for Deploy notice" }).click();
          await page.getByRole("menuitem", { name: "Deliveries" }).click();
        }
        const table = page.getByRole("table", { name: "Deliveries" });
        await expect(table).toContainText("Accepted");
        await expect(table).toContainText("Refused: wrong signature");
        await expect(table).not.toContainText("rejected_signature");
        await check(page);
        await page.screenshot({ path: `${SHOTS}/deliveries-${scheme}-${width}.png` });
      });

      test("the old ?tab ids open Automations on their kind", async ({ page }) => {
        await mockAutomations(page, { schedules: [SCHEDULE], hooks: [HOOK], patches: [] });
        await openThemed(page, "/o/acme/w/main/agents/daneel?tab=webhooks", scheme);
        await expect(page.getByText("Deploy notice")).toBeVisible();
        await expect(page.getByText("Morning summary")).toHaveCount(0);
        await expect(page.getByRole("radio", { name: "Webhooks 1" })).toBeChecked();
      });
    });
  }
}
