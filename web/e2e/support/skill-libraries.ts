import { expect, type Page } from "@playwright/test";

import { id, mockApi, now, org } from "./mock-api";
import type { Scheme } from "./open";

// P4-GSL (D-148), P5-SKLIB-3 (D-450): the git skill library screens, reached through the real UI with the mocked API.
// The data uses the real built-in GitHub package; the repository text is deliberately hostile
// (markup, a javascript: link, a very long unbroken word) to prove it renders as inert data.

export const LIBRARY_SCREENS = ["skills", "ws-skills", "libraries","add", "found", "edit", "add-list", "add-preview", "review", "instance-libraries", "instance-add"] as const;
export type LibraryScreen = (typeof LIBRARY_SCREENS)[number];

const SHA_A = "0123456789abcdef0123456789abcdef01234567";
const SHA_B = "89abcdef0123456789abcdef0123456789abcdef";
const HOSTILE = "Review <img src=x onerror=alert(1)> [click](javascript:alert(1)) " + "x".repeat(120);

type Lib = Record<string, unknown>;

// (P5-SKLIB-1, C-1122) A library is an address: `url` and the display form `repo`, no app connection; `token_set` says
// whether a token is saved, never the token.
const library: Lib = {
  id: id(700), scope: "org", org_id: id(1), label: "Team skills", url: "https://git.example.org/acme/agent-skills.git",
  repo: "git.example.org/acme/agent-skills", ref: null, path: "skills", status: "active", token_set: false, token_updated_at: null,
  last_head_sha: SHA_B, last_refresh_at: now, last_error: null,
  skill_counts: { agent: 1, workspace: 0, org: 1 }, pending_updates: 1, created_at: now,
  skills_offered: 17, skills_unavailable: 1,
};

// (P5-SKLIB-2, D-449) A library of the whole installation: listed in the organization's second section with its
// own numbers for this organization only, read-only, with Browse.
export const instanceLibrary: Lib = {
  id: id(701), scope: "instance", org_id: null, label: "Company skills", url: "https://git.example.org/it/skills.git",
  repo: "git.example.org/it/skills", ref: null, path: "", status: "active", token_set: true, token_updated_at: null,
  last_head_sha: SHA_B, last_refresh_at: now, last_error: null,
  skill_counts: { agent: 0, workspace: 0, org: 1 }, pending_updates: 1, created_at: now,
  skills_offered: 5, skills_unavailable: 0,
};

// What an organization with no private libraries and a broken one sees in the statuses.
const brokenLibrary: Lib = {
  ...library, id: id(702), label: "Private tools", url: "https://git.example.org/acme/private.git", repo: "git.example.org/acme/private", path: "",
  status: "error", last_error: "The repository needs an access token", last_head_sha: null, skill_counts: { agent: 0, workspace: 0, org: 0 }, pending_updates: 0,
  skills_offered: null, skills_unavailable: null,
};

const orgSkill = {
  id: id(710), tier: "org", scope: "organization", agent_id: null, workspace_id: null, org_id: id(1), name: "code-review",
  description: HOSTILE, body: "Review pull requests carefully.\n", enabled: true, files: [{ path: "checklist.md", size: 20, executable: false }],
  total_size: 52, overridden_in: null, updated_at: now, created_at: now,
  source: { kind: "library", library_id: id(700), label: "Team skills", repo: "git.example.org/acme/agent-skills", path: "code-review", commit_sha: SHA_A, pinned_at: now, detached: false, update_pending: true },
};

// A skill added from the instance's library, with an update waiting: the review works the same.
const sharedSkill = {
  ...orgSkill, id: id(711), name: "triage-issues", description: "Sort incoming issues", body: "Sort issues by label.\n", files: [], total_size: 22,
  source: { kind: "library", library_id: id(701), label: "Company skills", repo: "git.example.org/it/skills", path: "triage-issues", commit_sha: SHA_A, pinned_at: now, detached: false, update_pending: true },
};

const catalogue = {
  commit_sha: SHA_B, truncated: false, next_cursor: null,
  items: [
    { path: "code-review", name: "code-review", description: HOSTILE, valid: true, reason: null, size: 480, files: 2, added: [{ tier: "org", id: id(710) }] },
    { path: "release-notes", name: "release-notes", description: "Write release notes from merged pull requests", valid: true, reason: null, size: 900, files: 1, added: [] },
    { path: "Bad_Name", name: "Bad_Name", description: null, valid: false, reason: "the directory name is not a valid skill name", size: 100, files: 1, added: [] },
  ],
};

const sharedCatalogue = {
  commit_sha: SHA_B, truncated: false, next_cursor: null,
  items: [
    { path: "triage-issues", name: "triage-issues", description: "Sort incoming issues", valid: true, reason: null, size: 300, files: 1, added: [{ tier: "org", id: id(711) }] },
    { path: "incident-notes", name: "incident-notes", description: "Write up an incident", valid: true, reason: null, size: 500, files: 1, added: [] },
  ],
};

const content = (path: string, description: string) => ({
  commit_sha: SHA_B, path, name: path, valid: true, reason: null,
  description, body: "Collect merged pull requests.\n\n- Group by label\n- Keep it short\n",
  files: [{ path: "template.md", size: 30, executable: false, binary: false, content: "# Release notes\n\n## Highlights\n", content_b64: null }],
  size: 90, content_sha256: "a".repeat(64),
});

const info = (skill: typeof orgSkill, lib: Lib, updateId: string) => ({
  library: { id: lib.id, label: lib.label, repo: lib.repo, path: skill.name }, commit_sha: SHA_A, pinned_at: now, pinned_by: id(99), detached: false,
  update: {
    id: updateId, kind: "changed", commit_sha: SHA_B, created_at: now, content_sha256: "b".repeat(64), over_budget: false, reason: null,
    diff: {
      description: { old: "Review pull requests", new: HOSTILE, changed: true },
      body: { old: "Review pull requests carefully.\n", new: "Review pull requests carefully.\nCheck the tests first.\n", changed: true },
      files: [{ path: "checklist.md", change: "modified", old: "- tests\n", new: "- tests\n- docs\n", old_executable: false, new_executable: false, binary: false }],
    },
  },
});

/** What a check of a GitHub page address finds; the branch and the folder come from the address. */
const checked = () => ({
  url: "https://github.com/acme/skills.git", repo: "github.com/acme/skills", ref: "main", resolved: { kind: "branch", name: "main", default: false },
  commit_sha: SHA_B, path: "skills", skills: { found: 3, valid: 2, names: ["code-review", "release-notes"] }, truncated: false, suggested_label: "skills",
});

/** What the specs can see of the mocked API after a journey. */
export interface LibraryMock {
  /** The bodies of the writes, by "METHOD path". */
  writes: { key: string; body: unknown }[];
}

/**
 * Routes for the skill library screens; later routes win over the shared mock, the rest falls through to it. The lists are
 * stateful: an add appears in the table, a PATCH of a token flips `token_set`, an approve clears the pending update.
 */
export async function mockLibraries(page: Page): Promise<LibraryMock> {
  await mockApi(page);
  const mock: LibraryMock = { writes: [] };
  let orgLibs: Lib[] = [library, brokenLibrary];
  let instanceLibs: Lib[] = [];
  let instanceSkillUpdated = false;
  await page.route("**/v1/**", async (route) => {
    const req = route.request();
    const p = new URL(req.url()).pathname;
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    const body = () => (req.postDataJSON() ?? {}) as Record<string, unknown>;
    const note = () => mock.writes.push({ key: `${req.method()} ${p}`, body: req.postData() ? body() : null });
    if (p === `/v1/orgs/${org.id}/skill-libraries`) {
      if (req.method() === "POST") {
        note();
        const b = body();
        const added: Lib = { ...library, id: id(703), label: String(b.label), url: b.url, repo: "github.com/acme/skills", ref: b.ref ?? null, path: b.path ?? "", skill_counts: { agent: 0, workspace: 0, org: 0 }, pending_updates: 0 };
        orgLibs = [...orgLibs, added];
        return json(added, 201);
      }
      // The organization's own first, then the instance's (C-1128): the instance's numbers are this organization's.
      return json({ items: [...orgLibs, ...instanceLibs.map((l) => ({ ...l, skill_counts: instanceLibrary.skill_counts, pending_updates: instanceSkillUpdated ? 0 : 1 })), instanceLibrary], next_cursor: null });
    }
    if (p === `/v1/orgs/${org.id}/skill-libraries/check`) {
      note();
      return json(checked());
    }
    if (p === "/v1/admin/skill-libraries") {
      if (req.method() === "POST") {
        note();
        const b = body();
        const added: Lib = { ...instanceLibrary, id: id(704), label: String(b.label), url: b.url, repo: "github.com/acme/skills", path: b.path ?? "", orgs_using: 0, skill_counts: { agent: 0, workspace: 0, org: 0 }, pending_updates: 0 };
        instanceLibs = [...instanceLibs, added];
        return json(added, 201);
      }
      return json({ items: [{ ...instanceLibrary, orgs_using: 2 }, ...instanceLibs], next_cursor: null });
    }
    if (p === "/v1/admin/skill-libraries/check") {
      note();
      return json(checked());
    }
    if (req.method() === "PATCH" && p === `/v1/skill-libraries/${library.id}`) {
      note();
      const b = body();
      if (typeof b.token === "string") orgLibs = orgLibs.map((l) => (l.id === library.id ? { ...l, token_set: true, token_updated_at: now } : l));
      return json(orgLibs.find((l) => l.id === library.id));
    }
    if (req.method() === "POST" && p === `/v1/orgs/${org.id}/skills`) {
      note();
      return json({ ...sharedSkill, id: id(712), name: "incident-notes" }, 201);
    }
    if (p === `/v1/orgs/${org.id}/skills`) return json({ items: [orgSkill, sharedSkill], next_cursor: null });
    if (p === `/v1/orgs/${org.id}/skill-library-updates`) {
      return json({
        items: [
          { id: id(720), kind: "changed", created_at: now, skill: { id: id(710), name: "code-review", tier: "org" }, library: { id: id(700), label: "Team skills" } },
          ...(instanceSkillUpdated ? [] : [{ id: id(721), kind: "changed", created_at: now, skill: { id: id(711), name: "triage-issues", tier: "org" }, library: { id: id(701), label: "Company skills" } }]),
        ],
      });
    }
    if (p === `/v1/skill-libraries/${library.id}/skills`) {
      // Two pages (C-571 pages by 50): "Load more" follows `next_cursor` to the second one.
      if (new URL(req.url()).searchParams.get("cursor") === "c2") {
        const more = ["deploy-checks", "triage-issues"].map((name) => ({ path: name, name, description: "More from the next page", valid: true, reason: null, size: 300, files: 1, added: [] }));
        return json({ ...catalogue, next_cursor: null, items: more });
      }
      return json({ ...catalogue, next_cursor: "c2" });
    }
    if (p === `/v1/skill-libraries/${instanceLibrary.id}/skills`) {
      expect(new URL(req.url()).searchParams.get("org"), "an instance library is browsed as the organization").toBe(org.id);
      return json(sharedCatalogue);
    }
    if (p === `/v1/skill-libraries/${library.id}/skills/release-notes`) return json(content("release-notes", "Write release notes from merged pull requests"));
    if (p === `/v1/skill-libraries/${instanceLibrary.id}/skills/incident-notes`) return json(content("incident-notes", "Write up an incident"));
    if (p === `/v1/skills/${orgSkill.id}/library`) return json(info(orgSkill, library, id(720)));
    if (p === `/v1/skills/${sharedSkill.id}/library`) return json(info(sharedSkill, instanceLibrary, id(721)));
    if (req.method() === "POST" && p === `/v1/skills/${sharedSkill.id}/library/updates/${id(721)}/approve`) {
      note();
      instanceSkillUpdated = true;
      return json({ ...sharedSkill, source: { ...sharedSkill.source, commit_sha: SHA_B, update_pending: false } });
    }
    return route.fallback();
  });
  return mock;
}

const instancePages: LibraryScreen[] = ["instance-libraries", "instance-add"];

export async function openLibraryScreen(page: Page, screen: LibraryScreen, scheme: Scheme): Promise<LibraryMock> {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  const mock = await mockLibraries(page);
  await page.emulateMedia({ colorScheme: scheme });
  const onLibraries = screen === "libraries" || screen === "add" || screen === "found" || screen === "edit";
  await page.goto(instancePages.includes(screen) ? "/admin/skills" : screen === "ws-skills" ? "/o/acme/w/main/skills" : onLibraries ? "/o/acme/skills?tab=libraries" : "/o/acme/skills");
  await page.locator("h1").first().waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  expect(errors, "the skill library screen threw").toEqual([]);
  if (screen === "libraries" || screen === "instance-libraries" || screen === "skills" || screen === "ws-skills") return mock;
  if (screen === "add" || screen === "found" || screen === "instance-add") {
    await page.getByRole("button", { name: "Add a library" }).first().click();
    await page.getByLabel("Repository address", { exact: true }).waitFor();
    if (screen === "add") return mock;
    await page.getByLabel("Repository address", { exact: true }).fill("https://github.com/acme/skills/tree/main/skills");
    await page.getByRole("button", { name: "Check", exact: true }).click();
    await page.getByTestId("library-found").waitFor();
    return mock;
  }
  if (screen === "edit") {
    await page.getByRole("button", { name: "Edit Team skills" }).click();
    await page.getByLabel("Branch or tag").waitFor();
    return mock;
  }
  if (screen === "review") {
    await page.getByRole("button", { name: /^Code review/ }).first().click();
    await page.getByRole("button", { name: "Review update" }).click();
    await page.getByRole("button", { name: "Approve" }).waitFor();
    return mock;
  }
  await page.getByRole("button", { name: "Add from library…" }).click();
  await page.getByRole("list", { name: "Library skills" }).waitFor();
  if (screen === "add-list") return mock;
  await page.getByRole("button", { name: /^release-notes/ }).click();
  await page.getByLabel("Preview of what will be stored").waitFor();
  return mock;
}
