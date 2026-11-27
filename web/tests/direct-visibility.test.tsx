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
    wrap(<ChatTab agentId="a1" wsId="w" base="/o/x/w/y" audience={{ people: 3, owners: [{ id: "u9", display_name: "Ann" }], owners_total: 1, emergency_active_more: 0 }} myRole="member" readerId="me" />);
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
        audience={{ people: 2, owners: [{ id: "me", display_name: "Me" }], owners_total: 1, emergency_active_more: 0 }}
        myRole="owner"
        readerId="me"
      />,
    );
    expect((await screen.findAllByText(/All 2 people in this workspace can use this agent and read this conversation\./)).length).toBeGreaterThan(0);
  });

  it("(FIX-263) an emergency reader of a group conversation is told that all its people read it", async () => {
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
        emergency
        audience={{ people: 4, owners: [{ id: "u9", display_name: "Ann" }], owners_total: 1, emergency_active_more: 0 }}
        myRole={null}
        readerId="x"
      />,
    );
    expect((await screen.findAllByText(/All 4 people in this workspace can use this agent and read this conversation\./)).length).toBeGreaterThan(0);
    expect(screen.queryByText(/can see this conversation/)).toBeNull();
  });

  // (FIX-263) The emergency reader is not one of the people, so "Only you" never shows to them.
  it.each([
    [1, "One person in this workspace can use this agent and read this conversation."],
    [0, "Nobody in this workspace can use this agent right now."],
  ])("(FIX-263) an emergency reader of a group conversation in a workspace of %i", async (people, line) => {
    const g = conv("g1", "group", { title: "Team room" });
    mock([g]);
    const emergency = {
      id: "e1", by: { id: "x", display_name: "Ada Lovelace" }, reason: "Legal hold",
      opened_at: "2026-11-20T14:00:00Z", expires_at: "2026-11-20T18:00:00Z", ended_at: null, active: true,
    };
    wrap(
      <ChatPane
        agentId="a1"
        wsId="w"
        base="/o/x/w/y"
        convId="g1"
        direct={null}
        conversation={g}
        emergency
        audience={{ people, owners: [], owners_total: 0, emergency, emergency_active_more: 0 }}
        myRole={null}
        readerId="x"
      />,
    );
    // The line, followed by the emergency line, in the header's meta and under the greeting.
    const both = (text: string) => text.includes(`${line} Ada Lovelace, an organization owner, can read this workspace's conversations until`);
    expect((await screen.findAllByText((text) => both(text))).length).toBeGreaterThan(0);
    expect(screen.queryByText(/Only you can use this agent/)).toBeNull();
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
