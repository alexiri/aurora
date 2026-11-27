import { expect as baseExpect, test, type Page, type Route } from "@playwright/test";

import { serious } from "./support/axe";
import { id } from "./support/mock-api";
import { overflow } from "./support/open";
import { mockAgentTemplates } from "./support/agent-templates";

// FIX-250: the review of a new version shows each part's diff inside the section it belongs to,
// folded GitLab-style: three lines of context, "N unchanged lines" rows that expand, **Show the
// whole file**, unchanged sections collapsed to one line "Unchanged". Followed from the
// administration Overview's Review link, with the API mocked; both themes, 1280 and 390 px, no
// horizontal scroll, no serious axe violation.
const expect = baseExpect.configure({ timeout: 30_000 });

const SHA_B = "89abcdef0123456789abcdef0123456789abcdef";
const SHA_13 = "b".repeat(64);
const SHA_14 = "c".repeat(64);

const instructions = (line12: string) =>
  Array.from({ length: 24 }, (_, i) => (i === 11 ? line12 : `Step ${i + 1}: check the queue.`)).join("\n");
const skillBody = (line15: string) => Array.from({ length: 30 }, (_, i) => (i === 14 ? line15 : `Rule ${i + 1}: be precise.`)).join("\n");
const checklist = (line5: string) => Array.from({ length: 10 }, (_, i) => (i === 4 ? line5 : `- check ${i + 1}`)).join("\n");

// (C-1108) Version 1.3.0 is what people use now; 1.4.0 waits for review with a changed instruction
// line, a changed skill body line and a changed skill file line — everything else untouched.
const content13 = {
  name: "Schedule manager", summary: "Triage and schedule the team's work",
  readme: "# Schedule manager\n\nWhat it does.\n",
  effort: null, min_core: null, agent: { name: "Daneel", may_message_first: false },
  instructions: instructions("Step 12: triage the queue."),
  inputs: [], apps: [], skills: [{
    name: "code-review", use_it_when: "When a pull request is open", body: skillBody("Rule 15: review the diff."),
    files: [{ path: "checklist.md", size: 120, executable: false, sha256: "e".repeat(64), text: checklist("- check 5") }],
  }],
  skill_refs: [], helpers: [], memories: [], automations: [], has_avatar: false,
};
const content14 = {
  ...content13,
  instructions: instructions("Step 12: triage it carefully."),
  skills: [{
    name: "code-review", use_it_when: "When a pull request is open", body: skillBody("Rule 15: review the whole diff."),
    files: [{ path: "checklist.md", size: 124, executable: false, sha256: "f".repeat(64), text: checklist("- check the diff") }],
  }],
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
async function mockReviewAgainst130(page: Page, base: Record<string, unknown> = content13, next: Record<string, unknown> = content14): Promise<void> {
  const json = (route: Route, b: unknown, status = 200) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(b) });
  await page.route(/\/v1\/admin\/agent-templates(\?.*)?$/, (route) =>
    json(route, {
      items: [{
        template_id: "schedule-manager", name: "Schedule manager", summary: "Triage and schedule the team's work",
        source: { id: id(800), name: "Team templates" }, current_version: "1.3.0", offered: true,
        head_state: "ok", head_version: "1.4.0", head_problems: null, removed_upstream: false,
        needs_apps: [], needs_libraries: [],
        versions: [
          { version: "1.4.0", status: "pending_approval", commit_sha: SHA_B, created_at: SINCE, decided_at: null, decided_by: null, last_attempt_at: null, size_bytes: 4096, status_reason: null },
          { version: "1.3.0", status: "approved", commit_sha: SHA_B, created_at: SINCE, decided_at: SINCE, decided_by: { kind: "person", name: "Ada Lovelace" }, last_attempt_at: null, size_bytes: 3800, status_reason: null },
        ],
      }],
    }),
  );
  await page.route("**/v1/admin/agent-templates/schedule-manager/versions/1.4.0", (route) =>
    json(route, versionDetail("1.4.0", SHA_B, "pending_approval", SHA_14, next)),
  );
  await page.route("**/v1/admin/agent-templates/schedule-manager/versions/1.3.0", (route) =>
    json(route, versionDetail("1.3.0", SHA_B, "approved", SHA_13, base)),
  );
  await page.route(/\/v1\/admin\/attention(\?.*)?$/, (route) => json(route, { items: [waitingVersion], count: 1 }));
  await page.route(/\/v1\/attention\/counts(\?.*)?$/, (route) =>
    json(route, { administration: { count: 1, topics: { "internet-access": 1 } }, organization: null, workspace: null }),
  );
}

for (const width of [1280, 390]) {
  test.describe(`the review's diffs sit inside their sections (${width} px)`, () => {
    test.use({ viewport: { width, height: 900 } });
    for (const scheme of ["light", "dark"] as const) {
      test(`Overview → Review → the folded diffs, the unchanged sections collapsed (${scheme})`, async ({ page }) => {
        const errors: string[] = [];
        page.on("pageerror", (e) => errors.push(e.message));
        await mockAgentTemplates(page);
        await mockReviewAgainst130(page);
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

        // The what-changed block is short: the one-line summary, no diffs of its own.
        const block = page.getByRole("region", { name: "What changed since 1.3.0" });
        await expect(block.getByTestId("diff-summary")).toHaveText("3 changes: Instructions, 1 skill.");
        await expect(block.getByTestId("diff-view")).toHaveCount(0);

        // The Instructions diff is folded inside the Instructions section: two folds around the pair.
        const instructions = page.getByRole("region", { name: "Instructions", exact: true });
        const diff = instructions.getByRole("region", { name: "Diff of Instructions" });
        const folds = diff.getByTestId("diff-fold");
        await expect(folds).toHaveCount(2);
        await expect(folds.first()).toContainText("8 unchanged lines");
        await expect(folds.last()).toContainText("9 unchanged lines");
        await expect(diff.getByRole("button", { name: "Show all 8 unchanged lines (lines 1 to 8)" })).toBeVisible();
        await diff.getByRole("button", { name: "Show all 8 unchanged lines (lines 1 to 8)" }).click();
        await expect(diff.getByText("Step 1: check the queue.")).toBeVisible();
        // Show the whole file: every fold opens, the whole file is there.
        await diff.getByRole("switch", { name: "Show the whole file" }).click();
        await expect(folds).toHaveCount(0);
        await expect(diff.getByText("Step 24: check the queue.")).toBeVisible();

        // The skill's diff sits inside the skill's card, folded the same way.
        const skill = page.getByRole("region", { name: "Skill code-review", exact: true });
        const bodyDiff = skill.getByRole("region", { name: "Diff of Skill code-review body" });
        await expect(bodyDiff.getByTestId("diff-fold").first()).toContainText("11 unchanged lines");
        await bodyDiff.getByRole("button", { name: "Show all 11 unchanged lines (lines 1 to 11)" }).click();
        await expect(bodyDiff.getByText("Rule 1: be precise.")).toBeVisible();
        // The skill file's diff is in the same card, and the current text is one toggle away.
        await expect(skill.getByRole("region", { name: "Diff of File checklist.md of skill code-review" })).toBeVisible();
        await instructions.getByRole("button", { name: "Show the current text" }).click();
        await expect(instructions.getByLabel("Instructions", { exact: true })).toContainText("Step 12: triage it carefully.");

        // The sections nothing touched are collapsed to one line "Unchanged" that expands.
        const unchanged = page.getByTestId("diff-unchanged");
        await expect(unchanged).toHaveCount(7);
        const about = page.getByRole("region", { name: "About this template", exact: true }).getByTestId("diff-unchanged");
        await about.locator("summary").click();
        await expect(page.getByLabel("README.md source text")).toContainText("# Schedule manager");

        if (width === 390) {
          expect(await overflow(page), "the review page overflows horizontally").toBeLessThanOrEqual(0);
        }
        expect(errors, "the review journey threw").toEqual([]);
      });

      // FIX-271: what the new version drops is on the page, a new skill reads as new, and an unlock
      // is said in words and looked at first.
      test(`removed helper, removed and added skills, an unlocked memory (${scheme})`, async ({ page }) => {
        const errors: string[] = [];
        page.on("pageerror", (e) => errors.push(e.message));
        const helper = (path: string, text: string) => ({ path, size: text.length, executable: false, sha256: "a".repeat(64), text });
        const base = {
          ...content13,
          helpers: [helper("scripts/gone.sh", `echo ${"a-very-long-line-".repeat(12)}\n`)],
          memories: [{ key: "rules", content: "Be kind.", locked: true }],
        };
        const next = {
          ...content13,
          skills: [{ name: "release-notes", use_it_when: "When a release is cut", body: "Write the notes.\n", files: [] }],
          memories: [{ key: "rules", content: "Be kind.", locked: false }],
        };
        await mockAgentTemplates(page);
        await mockReviewAgainst130(page, base, next);
        await page.emulateMedia({ colorScheme: scheme });
        await page.goto("/admin/internet-access/templates/schedule-manager/1.4.0");
        await expect.poll(() => page.locator("html").getAttribute("data-theme")).toBe(scheme);
        await expect(page.getByRole("region", { name: "What changed since 1.3.0" })).toBeVisible();
        await expect(page.getByTestId("diff-attention")).toContainText("Memory rules — No longer locked.");
        await expect(page.getByTestId("memory-lock-change")).toHaveText("No longer locked.");

        const removedHelper = page.getByRole("region", { name: "Helper files", exact: true }).getByTestId("removed-file-scripts/gone.sh");
        await expect(removedHelper.getByText("Removed", { exact: true })).toBeVisible();
        await expect(removedHelper.getByTestId("diff-box")).toContainText("a-very-long-line-");
        const removedSkill = page.getByRole("region", { name: "Removed skill code-review", exact: true });
        await expect(removedSkill.locator("p").first().getByText("Removed", { exact: true })).toBeVisible();
        await expect(removedSkill.getByTestId("removed-file-checklist.md")).toBeVisible();
        const added = page.getByRole("region", { name: "Skill release-notes", exact: true });
        await expect(added.getByTestId("diff-unchanged")).toHaveCount(0);
        await expect(added.getByRole("region", { name: "Diff of Skill release-notes body", exact: true })).toContainText("(added)");
        await expect(page.getByRole("button", { name: "Approve" })).toBeEnabled();
        expect(await serious(page)).toEqual([]);
        expect(await overflow(page), "the review page overflows horizontally").toBeLessThanOrEqual(0);
        expect(errors, "the review journey threw").toEqual([]);
      });
    }
  });
}
