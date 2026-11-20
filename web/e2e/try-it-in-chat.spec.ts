import { expect, test, type Page } from "@playwright/test";
import { mkdirSync } from "node:fs";

import { FAKE_EVENT_SOURCE, mockChat } from "./support/chat-mock";
import { openJourney } from "./support/journey";
import { noFirstPrompt } from "./support/mock-api";
import { overflow } from "./support/open";

// P5-REFINE-13 item 1 (owner's bug report, 2026-11-20; C-1035): the Done step's "Try it in chat" must work from every
// place the connect journey opens. One click: the dialog is gone, the agent's Chat is shown, the box holds the app's
// suggested prompt and has focus, nothing is sent and the URL no longer carries `draft`. Click-throughs, not `href`
// checks: the earlier specs only read the link's href, which is how the Access-tab case (no `tab=chat`, so the page
// stayed on Access) and the empty-box case (the composer read `window.location` before the router updated it) shipped.

const SHOTS = "/tmp/aurora-screens/p5-refine-13";
mkdirSync(SHOTS, { recursive: true });

const PROMPT = "Which pull requests are waiting for my review?";
const ENTRIES = {
  "the agent's Access tab": "/o/acme/w/main/agents/daneel?tab=access",
  "workspace Apps": "/o/acme/w/main/apps?tab=apps",
  "workspace Apps opened by ?connect=github": "/o/acme/w/main/apps?tab=apps&connect=github",
  "the workspace Accounts tab": "/o/acme/w/main/apps?tab=accounts",
  "an app's own page": "/o/acme/w/main/apps/github",
} as const;
type Entry = keyof typeof ENTRIES;

/** Opens the journey from `entry` and brings it to Done for GitHub. */
async function giveGithub(page: Page, entry: Entry, scheme: "light" | "dark", opts: { firstPrompt?: string | null } = {}) {
  await page.addInitScript(FAKE_EVENT_SOURCE);
  // (set before the page opens: with `?connect=` the journey fetches the app's detail on load)
  noFirstPrompt.on = opts.firstPrompt === null;
  const saved = await openJourney(page, ENTRIES[entry], scheme);
  await mockChat(page);
  const dialog = page.getByTestId("ws-connect-journey");
  switch (entry) {
    case "the agent's Access tab":
      await page.getByRole("button", { name: "Give Daneel an app" }).first().click();
      await dialog.getByRole("button", { name: "GitHub", exact: true }).click();
      break;
    case "workspace Apps":
      // (P5-POLISH-7) GitHub already has an account in the mock: its card offers "Connect another".
      await page.getByRole("button", { name: "Connect another GitHub" }).click();
      await dialog.getByRole("button", { name: "Give Daneel access" }).click();
      break;
    case "workspace Apps opened by ?connect=github":
      await dialog.getByRole("button", { name: "Give Daneel access" }).click();
      break;
    case "the workspace Accounts tab":
      // The workspace's Accounts tab (no agent): the Add menu (accounts exist in the mock) has "Connect an app or website".
      await page.getByRole("button", { name: "Add", exact: true }).click();
      await page.getByRole("menuitem", { name: "Connect an app or website" }).click();
      await dialog.getByRole("button", { name: "GitHub", exact: true }).click();
      await dialog.getByRole("button", { name: "Give Daneel access" }).click();
      break;
    case "an app's own page":
      await page.getByRole("button", { name: "Connect GitHub" }).click();
      await dialog.getByRole("button", { name: "Give Daneel access" }).click();
      break;
  }
  await dialog.getByRole("button", { name: "Next" }).click();
  await dialog.getByLabel("Personal access token", { exact: true }).fill("not-a-real-token");
  await dialog.getByRole("button", { name: "Connect", exact: true }).click();
  await expect(dialog.getByRole("link", { name: opts.firstPrompt === null ? "Open chat" : "Try it in chat" })).toBeVisible();
  return saved;
}

test.afterEach(() => {
  noFirstPrompt.on = false;
});

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      for (const entry of Object.keys(ENTRIES) as Entry[]) {
        test(`Try it in chat from ${entry}: Chat opens, the box holds the prompt, nothing is sent`, async ({ page }) => {
          const sent: string[] = [];
          page.on("request", (r) => {
            if (r.method() === "POST" && /\/v1\/conversations\/[^/]+\/messages$/.test(new URL(r.url()).pathname)) sent.push(r.url());
          });
          await giveGithub(page, entry, scheme);
          await page.getByTestId("ws-connect-journey").getByRole("link", { name: "Try it in chat" }).click();

          await expect(page.getByTestId("ws-connect-journey")).toHaveCount(0);
          await expect(page.getByRole("heading", { name: "Direct chat" })).toBeVisible();
          // (the composer's field is a combobox for the emoji suggestions; it is labelled "Message")
          const box = page.getByLabel("Message", { exact: true });
          await expect(box).toHaveValue(PROMPT);
          await expect(box).toBeFocused();
          await expect.poll(() => new URL(page.url()).pathname).toBe("/o/acme/w/main/agents/daneel");
          await expect.poll(() => new URL(page.url()).searchParams.has("draft")).toBe(false);
          expect(new URL(page.url()).searchParams.get("tab") ?? "chat").toBe("chat");
          expect(await overflow(page), "horizontal page scroll").toBeLessThanOrEqual(0);
          await page.screenshot({ path: `${SHOTS}/try-it-${entry.replace(/\W+/g, "-")}-${scheme}-${width}.png` });
          expect(sent, "nothing was sent for the person").toEqual([]);
        });
      }

      test("with no suggested prompt, Open chat lands on Chat with an empty box (from Access too)", async ({ page }) => {
        const sent: string[] = [];
        page.on("request", (r) => {
          if (r.method() === "POST" && /\/messages$/.test(new URL(r.url()).pathname)) sent.push(r.url());
        });
        await giveGithub(page, "the agent's Access tab", scheme, { firstPrompt: null });
        await page.getByTestId("ws-connect-journey").getByRole("link", { name: "Open chat" }).click();
        await expect(page.getByTestId("ws-connect-journey")).toHaveCount(0);
        await expect(page.getByRole("heading", { name: "Direct chat" })).toBeVisible();
        await expect(page.getByLabel("Message", { exact: true })).toHaveValue("");
        expect(sent).toEqual([]);
      });
    });
  }
}
