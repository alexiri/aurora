import { expect, type Browser } from "@playwright/test";

/**
 * P4-ONB-1 (D-140, D-151 g): on a fresh `open` instance only instance admins may sign in until a
 * placement is saved. The kind dev and e2e stacks list this user in `auth.instance_admins`
 * (deploy/kind/values-dev.yaml, values-e2e.yaml); the journeys sign in as it first, in a context of
 * its own, and save `personal_org` (each new user gets their own organization and workspace) unless
 * one is saved already. Idempotent.
 */
export const PLACEMENT_ADMIN = "placement-admin@e2e.test";

export async function ensurePlacement(browser: Browser, baseURL: string) {
  const ctx = await browser.newContext({ baseURL });
  try {
    const page = await ctx.newPage();
    await page.goto(`/v1/auth/oidc/dev/start?login_hint=${encodeURIComponent(PLACEMENT_ADMIN)}&return_to=%2F`);
    await page.waitForURL((u) => !u.pathname.startsWith("/v1/") && !u.pathname.startsWith("/dev-idp"), { timeout: 30_000 });
    const origin = new URL(baseURL).origin;
    const cur = await ctx.request.get("/v1/admin/placement");
    expect(cur.status(), "placement admin signs in").toBe(200);
    if ((await cur.json()).saved === true) return;
    const put = await ctx.request.put("/v1/admin/placement", { data: { mode: "personal_org" }, headers: { origin } });
    expect(put.status(), "save placement").toBe(200);
  } finally {
    await ctx.close();
  }
}
