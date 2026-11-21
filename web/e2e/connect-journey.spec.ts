import { expect, test, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";

import { openJourney } from "./support/journey";
import { overflow } from "./support/open";
import { serious } from "./support/axe";

// P5-UX-3 (C-914; docs/14 §Agents, accounts and access §3) and P5-POLISH-5 (D-388): "Give <Agent> an app", the connect
// journey. From the agent's Access tab a token app is given with Read only in one go and the Access tab then lists the
// account with Read only; the App step groups the apps from the catalogue's readiness (the apps that wait for an admin
// appear only under "Needs an admin first", with Ask and Undo); the token step lists the permissions to give for the chosen
// access and a refused token keeps the field with the error under it; Done names the account and offers Try it in chat; the
// header and the footer do not move between steps, and below 640 px the journey is a full-screen sheet. Both themes at
// 1280 px and 390 px, no horizontal scroll, axe with no rule disabled. Screenshots stay outside the repo.

const SHOTS = "/tmp/aurora-screens/p5-polish-5";
mkdirSync(SHOTS, { recursive: true });

async function check(page: Page, name: string, scheme: string, width: number) {
  expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
  await page.screenshot({ path: `${SHOTS}/${name}-${scheme}-${width}.png` });
  expect(await serious(page)).toEqual([]);
}

const AGENT_ACCESS = "/o/acme/w/main/agents/daneel?tab=access";
const ACCOUNTS = "/o/acme/w/main/apps?tab=accounts";
const PROMPT = "Which pull requests are waiting for my review?";

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("from the agent's Access tab: Give Daneel an app → GitHub → Read only → token → Connect → Done, and the tab lists it", async ({ page }) => {
        const saved = await openJourney(page, AGENT_ACCESS, scheme);
        await page.getByRole("button", { name: "Give Daneel an app" }).first().click();
        // The title follows the steps ("Give Daneel an app", then "GitHub for Daneel"): hold on to the dialog itself.
        const dialog = page.getByTestId("ws-connect-journey");
        await expect(page.getByRole("dialog", { name: "Give Daneel an app" })).toBeVisible();
        await check(page, "app-step", scheme, width);
        // (FIX-210) Scrolling the list to the bottom moves only the tiles: the search and the tabs stay in the fixed header, and no tile is behind them.
        await dialog.getByRole("tabpanel").evaluate((panel) => {
          for (let el: HTMLElement | null = panel as HTMLElement; el; el = el.parentElement) {
            if (/(auto|scroll)/.test(getComputedStyle(el).overflowY) && el.scrollHeight > el.clientHeight) el.scrollTop = el.scrollHeight;
          }
        });
        const searchBox = dialog.getByRole("searchbox", { name: "Search apps" });
        const tabsBox = dialog.getByRole("tablist", { name: "Apps" });
        await expect(searchBox).toBeVisible();
        await expect(tabsBox).toBeVisible();
        const fixedBottom = ((await tabsBox.boundingBox())?.y ?? 0) + ((await tabsBox.boundingBox())?.height ?? 0);
        expect((await searchBox.boundingBox())!.y).toBeLessThan((await tabsBox.boundingBox())!.y);
        // The scrolling body starts below the tabs, so no tile can slide behind them.
        const bodyTop = await dialog.getByRole("tabpanel").evaluate((panel) => {
          for (let el: HTMLElement | null = panel as HTMLElement; el; el = el.parentElement) {
            if (/(auto|scroll)/.test(getComputedStyle(el).overflowY)) return el.getBoundingClientRect().top;
          }
          return -1;
        });
        expect(bodyTop, "the scrolling list starts below the search and tabs").toBeGreaterThanOrEqual(fixedBottom - 1);
        await check(page, "app-step-scrolled", scheme, width);
        const frame = async () => ({
          dialog: await dialog.boundingBox(),
          steps: await dialog.getByRole("list", { name: "Steps" }).boundingBox(),
          title: await dialog.getByRole("heading").first().boundingBox(),
        });
        const first = await frame();
        await dialog.getByRole("button", { name: "GitHub", exact: true }).click();

        // Access first: Read only is preselected; the Account step is skipped (no GitHub account of this agent's to reuse).
        await expect(page.getByRole("dialog", { name: "GitHub for Daneel" })).toBeVisible();
        const access = dialog.getByRole("radiogroup", { name: "What may Daneel do in GitHub?" });
        await expect(access.getByRole("radio", { name: /Read only/ })).toBeChecked();
        await check(page, "access-step", scheme, width);
        // The header does not move between steps, and (on a desktop) neither does the dialog (fixed height).
        const second = await frame();
        expect(second.title?.y).toBe(first.title?.y);
        expect(second.steps?.y).toBe(first.steps?.y);
        if (width > 640) expect(second.dialog).toEqual(first.dialog);
        else expect(second.dialog?.width).toBe(width);
        const footerTop = (await dialog.getByRole("button", { name: "Next" }).boundingBox())?.y;
        await dialog.getByRole("button", { name: "Next" }).click();

        // The numbered steps: the token page as a button, the permissions for Read only, then the field.
        await expect(dialog.getByRole("link", { name: "Open GitHub" })).toHaveAttribute("href", /github\.com\/settings/);
        await expect(dialog.getByText("Give it these permissions, for Read only:")).toBeVisible();
        await expect(dialog.getByLabel("Permissions to give the token")).toContainText("Contents · Read");
        await expect(dialog.getByLabel("Permissions to give the token")).toContainText("Metadata · Read");
        await dialog.getByLabel("Personal access token", { exact: true }).fill("not-a-real-token");
        await check(page, "sign-in-step", scheme, width);
        // The footer stays where it was: Back on the left, Connect on the right.
        expect((await dialog.getByRole("button", { name: "Connect", exact: true }).boundingBox())?.y).toBe(footerTop);
        await dialog.getByRole("button", { name: "Connect", exact: true }).click();

        await expect(dialog.getByText("Daneel can now read acme.")).toBeVisible();
        await expect(dialog.getByText(/Read only\. You can change this on/)).toBeVisible();
        expect(saved).toHaveLength(1);
        expect(saved[0]!.body).toMatchObject({ method_id: "token", scopes: ["read:org"], grant: { agent_ids: [expect.any(String)], level: "read" } });
        await check(page, "done-step", scheme, width);
        await expect(dialog.getByText(`“${PROMPT}”`)).toBeVisible();
        const chat = dialog.getByRole("link", { name: "Try it in chat" });
        await expect(chat).toBeVisible();
        const chatUrl = new URL((await chat.getAttribute("href"))!, "http://x");
        expect(chatUrl.searchParams.get("tab")).toBe("chat");
        expect(chatUrl.searchParams.get("draft")).toBe(PROMPT);
        await expect(dialog.getByRole("button", { name: "Another app" })).toBeVisible();

        await dialog.getByRole("button", { name: "Close", exact: true }).last().click();
        await expect(dialog).toBeHidden();
        const row = page.getByRole("list", { name: "Accounts Daneel can use" }).getByRole("listitem", { name: "acme-prod" });
        await expect(row.getByRole("combobox", { name: /Daneel's access to/ })).toHaveValue("read");
      });

      test("a refused token keeps the pasted value and says why under the field", async ({ page }) => {
        const saved = await openJourney(page, "/o/acme/w/main/apps?tab=apps&connect=github", scheme);
        const dialog = page.getByRole("dialog", { name: "GitHub for Daneel" });
        await dialog.getByRole("button", { name: "Give Daneel access" }).click();
        await dialog.getByRole("button", { name: "Next" }).click();
        const field = dialog.getByLabel("Personal access token", { exact: true });
        await field.fill("refused-token");
        await dialog.getByRole("button", { name: "Connect", exact: true }).click();
        await expect(dialog.getByText(/GitHub refused this token/)).toBeVisible();
        await expect(field).toHaveValue("refused-token");
        await expect(field).toHaveAttribute("type", "password");
        await check(page, "refused-token", scheme, width);
        expect(saved).toHaveLength(0);
      });

      test("Ask before changes sends level ask and the write permissions", async ({ page }) => {
        const saved = await openJourney(page, "/o/acme/w/main/apps?tab=apps&connect=github", scheme);
        const dialog = page.getByRole("dialog", { name: "GitHub for Daneel" });
        // From Apps & accounts the Agent step comes first; the workspace's one agent is already chosen.
        await dialog.getByRole("button", { name: "Give Daneel access" }).click();
        await dialog.getByText("Ask before changes", { exact: true }).click();
        await dialog.getByRole("button", { name: "Next" }).click();
        await expect(dialog.getByText("Give it these permissions, for Ask before changes:")).toBeVisible();
        await expect(dialog.getByLabel("Permissions to give the token")).toContainText("Contents · Read and write");
        await dialog.getByLabel("Personal access token", { exact: true }).fill("not-a-real-token");
        await dialog.getByRole("button", { name: "Connect", exact: true }).click();
        await expect(dialog.getByText("Daneel can now read acme, and asks you before each change.")).toBeVisible();
        expect(saved[0]!.body).toMatchObject({ scopes: ["repo", "read:org"], grant: { level: "ask" } });
      });

      test("the apps that wait for an admin are only under Needs an admin first, with Ask, the asked state and Undo", async ({ page }) => {
        await openJourney(page, `${ACCOUNTS}&connect=`, scheme);
        const dialog = page.getByRole("dialog");
        const ready = dialog.getByRole("region", { name: /^Ready to connect/ });
        await expect(ready.getByRole("button", { name: "GitHub", exact: true })).toBeVisible();
        for (const name of ["Sentry", "Linear", "Notion", "Microsoft 365", "Google Workspace"]) {
          await expect(ready.getByRole("button", { name, exact: true })).toHaveCount(0);
        }
        // Another website or API is the last tile of the Ready group.
        await expect(ready.getByRole("button").last()).toHaveText(/Another website or API/);
        // (P5-REFINE-13, access-7) Two tabs with counts; the apps that wait are on the second.
        await expect(dialog.getByRole("tab", { name: /^Ready · \d+$/, selected: true })).toBeVisible();
        await dialog.getByRole("tab", { name: "Needs an admin · 5" }).click();
        const group = dialog.getByRole("region", { name: "Needs an admin first · 5" });
        await expect(group.getByText("An admin needs to set up Sentry sign-in")).toBeVisible();
        await expect(group.getByRole("button", { name: /^Ask an admin$/ })).toHaveCount(5);
        await expect(dialog.getByRole("searchbox", { name: "Search apps" })).toBeVisible();
        await check(page, "needs-admin", scheme, width);
        await group.getByRole("button", { name: "Ask an admin" }).first().click();
        await expect(group.getByText(/Asked just now\. You'll get a notice when it's ready, and the agent's Apps & access tab will offer it\./)).toBeVisible();
        await check(page, "asked", scheme, width);
        await group.getByRole("button", { name: "Undo" }).click();
        await expect(group.getByRole("button", { name: "Ask an admin" })).toHaveCount(5);
      });

      test("search filters the groups", async ({ page }) => {
        await openJourney(page, `${ACCOUNTS}&connect=`, scheme);
        const dialog = page.getByRole("dialog");
        await dialog.getByRole("searchbox", { name: "Search apps" }).fill("sentry");
        // A search narrows both tabs: the counts follow it.
        await expect(dialog.getByRole("tab", { name: "Needs an admin · 1" })).toBeVisible();
        await expect(dialog.getByRole("tab", { name: "Ready · 0" })).toBeVisible();
        await dialog.getByRole("tab", { name: "Needs an admin · 1" }).click();
        await expect(dialog.getByRole("region", { name: "Needs an admin first · 1" })).toBeVisible();
        await expect(dialog.getByRole("button", { name: "GitHub", exact: true })).toHaveCount(0);
      });

      test("from Apps & accounts in a one-agent workspace the agent is preselected: Give Daneel access", async ({ page }) => {
        await openJourney(page, ACCOUNTS, scheme);
        await page.getByRole("button", { name: /^Add/ }).first().click();
        await page.getByRole("menuitem", { name: "Connect an app or website" }).click();
        const dialog = page.getByRole("dialog");
        await expect(dialog).toBeVisible();
        await dialog.getByRole("button", { name: "GitHub", exact: true }).click();
        // The Agent step: the workspace has one agent, so it is already chosen and the button says so.
        await expect(dialog.getByRole("checkbox", { name: /Daneel/ })).toBeChecked();
        await check(page, "agent-step", scheme, width);
        await dialog.getByRole("button", { name: "Give Daneel access" }).click();
        await expect(dialog.getByRole("radio", { name: /Read only/ })).toBeChecked();
      });

      test("Another website or API ends with the agent able to use the website, in one request", async ({ page }) => {
        const saved = await openJourney(page, `${ACCOUNTS}&connect=`, scheme);
        const dialog = page.getByRole("dialog");
        await dialog.getByRole("button", { name: "Another website or API" }).click();
        await dialog.getByRole("button", { name: "Give Daneel access" }).click();
        const form = page.getByRole("dialog", { name: "Website or API for Daneel" });
        await form.getByLabel("Name", { exact: true }).fill("Weather");
        await form.getByLabel("Website", { exact: true }).fill("api.weather.example");
        await form.getByLabel("Token", { exact: true }).fill("not-a-real-token");
        await check(page, "website", scheme, width);
        await form.getByRole("button", { name: "Connect", exact: true }).click();
        await expect(form.getByRole("status")).toContainText("Daneel");
        expect(saved).toHaveLength(1);
        expect(saved[0]!.path).toMatch(/\/secrets$/);
        expect(JSON.stringify(saved[0]!.body)).toContain("api.weather.example");
        expect(JSON.stringify(saved[0]!.body)).toContain("agent");
      });
    });
  }
}
