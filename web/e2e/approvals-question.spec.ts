import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { id, mockApi, ws } from "./support/mock-api";
import { overflow } from "./support/open";

// P5-UX-16 (D-348, D-353, C-982, C-983): the approval as a sentence with three answers. The API is mocked; the
// journey against the real stack (an agent pauses, Always allow, the agent continues, the Access tab keeps Ask before
// changes) is `scripts/e2e/approvals-park.sh` + the hosted suite.

const future = new Date(Date.now() + 14 * 60_000).toISOString();
const question = (n: number, over: Record<string, unknown> = {}) => ({
  id: id(700 + n), org_id: id(1), workspace_id: ws.id, status: "pending", created_at: new Date().toISOString(), deadline: future,
  host: "api.linear.app", method: "POST", path: "/graphql", app_id: "linear", conversation_id: id(300),
  agent: { id: id(11), handle: "daneel", name: "Daneel" }, summary: { source: "template", title: "comment on an issue", fields: [{ label: "Issue", value: "PLAT-412" }] },
  sentence: { agent: { id: id(11), name: "Daneel" }, action: "comment on an issue", app: { app_id: "linear", name: "Linear" }, account: { ref: { type: "connection", id: id(31) }, name: "Bob's Linear" } },
  asked_by: { kind: "access", agent_id: id(11), connection_id: id(31), tool_id: "comment" }, can_always_allow: true, ...over,
});
const ACCESS = question(1);
const RULE = question(2, {
  sentence: { agent: { id: id(11), name: "Daneel" }, action: "send a POST request to status.example.org" },
  asked_by: { kind: "rule", rule_id: id(81), rule_name: "Ask before changes on status.example.org", scope: "workspace" }, can_always_allow: false,
  host: "status.example.org", app_id: null, summary: { source: "generic", title: "POST status.example.org", fields: [] },
});

async function open(page: Page, scheme: "light" | "dark", items: unknown[], query = "") {
  const bodies: unknown[] = [];
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockApi(page);
  await page.route("**/v1/**", async (route) => {
    const req = route.request();
    const p = new URL(req.url()).pathname;
    const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
    if (req.method() === "GET" && p === `/v1/workspaces/${ws.id}/approvals`) return json({ items, next_cursor: null });
    if (req.method() === "POST" && /^\/v1\/approvals\/[^/]+\/decision$/.test(p)) {
      bodies.push(req.postDataJSON());
      return json({ ...(ACCESS as object), status: "approved", always_applied: { tool: { id: "comment", name: "Comment on an issue" }, also_approved: 2 } });
    }
    return route.fallback();
  });
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(`/o/acme/w/main/approvals${query}`);
  await page.waitForLoadState("networkidle");
  expect(errors, "the page threw").toEqual([]);
  return bodies;
}

for (const scheme of ["light", "dark"] as const) {
  test.describe(`approvals page, ${scheme}`, () => {
    test("each question reads as a sentence; Always allow is offered only for the access's question", async ({ page }) => {
      await open(page, scheme, [ACCESS, RULE]);
      await expect(page.getByRole("tab", { name: "Waiting" })).toBeVisible();
      await expect(page.getByText("comment on an issue")).toBeVisible();
      await expect(page.getByText("Bob's Linear")).toBeVisible();
      await expect(page.getByRole("button", { name: "Always allow" })).toHaveCount(1);
      await expect(page.getByTestId("asked-by-rule")).toContainText("Asked by the rule Ask before changes on status.example.org");
      expect(await overflow(page), "horizontal scroll").toBeFalsy();
      const axe = await new AxeBuilder({ page }).analyze();
      expect(axe.violations.map((v) => v.id)).toEqual([]);
    });

    test("Always allow sends always: true and says what it changed", async ({ page }) => {
      const bodies = await open(page, scheme, [ACCESS]);
      await page.getByRole("button", { name: "Always allow" }).click();
      await expect.poll(() => bodies).toEqual([{ decision: "approve", always: true }]);
      await expect(page.getByTestId("always-allowed")).toContainText("Daneel will no longer ask before it wants to comment on an issue in Linear.");
      await expect(page.getByTestId("always-allowed")).toContainText("2 other waiting requests");
    });

    test("?focus puts that question first", async ({ page }) => {
      await open(page, scheme, [ACCESS, RULE], `?focus=${RULE.id}`);
      const first = page.locator("li[id^='approval-'], div[id^='approval-']").first();
      await expect(first).toHaveAttribute("id", `approval-${RULE.id}`);
    });

    test("empty Waiting and History say what they mean", async ({ page }) => {
      await open(page, scheme, []);
      await expect(page.getByRole("heading", { name: "Nothing is waiting for you." })).toBeVisible();
      await page.getByRole("tab", { name: "History" }).click();
      await expect(page.getByText("No questions have been answered yet.")).toBeVisible();
    });
  });
}

test("390 px: no horizontal scroll", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 800 });
  await open(page, "light", [ACCESS, RULE]);
  expect(await overflow(page)).toBeFalsy();
});

// The longest card there is: an alert rule on a log count shows every part of the rule, and its query is up to 200
// characters, here without a single space to break at.
const LOG_QUERY = `message:${"upstream_timed_out.".repeat(10)}`;
const ALERT_RULE = question(3, {
  sentence: { agent: { id: id(11), name: "Daneel" }, action: "create an alert rule", app: { app_id: "grafana", name: "Grafana" } },
  summary: {
    source: "template", title: "create an alert rule",
    fields: [
      ["Title", "web 500s"], ["Folder", "night-folder"], ["Group", "agent-night"], ["Evaluate for", "120s"], ["Data source", "os-logs"],
      ["Query language", "lucene"], ["Log query", LOG_QUERY], ["Metric", "avg"], ["Metric field", "http_response_duration_milliseconds"],
      ["Buckets", "1m"], ["Bucket field", "@timestamp"], ["Time field", "@timestamp"], ["Looks back (seconds)", "960"],
      ["Ends before now (seconds)", "60"], ["Comparison", "gt"], ["Threshold", "25"], ["Reducer", "mean"], ["Second threshold", "50"],
      ["Condition step", "C"], ["No data state", "NoData"], ["On query error", "Error"], ["Paused", "false"], ["Managed by", "night"],
      ["Service", "web"], ["Expires", "2026-12-02"], ["Summary", "web 500s"], ["Expires annotation", "2026-12-02"], ["Receiver", "oncall"],
      ["Evaluations to resolve", "2"],
    ].map(([label, value]) => ({ label, value })),
  },
});

test("390 px: a log alert rule's card shows every row and its whole query, without horizontal scroll", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 800 });
  await open(page, "light", [ALERT_RULE]);
  await expect(page.getByText("Evaluations to resolve")).toBeVisible();
  const query = page.getByText(LOG_QUERY);
  await expect(query).toBeVisible();
  expect(await overflow(page), "horizontal scroll").toBeFalsy();
  // Wrapped inside the viewport, not clipped by a parent.
  const box = (await query.boundingBox())!;
  expect(box.x + box.width).toBeLessThanOrEqual(390);});
