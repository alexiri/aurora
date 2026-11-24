import { expect, type Page } from "@playwright/test";

import { id, mockApi, now, org } from "./mock-api";
import type { Scheme } from "./open";

// P6-TPL-4 (C-1114): the Administration › What agents may do › Agent templates tab, reached through the real UI
// with the mocked API. The data uses the real built-in GitHub package for the apps a template needs; every
// address and commit is plain text, as the server stores it.

export const TEMPLATE_SCREENS = ["tab", "add"] as const;
export type TemplateScreen = (typeof TEMPLATE_SCREENS)[number];

const SHA_A = "0123456789abcdef0123456789abcdef01234567";
const SHA_B = "89abcdef0123456789abcdef0123456789abcdef";

type Source = Record<string, unknown>;
type Template = Record<string, unknown>;

// (C-1107) A source is a git address with a schedule; `token_set` says whether a token is saved, never the token.
const source: Source = {
  id: id(800), name: "Team templates", url: "https://github.com/acme/agent-templates.git", repo: "github.com/acme/agent-templates",
  ref: null, path: "templates", packages_url: "https://github.com/acme/agent-templates/releases/download/{version}/{id}-{version}.zip",
  status: "ok", check_failed_at: null, check_failed_reason: null, token_set: false, token_updated_at: null,
  last_checked_at: now, last_head_sha: SHA_A, update_interval: "1d", contact: null, homepage: null,
  templates: [{ template_id: "schedule-manager", dir: "templates/schedule-manager", name: "Schedule manager", summary: "Triage and schedule the team's work", state: "ingested", head_state: "ok", head_version: "1.1.0" }],
  created_at: now, updated_at: now,
};

// (C-1108) The template of that source: an approved version offered, a newer one waiting for review.
const template: Template = {
  template_id: "schedule-manager", name: "Schedule manager", summary: "Triage and schedule the team's work",
  source: { id: id(800), name: "Team templates" }, current_version: "1.0.0", offered: true,
  head_state: "ok", head_version: "1.1.0", head_problems: null, removed_upstream: false,
  needs_apps: [{ app_id: "github", installed: true }], needs_libraries: [],
  versions: [
    { version: "1.1.0", status: "pending_approval", commit_sha: SHA_B, created_at: now, decided_at: null, decided_by: null, last_attempt_at: null, size_bytes: 4096, status_reason: null },
    { version: "1.0.0", status: "approved", commit_sha: SHA_A, created_at: now, decided_at: now, decided_by: { kind: "person", name: "Ada Lovelace" }, last_attempt_at: null, size_bytes: 3800, status_reason: null },
  ],
};

/** What a check of a GitHub page address finds; the branch and the folder come from the address. */
const preview = () => ({
  url: "https://github.com/acme/agent-templates.git", repo: "github.com/acme/agent-templates",
  ref: "main", resolved: { kind: "branch", name: "main", default: true }, commit_sha: SHA_B, path: "templates",
  manifest: { name: "Acme agent templates", description: "The team's agent templates" },
  templates: [{ template_id: "schedule-manager", dir: "templates/schedule-manager", name: "Schedule manager", summary: "Triage and schedule the team's work", version: "1.1.0", state: "new", problems: [] }],
  truncated: false,
});

/** What a check of one saved source reports (C-1108): a read failure is a result, not an error. */
const checkResult = (sourceId: string) => ({
  source_id: sourceId, status: "updated", commit_sha: SHA_B, check_failed_reason: null, problems: [], ingested: [], waiting: [],
});

/** What the specs can see of the mocked API after a journey. */
export interface TemplateMock {
  /** The bodies of the writes, by "METHOD path". */
  writes: { key: string; body: unknown }[];
}

/**
 * Routes for the Agent templates tab; later routes win over the shared mock, the rest falls through to it. The
 * lists are stateful: an add appears in the table, a check updates the head, a remove empties it.
 */
export async function mockAgentTemplates(page: Page): Promise<TemplateMock> {
  await mockApi(page);
  const mock: TemplateMock = { writes: [] };
  let sources: Source[] = [source];
  let templates: Template[] = [template];
  await page.route("**/v1/**", async (route) => {
    const req = route.request();
    const p = new URL(req.url()).pathname;
    const json = (b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
    const note = () => mock.writes.push({ key: `${req.method()} ${p}`, body: req.postData() ? (req.postDataJSON() ?? {}) : null });
    if (p === "/v1/admin/agent-template-sources/preview") {
      note();
      return json(preview());
    }
    if (p === "/v1/admin/agent-template-sources") {
      if (req.method() === "POST") {
        note();
        const b = req.postDataJSON() as Record<string, unknown>;
        const added: Source = {
          ...source, id: id(801), name: "acme/agent-templates", url: String(b.url), repo: "github.com/acme/agent-templates",
          ref: (b.ref as string) ?? null, path: (b.path as string) ?? "", last_checked_at: now, last_head_sha: SHA_B,
        };
        sources = [...sources, added];
        return json(added, 201);
      }
      return json({ items: sources });
    }
    if (p === `/v1/admin/agent-template-sources/${id(800)}/check`) {
      note();
      sources = sources.map((s) => (s.id === id(800) ? { ...s, last_checked_at: now, last_head_sha: SHA_B, status: "ok" } : s));
      return json(checkResult(id(800)));
    }
    if (req.method() === "PATCH" && p === `/v1/admin/agent-template-sources/${id(800)}`) {
      note();
      const b = req.postDataJSON() as Record<string, unknown>;
      sources = sources.map((s) => (s.id === id(800) ? { ...s, ...b, updated_at: now } : s));
      return json(sources.find((s) => s.id === id(800)));
    }
    if (req.method() === "DELETE" && p === `/v1/admin/agent-template-sources/${id(800)}`) {
      note();
      sources = sources.filter((s) => s.id !== id(800));
      templates = templates.map((t) => ({ ...t, removed_upstream: true }));
      return json(null);
    }
    if (p === "/v1/admin/agent-templates") {
      return json({ items: templates });
    }
    if (req.method() === "PATCH" && p === `/v1/admin/agent-templates/schedule-manager`) {
      note();
      const b = req.postDataJSON() as { offered: boolean };
      templates = templates.map((t) => ({ ...t, offered: b.offered }));
      return json(templates.find((t) => t.template_id === "schedule-manager"));
    }
    if (p === `/v1/orgs/${org.id}/agent-templates`) return json({ items: [] });
    return route.fallback();
  });
  return mock;
}

export async function openTemplateScreen(page: Page, screen: TemplateScreen, scheme: Scheme): Promise<TemplateMock> {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  const mock = await mockAgentTemplates(page);
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto("/admin/internet-access?tab=templates");
  await page.locator("h1").first().waitFor();
  await page.waitForLoadState("networkidle");
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  expect(errors, "the agent templates screen threw").toEqual([]);
  if (screen === "add") {
    await page.getByRole("button", { name: "Add a repository" }).click();
    await page.getByRole("dialog", { name: "Add a template repository" }).waitFor();
  }
  return mock;
}
