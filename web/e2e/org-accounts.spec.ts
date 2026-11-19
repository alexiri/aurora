import { expect, test, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";

import { agent, id, mockApi, now, org, ws } from "./support/mock-api";
import { overflow } from "./support/open";
import { serious } from "./support/axe";

// P5-UX-11 (C-936, C-937, C-947, C-990, SEC-131; docs/14 §Agents, accounts and access §2 and §11): accounts for the whole
// organization. Three journeys against a stateful mock of the organization's account:
//  1. an organization owner connects AWS for the organization with the limit Read only and gives it to two agents in two
//     workspaces, each with its own choice;
//  2. a workspace owner gives the same account to a third agent: only Read only can be chosen ("Limited by Acme Labs"),
//     and the row shows no Reconnect or Remove;
//  3. the organization owner raises the limit to Ask before changes: Ask before changes can then be chosen, Change without
//     asking cannot.
// Both themes at 1280 px and 390 px, no horizontal scroll, axe with no rule disabled. Screenshots stay outside the repo.

const SHOTS = "/tmp/aurora-screens/p5-ux-11";
mkdirSync(SHOTS, { recursive: true });

async function check(page: Page, name: string, scheme: string, width: number) {
  expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
  await page.screenshot({ path: `${SHOTS}/${name}-${scheme}-${width}.png` });
  expect(await serious(page)).toEqual([]);
}

const LEVELS = ["read", "ask", "act"] as const;
type Level = (typeof LEVELS)[number];
const LABEL: Record<Level, string> = { read: "Read only", ask: "Ask before changes", act: "Change without asking" };

const C_ORG = id(70);
const WS2 = { ...ws, id: id(3), slug: "platform", name: "Platform" };
const giskard = { id: id(12), name: "Giskard", handle: "giskard", workspace_id: WS2.id };
const hari = { id: id(13), name: "Hari", handle: "hari", workspace_id: ws.id };
const full = (a: { id: string; name: string; handle: string; workspace_id?: string }) => ({
  ...agent,
  id: a.id,
  name: a.name,
  handle: a.handle,
  is_default: false,
  workspace_id: a.workspace_id ?? ws.id,
});
const wsOf = (agentId: string) => (agentId === giskard.id ? WS2 : ws);

type Mock = {
  /** The organization's AWS account once connected, its limit and who holds which level. */
  account: { limit: Level; agents: { id: string; level: Level }[] } | null;
  connects: Record<string, unknown>[];
  puts: { agent: string; level: string }[];
  patches: Record<string, unknown>[];
};

/** `person`: the organization owner, or a workspace owner who is not an organization admin. */
async function mockOrg(page: Page, person: "org-owner" | "ws-owner", initial: Mock["account"] = null): Promise<Mock> {
  const state: Mock = { account: initial, connects: [], puts: [], patches: [] };
  await mockApi(page);
  const people = [giskard, hari, agent];
  const who = { action: "manage_org_account", role: "org_admin", people: [{ id: id(99), name: "Ada Lovelace" }], you_can: person === "org-owner" };
  const row = (scope: "org" | "ws") => {
    const a = state.account;
    if (!a) return null;
    const levels = LEVELS.slice(0, LEVELS.indexOf(a.limit) + 1);
    const holders = a.agents.filter((h) => scope === "org" || wsOf(h.id).id === ws.id);
    return {
      ref: { type: "connection", id: C_ORG },
      kind: "app",
      name: "platform-prod",
      scope: "organization",
      levels,
      limit: a.limit,
      app: { app_id: "aws-keys", name: "AWS", custom_only: false },
      health: { state: "not_used", last_used_at: now },
      managed_by: who,
      agents: holders.map((h) => {
        const p = people.find((x) => x.id === h.id)!;
        return { agent: { id: p.id, name: p.name, handle: p.handle }, workspace: { id: wsOf(h.id).id, name: wsOf(h.id).name }, level: h.level };
      }),
      can: { give_access: true, change_access: true, manage: person === "org-owner" },
    };
  };
  await page.route("**/v1/**", async (route) => {
    const req = route.request();
    const p = new URL(req.url()).pathname;
    const m = req.method();
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    if (p === `/v1/orgs/${org.id}/workspaces`)
      return json({
        items: [
          { id: ws.id, name: ws.name, slug: ws.slug, my_role: "owner" },
          { id: WS2.id, name: WS2.name, slug: WS2.slug, my_role: "owner" },
        ],
        next_cursor: null,
      });
    if (p === `/v1/workspaces/${ws.id}/agents`) return json({ items: [agent, full(hari)], next_cursor: null });
    if (p === `/v1/workspaces/${WS2.id}/agents`) return json({ items: [full(giskard)], next_cursor: null });
    if (p === `/v1/orgs/${org.id}/accounts`) return json({ items: state.account ? [row("org")] : [], next_cursor: null });
    if (p === `/v1/workspaces/${ws.id}/accounts`) return json({ items: state.account ? [row("ws")] : [], next_cursor: null });
    if (p === `/v1/apps/aws-keys/connect` && m === "POST") {
      const body = req.postDataJSON() as Record<string, unknown>;
      state.connects.push(body);
      state.account = { limit: (body.limit as Level) ?? "read", agents: [] };
      return json({ kind: "connected", connection: { id: C_ORG, scope: "organization", app_id: "aws-keys", scopes: [], missing_scopes: [], label: "platform-prod", status: "active" }, grants: [], missing_scopes: [] });
    }
    const put = p.match(/^\/v1\/agents\/([^/]+)\/grants\/connections\/([^/]+)$/);
    if (put && m === "PUT") {
      const body = req.postDataJSON() as { level: Level };
      const lim = state.account?.limit ?? "read";
      if (LEVELS.indexOf(body.level) > LEVELS.indexOf(lim)) return json({ error: { code: "above_account_limit", message: "Above the limit." } }, 422);
      state.puts.push({ agent: put[1]!, level: body.level });
      state.account!.agents = [...state.account!.agents.filter((h) => h.id !== put[1]), { id: put[1]!, level: body.level }];
      return json({});
    }
    if (p === `/v1/agents/${agent.id}/grants`) return json({ secrets: [], connections: [], converted_by_app: [] });
    if (p === `/v1/connections/${C_ORG}` && m === "PATCH") {
      const body = req.postDataJSON() as { limit?: Level };
      state.patches.push(body);
      if (body.limit) state.account!.limit = body.limit;
      return json({ id: C_ORG });
    }
    if (p === `/v1/connections/${C_ORG}/limit-preview`) return json({ lowered: [] });
    return route.fallback();
  });
  return state;
}

async function open(page: Page, path: string, scheme: "light" | "dark") {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  expect(errors, `${path} threw`).toEqual([]);
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("an organization owner connects AWS for the whole organization with Read only and gives it to two agents in two workspaces", async ({ page }) => {
        const state = await mockOrg(page, "org-owner");
        await open(page, "/o/acme/apps?tab=accounts", scheme);
        await page.getByRole("button", { name: "Add", exact: true }).click();
        await page.getByRole("menuitem", { name: "Connect an app or website" }).click();
        const dialog = page.getByTestId("org-connect-journey");
        await expect(page.getByRole("dialog", { name: "Connect an app or website for Acme Labs" })).toBeVisible();
        await dialog.getByRole("button", { name: /^AWS/ }).click();

        // The Access step sets the account's limit, not an agent's access: Read only is preselected.
        const limit = dialog.getByRole("radiogroup", { name: "Agents may at most:" });
        await expect(limit.getByRole("radio", { name: /Read only/ })).toBeChecked();
        await check(page, "limit-step", scheme, width);
        await dialog.getByRole("button", { name: "Next" }).click();
        await dialog.getByLabel("Personal access token", { exact: true }).fill("not-a-real-token");
        await dialog.getByRole("button", { name: "Connect", exact: true }).click();

        // Done: no agent can use it yet; no grant was sent with the account.
        await expect(dialog.getByText("AWS is connected for Acme Labs. No agent can use it yet.")).toBeVisible();
        expect(state.connects).toHaveLength(1);
        expect(state.connects[0]).toMatchObject({ scope: "organization", limit: "read" });
        expect(state.connects[0]).not.toHaveProperty("grant");
        await dialog.getByRole("button", { name: "Give access" }).click();

        // The picker: "<workspace> · <agent>", each ticked agent with its own choice within the limit.
        const picker = page.getByRole("dialog", { name: "Give access to platform-prod" });
        await expect(picker.getByText("Main workspace · Daneel")).toBeVisible();
        await expect(picker.getByText("Platform · Giskard")).toBeVisible();
        await picker.getByRole("checkbox", { name: "Give Giskard access" }).check();
        await picker.getByRole("checkbox", { name: "Give Hari access" }).check();
        await expect(picker.getByRole("radio", { name: /Change without asking/ })).toHaveCount(0);
        await check(page, "give-access", scheme, width);
        await picker.getByRole("button", { name: "Give access", exact: true }).click();
        await expect(picker).toBeHidden();
        expect(state.puts.map((x) => x.level)).toEqual(["read", "read"]);
        expect(new Set(state.puts.map((x) => x.agent))).toEqual(new Set([giskard.id, hari.id]));

        // The organization's row says who can use it, in both workspaces.
        await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
        const rowItem = page.getByRole("listitem", { name: "platform-prod" });
        await expect(rowItem.getByText("Agents may at most:")).toBeVisible();
        // (P5-POLISH-9, E2) Outside its own workspace an agent carries its workspace: "Giskard · Main workspace · Read only".
        await expect(rowItem.getByRole("button", { name: /Giskard · .* · Read only/ })).toBeVisible();
        await expect(rowItem.getByRole("button", { name: /Hari · .* · Read only/ })).toBeVisible();
        await check(page, "org-row", scheme, width);
      });

      test("a workspace owner gives the same account to a third agent: Read only is the only choice, and there is no Reconnect or Remove", async ({ page }) => {
        const state = await mockOrg(page, "ws-owner", { limit: "read", agents: [{ id: giskard.id, level: "read" }, { id: hari.id, level: "read" }] });
        await open(page, "/o/acme/w/main/agents/daneel?tab=access", scheme);
        const row = page.getByRole("listitem", { name: "platform-prod" });
        await expect(row.getByText("Shared by your organization")).toBeVisible();
        await expect(row.getByRole("button", { name: /More for/ })).toHaveCount(0);
        await row.getByRole("button", { name: "Give Daneel access" }).click();
        // Read only is the only level offered, so it is text, not a select.
        await expect(row.getByText("Read only", { exact: true })).toBeVisible();
        await expect(row.getByRole("combobox", { name: /Daneel's access to/ })).toHaveCount(0);
        await expect(row.getByText("Limited by Acme Labs.")).toBeVisible();
        await expect(row.getByRole("button", { name: /Reconnect|Remove/ })).toHaveCount(0);
        expect(state.puts).toEqual([{ agent: agent.id, level: "read" }]);
        await check(page, "workspace-owner", scheme, width);
      });

      test("raising the limit to Ask before changes lets Ask before changes be chosen, and still not Change without asking", async ({ page }) => {
        const state = await mockOrg(page, "org-owner", { limit: "read", agents: [{ id: giskard.id, level: "read" }] });
        await open(page, "/o/acme/apps?tab=accounts", scheme);
        const row = page.getByRole("listitem", { name: "platform-prod" });
        await expect(row.getByText("Read only", { exact: true }).first()).toBeVisible();
        await row.getByRole("button", { name: "Change the limit of platform-prod" }).click();
        const dlg = page.getByRole("dialog", { name: "Limit for platform-prod" });
        await dlg.getByText("Ask before changes", { exact: true }).click();
        // Raising asks for no preview and changes no agent.
        await check(page, "limit-dialog", scheme, width);
        await dlg.getByRole("button", { name: /^(Save|Raise)/ }).click();
        await expect(dlg).toBeHidden();
        expect(state.patches).toEqual([{ limit: "ask" }]);
        expect(state.puts).toEqual([]);

        await row.getByRole("button", { name: "More for platform-prod" }).click();
        await page.getByRole("menuitem", { name: "Give access" }).click();
        const picker = page.getByRole("dialog", { name: "Give access to platform-prod" });
        await picker.getByRole("checkbox", { name: "Give Hari access" }).check();
        await expect(picker.getByRole("radio", { name: /Ask before changes/ })).toBeVisible();
        await expect(picker.getByRole("radio", { name: /Change without asking/ })).toHaveCount(0);
        await check(page, "after-raise", scheme, width);
      });
    });
  }
}
