import { screen } from "@testing-library/react";
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

  it("an ended grant: no emergency line, the thread still read-only in emergency mode", async () => {
    wrap(thread({ emergency: true, audience: audience({ ...ACTIVE_GRANT, active: false, ended_at: "2026-11-20T16:00:00Z" }) }));
    const meta = await screen.findByTestId("thread-meta");
    expect(meta).not.toHaveTextContent("emergency access");
    expect(screen.getByTestId("emergency-thread-notice")).toBeInTheDocument();
  });
});
