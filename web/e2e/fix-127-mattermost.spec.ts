import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { openMattermost } from "./support/mattermost-flow";
import { overflow } from "./support/open";

// FIX-127 (D-195(g), C-710..C-714): Mattermost as one guided path, on the mock API.
//   instance admin adds Mattermost with zero typed fields → org admin checks the credentials,
//   picks the team, connects → done screen: attach an agent. (Binding a channel from the agent tab
//   is Part C.) Every new screen: both themes, 390 px with no horizontal scroll, axe.

const ORG_CHANNELS = "/o/acme/channels";

async function serious(page: Page) {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
}

test("journey: add Mattermost with zero typed fields, connect it picking a team, attach an agent", async ({ page }) => {
  const mock = await openMattermost(page, "/admin/connectors", "light");

  // Instance: one click from the choose step, nothing typed.
  await page.getByRole("button", { name: "Add connector" }).click();
  await page.getByRole("button", { name: "Add Mattermost" }).click();
  await page.getByRole("heading", { name: "Mattermost" }).first().waitFor();
  const added = mock.calls.find((c) => c.key === "POST /v1/admin/connectors");
  expect(added?.body).toMatchObject({ id: "mattermost", managed: true, settings: {}, secrets: {} });

  // Org: the connector is usable; check the credentials, pick the team, connect.
  await page.goto(ORG_CHANNELS);
  await page.getByRole("button", { name: /Connect Mattermost/ }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Your Mattermost server").fill("https://chat.example.com");
  await dialog.getByLabel("Bot access token").fill("bot-secret");
  await dialog.getByRole("button", { name: "Check" }).click();
  await expect(dialog.getByText("No admin token: replies to individuals arrive as DMs.")).toBeVisible();
  await dialog.getByLabel("Team").selectOption("alpha");
  await dialog.getByRole("button", { name: "Connect" }).click();

  // Done screen: who the bot is, and the three next steps.
  const done = page.getByTestId("connected-done");
  await expect(done).toContainText("Mattermost is connected as @daneel in Alpha Team.");
  await expect(done.getByRole("link", { name: "Open a DM with @daneel" })).toHaveAttribute("href", "https://chat.example.com/alpha/messages/@daneel");
  await expect(done.getByRole("link", { name: "Link your Mattermost account" })).toHaveAttribute("href", "/account/channels");
  await done.getByLabel("Agent", { exact: true }).selectOption({ label: "Daneel" });
  await done.getByRole("button", { name: "Attach with the shared bot" }).click();
  await expect(done.getByText(/Attached\./)).toBeVisible();

  const probe = mock.calls.find((c) => c.key.endsWith(":probe"));
  expect(probe?.body).toEqual({ connector_id: "mattermost", config: { server_url: "https://chat.example.com" }, credentials: { bot_token: "bot-secret" } });
  expect(mock.calls.find((c) => c.key === "POST /v1/orgs/00000000-0000-4000-8000-000000000001/channel-integrations")?.body).toEqual({
    connector_id: "mattermost",
    config: { server_url: "https://chat.example.com", team: "alpha" },
    credentials: { bot_token: "bot-secret" },
  });
  expect(mock.calls.some((c) => c.key.endsWith("/presences") && JSON.stringify(c.body).includes('"mode":"shared"'))).toBe(true);
});

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      const screens: [string, (page: Page) => Promise<void>, { added?: boolean; state?: "not_added" | "waiting_for_host" | "disabled" | "needs_approval" }][] = [
        ["org channels: not added", async () => {}, {}],
        ["org channels: waiting for the host", async () => {}, { state: "waiting_for_host" }],
        [
          "stepper step 1",
          async (page) => {
            await page.getByRole("button", { name: /Connect Mattermost/ }).click();
            await page.getByLabel("Your Mattermost server").waitFor();
          },
          { added: true },
        ],
        [
          "stepper step 2",
          async (page) => {
            await page.getByRole("button", { name: /Connect Mattermost/ }).click();
            await page.getByLabel("Your Mattermost server").fill("https://chat.example.com");
            await page.getByLabel("Bot access token").fill("t");
            await page.getByRole("button", { name: "Check" }).click();
            await page.getByLabel("Team").waitFor();
          },
          { added: true },
        ],
        [
          "done screen",
          async (page) => {
            await page.getByRole("button", { name: /Connect Mattermost/ }).click();
            await page.getByLabel("Your Mattermost server").fill("https://chat.example.com");
            await page.getByLabel("Bot access token").fill("t");
            await page.getByRole("button", { name: "Check" }).click();
            await page.getByLabel("Team").selectOption("alpha");
            await page.getByRole("button", { name: "Connect" }).click();
            await page.getByTestId("connected-done").waitFor();
          },
          { added: true },
        ],
      ];
      for (const [name, act, opts] of screens) {
        test(`${name}: no horizontal scroll, no serious axe violations`, async ({ page }) => {
          await openMattermost(page, ORG_CHANNELS, scheme, opts);
          await act(page);
          expect(await overflow(page)).toBeLessThanOrEqual(0);
          expect(await serious(page)).toEqual([]);
        });
      }

      test("admin setup: the available callout", async ({ page }) => {
        await openMattermost(page, "/admin/setup", scheme);
        await expect(page.getByText("Mattermost is available — add it.")).toBeVisible();
        expect(await overflow(page)).toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
      });
    });
  }
}
