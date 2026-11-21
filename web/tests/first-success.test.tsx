import { fireEvent, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { FirstSuccessCards } from "@/components/chat/first-success";
import { pendingFirstSuccesses } from "@/lib/chat/first-success";
import { json } from "./support/api";
import { wrap } from "./support/render";

// P5-REFINE-11 item 9 (rec 10): four first-success cards, each shown once and dismissed per person on the server.

const ev = (seq: number, type: string, payload: Record<string, unknown>, ts?: string) => ({
  seq,
  type,
  ts: ts ?? new Date(Date.now() - 5 * 60_000).toISOString(),
  payload,
});
const answered = () => [
  ev(1, "turn.queued", { turn_id: "t1", text: "hi" }),
  ev(2, "text", { turn_id: "t1", text: "hello" }),
  ev(3, "turn.finished", { turn_id: "t1", status: "done" }),
];

describe("pendingFirstSuccesses", () => {
  it("first answer: a finished turn with the agent's words; counts the tools it used", () => {
    expect(pendingFirstSuccesses(answered() as never, [], null, new Date())).toEqual([{ key: "first-answer", tools: 0 }]);
    const withTool = [...answered(), ev(4, "tool.started", { turn_id: "t1" })];
    expect(pendingFirstSuccesses(withTool as never, [], null, new Date())).toEqual([{ key: "first-answer", tools: 1 }]);
  });
  it("is not shown again once dismissed, and not for old history", () => {
    expect(pendingFirstSuccesses(answered() as never, ["first-answer"], null, new Date())).toEqual([]);
    const old = answered().map((e) => ({ ...e, ts: "2020-10-01T10:00:00Z" }));
    expect(pendingFirstSuccesses(old as never, [], null, new Date())).toEqual([]);
  });
  it("first app, first approval and first run each have their own key", () => {
    const events = [
      ev(1, "notice", { code: "app_access_given", app_id: "gcal", app_name: "My calendar" }),
      ev(2, "approval.decided", { decision: "approved" }),
      ev(3, "turn.finished", { turn_id: "t9", status: "done" }),
    ];
    const keys = pendingFirstSuccesses(events as never, [], { kind: "schedule", title: "Morning summary" }, new Date()).map((c) => c.key);
    expect(keys).toEqual(["first-app", "first-approval", "first-run"]);
  });
});

function renderCards(initialSeen: string[], over: Record<string, unknown> = {}) {
  let seen = initialSeen;
  const patches: unknown[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const path = new URL(req.url, "http://localhost").pathname;
      if (path === "/v1/me/onboarding" && req.method === "PATCH") {
        const body = (await req.clone().json()) as { notice_seen: string };
        patches.push(body);
        seen = [...seen, body.notice_seen];
        return json(200, { notices_seen: seen });
      }
      if (path === "/v1/me/onboarding") return json(200, { notices_seen: seen });
      if (path.startsWith("/v1/apps/")) return json(200, { first_prompt: "What's on my calendar this week?" });
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
  const props = { events: answered() as never, conversation: null, agentName: "Daneel", wsId: "w", ...over };
  // `moreControls: undefined`: leave the onboarding query to the mock above (it carries `notices_seen`).
  wrap(<FirstSuccessCards {...props} />, { moreControls: undefined });
  return patches;
}

describe("FirstSuccessCards", () => {
  it("shows the first-answer card with its words and remembers Got it on the server", async () => {
    const patches = renderCards([]);
    expect(await screen.findByText("Daneel answered. It thought with the Company key and changed nothing outside this chat.")).toBeInTheDocument();
    expect(screen.queryByText("Live")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Got it" }));
    await waitFor(() => expect(patches).toEqual([{ notice_seen: "first-answer" }]));
    await waitFor(() => expect(screen.queryByTestId("first-success-first-answer")).toBeNull());
  });
  it("shows nothing for a person who already dismissed it", async () => {
    renderCards(["first-answer"]);
    await new Promise((r) => setTimeout(r, 100));
    expect(screen.queryByTestId("first-successes")).toBeNull();
  });
  it("the app card offers the app's first prompt and puts it in the message box, not sent", async () => {
    const onTry = vi.fn();
    renderCards([], { events: [ev(1, "notice", { code: "app_access_given", app_id: "gcal", app_name: "My calendar" })], onTry });
    expect(await screen.findByText(/Daneel can now read My calendar\./)).toBeInTheDocument();
    fireEvent.click(await screen.findByRole("button", { name: "Put it in the message box" }));
    expect(onTry).toHaveBeenCalledWith("What's on my calendar this week?");
  });
});
