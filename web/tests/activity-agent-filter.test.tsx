import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ActivityView } from "@/components/approvals/activity-view";

// P5-AGENT-2: the agent page's "All activity" link is `…/activity?agent=<id>`; the page starts filtered and keeps the URL in step.

const json = (body: unknown) => new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } });
function mock() {
  const activityQueries: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input as Request;
      const url = new URL(req.url, "http://localhost");
      if (url.pathname === "/v1/workspaces/w1/activity") {
        activityQueries.push(url.search);
        return json({ items: [], next_cursor: null });
      }
      if (url.pathname === "/v1/workspaces/w1/agents") {
        return json({ items: [{ id: "a1", name: "Scout", handle: "scout" }, { id: "a2", name: "Ada", handle: "ada" }], next_cursor: null });
      }
      return json({ items: [] });
    }),
  );
  return activityQueries;
}
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  window.history.replaceState(null, "", "/");
});

function renderView() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <ActivityView wsId="w1" base="/o/x/w/y" />
    </QueryClientProvider>,
  );
}

describe("Activity page agent filter", () => {
  it("starts filtered by ?agent= and writes a changed filter back to the URL", async () => {
    window.history.replaceState(null, "", "/o/x/w/y/activity?agent=a1");
    const queries = mock();
    renderView();
    await waitFor(() => expect(queries.some((q) => q.includes("agent_id=a1"))).toBe(true));
    const select = (await screen.findByLabelText("Agent")) as HTMLSelectElement;
    await waitFor(() => expect(select.value).toBe("a1"));
    fireEvent.change(select, { target: { value: "a2" } });
    expect(window.location.search).toBe("?agent=a2");
    fireEvent.change(select, { target: { value: "" } });
    expect(window.location.search).toBe("");
  });

  it("without the parameter shows every agent", async () => {
    const queries = mock();
    renderView();
    await waitFor(() => expect(queries.length).toBeGreaterThan(0));
    expect(queries.every((q) => !q.includes("agent_id"))).toBe(true);
  });
});
