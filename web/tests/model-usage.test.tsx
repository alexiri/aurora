import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ActivityView } from "@/components/approvals/activity-view";
import { seedMoreControls } from "./support/render";

// P5-MODELS-3 (C-879, C-880): the request log's model column, and a turn's tokens per model.

const json = (body: unknown) => new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
const entry = (id: string, over: Record<string, unknown>) => ({
  id, ts: "2026-11-17T10:00:00Z", agent_id: "a1", method: "POST", host: "llm.example.org", path: "/v1/messages",
  decision: "allow", status: 200, lane: "mitm", credential_kind: "agent", injected: 1, latency_ms: 20, resumed: false, ...over,
});

describe("request log model column", () => {
  it("shows the model of an LLM request and a dash for any other", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: Request | string | URL) => {
        const url = new URL((input as Request).url, "http://localhost");
        if (url.pathname === "/v1/workspaces/w1/activity") {
          return json({ items: [entry("e1", { llm_model: "glm-4.6" }), entry("e2", { host: "api.github.com", path: "/user", llm_model: null })], next_cursor: null });
        }
        return json({ items: [], next_cursor: null });
      }),
    );
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    // The request log is Expert (D-406, `log.requests`): the switch is on.
    seedMoreControls(qc);
    render(
      <QueryClientProvider client={qc}>
        <ActivityView wsId="w1" base="/o/x/w/y" />
      </QueryClientProvider>,
    );
    const table = await screen.findByRole("table", { name: "Request activity" });
    expect(within(table).getByRole("columnheader", { name: "Model" })).toBeTruthy();
    const cells = within(table).getAllByTestId("llm-model").map((c) => c.textContent);
    expect(cells).toEqual(["glm-4.6", "—"]);
  });
});
