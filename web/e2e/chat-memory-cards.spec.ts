import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { DEFAULT, MINE, openChat, type Ev, type Fixture } from "./support/chat-mock";
import { agent } from "./support/mock-api";
import { overflow } from "./support/open";

// P5-MEMORY-4 (mem-5, C-1172): the chat says what the agent remembered, or why it could not. The two events are
// recorded transcript events with the mockup's words (docs/design/skills-memory/after/mem-5.png). Who may View, Undo
// or save is covered by the vitest tests and, on the server, by the memory routes' own checks.

const at = (m: number) => new Date(Date.now() - m * 60_000).toISOString();
const ADDED = "PLAT is now the key for the platform team's board.";
const LOCKED = "“Jira project keys” is locked, so Daneel can't change it.";

const ask = (seq: number, m: number, turn: string, text: string): Ev => ({ seq, type: "turn.queued", ts: at(m), payload: { turn_id: turn, speaker_label: "Bob Builder", door: "dashboard", text, attachments: [] } });
const say = (seq: number, m: number, turn: string, text: string): Ev => ({ seq, type: "text", ts: at(m), payload: { turn_id: turn, message_id: `m${seq}`, text } });
const call = (seq: number, m: number, turn: string, id: string, ok: boolean): Ev[] => [
  { seq, type: "tool.started", ts: at(m), payload: { turn_id: turn, call_id: id, name: "Bash", summary: "Bash: aurora-agent call aurora memory_save key=…" } },
  { seq: seq + 1, type: "tool.finished", ts: at(m), payload: { turn_id: turn, call_id: id, ok, summary: ok ? "saved" : "tool_failed: memory_locked" } },
];

const EVENTS: Ev[] = [
  ask(1, 20, "t1", "Remember that the Grafana service account still needs read access to the prod folder."),
  ...call(2, 19, "t1", "c1", true),
  {
    seq: 4, type: "memory.saved", ts: at(19),
    payload: { turn_id: "t1", key: "grafana-service-account", title: "Grafana service account", preview: "Needs read access to the prod folder.", revision: 1, created: true, previous_revision: null },
  },
  say(5, 19, "t1", "Saved. I'll check it the next time we talk about Grafana access."),
  { seq: 6, type: "turn.finished", ts: at(19), payload: { turn_id: "t1", status: "done" } },
  ask(7, 11, "t2", "Also add that PLAT is now the key for the platform team's board."),
  ...call(8, 10, "t2", "c2", false),
  { seq: 10, type: "memory.refused", ts: at(10), payload: { turn_id: "t2", key: "jira-project-keys", title: "Jira project keys", preview: ADDED, code: "memory_locked", message: LOCKED } },
  say(11, 10, "t2", "I couldn't add that one: the memory is locked. You can save it yourself, or unlock it and I'll try again."),
  { seq: 12, type: "turn.finished", ts: at(10), payload: { turn_id: "t2", status: "done" } },
];

const fx: Fixture = { ...DEFAULT, events: { [MINE.id]: EVENTS } };

const memory = (key: string, title: string, content: string, locked: boolean) => ({
  key, title, description: null, content, index_line: `${key}: ${title}`, revision: 1, locked, locked_at: locked ? at(60) : null, locked_by: null,
  created_at: at(60 * 24), updated_at: at(60 * 24), added_by: { kind: "unknown" }, changed_by: { kind: "unknown" }, last_used: null,
});

/** Opens the chat, then lays the memory routes over the shared mock. */
async function open(page: Page, scheme: "light" | "dark") {
  const deleted: string[] = [];
  // Written just before its event (the card reads a memory younger than the event as another one).
  const written = new Date(Date.parse(EVENTS.find((e) => e.type === "memory.saved")!.ts) - 1000).toISOString();
  const items = [
    { ...memory("grafana-service-account", "Grafana service account", "Needs read access to the prod folder.", false), created_at: written, updated_at: written },
    memory("jira-project-keys", "Jira project keys", "OPS is the ops board.", true),
  ];
  await openChat(page, scheme, fx, `?c=${MINE.id}`);
  const base = `/v1/agents/${agent.id}/memories`;
  await page.route("**/v1/**", async (route) => {
    const method = route.request().method();
    const p = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (p === base && method === "GET") return json({ items: items.filter((m) => !deleted.includes(m.key)), next_cursor: null });
    if (p.startsWith(`${base}/`)) {
      const key = p.slice(base.length + 1);
      const found = items.find((m) => m.key === key && !deleted.includes(m.key));
      if (method === "GET") return found ? json(found) : json({ error: { code: "not_found", message: "Memory not found." } }, 404);
      if (method === "DELETE") {
        deleted.push(key);
        return route.fulfill({ status: 204 });
      }
    }
    return route.fallback();
  });
  // The chat was opened before these routes: read the cards' memories again through them.
  await page.reload();
  await expect(page.getByTestId("memory-card")).toHaveCount(2);
  return { deleted };
}

async function serious(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => v.id);
}

const SHOTS = process.env.AURORA_E2E_SHOTS;

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`memory cards in the chat, ${scheme}, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("both cards say what happened, and nothing is called Deny", async ({ page }) => {
        await open(page, scheme);
        const [saved, refused] = [page.getByTestId("memory-card").nth(0), page.getByTestId("memory-card").nth(1)];
        await expect(saved).toContainText("Daneel remembered");
        await expect(saved).toContainText("Grafana service account");
        await expect(saved).toContainText("Needs read access to the prod folder.");
        await expect(saved.getByRole("link", { name: "View" })).toBeVisible();
        await expect(saved.getByRole("button", { name: "Undo" })).toBeVisible();
        await expect(refused).toContainText("Daneel tried to remember this but couldn't");
        await expect(refused).toContainText(`${LOCKED} What it wanted to add:`);
        await expect(refused).toContainText(ADDED);
        await expect(refused.getByRole("button", { name: "Save it yourself" })).toBeVisible();
        await expect(refused.getByRole("button", { name: "Dismiss" })).toBeVisible();
        const log = page.getByRole("log", { name: "Conversation" });
        await expect(log.getByText("Failed").first()).toBeVisible();
        await expect(log.getByText(/deny/i)).toHaveCount(0);
        expect(await overflow(page), "horizontal scroll").toBeFalsy();
        // Each card stays inside the chat column.
        for (const card of [saved, refused]) {
          const box = (await card.boundingBox())!;
          expect(box.x + box.width).toBeLessThanOrEqual(width);
          expect(box.width).toBeLessThanOrEqual(570);
        }
        expect(await serious(page)).toEqual([]);
        if (SHOTS) {
          // For the comparison with the mockup: a tall window, so the whole conversation is in view.
          await page.setViewportSize({ width, height: 1500 });
          await page.getByRole("log", { name: "Conversation" }).screenshot({ path: `${SHOTS}/cards-${scheme}-${width}.png` });
        }
      });

      test("Undo takes the memory away and says so; Dismiss folds the refusal", async ({ page }) => {
        const { deleted } = await open(page, scheme);
        const [saved, refused] = [page.getByTestId("memory-card").nth(0), page.getByTestId("memory-card").nth(1)];
        await saved.getByRole("button", { name: "Undo" }).click();
        await expect(saved).toContainText("Undone ·");
        await expect(saved.getByRole("button", { name: "Redo" })).toBeVisible();
        await expect(page.getByTestId("undo-toast")).toContainText("Memory undone");
        expect(deleted).toEqual(["grafana-service-account"]);
        await refused.getByRole("button", { name: "Dismiss" }).click();
        await expect(refused).not.toContainText(ADDED);
        await expect(refused.getByRole("button", { name: "Show" })).toBeVisible();
        expect(await overflow(page), "horizontal scroll").toBeFalsy();
        expect(await serious(page)).toEqual([]);
        if (SHOTS) await page.screenshot({ path: `${SHOTS}/undone-${scheme}-${width}.png` });
      });

      test("View opens the memory on the Memory page", async ({ page }) => {
        await open(page, scheme);
        await page.getByTestId("memory-card").nth(0).getByRole("link", { name: "View" }).click();
        await expect(page.getByRole("article", { name: "Grafana service account" })).toBeVisible();
        await expect(page).toHaveURL(/tab=memory&memory=grafana-service-account/);
      });

      test("Save it yourself opens the locked memory for editing with the text appended, not saved", async ({ page }) => {
        await open(page, scheme);
        await page.getByTestId("memory-card").nth(1).getByRole("button", { name: "Save it yourself" }).click();
        await expect(page.getByRole("form", { name: "Edit Jira project keys" })).toBeVisible();
        await expect(page.getByRole("textbox", { name: "What should Daneel remember?" })).toHaveValue(`OPS is the ops board.\n\n${ADDED}`);
        await expect(page.getByRole("button", { name: "Save", exact: true })).toBeVisible();
        await expect(page).toHaveURL(/tab=memory&memory=jira-project-keys/);
        expect(page.url()).not.toContain("PLAT");
        expect(await overflow(page), "horizontal scroll").toBeFalsy();
      });
    });
  }
}
