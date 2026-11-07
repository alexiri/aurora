import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ChatPane, ChatTab } from "@/components/agents/chat-tab";

vi.mock("@/lib/chat/use-transcript", () => ({
  useTranscript: () => ({ state: { events: [], deltas: {}, activity: {}, lastSeq: 0 }, loading: false, error: null, live: true }),
}));

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const conv = (id: string, kind: string, over: Record<string, unknown> = {}) => ({
  id, agent_id: "a1", kind, external_ref: id, title: null, direct_user_id: null, direct_user: null, origin_conversation_id: null,
  last_seq: 0, app_turn_streak: 0, app_turn_capped: false, active_turn: null, last_activity_at: null,
  created_at: "2026-01-01T00:00:00Z", ...over,
});

function mock(list: unknown[]) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const path = new URL(req.url, "http://localhost").pathname;
      if (req.method === "POST" && path === "/v1/agents/a1/conversations/direct") return json(200, conv("mine", "direct", { direct_user_id: "me" }));
      if (path === "/v1/agents/a1/conversations") return json(200, { items: list, next_cursor: null });
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}
function wrap(ui: React.ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("direct conversation visibility (RFE-009)", () => {
  it("shows the privacy banner on the member's own direct chat and no owner group", async () => {
    mock([conv("mine", "direct", { direct_user_id: "me" }), conv("g1", "group", { title: "Team room" })]);
    wrap(<ChatTab agentId="a1" wsId="w" base="/o/x/w/y" />);
    expect(await screen.findByText("Only you and workspace owners can see this conversation.")).toBeTruthy();
    expect(await screen.findByText("Team room")).toBeTruthy();
    expect(screen.queryByText("Direct conversations of other members")).toBeNull();
  });

  it("gives owners a read-only group for other members' direct conversations", async () => {
    mock([
      conv("mine", "direct", { direct_user_id: "me" }),
      conv("theirs", "direct", { direct_user_id: "u2", direct_user: { id: "u2", display_name: "Ann" } }),
    ]);
    wrap(<ChatTab agentId="a1" wsId="w" base="/o/x/w/y" />);
    expect(await screen.findByText("Direct conversations of other members")).toBeTruthy();
    expect(screen.getByText("visible to workspace owners")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /Ann/ }));
    expect(await screen.findByText(/recorded in the audit log/)).toBeTruthy();
    expect(screen.queryByLabelText("Message")).toBeNull();
    expect(screen.queryByRole("button", { name: "Send" })).toBeNull();
  });

  it("keeps the composer on a normal pane", () => {
    mock([]);
    wrap(<ChatPane agentId="a1" wsId="w" base="/o/x/w/y" convId="c1" direct={null} />);
    expect(screen.getByLabelText("Message")).toBeTruthy();
  });
});
