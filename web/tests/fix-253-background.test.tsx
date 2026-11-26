import { fireEvent, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { ChatPane } from "@/components/agents/chat-tab";
import { resetStopped } from "@/lib/chat/stopped";
import { json } from "./support/api";
import { wrap } from "./support/render";

// (FIX-253, D-490) The **Background activity** notice: what the harness child keeps doing after
// the turn ended, with Stop; the durable `background_stopped` notice mutes the row.

const st = vi.hoisted(() => ({ events: [] as unknown[] }));
vi.mock("@/lib/chat/use-transcript", () => ({
  useTranscript: () => ({
    state: { events: st.events, deltas: {}, activity: {}, purged: {}, lastSeq: st.events.length },
    loading: false,
    error: null,
    live: true,
  }),
}));

interface Call {
  method: string;
  path: string;
}
function mock(routes: Record<string, (c: Call) => Response> = {}) {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const url = new URL(req.url, "http://localhost");
      const call = { method: req.method, path: url.pathname };
      calls.push(call);
      const h = routes[`${req.method} ${url.pathname}`];
      return h ? h(call) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
  return calls;
}

const e = (seq: number, type: string, payload: Record<string, unknown>) => ({ seq, type, ts: "2026-11-08T12:00:00Z", payload });
const pane = () =>
  wrap(<ChatPane agentId="a1" wsId="w" base="/o/x/w/y" convId="c1" direct={null} agent={{ id: "a1", name: "Daneel", handle: "daneel" }} />);

describe("background activity notice (FIX-253, D-490)", () => {
  it("shows the entry with a Stop button that aborts the turn", async () => {
    st.events = [
      e(1, "turn.queued", { turn_id: "t1", speaker_label: "Bob", text: "Run the suite" }),
      e(2, "turn.finished", { turn_id: "t1", status: "done" }),
      e(3, "notice", {
        code: "background_activity",
        turn_id: "t1",
        text: "Background activity, still running: `npm run e2e:full`",
        items: [{ command: "npm run e2e:full", started_at: 1700000000 }],
      }),
    ];
    const calls = mock({ "POST /v1/turns/t1/abort": () => json(202, { id: "t1", abort_requested: true }) });
    pane();
    const row = screen.getByTestId("background-activity");
    expect(within(row).getByText(/Background activity, still running/)).toBeTruthy();
    fireEvent.click(within(row).getByRole("button", { name: "Stop" }));
    await vi.waitFor(() => expect(calls.some((c) => c.method === "POST" && c.path === "/v1/turns/t1/abort")).toBe(true));
  });

  it("mutes the row once the durable background_stopped notice exists", () => {
    st.events = [
      e(1, "turn.finished", { turn_id: "t1", status: "done" }),
      e(2, "notice", {
        code: "background_activity",
        turn_id: "t1",
        text: "Background activity, still running: `npm run e2e:full`",
        items: [{ command: "npm run e2e:full", started_at: 1700000000 }],
      }),
      e(3, "notice", { code: "background_stopped", turn_id: "t1", text: "Background activity stopped." }),
    ];
    mock();
    pane();
    expect(screen.queryByTestId("background-activity")).toBeNull();
    expect(screen.getByText("Background activity stopped.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
  });

  // (FIX-253 review) While the release is in flight the entry says **Stop requested** and the
  // button is out: the durable stopped notice comes only with the supervisor's acknowledgement.
  it("shows Stop requested and disables the button until the release is acknowledged", () => {
    st.events = [
      e(1, "turn.finished", { turn_id: "t1", status: "done" }),
      e(2, "notice", {
        code: "background_activity",
        turn_id: "t1",
        text: "Background activity, still running: `npm run e2e:full`",
        items: [{ command: "npm run e2e:full", started_at: 1700000000 }],
      }),
      e(3, "notice", { code: "background_stop_requested", turn_id: "t1", text: "Stop requested." }),
    ];
    const calls = mock({ "POST /v1/turns/t1/abort": () => json(202, { id: "t1", abort_requested: true }) });
    pane();
    expect(screen.getByText("Stop requested.")).toBeTruthy();
    const row = screen.getByTestId("background-activity");
    const stop = within(row).getByRole("button", { name: "Stop" }) as HTMLButtonElement;
    expect(stop.disabled).toBe(true);
    fireEvent.click(stop);
    expect(calls.some((c) => c.method === "POST" && c.path === "/v1/turns/t1/abort")).toBe(false);
  });
});

// (FIX-253, D-490) The self-check notice with its **Remind it in 15 minutes** action: one press
// sets one one-time reminder; the durable `no_wakeup_set` notice disables it.
describe("reminder notice (FIX-253, D-490)", () => {
  const noWakeup = e(2, "notice", {
    code: "no_wakeup",
    turn_id: "t1",
    text: "Daneel said it would continue, but nothing is set to wake it.",
  });

  it("offers Remind it in 15 minutes and posts to the conversation's reminder route", async () => {
    st.events = [
      e(1, "turn.finished", { turn_id: "t1", status: "done" }),
      { ...noWakeup },
    ];
    const calls = mock({ "POST /v1/conversations/c1/reminder": () => json(200, { reminder_for: "23:50" }) });
    pane();
    const row = screen.getByTestId("no-wakeup");
    expect(within(row).getByText(/said it would continue/)).toBeTruthy();
    fireEvent.click(within(row).getByRole("button", { name: "Remind it in 15 minutes" }));
    await vi.waitFor(() =>
      expect(calls.some((c) => c.method === "POST" && c.path === "/v1/conversations/c1/reminder")).toBe(true),
    );
  });

  it("disables the button once the durable no_wakeup_set notice exists", () => {
    st.events = [
      { ...noWakeup },
      e(3, "notice", { code: "no_wakeup_set", turn_id: "t1", reminder_for: "23:50", text: "A reminder is set for 23:50." }),
    ];
    const calls = mock();
    pane();
    expect(screen.getByText("A reminder is set for 23:50.")).toBeTruthy();
    const row = screen.getByTestId("no-wakeup");
    const remind = within(row).getByRole("button", { name: "Remind it in 15 minutes" }) as HTMLButtonElement;
    expect(remind.disabled).toBe(true);
    fireEvent.click(remind);
    expect(calls.some((c) => c.path === "/v1/conversations/c1/reminder")).toBe(false);
  });

  it("a set notice of another turn leaves this notice's button alone", () => {
    st.events = [
      { ...noWakeup },
      e(3, "notice", { code: "no_wakeup_set", turn_id: "t2", reminder_for: "23:50", text: "A reminder is set for 23:50." }),
    ];
    mock();
    pane();
    const row = screen.getByTestId("no-wakeup");
    const remind = within(row).getByRole("button", { name: "Remind it in 15 minutes" }) as HTMLButtonElement;
    expect(remind.disabled).toBe(false);
  });
});
