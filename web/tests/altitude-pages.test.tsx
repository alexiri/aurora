import { cleanup, fireEvent, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ActivityView } from "@/components/approvals/activity-view";
import { wrap } from "./support/render";
import { json, mockFetch } from "./support/api";

// The altitude lint's rendering half (D-406, brief P5-REFINE-3 Scope 5c): with "Show more controls" off, the Expert strings
// of a page that exists today never render; with it on they do.

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const entry = {
  id: "e1", ts: "2026-11-10T10:00:00Z", agent_id: "a1", method: "GET", host: "api.example.com", path: "/x", decision: "default_denied",
  rule_name: null, app_id: null, tool_id: null, status: 403, llm_model: null, context_label: null,
};

function routes() {
  mockFetch({
    "GET /v1/workspaces/w1/activity": () => json(200, { items: [entry], next_cursor: null }),
    "GET /v1/workspaces/w1/agents": () => json(200, { items: [{ id: "a1", name: "Scout", handle: "scout" }], next_cursor: null }),
  });
}

describe("Activity at Use", () => {
  it("shows a collapsed line instead of the request log, and no Expert strings", async () => {
    routes();
    wrap(<ActivityView wsId="w1" base="/o/x/w/y" />, { moreControls: false });
    expect(await screen.findByText(/Technical view: every request/)).toBeInTheDocument();
    expect(screen.queryByRole("table", { name: "Request activity" })).toBeNull();
    expect(screen.getByText(/1 more control hidden/)).toBeInTheDocument();
    // Show on the footer turns the switch on; the table then renders in place.
    fireEvent.click(screen.getAllByRole("button", { name: "Show" })[0]!);
    expect(await screen.findByRole("table", { name: "Request activity" })).toBeInTheDocument();
  });

  it("(workspace-2) the technical table fits its card: five columns, the rule under the decision, the app under the request, no forced width", async () => {
    mockFetch({
      "GET /v1/workspaces/w1/activity": () => json(200, { items: [{ ...entry, decision: "allow", rule_name: "Block evil.example.org", app_id: "github", tool_id: "create_issue" }], next_cursor: null }),
      "GET /v1/workspaces/w1/agents": () => json(200, { items: [{ id: "a1", name: "Scout", handle: "scout" }], next_cursor: null }),
    });
    wrap(<ActivityView wsId="w1" base="/o/x/w/y" />, { moreControls: true });
    const table = await screen.findByRole("table", { name: "Request activity" });
    expect(screen.getAllByRole("columnheader").map((h) => h.textContent)).toEqual(["Request", "When", "Agent", "Model", "Decision"]);
    expect(table.className).not.toContain("min-w-max");
    expect(screen.getByTestId("rule-name").textContent).toBe("Rule: Block evil.example.org");
    expect(screen.getByTestId("app-tool").textContent).toBe("App: github / create_issue");
  });

  it("shows the request log with Show more controls on", async () => {
    routes();
    wrap(<ActivityView wsId="w1" base="/o/x/w/y" />, { moreControls: true });
    expect(await screen.findByRole("table", { name: "Request activity" })).toBeInTheDocument();
    expect(screen.queryByText(/Technical view: every request/)).toBeNull();
  });
});
