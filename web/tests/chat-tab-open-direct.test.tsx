import { screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ChatTab } from "@/components/agents/chat-tab";
import { json } from "./support/api";
import { wrap } from "./support/render";

// P5-REFINE-13 (Try it in chat on a phone): on a phone the Chat page is two views, the list and a thread. A link that means
// "open the chat with this agent" carries `open=direct` and lands on the direct conversation's thread, with its box; a plain
// link to the Chat tab (and a draft alone) lands on the list.

let search = "";
vi.mock("next/navigation", () => ({ useRouter: () => ({ replace: vi.fn() }), useSearchParams: () => new URLSearchParams(search) }));
vi.mock("@/lib/chat/use-transcript", () => ({
  useTranscript: () => ({ state: { events: [], deltas: {}, activity: {}, lastSeq: 0 }, loading: false, error: null, live: true }),
}));

const direct = {
  id: "mine", agent_id: "a1", kind: "direct", external_ref: "mine", title: null, direct_user_id: "me", direct_user: null, origin_conversation_id: null,
  last_seq: 0, app_turn_streak: 0, app_turn_capped: false, active_turn: null, last_activity_at: null, archived_at: null, created_at: "2026-01-01T00:00:00Z",
};

beforeEach(() => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const path = new URL(req.url, "http://localhost").pathname;
      if (req.method === "POST" && path === "/v1/agents/a1/conversations/direct") return json(200, direct);
      if (req.method === "GET" && path === "/v1/agents/a1/conversations") return json(200, { items: [], next_cursor: null });
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
});

/** The two views of the chat panel: the wrapper of the thread is `flex` when it is the one shown on a phone, `hidden` when not. */
async function threadShown(): Promise<boolean> {
  const box = await screen.findByLabelText("Message", { exact: true });
  let el: HTMLElement | null = box;
  while (el && !/\bmin-w-0\b.*\bflex-1\b/.test(el.className)) el = el.parentElement;
  return !!el && /(^|\s)flex(\s|$)/.test(el.className) && !/(^|\s)hidden(\s|$)/.test(el.className);
}

describe("the Chat page on a phone", () => {
  it("open=direct opens the direct conversation's thread", async () => {
    search = "tab=chat&open=direct";
    wrap(<ChatTab agentId="a1" wsId="w" base="/o/x/w/y" />);
    expect(await threadShown()).toBe(true);
  });

  it("open=direct with a draft opens the thread and fills the box", async () => {
    search = "tab=chat&open=direct&draft=Which%20pull%20requests%3F";
    wrap(<ChatTab agentId="a1" wsId="w" base="/o/x/w/y" />);
    expect(await threadShown()).toBe(true);
    expect(await screen.findByDisplayValue("Which pull requests?")).toBeTruthy();
  });

  it("a plain link to the Chat tab, and a draft alone, land on the list", async () => {
    search = "tab=chat&draft=hi";
    wrap(<ChatTab agentId="a1" wsId="w" base="/o/x/w/y" />);
    expect(await threadShown()).toBe(false);
  });
});
