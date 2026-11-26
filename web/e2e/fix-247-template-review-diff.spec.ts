import { expect as baseExpect, test, type Page, type Route } from "@playwright/test";

import { serious } from "./support/axe";
import { id } from "./support/mock-api";
import { overflow } from "./support/open";
import { mockAgentTemplates } from "./support/agent-templates";

// FIX-247: the administrator's review of a new template version shows everything that changed against
// the version people use now, up front: the summary in words, the "Look at these first" block for the
// changes that widen what the agent can do or reach, the whole-line diffs and the unchanged parts
// collapsed — before the full content. Followed from the administration Overview's Review link, with
// the API mocked; both themes, no horizontal scroll at 390 px, no serious axe violation.
const expect = baseExpect.configure({ timeout: 30_000 });

const SHA_B = "89abcdef0123456789abcdef0123456789abcdef";
const SHA_13 = "b".repeat(64);
const SHA_14 = "c".repeat(64);

// (C-1108) Version 1.3.0 is what people use now; 1.4.0 waits for review with a changed instruction,
// a changed skill, a new app (Slack) and a new schedule — one change of each widening kind.
const content13 = {
  name: "Schedule manager", summary: "Triage and schedule the team's work",
  readme: "# Schedule manager\n\nWhat it does.\n",
  effort: null, min_core: null, agent: { name: "Daneel", may_message_first: false },
  instructions: "Triage the queue.",
  inputs: [],
  apps: [{ app: "github", required: true, level: "ask", tools: null, why: "Reads the pull requests" }],
  skills: [{ name: "code-review", use_it_when: "When a pull request is open", body: "Review carefully.\n", files: [] }],
  skill_refs: [], helpers: [], memories: [], automations: [], has_avatar: false,
};
const content14 = {
  ...content13,
  instructions: "Triage carefully.",
  apps: [...content13.apps, { app: "slack", required: false, level: "read", tools: null, why: "Posts to the channel" }],
  skills: [{ name: "code-review", use_it_when: "When a pull request is open", body: "Review every pull request carefully.\n", files: [] }],
  automations: [{ kind: "schedule", name: "morning-triage", prompt: "Triage the queue.", cron: "0 9 * * *" }],
};

const versionDetail = (version: string, sha: string, status: string, sha256: string, content: Record<string, unknown>) => ({
  template_id: "schedule-manager", version, status, status_reason: null, commit_sha: sha, content_sha256: sha256,
  archive_sha256: null, size_bytes: 4096,
  source: { id: id(800), name: "Team templates", repo: "github.com/acme/agent-templates", ref: null, path: "templates" },
  content,
});

const SINCE = "2026-11-26T10:00:00Z";
const common = { level: "administration", actors: [], actors_total: 0, reason: null, own: false, can_decline: false, state: "open", closed_at: null, message: null, closed_by_role: null, closed_by: null };
const waitingVersion = {
  ...common, id: `agent_template_version:${id(901)}`, topic: "internet-access", kind: "agent_template_version", subject_id: id(901),
  title: "Schedule manager 1.4.0 is waiting for review",
  detail: "A new agent template version was read from its source. Review and approve or reject it; people cannot use it until you approve.",
  since: SINCE, href: "/admin/internet-access/templates/schedule-manager/1.4.0", decision: true,
  action: { label: "Review", href: "/admin/internet-access/templates/schedule-manager/1.4.0" },
};

/** The review routes for 1.4.0 against 1.3.0, registered last so they win over the shared mocks. */
async function mockReviewAgainst130(page: Page): Promise<{ writes: { key: string; body: unknown }[] }> {
  const writes: { key: string; body: unknown }[] = [];
  const json = (route: Route, b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
  await page.route(/\/v1\/admin\/agent-templates(\?.*)?$/, (route) =>
    json(route, {
      items: [{
        template_id: "schedule-manager", name: "Schedule manager", summary: "Triage and schedule the team's work",
        source: { id: id(800), name: "Team templates" }, current_version: "1.3.0", offered: true,
        head_state: "ok", head_version: "1.4.0", head_problems: null, removed_upstream: false,
        needs_apps: [{ app_id: "github", installed: true }, { app_id: "slack", installed: true }], needs_libraries: [],
        versions: [
          { version: "1.4.0", status: "pending_approval", commit_sha: SHA_B, created_at: SINCE, decided_at: null, decided_by: null, last_attempt_at: null, size_bytes: 4096, status_reason: null },
          { version: "1.3.0", status: "approved", commit_sha: SHA_B, created_at: SINCE, decided_at: SINCE, decided_by: { kind: "person", name: "Ada Lovelace" }, last_attempt_at: null, size_bytes: 3800, status_reason: null },
        ],
      }],
    }),
  );
  await page.route("**/v1/admin/agent-templates/schedule-manager/versions/1.4.0", (route) =>
    json(route, versionDetail("1.4.0", SHA_B, "pending_approval", SHA_14, content14)),
  );
  await page.route("**/v1/admin/agent-templates/schedule-manager/versions/1.3.0", (route) =>
    json(route, versionDetail("1.3.0", SHA_B, "approved", SHA_13, content13)),
  );
  await page.route("**/v1/admin/agent-templates/schedule-manager/versions/1.4.0/approve", (route) => {
    writes.push({ key: "POST /v1/admin/agent-templates/schedule-manager/versions/1.4.0/approve", body: route.request().postDataJSON() });
    return json(route, versionDetail("1.4.0", SHA_B, "approved", SHA_14, content14));
  });
  await page.route(/\/v1\/admin\/attention(\?.*)?$/, (route) => json(route, { items: [waitingVersion], count: 1 }));
  await page.route(/\/v1\/attention\/counts(\?.*)?$/, (route) =>
    json(route, { administration: { count: 1, topics: { "internet-access": 1 } }, organization: null, workspace: null }),
  );
  return { writes };
}

for (const width of [1280, 390]) {
  test.describe(`the review's diff against the current version (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });
    for (const scheme of ["light", "dark"] as const) {
      test(`Overview → Review → what changed since 1.3.0 → approve (${scheme})`, async ({ page }) => {
        const errors: string[] = [];
        page.on("pageerror", (e) => errors.push(e.message));
        await mockAgentTemplates(page);
        const mock = await mockReviewAgainst130(page);
        await page.emulateMedia({ colorScheme: scheme });
        await page.goto("/admin");
        await expect.poll(() => page.locator("html").getAttribute("data-theme")).toBe(scheme);
        await page.waitForLoadState("networkidle");

        const section = page.getByTestId("admin-attention");
        await expect(section.getByText("Schedule manager 1.4.0 is waiting for review")).toBeVisible();
        await section.getByRole("link", { name: "Review: Schedule manager 1.4.0 is waiting for review" }).click();
        await expect(page).toHaveURL(/\/admin\/internet-access\/templates\/schedule-manager\/1\.4\.0$/);
        await expect(page.getByRole("heading", { name: "Schedule manager 1.4.0" })).toBeVisible();
        expect(await serious(page)).toEqual([]);

        // The diff comes first, before the full content: summary, the widening changes, the diffs.
        const diff = page.getByRole("region", { name: "What changed since 1.3.0" });
        await expect(diff.getByText("4 changes: Instructions, 1 skill, a new app, a new schedule.")).toBeVisible();
        const attention = diff.getByTestId("diff-attention");
        await expect(attention.getByText("Look at these first")).toBeVisible();
        await expect(attention.getByText("App slack", { exact: true })).toBeVisible();
        await expect(attention.getByText("Schedule morning-triage", { exact: true })).toBeVisible();
        await expect(diff.getByText("- Triage the queue.")).toBeVisible();
        await expect(diff.getByText("+ Triage carefully.")).toBeVisible();
        await expect(diff.getByTestId("diff-unchanged")).toContainText("Unchanged: Name, Summary, About this template, Agent name, May message first, Effort, Skills from libraries, Helper files, Memories, Inputs, Minimum Aurora version, Avatar");
        // The full content stays below the diff.
        expect(
          await page.evaluate(() => {
            const diff = document.getElementById("what-changed-h")!;
            const about = [...document.querySelectorAll<HTMLHeadingElement>("h2")].find((h) => h.textContent === "About this template")!;
            return !!(diff.compareDocumentPosition(about) & Node.DOCUMENT_POSITION_FOLLOWING);
          }),
        ).toBeTruthy();

        if (width === 390) {
          expect(await overflow(page), "the review page overflows horizontally").toBeLessThanOrEqual(0);
        }

        await page.getByRole("button", { name: "Approve" }).click();
        await expect.poll(() => mock.writes.at(-1)).toEqual({ key: "POST /v1/admin/agent-templates/schedule-manager/versions/1.4.0/approve", body: { content_sha256: SHA_14 } });
        expect(errors, "the review journey threw").toEqual([]);
      });
    }
  });
}
