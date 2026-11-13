import { expect, type Page } from "@playwright/test";

import { id, mockApi, now, org } from "./mock-api";
import type { Scheme } from "./open";

// P4-GSL (D-148): the git skill library screens, reached through the real UI with the mocked API.
// The data uses the real built-in GitHub package; the repository text is deliberately hostile
// (markup, a javascript: link, a very long unbroken word) to prove it renders as inert data.

export const LIBRARY_SCREENS = ["libraries", "connect", "add-list", "add-preview", "review"] as const;
export type LibraryScreen = (typeof LIBRARY_SCREENS)[number];

const SHA_A = "0123456789abcdef0123456789abcdef01234567";
const SHA_B = "89abcdef0123456789abcdef0123456789abcdef";
const HOSTILE = "Review <img src=x onerror=alert(1)> [click](javascript:alert(1)) " + "x".repeat(120);

const library = {
  id: id(700), label: "Team skills", app_id: "github", connection: { id: id(40), label: "acme-skills-token", status: "active" },
  repo: "acme/agent-skills", ref: null, path: "skills", status: "active", last_head_sha: SHA_B, last_refresh_at: now, last_error: null,
  skill_counts: { agent: 1, workspace: 0, org: 1 }, pending_updates: 1, created_at: now,
};

const orgSkill = {
  id: id(710), tier: "org", scope: "organization", agent_id: null, workspace_id: null, org_id: id(1), name: "code-review",
  description: HOSTILE, body: "Review pull requests carefully.\n", enabled: true, files: [{ path: "checklist.md", size: 20, executable: false }],
  total_size: 52, overridden_in: null, updated_at: now, created_at: now,
  source: { kind: "library", library_id: id(700), label: "Team skills", repo: "acme/agent-skills", path: "code-review", commit_sha: SHA_A, pinned_at: now, detached: false, update_pending: true },
};

const catalogue = {
  commit_sha: SHA_B, truncated: false, next_cursor: null,
  items: [
    { path: "code-review", name: "code-review", description: HOSTILE, valid: true, reason: null, size: 480, files: 2, added: [{ tier: "org", id: id(710) }] },
    { path: "release-notes", name: "release-notes", description: "Write release notes from merged pull requests", valid: true, reason: null, size: 900, files: 1, added: [] },
    { path: "Bad_Name", name: "Bad_Name", description: null, valid: false, reason: "the directory name is not a valid skill name", size: 100, files: 1, added: [] },
  ],
};

const content = {
  commit_sha: SHA_B, path: "release-notes", name: "release-notes", valid: true, reason: null,
  description: "Write release notes from merged pull requests", body: "Collect merged pull requests.\n\n- Group by label\n- Keep it short\n",
  files: [{ path: "template.md", size: 30, executable: false, binary: false, content: "# Release notes\n\n## Highlights\n", content_b64: null }],
  size: 90, content_sha256: "a".repeat(64),
};

const info = {
  library: { id: id(700), label: "Team skills", repo: "acme/agent-skills", path: "code-review" }, commit_sha: SHA_A, pinned_at: now, pinned_by: id(99), detached: false,
  update: {
    id: id(720), kind: "changed", commit_sha: SHA_B, created_at: now, content_sha256: "b".repeat(64), over_budget: false, reason: null,
    diff: {
      description: { old: "Review pull requests", new: HOSTILE, changed: true },
      body: { old: "Review pull requests carefully.\n", new: "Review pull requests carefully.\nCheck the tests first.\n", changed: true },
      files: [{ path: "checklist.md", change: "modified", old: "- tests\n", new: "- tests\n- docs\n", old_executable: false, new_executable: false, binary: false }],
    },
  },
};

/** Routes for the skill library screens; later routes win over the shared mock, the rest falls through to it. */
export async function mockLibraries(page: Page) {
  await mockApi(page);
  await page.route("**/v1/**", async (route) => {
    const p = new URL(route.request().url()).pathname;
    const json = (b: unknown) => route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(b) });
    if (p === `/v1/orgs/${org.id}/skill-libraries`) return json({ items: [library], next_cursor: null });
    if (p === `/v1/orgs/${org.id}/skills`) return json({ items: [orgSkill], next_cursor: null });
    if (p === `/v1/orgs/${org.id}/skill-library-updates`) {
      return json({ items: [{ id: id(720), kind: "changed", created_at: now, skill: { id: id(710), name: "code-review", tier: "org" }, library: { id: id(700), label: "Team skills" } }] });
    }
    if (p === `/v1/skill-libraries/${library.id}/skills`) {
      // Two pages (C-571 pages by 50): "Load more" follows `next_cursor` to the second one.
      if (new URL(route.request().url()).searchParams.get("cursor") === "c2") {
        const more = ["deploy-checks", "triage-issues"].map((name) => ({ path: name, name, description: "More from the next page", valid: true, reason: null, size: 300, files: 1, added: [] }));
        return json({ ...catalogue, next_cursor: null, items: more });
      }
      return json({ ...catalogue, next_cursor: "c2" });
    }
    if (p === `/v1/skill-libraries/${library.id}/skills/release-notes`) return json(content);
    if (p === `/v1/skills/${orgSkill.id}/library`) return json(info);
    if (p === `/v1/orgs/${org.id}/connections`) {
      return json({ items: [{ app_id: "github", app_version: "1.0.0", created_at: now, id: id(40), injecting: [], label: "acme-skills-token", metadata: {}, missing_scopes: [], org_id: id(1), package_id: "github", scope: "organization", scopes: [], status: "active", updated_at: now }] });
    }
    return route.fallback();
  });
}

export async function openLibraryScreen(page: Page, screen: LibraryScreen, scheme: Scheme) {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await mockLibraries(page);
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(screen === "libraries" || screen === "connect" ? "/o/acme/skills/libraries" : "/o/acme/skills");
  await page.locator("h1").first().waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  expect(errors, "the skill library screen threw").toEqual([]);
  if (screen === "libraries") return;
  if (screen === "connect") {
    await page.getByRole("button", { name: "Connect a repository" }).first().click();
    await page.getByLabel("Repository", { exact: true }).waitFor();
    return;
  }
  if (screen === "review") {
    await page.getByRole("button", { name: /code-review/ }).first().click();
    await page.getByRole("button", { name: "Review update" }).click();
    await page.getByRole("button", { name: "Approve" }).waitFor();
    return;
  }
  await page.getByRole("button", { name: "Add from library…" }).click();
  await page.getByRole("list", { name: "Library skills" }).waitFor();
  if (screen === "add-list") return;
  await page.getByRole("button", { name: /^release-notes/ }).click();
  await page.getByLabel("Preview of what will be stored").waitFor();
}
