import { expect, test, type Page } from "@playwright/test";

// Stage 1 of P2-UI (docs/18 §Dashboard visual design): theme switch, narrow shell, brand and CSP.
// The API is mocked so the spec needs no backend; no request for a theme ever reaches /v1.

async function mockApi(page: Page, apiCalls: string[]) {
  await page.route("**/v1/**", async (route) => {
    const url = new URL(route.request().url());
    apiCalls.push(url.pathname);
    const json = (body: unknown, status = 200) =>
      route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
    if (url.pathname === "/v1/me") {
      return json({ user: { id: "u1", display_name: "Ada", email: "ada@example.test", is_instance_admin: false }, memberships: [], default_org_id: null });
    }
    return json({ items: [] });
  });
}

// Dev-mode hydration can lag behind first paint; retry until the control reacts.
async function pick(page: Page, name: string) {
  await expect(async () => {
    await page.getByRole("radio", { name }).first().click({ force: true });
    await expect(page.getByRole("radio", { name }).first()).toBeChecked({ timeout: 1000 });
  }).toPass({ timeout: 20_000 });
}

const theme = (page: Page) => page.locator("html").getAttribute("data-theme");

test.describe("theme", () => {
  test("follows prefers-color-scheme and re-themes live on Match system", async ({ page }) => {
    await mockApi(page, []);
    await page.emulateMedia({ colorScheme: "light" });
    await page.goto("/account");
    await expect.poll(() => theme(page)).toBe("light");
    await page.emulateMedia({ colorScheme: "dark" });
    await expect.poll(() => theme(page)).toBe("dark");
  });

  test("an explicit choice persists across reloads and overrides the system", async ({ page }) => {
    const calls: string[] = [];
    await mockApi(page, calls);
    await page.emulateMedia({ colorScheme: "light" });
    await page.goto("/account");
    await pick(page, "Dark");
    await expect.poll(() => theme(page)).toBe("dark");
    await page.reload();
    await expect.poll(() => theme(page)).toBe("dark");
    expect((await page.context().cookies()).find((c) => c.name === "aurora_theme")?.value).toBe("dark");
    expect(calls.filter((p) => p.toLowerCase().includes("theme"))).toEqual([]);
  });

  test("the server-rendered HTML already carries data-theme for a stored explicit choice", async ({ page, baseURL }) => {
    await mockApi(page, []);
    const res = await page.request.get(`${baseURL}/account`, { headers: { cookie: "aurora_theme=dark" } });
    expect(await res.text()).toMatch(/<html[^>]*data-theme="dark"/);
  });

  test("theme switch is a keyboard-operable radio group", async ({ page }) => {
    await mockApi(page, []);
    await page.goto("/account");
    await expect(page.getByRole("radiogroup", { name: "Theme" }).first()).toBeVisible();
    await pick(page, "Light");
    await page.getByRole("radio", { name: "Light" }).first().focus();
    await page.keyboard.press("ArrowRight");
    await expect(page.getByRole("radio", { name: "Dark" }).first()).toBeChecked();
  });
});

test.describe("narrow shell", () => {
  test.use({ viewport: { width: 390, height: 800 } });

  test("menu button opens the nav dialog, Esc closes it and focus returns", async ({ page }) => {
    await mockApi(page, []);
    await page.goto("/account");
    const menu = page.getByRole("button", { name: "Open menu" });
    await expect(async () => {
      await menu.click();
      await expect(page.getByRole("dialog")).toBeVisible({ timeout: 1500 });
    }).toPass({ timeout: 20_000 });
    await expect(page.getByRole("dialog").getByRole("link", { name: "Profile" })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toBeHidden();
    await expect(menu).toBeFocused();
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
    expect(overflow).toBeLessThanOrEqual(0);
  });
});

test("brand assets are served from the dashboard origin", async ({ page, baseURL }) => {
  await mockApi(page, []);
  for (const path of ["/icon.svg", "/apple-icon.png"]) {
    const res = await page.request.get(`${baseURL}${path}`);
    expect(res.status(), path).toBe(200);
  }
  const external: string[] = [];
  page.on("request", (r) => {
    if (!r.url().startsWith(baseURL!) && !r.url().startsWith("data:")) external.push(r.url());
  });
  await page.goto("/account");
  await expect(page.getByRole("img", { name: "Aurora" }).first()).toBeVisible();
  expect(external).toEqual([]);
});
