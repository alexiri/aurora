import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ActivityView } from "@/components/approvals/activity-view";
import { TurnUsage, UNVERIFIED_TEXT } from "@/components/chat/turn-usage";

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

describe("TurnUsage", () => {
  it("lists tokens per model, subagents included", () => {
    render(
      <TurnUsage
        usage={{
          input_tokens: 1500, output_tokens: 300, cache_read_tokens: 40, cache_write_tokens: 5,
          by_model: [
            { model: "claude-sonnet", input_tokens: 1000, output_tokens: 200, cache_read_tokens: 40, cache_write_tokens: 5 },
            { model: "claude-haiku", input_tokens: 500, output_tokens: 100, cache_read_tokens: 0, cache_write_tokens: 0 },
          ],
        }}
      />,
    );
    const rows = within(screen.getByRole("table", { name: "Tokens per model" })).getAllByRole("row");
    expect(rows).toHaveLength(3);
    expect(rows[1]!.textContent).toContain("claude-sonnet1,000200405");
    expect(rows[2]!.textContent).toContain("claude-haiku500100");
    expect(screen.queryByTestId("unverified-models")).toBeNull();
  });

  it("counts a turn without by_model under its model, and marks an unverified turn", () => {
    render(<TurnUsage usage={{ model: "claude-sonnet", input_tokens: 7, output_tokens: 3, unverified_models: true }} />);
    const rows = within(screen.getByRole("table", { name: "Tokens per model" })).getAllByRole("row");
    expect(rows).toHaveLength(2);
    expect(rows[1]!.textContent).toContain("claude-sonnet730");
    expect(screen.getByTestId("unverified-models")).toHaveTextContent(UNVERIFIED_TEXT);
  });
});
