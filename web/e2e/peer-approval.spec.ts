import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { DEFAULT, MINE, openChat, type Ev, type Fixture } from "./support/chat-mock";
import { id, ws } from "./support/mock-api";
import { overflow } from "./support/open";

// FIX-252 (D-489): an approval a delegated agent needs travels back to the conversation the person asked in.
// The API is mocked here; the authorization side (the stored requester only, `can_approve` at the approval's
// workspace, the decide route's own check) is the Rust tests (it/peer_tasks.rs, it/approval_notify.rs and the
// route inventory's REQUESTER list).

const at = (m: number) => new Date(Date.now() - m * 60_000).toISOString();
/** The workspace the approval waits in (the receiving agent's), not the viewer's. */
const TARGET = id(50);
const APPROVAL = id(60);
const TASK = id(70);

const said: Ev = { seq: 2, type: "turn.queued", ts: at(4), payload: { turn_id: "t1", speaker_label: "Ada Lovelace", door: "dashboard", text: "Ask Fixer to push to GitLab." } };
const waiting: Ev = {
  seq: 3, type: "notice", ts: at(2),
  payload: {
    code: "peer_waiting_for_approval", text: "Fixer is waiting for your approval to push to GitLab (asked for by Daneel)",
    approval_id: APPROVAL, peer_task_id: TASK, workspace_id: TARGET, status: "open",
  },
};
const decided: Ev = {
  seq: 4, type: "notice", ts: at(1),
  payload: {
    code: "peer_approval_decided", text: "The approval to push to GitLab was approved by Ada Lovelace.",
    approval_id: APPROVAL, peer_task_id: TASK, workspace_id: TARGET, outcome: "approved",
  },
};

const fx = (events: Ev[]): Fixture => ({ ...DEFAULT, events: { [MINE.id]: events } });

type Posted = { path: string; body: unknown };

/**
 * Opens the chat, then lays this journey's own answers over the shared mock (the last-registered
 * v1 wildcard handler sees each request first and falls back to the shared one for everything else).
 */
async function open(page: Page, scheme: "light" | "dark", events: Ev[], role: "owner" | "member") {
  const posts: Posted[] = [];
  await openChat(page, scheme, fx(events), `?c=${MINE.id}`);
  await page.route("**/v1/**", async (route) => {
    const req = route.request();
    const url = new URL(req.url());
    const p = url.pathname;
    const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
    if (req.method() === "GET" && p === `/v1/workspaces/${TARGET}`) {
      // The target workspace says this reader is a plain member there: the API decides, not the client.
      return json({ ...ws, id: TARGET, slug: "fixer", name: "Fixer's workspace", my_role: role, contacts: {} });
    }
    if (req.method() === "POST") {
      const text = req.postData() ?? "";
      posts.push({ path: p, body: text ? JSON.parse(text) : null });
      if (/^\/v1\/approvals\/[^/]+\/decision$/.test(p)) return json({ id: APPROVAL, status: "approved" });
      if (p === `/v1/approvals/${APPROVAL}/ask-approver`) return json({ asked: 1 });
      if (p === `/v1/peer-tasks/${TASK}/cancel`) return json({ id: TASK, status: "cancelled" });
    }
    return route.fallback();
  });
  return posts;
}

async function serious(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => v.id);
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`peer approval card, ${scheme}, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("a reader who may not decide there sees Ask an approver and Cancel the task — never Approve/Deny", async ({ page }) => {
        const posts = await open(page, scheme, [said, waiting], "member");
        const card = page.getByTestId("peer-approval-card");
        await expect(card).toContainText("Fixer is waiting for your approval to push to GitLab (asked for by Daneel)");
        await expect(card.getByRole("button", { name: "Approve", exact: true })).toHaveCount(0);
        await expect(card.getByRole("button", { name: "Deny", exact: true })).toHaveCount(0);
        expect(await overflow(page), "horizontal scroll").toBeFalsy();
        expect(await serious(page)).toEqual([]);
        await card.getByRole("button", { name: "Ask an approver" }).click();
        await expect(card.getByTestId("peer-asked")).toBeVisible();
        await expect.poll(() => posts.map((p) => p.path)).toEqual([`/v1/approvals/${APPROVAL}/ask-approver`]);
        await card.getByRole("button", { name: "Cancel the task" }).click();
        await expect.poll(() => posts.map((p) => p.path)).toHaveLength(2);
      });

      test("a reader who may decide there gets Approve and Deny, and the answer goes to the decide route", async ({ page }) => {
        // The notice names the viewer's own workspace: the shared mock already answers `my_role: "owner"`.
        const posts = await open(page, scheme, [said, { ...waiting, payload: { ...waiting.payload, workspace_id: ws.id } }], "owner");
        const card = page.getByTestId("peer-approval-card");
        await expect(card.getByRole("button", { name: "Approve", exact: true })).toBeVisible();
        await expect(card.getByRole("button", { name: "Deny", exact: true })).toBeVisible();
        expect(await overflow(page), "horizontal scroll").toBeFalsy();
        expect(await serious(page)).toEqual([]);
        await card.getByRole("button", { name: "Approve", exact: true }).click();
        await expect
          .poll(() => posts.filter((p) => p.path.endsWith("/decision")))
          .toEqual([{ path: `/v1/approvals/${APPROVAL}/decision`, body: { decision: "approve" } }]);
      });

      test("the outcome notice settles the wait to one line", async ({ page }) => {
        await open(page, scheme, [said, waiting, decided], "member");
        await expect(page.getByTestId("peer-approval-card")).toHaveCount(0);
        await expect(page.getByTestId("peer-approval-outcome")).toContainText("Approved");
        await expect(page.getByTestId("peer-approval-outcome")).toContainText("The approval to push to GitLab was approved by Ada Lovelace.");
        expect(await overflow(page), "horizontal scroll").toBeFalsy();
        expect(await serious(page)).toEqual([]);
      });
    });
  }
}
