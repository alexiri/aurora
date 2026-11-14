import { expect, type Page } from "@playwright/test";

import { id, mockApi, now } from "./mock-api";
import type { Scheme } from "./open";

// P5-ASRC: the Admin › Addon sources screens, reached through the real UI with the mocked API.
// The repository text is deliberately hostile (markup, a javascript: link, a very long unbroken
// word) to prove it renders as inert data.

export const ADDON_SCREENS = ["list", "add-review", "key-changed", "schedule", "details"] as const;
export type AddonScreen = (typeof ADDON_SCREENS)[number];

const HOSTILE = "App <img src=x onerror=alert(1)> [click](javascript:alert(1)) " + "x".repeat(120);
const FP = "sha256:0123456789ab";
const NEW_FP = "sha256:ba9876543210";

const addons = [
  {
    id: "indigo_sso",
    name: "Indigo SSO",
    description: HOSTILE,
    versions: [
      { version: "1.1.0", min_core: null, ingestable: true, not_ingestable_reason: null, installed_version: "1.0.0", update_available: true },
      { version: "1.0.0", min_core: null, ingestable: true, not_ingestable_reason: null, installed_version: "1.0.0", update_available: false },
      { version: "2.0.0", min_core: "99.0.0", ingestable: false, not_ingestable_reason: "core_too_new", installed_version: "1.0.0", update_available: false },
    ],
  },
];

const base = {
  id: id(800),
  name: "CERN addons",
  url: "https://raw.githubusercontent.com/cern/aurora-addons/HEAD/aurora-addons.yaml",
  homepage: "https://aurora.cern.example",
  contact: null,
  publisher: "cern",
  key_fingerprint: FP,
  key_description: "CERN release key",
  update_interval: "1d",
  status: "ok",
  check_failed_at: null,
  check_failed_reason: null,
  new_key_fingerprint: null,
  last_checked_at: now,
  manifest_digest: "d".repeat(64),
  addons,
  addon_count: 1,
  removed_upstream: [{ package_id: "indigo_sso", version: "0.9.0" }],
  created_at: now,
  updated_at: now,
};

const sources = [
  base,
  { ...base, id: id(801), name: "Rotated repo", status: "signing_key_changed", new_key_fingerprint: NEW_FP, url: "https://raw.githubusercontent.com/acme/rotated-addons/HEAD/aurora-addons.yaml" },
  { ...base, id: id(802), name: "Flaky repo", status: "check_failed", check_failed_reason: "the source is unreachable: connection refused", update_interval: "never", addons: [], addon_count: 0, removed_upstream: [] },
];

const preview = {
  url: base.url,
  repo: { name: "CERN addons", description: HOSTILE, homepage: "https://aurora.cern.example", contact: "mailto:aurora-admin@cern.example" },
  publisher: "cern",
  key_fingerprint: FP,
  key_description: "CERN release key",
  addons,
  packages_new: 1,
};

export async function mockAddonSources(page: Page) {
  await mockApi(page, {
    "/v1/admin/addon-sources": { items: sources },
    "/v1/admin/addon-sources/preview": preview,
  });
}

/** Opens one screen of Admin › Addon sources through the UI (themed, hydrated). */
export async function openAddonScreen(page: Page, screen: AddonScreen, scheme: Scheme) {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockAddonSources(page);
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto("/admin/addon-sources");
  await page.getByRole("heading", { name: "Addon sources", level: 1 }).waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  if (screen === "add-review") {
    await page.getByRole("button", { name: "Add source" }).first().click();
    await page.getByLabel("Repository or manifest URL").fill("https://github.com/cern/aurora-addons");
    await page.getByRole("button", { name: "Review repository" }).click();
    await page.getByRole("button", { name: "Trust this key and add source" }).waitFor();
  } else if (screen === "key-changed") {
    await page.getByRole("button", { name: "Review key: Rotated repo" }).click();
    await page.getByRole("button", { name: "Trust the new key" }).waitFor();
  } else if (screen === "schedule") {
    await page.getByRole("button", { name: "Edit the schedule of CERN addons" }).click();
    await page.getByRole("button", { name: "Save schedule" }).waitFor();
  } else if (screen === "details") {
    await page.getByRole("button", { name: "1 addon: CERN addons" }).click();
    await page.getByText("Removed upstream").waitFor();
  }
  expect(errors, `${screen} threw`).toEqual([]);
}
