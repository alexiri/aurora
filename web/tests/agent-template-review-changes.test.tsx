import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ReviewChangesDialog } from "@/components/templates/review-changes-dialog";
import { AgentTemplateSection } from "@/components/templates/template-section";
import type { TemplateUpdateOffer } from "@/lib/api/agent-templates";
import { json, mockFetch } from "./support/api";

// (C-1117, C-1112) **Review changes**, the dialog behind the agent's Template section: the new
// version's README in a fold, every part with its state in words and the full diff, the checkboxes
// (ticked for `unchanged_here` and `new`), the inputs, the apps with **Give access** links, and an
// apply that sends exactly the ticked parts, bound to the shown hash.

const sha = "c".repeat(64);

const library = {
  id: "11111111-1111-1111-1111-111111111111",
  label: "Acme skills",
  path: "skills",
  repo: "acme/skills",
  scope: "organization",
  commit_sha: "abcd1234abcd1234abcd1234abcd1234abcd1234",
};

const offer: TemplateUpdateOffer = {
  version: "1.1.0",
  content_sha256: sha,
  readme: "# New in 1.1.0\n\nLess noise.",
  changelog: null,
  inputs_needed: [
    { id: "region", label: "Region", kind: "text", required: true, default: "eu", help: null, max_length: null, pattern: null },
    { id: "mode", label: "Mode", kind: "choice", required: false, default: "balanced", help: null, max_length: null, options: ["balanced", "fast"], pattern: null },
  ],
  apps_added: [{ app_ids: ["github"], level: "act", required: true, why: "It talks to GitHub." }],
  parts: [
    { kind: "instructions", key: "", state: "edited_here", current: "Line one.\nLine two.", proposed: "Line one.\nLine two changed.", skill_ref: null },
    { kind: "skill", key: "tidy", state: "removed_here", current: "keep the place tidy", proposed: null, skill_ref: null },
    { kind: "helpers", key: "", state: "unchanged_here", current: "old helper text", proposed: "new helper text", skill_ref: null },
    { kind: "library_skill", key: "formatter", state: "new", current: null, proposed: null, skill_ref: { index: 0, kind: "library", name: "Formatter", repo: "acme/skills", why: "It formats the texts.", required: true, available: true, library } },
    { kind: "library_skill", key: "other", state: "new", current: null, proposed: null, skill_ref: { index: 1, kind: "library", name: "Other skill", repo: "other/lib", why: "It helps.", required: true, available: false, library: null, unavailable_reason: "library_not_added" } },
  ],
};

const applied = { template: { template_id: "schedule-manager", name: "Schedule manager", version: "1.1.0" }, available: true, notes: [] };

function serve(extra: Record<string, (req: Request) => Response | Promise<Response>> = {}) {
  return mockFetch({
    "POST /v1/agents/a1/template/apply": () => json(200, applied),
    ...extra,
  });
}

function mountDialog(onStale: () => void = () => {}, onClose: () => void = () => {}) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <ReviewChangesDialog
        agentId="a1"
        agentName="Daneel"
        templateName="Schedule manager"
        offer={offer}
        agentBase="/o/o1/w/w1/agents/daneel"
        onStale={onStale}
        onClose={onClose}
      />
    </QueryClientProvider>,
  );
}

function part(i: number) {
  return screen.getByTestId(`offer-part-${i}`);
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

beforeEach(() => {
  // jsdom has no `matchMedia`; the dialog's `fixedHeight` asks whether the viewport is wide.
  vi.stubGlobal("matchMedia", (query: string) => ({ matches: true, media: query, addEventListener: () => {}, removeEventListener: () => {} }));
});

describe("the Review changes dialog (C-1117)", () => {
  it("shows the new version's README as a fold and every part with its state in words", async () => {
    serve();
    mountDialog();
    const dlg = await screen.findByRole("dialog", { name: "Review changes" });
    expect(dlg).toHaveTextContent("Version 1.1.0 of Schedule manager. Daneel keeps working as it is until you apply it.");
    // The README fold: shown, never applied.
    const readme = within(dlg).getByTestId("offer-readme");
    fireEvent.click(readme.querySelector("summary")!);
    expect(within(readme).getByText("Text from Schedule manager 1.1.0. It is shown, never applied.")).toBeTruthy();
    expect(within(readme).getByText("Less noise.")).toBeTruthy();
    // Each part's state, in words.
    expect(within(part(0)).getByTestId("part-state")).toHaveTextContent("You changed it since. Leave it unticked to keep yours.");
    expect(within(part(1)).getByTestId("part-state")).toHaveTextContent("It is not on Daneel any more. Tick to add it back.");
    expect(within(part(2)).getByTestId("part-state")).toHaveTextContent("The template changed it; you have not. Tick to take the new text.");
    expect(within(part(3)).getByTestId("part-state")).toHaveTextContent("New in the template.");
    expect(within(part(4)).getByTestId("part-state")).toHaveTextContent("New in the template.");
  });

  it("ticks the checkboxes for `unchanged_here` and `new` parts only", () => {
    serve();
    mountDialog();
    expect((within(part(0)).getByLabelText("Take this change") as HTMLInputElement).checked).toBe(false);
    expect((within(part(1)).getByLabelText("Take this change") as HTMLInputElement).checked).toBe(false);
    expect((within(part(2)).getByLabelText("Take this change") as HTMLInputElement).checked).toBe(true);
    expect((within(part(3)).getByLabelText("Take this change") as HTMLInputElement).checked).toBe(true);
    // The unreachable new library skill has no checkbox at all (C-1117).
    expect(within(part(4)).queryByLabelText("Take this change")).not.toBeInTheDocument();
    expect(screen.getAllByLabelText("Take this change")).toHaveLength(4);
  });

  it("shows the full diff of a part that differs on both sides, and the one-sided texts otherwise", () => {
    serve();
    mountDialog();
    const diff = screen.getByLabelText("Diff of Instructions, current against the new version");
    expect(diff).toHaveTextContent("- Line two.");
    expect(diff).toHaveTextContent("+ Line two changed.");
    // `removed_here`: only what the agent has now; `unchanged_here`: current against proposed.
    expect(screen.getByLabelText("Skill tidy as Daneel has it now")).toHaveTextContent("keep the place tidy");
    expect(screen.getByLabelText("Diff of Helper files, current against the new version")).toBeTruthy();
  });

  it("offers no checkbox for an unreachable new library skill and says why instead", () => {
    serve();
    mountDialog();
    expect(within(part(4)).getByTestId("ref-unavailable")).toHaveTextContent("Needs the skill library other/lib. An organization admin can add it.");
  });

  it("says where a reachable library skill comes from, without its content", () => {
    serve();
    mountDialog();
    expect(within(part(3)).getByTestId("part-state")).toHaveTextContent("New in the template.");
    expect(part(3)).toHaveTextContent("From Acme skills (acme/skills) at abcd123. Its content is the library's and is never part of this update; the library proposes its own changes.");
  });

  it("asks the inputs the new version needs, with the defaults prefilled", () => {
    serve();
    mountDialog();
    expect(within(screen.getByTestId("offer-inputs")).getByText("The new version needs these to fill in its texts.")).toBeTruthy();
    expect((screen.getByLabelText("Region") as HTMLInputElement).value).toBe("eu");
    expect((screen.getByLabelText("Mode (optional)") as HTMLSelectElement).value).toBe("balanced");
  });

  it("links each new app to the Access tab's journey, without giving access itself", () => {
    serve();
    mountDialog();
    expect(within(screen.getByTestId("offer-apps")).getByText("The update does not give access. Daneel can use these once you give access on the Apps & access page.")).toBeTruthy();
    const link = within(screen.getByTestId("offer-app")).getByRole("link", { name: "Give access" });
    expect(link.getAttribute("href")).toBe("/o/o1/w/w1/agents/daneel?tab=access&connect=github");
  });

  it("apply sends exactly the ticked parts, bound to the shown hash (C-1112)", async () => {
    const bodies: unknown[] = [];
    serve({
      "POST /v1/agents/a1/template/apply": async (req) => {
        bodies.push(JSON.parse(await req.clone().text()));
        return json(200, applied);
      },
    });
    mountDialog();
    await screen.findByRole("dialog", { name: "Review changes" });
    // Take an edited part, leave an unchanged one.
    fireEvent.click(within(part(0)).getByLabelText("Take this change"));
    fireEvent.click(within(part(2)).getByLabelText("Take this change"));
    fireEvent.change(screen.getByLabelText("Region"), { target: { value: "eu-west" } });
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(bodies).toHaveLength(1));
    expect(bodies[0]).toEqual({
      version: "1.1.0",
      content_sha256: sha,
      parts: [
        { kind: "instructions", key: "" },
        { kind: "library_skill", key: "formatter" },
      ],
      skill_refs: [{ index: 0, library_id: library.id, commit_sha: library.commit_sha }],
      inputs: { region: "eu-west", mode: "balanced" },
    });
  });

  it("skip this version sends no parts: the provenance still moves (C-1112)", async () => {
    const bodies: unknown[] = [];
    serve({
      "POST /v1/agents/a1/template/apply": async (req) => {
        bodies.push(JSON.parse(await req.clone().text()));
        return json(200, applied);
      },
    });
    mountDialog();
    await screen.findByRole("dialog", { name: "Review changes" });
    fireEvent.click(screen.getByRole("button", { name: "Skip this version" }));
    await waitFor(() => expect(bodies).toHaveLength(1));
    expect(bodies[0]).toEqual({ version: "1.1.0", content_sha256: sha, parts: [] });
  });

  it("a 409 template_version_changed closes the dialog and the section says the template was updated (C-1112)", async () => {
    // The section level: the offer stands, the apply moves under the person's feet.
    mockFetch({
      "GET /v1/agents/a1/template": () =>
        json(200, {
          template: { template_id: "schedule-manager", name: "Schedule manager", version: "1.0.0" },
          available: true,
          notes: [],
          update: offer,
        }),
      "GET /v1/agents/a1/helpers": () => json(200, { items: [] }),
      "POST /v1/agents/a1/template/apply": () => json(409, { error: { code: "template_version_changed", message: "The template moved on." } }),
    });
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <AgentSectionWithUpdate />
      </QueryClientProvider>,
    );
    await screen.findByTestId("template-update");
    fireEvent.click(screen.getByRole("button", { name: "Review changes" }));
    await screen.findByRole("dialog", { name: "Review changes" });
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    // The dialog closes, the offer is read again, and the section says what happened.
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Review changes" })).not.toBeInTheDocument());
    expect(screen.getByTestId("template-stale")).toHaveTextContent("This template was updated. Check the changes, then apply.");
  });
});

/** The section around the dialog, for the stale flow: an agent with an update offer. */
function AgentSectionWithUpdate() {
  return (
    <AgentTemplateSection
      agent={{ id: "a1", name: "Daneel", handle: "daneel", template: { template_id: "schedule-manager", name: "Schedule manager", version: "1.0.0" } }}
      agentBase="/o/o1/w/w1/agents/daneel"
    />
  );
}
