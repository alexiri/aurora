import { cleanup, fireEvent, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ActivityView } from "@/components/approvals/activity-view";
import { activityKind, activityOutcome, activitySentence } from "@/lib/labels";
import { wrap } from "./support/render";
import { json, mockFetch } from "./support/api";

// P5-REFINE-16 workspace-2 (part A): sentences a person reads, filters in words, the raw request under Details.

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

const base = { id: "x", ts: "2026-11-10T10:00:00Z", agent_id: "a1", status: 200, rule_name: null, app_id: null, tool_id: null, llm_model: null, context_label: null, approval_id: null };
const answer = { ...base, id: "e1", method: "POST", host: "litellm.svc.cluster.local", path: "/v1/messages", decision: "allow", llm_model: "Workspace default" };
const blocked = { ...base, id: "e2", method: "POST", host: "api.github.com", path: "/repos", decision: "blocked", status: 403, rule_name: "Block evil.example.org", app_id: "github" };
const asked = { ...base, id: "e3", method: "POST", host: "docs.example.org", path: "/x", decision: "approval_pending", approval_id: "ap1" };

function routes() {
  mockFetch({
    "GET /v1/workspaces/w1/activity": () => json(200, { items: [answer, blocked, asked], next_cursor: null }),
    "GET /v1/workspaces/w1/agents": () => json(200, { items: [{ id: "a1", name: "Dani", handle: "dani" }], next_cursor: null }),
    "GET /v1/apps": () => json(200, { items: [{ app_id: "github", name: "GitHub" }] }),
  });
}

describe("activity sentence templates", () => {
  it("classifies a model call as an answer, not a tool call", () => {
    expect(activityKind(answer)).toBe("answer");
    expect(activityOutcome(answer)).toBe("answered");
    expect(activitySentence(answer, "Dani")).toEqual({ title: "Dani answered you", meta: "Used Workspace default" });
  });
  it("names the rule that blocked a call to an app", () => {
    expect(activitySentence(blocked, "Dani", "GitHub")).toEqual({ title: "Dani called GitHub", meta: "Blocked by your rule ‘Block evil.example.org’" });
  });
  it("an approval row says it is waiting", () => {
    expect(activityKind(asked)).toBe("approval");
    expect(activitySentence(asked, "Dani")).toEqual({ title: "Dani asked before changing docs.example.org", meta: "Waiting for you in Approvals" });
  });
});

describe("ActivityView sentences", () => {
  it("shows sentence rows with no raw request, filters in words, and the raw request under Details", async () => {
    routes();
    wrap(<ActivityView wsId="w1" base="/o/x/w/y" />, { moreControls: false });
    const list = await screen.findByTestId("activity-sentences");
    expect(within(list).getByText("Dani answered you")).toBeInTheDocument();
    expect(within(list).getByText("Dani called GitHub")).toBeInTheDocument();
    expect(list).not.toHaveTextContent("litellm");
    expect(screen.getByRole("option", { name: "Any outcome" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Waited for you" })).toBeInTheDocument();
    expect(screen.queryByText(/blocked_by_policy|default_denied/)).toBeNull();
    fireEvent.click(within(list).getAllByRole("button", { name: "Details" })[0]!);
    expect(screen.getByTestId("activity-raw")).toHaveTextContent("POST litellm.svc.cluster.local/v1/messages · allow · 200");
  });

  it("the outcome filter narrows the list", async () => {
    routes();
    wrap(<ActivityView wsId="w1" base="/o/x/w/y" />, { moreControls: false });
    const list = await screen.findByTestId("activity-sentences");
    fireEvent.change(screen.getByLabelText("Outcome"), { target: { value: "blocked" } });
    expect(within(list).queryByText("Dani answered you")).toBeNull();
    expect(within(list).getByText("Dani called GitHub")).toBeInTheDocument();
  });
});
