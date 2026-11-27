import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ReviewUpdates } from "@/components/agents/review-updates";
import { json, mockFetch } from "./support/api";

// (FIX-251, D-488(a)) **Review updates**, the Agents page's walkthrough: each agent that needs an
// update, one review at a time; an agent whose offer is gone by the time it is read is skipped;
// closing a review moves to the next; the last close ends the walkthrough.

const sha = "c".repeat(64);
const mergeSha = "d".repeat(64);

const offer = {
  version: "1.2.0",
  content_sha256: sha,
  readme: "",
  changelog: null,
  inputs_needed: [],
  apps_added: [],
  parts: [
    {
      kind: "instructions",
      key: "",
      state: "merged",
      current: "Line one.",
      proposed: "Line one changed.",
      skill_ref: null,
      merge: { state: "merged", merge_sha256: mergeSha, result: "Line one changed.", conflicts: [] },
    },
  ],
};

const statusOf = (update: unknown) => ({
  template: { template_id: "schedule-manager", name: "Schedule manager", version: "1.0.0" },
  available: true,
  notes: [],
  update,
});

const applied = { template: { template_id: "schedule-manager", name: "Schedule manager", version: "1.2.0" }, available: true, notes: [] };

function mount(onClose: () => void) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <ReviewUpdates
        agents={[
          { id: "a1", name: "Daneel", handle: "daneel" },
          { id: "a2", name: "Kojo", handle: "kojo" },
        ]}
        agentBaseOf={(handle) => `/o/o1/w/w1/agents/${handle}`}
        onClose={onClose}
      />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  // jsdom has no `matchMedia`; the dialog's `fixedHeight` asks whether the viewport is wide.
  vi.stubGlobal("matchMedia", (query: string) => ({ matches: true, media: query, addEventListener: () => {}, removeEventListener: () => {} }));
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("the Review updates walkthrough (FIX-251)", () => {
  it("skips an agent whose offer is gone, reviews the next one, and ends after it closes", async () => {
    const bodies: unknown[] = [];
    const onClose = vi.fn();
    mockFetch({
      // Daneel's offer is gone (applied elsewhere): the step is skipped, never shown.
      "GET /v1/agents/a1/template": () => json(200, statusOf(null)),
      "GET /v1/agents/a2/template": () => json(200, statusOf(offer)),
      "POST /v1/agents/a2/template/apply": async (req) => {
        bodies.push(JSON.parse(await req.clone().text()));
        return json(200, applied);
      },
    });
    mount(onClose);
    // Kojo's review opens (Daneel's never does), bound to Kojo's apply route.
    await screen.findByRole("dialog", { name: "Review changes" });
    expect(screen.getByTestId("memories-note")).toHaveTextContent("Memories are never changed by an update.");
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(bodies).toEqual([{ version: "1.2.0", content_sha256: sha, parts: [{ kind: "instructions", key: "", merge_sha256: mergeSha }], skill_refs: [], inputs: {} }]));
    // The last close ends the walkthrough.
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it("ends the walkthrough when the person stops it while an offer is being read", async () => {
    const onClose = vi.fn();
    mockFetch({
      "GET /v1/agents/a1/template": () => json(200, statusOf(offer)),
      "GET /v1/agents/a2/template": () => json(200, statusOf(offer)),
    });
    mount(onClose);
    await screen.findByRole("dialog", { name: "Review changes" });
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    // Closing the first review moves to the next agent's, which is being read.
    await screen.findByTestId("review-updates-reading");
    fireEvent.click(screen.getByRole("button", { name: "Stop for now" }));
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });
});
