import { expect as baseExpect, test, type Page } from "@playwright/test";

import { serious } from "./support/axe";
import { id, mockApi, now, ws } from "./support/mock-api";
import { overflow } from "./support/open";

// FIX-251 (D-488): the whole update journey follows real links, with the API mocked. A new version
// of an offered template is approved; the workspace's attention feed tells the people with agents
// made from it ("available for N agents"), the action lands on the Agents page filtered
// `?needs_update=1`, and **Review updates** walks the agents that are behind, one review at a time.
// A skill the person edited merges cleanly and applies with the merge's hash; a part both sides
// changed is a conflict that waits for a choice per hunk (Keep mine here) before it may apply; when
// the last review closes the walkthrough ends on the unfiltered page and nothing is marked any more.
// Themed, no horizontal scroll at 390 px, no serious axe violation on the pages the journey touches.
const expect = baseExpect.configure({ timeout: 30_000 });

const VERSION = "1.4.0";
const OFFER_SHA = "c".repeat(64);
const MERGED_SHA = "d".repeat(64);
const CONFLICT_SHA = "e".repeat(64);

const tpl = (version: string) => ({ template_id: "schedule-manager", name: "Schedule manager", version });

const readme = "# New in 1.4.0\n\nQuieter triage, and the greeting skill grew.\n";

// Daneel's offer: the person edited the greet skill since 1.0.0, the new version changed it too,
// and the three-way merge composes both edits — a clean merge, ticked by default.
const daneelOffer = {
  version: VERSION,
  content_sha256: OFFER_SHA,
  readme,
  changelog: null,
  inputs_needed: [],
  apps_added: [],
  parts: [
    {
      kind: "skill", key: "greet", state: "merged", skill_ref: null,
      current: "Say hello once a day.",
      proposed: "Say hello once a day, right at nine.",
      merge: { state: "merged", merge_sha256: MERGED_SHA, result: "Say hello once a day, right at nine.", conflicts: [] },
    },
  ],
};

// Kojo's offer: the instructions changed on both sides in the same place — one conflict hunk.
const kojoOffer = {
  version: VERSION,
  content_sha256: OFFER_SHA,
  readme,
  changelog: null,
  inputs_needed: [],
  apps_added: [],
  parts: [
    {
      kind: "instructions", key: "", state: "conflict", skill_ref: null,
      current: "Triage the team's pull requests.",
      proposed: null,
      merge: {
        state: "conflict", merge_sha256: CONFLICT_SHA, result: null,
        conflicts: [{ unit: "text", ours: "Triage the team's pull requests quietly.", theirs: "Triage the team's pull requests first." }],
      },
    },
  ],
};

// The list fixture, with the template provenance and the behind-mark the dto now carries (FIX-251).
const agent0 = {
  id: id(11), org_id: id(1), workspace_id: id(2), handle: "daneel", name: "Daneel", kind: "hosted", is_default: true,
  avatar_url: null, brief: "Triage the team's pull requests and issues.", harness: null, model_provider: null,
  model_endpoint_id: null, model_id: null, effort: null, proactive_messaging: "ask", frozen: null,
  effective_frozen: false, created_at: now, updated_at: now, model_roles: null, subagent_model: "inherit", byo: null,
  template: tpl("1.0.0"),
  needs_update: false,
};

const agentRow = (n: number, handle: string, name: string, applied: boolean) => ({
  ...agent0,
  id: id(n), handle, name,
  template: tpl(applied ? VERSION : "1.0.0"),
  needs_update: !applied,
});

/** The workspace attention item exactly as `template_update_item` builds it (FIX-251, D-488(a)). */
const notice = () => ({
  id: `template_update:schedule-manager:${VERSION}`,
  kind: "template_update", level: "workspace", topic: "agents", subject_id: `schedule-manager:${VERSION}`,
  title: `Schedule manager ${VERSION} is available for 2 agents in Main workspace`,
  detail: "Your changes are kept where they can be merged; memories are never changed.",
  since: now, href: "/o/acme/w/main/agents?needs_update=1", decision: false,
  actors: [], actors_total: 0, reason: null, own: false, can_decline: false,
  action: { label: "Review updates", href: "/o/acme/w/main/agents?needs_update=1" },
  state: "open", closed_at: null, message: null, closed_by_role: null, closed_by: null,
});

const status = (offer: unknown, applied: boolean) => ({
  template: tpl(applied ? VERSION : "1.0.0"),
  available: true,
  notes: [],
  update: applied ? null : offer,
});

interface Journey {
  /** The apply writes, by "METHOD path". */
  writes: { key: string; body: unknown }[];
  /** The agents whose update was applied; the list and the statuses answer from it. */
  applied: Set<string>;
}

/** The shared mock plus the stateful journey routes (registered after `mockApi`, so they match first). */
async function mockJourney(page: Page): Promise<Journey> {
  await mockApi(page);
  const journey: Journey = { writes: [], applied: new Set() };
  await page.route("**/v1/**", async (route) => {
    const req = route.request();
    const p = new URL(req.url()).pathname;
    const json = (b: unknown, s = 200) => route.fulfill({ status: s, contentType: "application/json", body: JSON.stringify(b) });
    if (p === `/v1/workspaces/${ws.id}/attention`) {
      const waiting = journey.applied.size < 2;
      return json({ items: waiting ? [notice()] : [], count: waiting ? 1 : 0 });
    }
    if (p === "/v1/attention/counts") {
      const waiting = journey.applied.size < 2;
      return json({ administration: null, organization: null, workspace: { workspace_id: ws.id, count: waiting ? 1 : 0, topics: waiting ? { agents: 1 } : {} } });
    }
    if (p === `/v1/workspaces/${ws.id}/agents`) {
      return json({
        items: [
          agentRow(11, "daneel", "Daneel", journey.applied.has(id(11))),
          agentRow(12, "kojo", "Kojo", journey.applied.has(id(12))),
        ],
        next_cursor: null,
      });
    }
    if (/^\/v1\/agents\/[^/]+\/helpers$/.test(p)) return json({ items: [] });
    for (const [aid, offer] of [
      [id(11), daneelOffer],
      [id(12), kojoOffer],
    ] as const) {
      if (p === `/v1/agents/${aid}/template`) return json(status(offer, journey.applied.has(aid)));
      if (p === `/v1/agents/${aid}/template/apply` && req.method() === "POST") {
        journey.writes.push({ key: `POST ${p}`, body: req.postDataJSON() });
        journey.applied.add(aid);
        return json(status(offer, true));
      }
    }
    return route.fallback();
  });
  return journey;
}

async function open(page: Page, scheme: "light" | "dark", path: string) {
  const errors: string[] = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => {
    if (m.type() === "error" && !/^Failed to load resource/.test(m.text())) errors.push(m.text());
  });
  const journey = await mockJourney(page);
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(path);
  await page.locator("h1, h2").first().waitFor();
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  return { errors, journey };
}

for (const scheme of ["light", "dark"] as const) {
  for (const width of [1280, 390]) {
    test.describe(`the template update journey (${scheme}, ${width} px)`, () => {
      test.use({ viewport: { width, height: width === 1280 ? 900 : 844 } });

      test("the notice leads to the review and both merges apply, then nothing is left", async ({ page }) => {
        const { errors, journey } = await open(page, scheme, "/o/acme/w/main");

        // The workspace's Needs you card carries the update notice, informational, with the one
        // action that lands on the filtered Agents page.
        const waiting = page.getByRole("list", { name: "Waiting for you" });
        await expect(waiting.getByText(`Schedule manager ${VERSION} is available for 2 agents in Main workspace`)).toBeVisible();
        await expect(waiting.getByText("Your changes are kept where they can be merged; memories are never changed.")).toBeVisible();
        const review = waiting.getByRole("link", { name: "Review updates" });
        await expect(review).toHaveAttribute("href", "/o/acme/w/main/agents?needs_update=1");
        expect(await overflow(page), "Overview overflows horizontally").toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);

        await review.click();
        await expect(page).toHaveURL(/\/o\/acme\/w\/main\/agents\?needs_update=1$/);
        const banner = page.getByTestId("needs-update");
        await expect(banner).toContainText("2 agents need an update.");
        await expect(page.getByTestId("agent-needs-update")).toHaveCount(2);
        expect(await overflow(page), "Agents overflows horizontally").toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);

        // The walkthrough reviews the agents that are behind, one at a time. Daneel first.
        await banner.getByRole("button", { name: "Review updates" }).click();
        const dlg = page.getByRole("dialog", { name: "Review changes" });
        await expect(dlg).toContainText(`Version ${VERSION} of Schedule manager. Daneel keeps working as it is until you apply it.`);
        await expect(dlg.getByTestId("memories-note")).toHaveText("Memories are never changed by an update.");
        // The locally edited skill is a clean merge: ticked by default, diffed against the current text.
        const part = dlg.getByTestId("offer-part-0");
        await expect(part.getByTestId("part-state")).toHaveText(
          "Merged with your changes: your edit stays, and the new version's change is applied around it. Tick to apply the merged text; leave it unticked to keep yours.",
        );
        await expect(part.getByRole("checkbox")).toBeChecked();
        expect(await serious(page)).toEqual([]);

        await dlg.getByRole("button", { name: "Apply" }).click();
        // The walkthrough moves to the next agent behind, reading its offer fresh.
        await expect(dlg).toContainText("Kojo keeps working as it is until you apply it.");
        // The part both sides changed waits for a choice: ticking it holds Apply until every hunk
        // is resolved (the server would refuse with `template_merge_unresolved`).
        const conflict = dlg.getByTestId("offer-part-0");
        await expect(conflict.getByTestId("part-state")).toContainText("You both changed it.");
        await conflict.getByRole("checkbox").check();
        await expect(dlg.getByTestId("merge-unresolved")).toBeVisible();
        await expect(dlg.getByRole("button", { name: "Apply" })).toBeDisabled();
        await conflict.getByTestId("merge-conflict-0").getByRole("radio", { name: "Keep mine" }).check();
        await expect(dlg.getByRole("button", { name: "Apply" })).toBeEnabled();
        await dlg.getByRole("button", { name: "Apply" }).click();

        // The last close ends the walkthrough: back on the unfiltered page, both agents listed,
        // nothing marked. It follows the last apply's answer, so every write is in by then (a click
        // returns before its request is sent).
        await expect(dlg).toBeHidden();
        await expect(page).toHaveURL(/\/o\/acme\/w\/main\/agents$/);
        await expect(page.getByRole("link", { name: "Daneel", exact: true })).toBeVisible();
        await expect(page.getByRole("link", { name: "Kojo", exact: true })).toBeVisible();
        await expect(page.getByTestId("needs-update")).toHaveCount(0);
        await expect(page.getByTestId("agent-needs-update")).toHaveCount(0);

        // The applies, exactly these two: the merged part bound to its merge hash; the conflict part
        // with one resolution per hunk, in the offer's order.
        expect(journey.writes).toEqual([
          {
            key: `POST /v1/agents/${id(11)}/template/apply`,
            body: {
              version: VERSION,
              content_sha256: OFFER_SHA,
              parts: [{ kind: "skill", key: "greet", merge_sha256: MERGED_SHA }],
              skill_refs: [],
              inputs: {},
            },
          },
          {
            key: `POST /v1/agents/${id(12)}/template/apply`,
            body: {
              version: VERSION,
              content_sha256: OFFER_SHA,
              parts: [{ kind: "instructions", key: "", merge_sha256: CONFLICT_SHA, resolutions: [{ unit: "text", choice: "keep_mine" }] }],
              skill_refs: [],
              inputs: {},
            },
          },
        ]);

        expect(await overflow(page), "Agents overflows horizontally").toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
        expect(errors).toEqual([]);
      });

      test("the agent page marks the update and the Manage tab offers the review", async ({ page }) => {
        const { errors } = await open(page, scheme, "/o/acme/w/main/agents?needs_update=1");
        await expect(page.getByTestId("needs-update")).toContainText("2 agents need an update.");

        await page.getByRole("link", { name: "Daneel", exact: true }).click();
        await expect(page).toHaveURL(/\/o\/acme\/w\/main\/agents\/daneel$/);
        const link = page.getByRole("link", { name: "Update available for Daneel" });
        await expect(link).toHaveAttribute("href", "/o/acme/w/main/agents/daneel?tab=manage");
        await expect(link.getByTestId("agent-header-needs-update")).toHaveText("Update available");
        await link.click();

        // The Manage tab's Template section is the review the mark points at.
        await expect(page).toHaveURL(/\/o\/acme\/w\/main\/agents\/daneel\?tab=manage$/);
        const section = page.getByTestId("agent-template-section");
        await expect(section.getByTestId("template-update")).toHaveText(
          `Version ${VERSION} is available. Daneel keeps working as it is until you apply it.`,
        );
        await expect(section.getByRole("button", { name: "Review changes" })).toBeVisible();
        expect(await overflow(page), "Manage tab overflows horizontally").toBeLessThanOrEqual(0);
        expect(await serious(page)).toEqual([]);
        expect(errors).toEqual([]);
      });
    });
  }
}
