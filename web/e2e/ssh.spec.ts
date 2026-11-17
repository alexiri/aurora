import { expect, test, type Page } from "@playwright/test";

import { mockApi } from "./support/mock-api";
import { overflow, titled, type Scheme } from "./support/open";
import { serious } from "./support/files";

// P4-SSH-1 (docs/14 §Account › SSH keys and §Agent page › SSH, C-795): the keys page and the agent's SSH page against the
// shared mock plus the three SSH routes. Zero serious/critical axe violations in both themes at 1280 and 390 px, no
// horizontal scroll at 390 px, and the certificate download carries the name OpenSSH expects.

const KEYS = [
  {
    id: "00000000-0000-4000-8000-0000000000a1", name: "work laptop with a rather long descriptive name", algorithm: "ssh-ed25519",
    public_key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIN5ElGab515YLw7PcnKusC/LWNtScxqkSkx78EjhVfhl",
    fingerprint: "SHA256:gOAeoDRLhPIlNGAongiqcd5fskHrxCKhw3I7so57P3c", created_at: "2026-01-01T00:00:00Z", last_used_at: "2026-01-02T10:00:00Z",
  },
  {
    id: "00000000-0000-4000-8000-0000000000a2", name: "phone", algorithm: "ssh-ed25519",
    public_key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBGWjypTNLYL3ngKfpKTiXQ1eTzZPmP2FNrzx+Jnrn4/",
    fingerprint: "SHA256:sQIscfRbf0fNpTJG0KE1RauEAvmA/5t3fm1+Ib25QGg", created_at: "2026-01-03T00:00:00Z", last_used_at: null,
  },
];

// (P4-SSH-2, C-793) Open sessions: one of the caller's own (account list), and the agent page's list with the person.
const SESSION = {
  id: "00000000-0000-4000-8000-0000000000b1", agent: { id: "00000000-0000-4000-8000-0000000000c1", name: "Daneel" },
  workspace_id: "00000000-0000-4000-8000-0000000000d1",
  key: { id: KEYS[0]!.id, name: KEYS[0]!.name, fingerprint: KEYS[0]!.fingerprint }, client_address: "203.0.113.0",
  opened_at: "2026-11-16T10:00:00Z", last_activity_at: "2026-11-16T10:05:00Z", channels_open: 0, channels_total: 3,
};

async function mockSsh(page: Page, over: { terminators?: number; keys?: typeof KEYS; sessions?: (typeof SESSION)[] } = {}) {
  await mockApi(page);
  const reply = (route: import("@playwright/test").Route, body: unknown, status = 200) =>
    route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });
  await page.route("**/v1/instance", (route) =>
    reply(route, {
      version: "0.1.0", signup_mode: "closed", setup_errors: [],
      features: { hosted: true, previews: true, terminal: true, ssh: true },
      previews: { enabled: true, domain: "previews.example.org" },
      oidc_providers: [{ id: "corp", display_name: "Corporate SSO", groups: true }],
    }),
  );
  await page.route("**/v1/me/ssh-keys", (route) => reply(route, { keys: over.keys ?? KEYS }));
  await page.route("**/v1/me/ssh-sessions", (route) => reply(route, { sessions: over.sessions ?? [SESSION] }));
  await page.route("**/v1/agents/*/ssh-sessions", (route) =>
    reply(route, { sessions: (over.sessions ?? [SESSION]).map((s) => ({ ...s, user: { id: "00000000-0000-4000-8000-0000000000e1", label: "Bob Builder" } })) }),
  );
  await page.route("**/v1/admin/terminators", (route) =>
    reply(route, {
      terminators: [{
        id: "00000000-0000-4000-8000-0000000000f1", version: "0.1.0", host_key_fingerprint: "SHA256:hostkeyfingerprintaaaaaaaaaaaaaaaaaaaaaaaaaa",
        sessions_open: 1, last_seen_at: "2026-11-16T10:00:00Z", live: (over.terminators ?? 1) > 0,
      }],
    }),
  );
  const live = over.terminators ?? 1;
  await page.route("**/v1/ssh", (route) =>
    reply(route, {
      host: "ssh.example.org", port: 22, cert_ttl_secs: 900, terminators_live: live,
      host_keys: live ? [{ public_key: "ssh-ed25519 AAAAHOSTKEY", fingerprint: "SHA256:hostkeyfingerprintaaaaaaaaaaaaaaaaaaaaaaaaaa" }] : [],
      known_hosts: live ? ["ssh.example.org ssh-ed25519 AAAAHOSTKEY"] : [],
    }),
  );
  await page.route("**/v1/agents/*/ssh-certificate", (route) =>
    reply(
      route,
      {
        certificate: "ssh-ed25519-cert-v01@openssh.com AAAACERT aurora:agent", serial: "7", principal: "p", key_id: "k",
        valid_after: "2026-01-01T00:00:00Z", valid_before: "2026-01-01T00:15:00Z", username: "p", host: "ssh.example.org", port: 22, known_hosts: [],
      },
      201,
    ),
  );
}

async function open(page: Page, path: string, scheme: Scheme) {
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  await titled(page);
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`${scheme} theme, ${width} px`, () => {
      test.use({ viewport: { width, height: 900 } });

      test("Account › SSH keys: table, no serious axe violations, no horizontal scroll", async ({ page }) => {
        await mockSsh(page);
        await open(page, "/account/ssh", scheme);
        await expect(page.getByRole("heading", { name: "SSH keys", level: 2 })).toBeVisible();
        await expect(page.getByRole("table", { name: "SSH keys" }).getByText("work laptop with a rather long descriptive name")).toBeVisible();
        await expect(page.getByRole("button", { name: "Add key" })).toBeVisible();
        expect(await serious(page)).toEqual([]);
        if (width === 390) expect(await overflow(page)).toBeLessThanOrEqual(0);
        // The Add key dialog too.
        await page.getByRole("button", { name: "Add key" }).first().click();
        await expect(page.getByRole("dialog", { name: "Add SSH key" })).toBeVisible();
        expect(await serious(page)).toEqual([]);
        if (width === 390) expect(await overflow(page)).toBeLessThanOrEqual(0);
      });

      test("agent page › SSH: connect card, host key, what works", async ({ page }) => {
        await mockSsh(page);
        await open(page, "/o/acme/w/main/agents/daneel?tab=ssh", scheme);
        await expect(page.getByRole("heading", { name: "Connect with SSH" })).toBeVisible();
        await expect(page.getByRole("heading", { name: "Host key" })).toBeVisible();
        await expect(page.getByRole("heading", { name: "What works" })).toBeVisible();
        await expect(page.getByLabel("the known_hosts lines", { exact: true })).toContainText("ssh.example.org ssh-ed25519 AAAAHOSTKEY");
        expect(await serious(page)).toEqual([]);
        if (width === 390) expect(await overflow(page)).toBeLessThanOrEqual(0);
      });

      test("Account › SSH keys: the active sessions list and its disconnect confirmation", async ({ page }) => {
        await mockSsh(page);
        let deleted = false;
        await page.route("**/v1/ssh-sessions/*", (route) => {
          deleted = true;
          return route.fulfill({ status: 202, contentType: "application/json", body: JSON.stringify({ disconnecting: true }) });
        });
        await open(page, "/account/ssh", scheme);
        await expect(page.getByRole("heading", { name: "Active SSH sessions", level: 2 })).toBeVisible();
        const table = page.getByRole("table", { name: "Active SSH sessions" });
        await expect(table.getByText("Daneel")).toBeVisible();
        expect(await serious(page)).toEqual([]);
        if (width === 390) expect(await overflow(page)).toBeLessThanOrEqual(0);
        await page.getByRole("button", { name: /Disconnect the session on Daneel/ }).click();
        const dialog = page.getByRole("dialog", { name: "Disconnect SSH session" });
        await expect(dialog).toBeVisible();
        expect(await serious(page)).toEqual([]);
        if (width === 390) expect(await overflow(page)).toBeLessThanOrEqual(0);
        await dialog.getByRole("button", { name: "Disconnect" }).click();
        await expect(page.getByText(/The session ends within 30 seconds/)).toBeVisible();
        expect(deleted).toBe(true);
      });

      test("agent page › SSH: the active sessions list shows the person", async ({ page }) => {
        await mockSsh(page);
        await open(page, "/o/acme/w/main/agents/daneel?tab=ssh", scheme);
        await expect(page.getByRole("heading", { name: "Active sessions", level: 2 })).toBeVisible();
        const table = page.getByRole("table", { name: "Active SSH sessions" });
        await expect(table.getByText("Bob Builder")).toBeVisible();
        expect(await serious(page)).toEqual([]);
        if (width === 390) expect(await overflow(page)).toBeLessThanOrEqual(0);
      });

      test("Admin › Overview: the SSH line", async ({ page }) => {
        await mockSsh(page);
        await open(page, "/admin", scheme);
        const line = page.getByTestId("ssh-line");
        await expect(line).toBeVisible();
        await expect(line).toContainText("Terminator up");
        await expect(line).toContainText("1 terminator replica live, 1 open session");
        expect(await serious(page)).toEqual([]);
        if (width === 390) expect(await overflow(page)).toBeLessThanOrEqual(0);
      });

      test("agent page › SSH with no terminator shows the notice", async ({ page }) => {
        await mockSsh(page, { terminators: 0 });
        await open(page, "/o/acme/w/main/agents/daneel?tab=ssh", scheme);
        await expect(page.getByText("SSH is enabled but no SSH server is running; ask your administrator.")).toBeVisible();
        expect(await serious(page)).toEqual([]);
        if (width === 390) expect(await overflow(page)).toBeLessThanOrEqual(0);
      });
    });
  }
}

test.describe("certificate download", () => {
  test.use({ viewport: { width: 1280, height: 900 } });
  test("Get a certificate offers id_ed25519-cert.pub with the certificate line", async ({ page }) => {
    await mockSsh(page);
    await open(page, "/o/acme/w/main/agents/daneel?tab=ssh", "light");
    await page.getByRole("button", { name: "Get a certificate" }).click();
    const link = page.getByRole("link", { name: /Download id_ed25519-cert\.pub/ });
    await expect(link).toBeVisible();
    await expect(page.getByText(/Valid for 15 minutes; get a new one to reconnect later/)).toBeVisible();
    const [download] = await Promise.all([page.waitForEvent("download"), link.click()]);
    expect(download.suggestedFilename()).toBe("id_ed25519-cert.pub");
  });
});
