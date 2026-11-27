import { fireEvent, screen, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ChatThread } from "@/components/chat/chat-thread";
import type { WorkspaceAudience } from "@/lib/chat/audience";
import { setDisplayTimeZone } from "@/lib/format-date";
import { wrap } from "./support/render";
import { json, mockFetch } from "./support/api";

// (FIX-236, C-1165) Emergency read access in the thread: the reader sees the conversation and the
// visibility line, and every action is gone — no composer, no Stop, no "…" menu — against the owner
// view of the same conversation, which keeps the "…" menu and the Stop.

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
}));
vi.mock("next/link", () => ({
  default: ({ href, children, ...rest }: { href: string; children: ReactNode }) => (
    <a href={href} {...rest}>
      {children}
    </a>
  ),
}));

const audience = (emergency: WorkspaceAudience["emergency"]) => ({
  people: 2,
  owners: [{ id: "u1", display_name: "Ada Lovelace" }],
  owners_total: 1,
  emergency_active_more: 0,
  emergency,
});

const ACTIVE_GRANT = {
  id: "e1",
  by: { id: "x", display_name: "Ada Lovelace" },
  reason: "Legal hold",
  opened_at: "2026-11-20T14:00:00Z",
  expires_at: "2026-11-20T18:00:00Z",
  ended_at: null,
  active: true,
};

function serve() {
  mockFetch({
    "GET /v1/conversations/c1/events": () => json(200, { items: [], last_seq: 0 }),
    "GET /v1/agents/a1/answer-readiness": () => json(200, { can_answer: true }),
  });
}

const MESSAGES = [
  { seq: 1, type: "turn.queued", ts: "2026-11-20T10:00:00Z", payload: { turn_id: "t1", speaker_label: "Bob Builder", door: "dashboard", text: "Shall we ship it?", attachments: [] } },
  { seq: 2, type: "text", ts: "2026-11-20T10:01:00Z", payload: { turn_id: "t1", message_id: "m2", text: "Yes, today." } },
];

function serveMessages() {
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: vi.fn(async () => undefined) } });
  mockFetch({
    "GET /v1/conversations/c1/events": () => json(200, { items: MESSAGES, last_seq: 2 }),
    "GET /v1/agents/a1/answer-readiness": () => json(200, { can_answer: true }),
  });
}

beforeEach(() => {
  vi.unstubAllGlobals();
  // The visibility line's "until 18:00" pins the clock; the zone is fixed for the run.
  setDisplayTimeZone("UTC");
  serve();
});

afterEach(() => setDisplayTimeZone(null));

function thread(props: Record<string, unknown> = {}) {
  return (
    <ChatThread
      agentId="a1"
      wsId="w1"
      base="/o/acme/w/main"
      convId="c1"
      direct={null}
      audience={audience(null)}
      myRole={null}
      readerId={null}
      onGone={() => undefined}
      {...props}
    />
  );
}

describe("emergency read access in the thread (FIX-236)", () => {
  it("the reader reads a direct chat: read-only notice, the emergency line, and no actions at all", async () => {
    wrap(thread({ emergency: true, audience: audience(ACTIVE_GRANT) }));
    const notice = await screen.findByTestId("emergency-thread-notice");
    expect(notice).toHaveTextContent("Read-only. You hold emergency read access: everything you open is recorded, and you can read, not act.");
    // The line: who can read the chat, with the emergency sentence appended.
    const meta = await screen.findByTestId("thread-meta");
    expect(meta).toHaveTextContent(
      "Ada Lovelace, an organization owner, can read this workspace's conversations until 18:00 (emergency access).",
    );
    // A list assertion over the actions: none of them exist.
    expect(screen.queryByRole("button", { name: "Conversation actions" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
  });

  it("the owner view of the same conversation keeps its actions (for contrast)", async () => {
    wrap(thread({ ownerView: "Bob Builder" }));
    expect(await screen.findByText(/Read-only view\. This is Bob Builder/)).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "Conversation actions" })).toBeInTheDocument();
    expect(screen.queryByTestId("emergency-thread-notice")).toBeNull();
  });

  // (FIX-280) A conversation without a composer offers no Reply, for anyone; Copy and Copy link stay.
  it.each([
    ["the reader", { emergency: true }],
    ["a co-owner reading another member's direct chat", { ownerView: "Bob Builder" }],
    ["anyone in an archived conversation", { archived: true }],
  ])("%s gets no Reply, and keeps Copy and Copy link", async (_who, props) => {
    serveMessages();
    wrap(thread(props));
    const message = (await screen.findByText("Shall we ship it?")).closest("[data-seq]") as HTMLElement;
    expect(screen.queryByRole("button", { name: "Reply" })).toBeNull();
    expect(within(message).getByRole("button", { name: "Copy message" })).toBeInTheDocument();
    // Radix opens a dropdown on keydown Enter (jsdom has no pointer capture).
    fireEvent.keyDown(within(message).getByRole("button", { name: "More message actions" }), { key: "Enter" });
    expect(await screen.findByRole("menuitem", { name: "Copy link" })).toBeInTheDocument();
  });

  it("a conversation with a composer offers Reply, and it opens the reply bar", async () => {
    serveMessages();
    wrap(thread({ ownDirect: true }));
    await screen.findByLabelText("Message");
    const replies = await screen.findAllByRole("button", { name: "Reply" });
    expect(replies).toHaveLength(2);
    fireEvent.click(replies[0]!);
    expect(await screen.findByRole("group", { name: "Replying to" })).toBeInTheDocument();
  });

  it("an ended grant: no emergency line, the thread still read-only in emergency mode", async () => {
    wrap(thread({ emergency: true, audience: audience({ ...ACTIVE_GRANT, active: false, ended_at: "2026-11-20T16:00:00Z" }) }));
    const meta = await screen.findByTestId("thread-meta");
    expect(meta).not.toHaveTextContent("emergency access");
    expect(screen.getByTestId("emergency-thread-notice")).toBeInTheDocument();
  });
});
