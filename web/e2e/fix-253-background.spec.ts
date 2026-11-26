import { expect, test, type Page } from "@playwright/test";

import { DEFAULT, MINE, openChat, type Ev, type Fixture } from "./support/chat-mock";

// (FIX-253, D-490) The **Background activity** notice: what the harness child keeps doing after
// the turn ended, with Stop; the durable `background_stopped` notice mutes the row. The API is
// mocked; the platform side is the API tests' `background_activity_is_visible_deduped_and_stoppable`.

const at = (m: number) => new Date(Date.now() - m * 60_000).toISOString();
const said: Ev = { seq: 1, type: "turn.queued", ts: at(6), payload: { turn_id: "t1", speaker_label: "Ada Lovelace", door: "dashboard", text: "Run the e2e suite." } };
const done: Ev = { seq: 2, type: "turn.finished", ts: at(5), payload: { turn_id: "t1", status: "done" } };
const activity: Ev = {
  seq: 3, type: "notice", ts: at(4),
  payload: {
    code: "background_activity", turn_id: "t1",
    text: "Background activity, still running: `npm run e2e:full`",
    items: [{ command: "npm run e2e:full", started_at: 1700000000 }],
  },
};
const stopped: Ev = { seq: 4, type: "notice", ts: at(1), payload: { code: "background_stopped", turn_id: "t1", text: "Background activity stopped." } };

const fx = (events: Ev[]): Fixture => ({ ...DEFAULT, events: { [MINE.id]: events } });

test("the running entry shows its command and Stop calls the abort route", async ({ page }) => {
  const stops: string[] = [];
  await openChat(page, "light", fx([said, done, activity]), `?c=${MINE.id}`);
  await page.route("**/v1/turns/t1/abort", async (route) => {
    stops.push(new URL(route.request().url()).pathname);
    await route.fulfill({ status: 202, contentType: "application/json", body: JSON.stringify({ id: "t1", abort_requested: true }) });
  });
  const row = page.getByTestId("background-activity");
  await expect(row).toContainText("Background activity, still running: `npm run e2e:full`");
  await row.getByRole("button", { name: "Stop" }).click();
  await expect.poll(() => stops).toHaveLength(1);
});

test("once stopped the row mutes away and the durable line stays", async ({ page }) => {
  await openChat(page, "light", fx([said, done, activity, stopped]), `?c=${MINE.id}`);
  await expect(page.getByTestId("background-activity")).toHaveCount(0);
  await expect(page.getByText("Background activity stopped.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Stop" })).toHaveCount(0);
});

// (FIX-253, D-490) The self-check notice with its **Remind it in 15 minutes** action; the durable
// `no_wakeup_set` notice disables the button.
const wakeup: Ev = { seq: 3, type: "notice", ts: at(4), payload: { code: "no_wakeup", turn_id: "t1", text: "Daneel said it would continue, but nothing is set to wake it." } };
const reminderSet: Ev = { seq: 4, type: "notice", ts: at(1), payload: { code: "no_wakeup_set", turn_id: "t1", reminder_for: "09:15", text: "A reminder is set for 09:15." } };

test("the reminder notice offers Remind it in 15 minutes and posts once", async ({ page }) => {
  const posts: string[] = [];
  await openChat(page, "light", fx([said, done, wakeup]), `?c=${MINE.id}`);
  await page.route("**/v1/conversations/*/reminder", async (route) => {
    posts.push(new URL(route.request().url()).pathname);
    await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ reminder_for: "09:15" }) });
  });
  const row = page.getByTestId("no-wakeup");
  await expect(row).toContainText("said it would continue, but nothing is set to wake it");
  await row.getByRole("button", { name: "Remind it in 15 minutes" }).click();
  await expect.poll(() => posts).toHaveLength(1);
});

test("once the reminder is set the button is out and the durable line says the time", async ({ page }) => {
  await openChat(page, "light", fx([said, done, wakeup, reminderSet]), `?c=${MINE.id}`);
  const row = page.getByTestId("no-wakeup");
  await expect(page.getByText("A reminder is set for 09:15.")).toBeVisible();
  await expect(row.getByRole("button", { name: "Remind it in 15 minutes" })).toBeDisabled();
});

/** Accessibility of the running-entry row, in both schemes. */
for (const scheme of ["light", "dark"] as const) {
  test(`background activity row has no serious axe violations (${scheme})`, async ({ page }) => {
    const { AxeBuilder } = (await import("@axe-core/playwright")) as typeof import("@axe-core/playwright");
    await openChat(page, scheme, fx([said, done, activity]), `?c=${MINE.id}`);
    const { violations } = await new AxeBuilder({ page }).analyze();
    expect(
      violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => v.id),
    ).toEqual([]);
  });
}
