import { fireEvent, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ChatPane, ChatTab } from "@/components/agents/chat-tab";
import { wrap } from "./support/render";
import { json } from "./support/api";

vi.mock("@/lib/chat/use-transcript", () => ({
  useTranscript: () => ({ state: { events: [], deltas: {}, activity: {}, lastSeq: 0 }, loading: false, error: null, live: true }),
}));

const conv = (id: string, kind: string, over: Record<string, unknown> = {}) => ({
  id, agent_id: "a1", kind, external_ref: id, title: null, title_source: "auto", direct_user_id: null, direct_user: null, origin_conversation_id: null,
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

describe("direct conversation visibility (RFE-009, FIX-235 D-479)", () => {
  it("shows the visibility line on the member's own direct chat and no owner group", async () => {
    mock([conv("mine", "direct", { direct_user_id: "me" }), conv("g1", "group", { title: "Team room" })]);
    wrap(<ChatTab agentId="a1" wsId="w" base="/o/x/w/y" audience={{ people: 3, owners: [{ id: "u9", display_name: "Ann" }], owners_total: 1 }} myRole="member" readerId="me" />);
    // The header's meta line and the empty state under the greeting both carry it.
    expect((await screen.findAllByText(/All 3 people in this workspace can use this agent\. This chat is visible to you and the workspace's owner, Ann\./)).length).toBeGreaterThan(0);
    expect(await screen.findByText("Team room")).toBeTruthy();
    expect(screen.queryByText("Other members' direct chats")).toBeNull();
  });

  it("shows the line on a non-direct conversation too", async () => {
    const g = conv("g1", "group", { title: "Team room" });
    mock([g]);
    wrap(
      <ChatPane
        agentId="a1"
        wsId="w"
        base="/o/x/w/y"
        convId="g1"
        direct={null}
        conversation={g}
        audience={{ people: 2, owners: [{ id: "me", display_name: "Me" }], owners_total: 1 }}
        myRole="owner"
        readerId="me"
      />,
    );
    expect((await screen.findAllByText(/All 2 people in this workspace can use this agent and read this conversation\./)).length).toBeGreaterThan(0);
  });

  it("gives owners a read-only group for other members' direct conversations", async () => {
    mock([
      conv("mine", "direct", { direct_user_id: "me" }),
      conv("theirs", "direct", { direct_user_id: "u2", direct_user: { id: "u2", display_name: "Ann" } }),
    ]);
    wrap(<ChatTab agentId="a1" wsId="w" base="/o/x/w/y" />);
    expect(await screen.findByText("Other members' direct chats")).toBeTruthy();
    expect(screen.getByText("Read-only")).toBeTruthy();
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
