import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ChatPane } from "@/components/agents/chat-tab";
import { UndoToastHost, dismissUndoToast } from "@/components/ui/undo-toast";
import { clearMemoryDraft, peekMemoryDraft } from "@/lib/chat/memory-draft";
import { STREAM_EVENT_TYPES } from "@/lib/chat/transcript";

import { err, json } from "./support/api";
import { wrap } from "./support/render";

// P5-MEMORY-4 (mem-5, C-1172): the chat says what the agent remembered, or why it could not. The words are the
// mockup's (docs/design/skills-memory/after/mem-5.png); who may View, Undo or save follows the workspace.

const nav = vi.hoisted(() => ({ push: vi.fn() }));
vi.mock("next/navigation", async (original) => ({
  ...(await original<typeof import("next/navigation")>()),
  useRouter: () => ({ push: nav.push, replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(),
}));

const st = vi.hoisted(() => ({ events: [] as unknown[] }));
vi.mock("@/lib/chat/use-transcript", () => ({
  useTranscript: () => ({ state: { events: st.events, deltas: {}, activity: {}, purged: {}, lastSeq: st.events.length }, loading: false, error: null, live: true }),
}));

const TS = "2026-11-08T12:00:00Z";
const e = (seq: number, type: string, payload: Record<string, unknown>, ts = TS) => ({ seq, type, ts, payload });
const savedPayload = {
  turn_id: "t1", key: "grafana-service-account", title: "Grafana service account", preview: "Needs read access to the prod folder.",
  revision: 1, created: true, previous_revision: null,
};
const changedPayload = { ...savedPayload, revision: 3, created: false, previous_revision: 2 };
const refusedPayload = {
  turn_id: "t1", key: "jira-project-keys", title: "Jira project keys", preview: "PLAT is now the key for the platform team's board.",
  code: "memory_locked", message: "“Jira project keys” is locked, so Daneel can't change it.",
};

const memory = (over: Record<string, unknown> = {}) => ({
  key: "grafana-service-account", title: "Grafana service account", description: "About Grafana", content: "Needs read access to the prod folder.",
  index_line: "", revision: 1, locked: false, locked_at: null, locked_by: null,
  created_at: "2026-11-08T11:59:59Z", updated_at: "2026-11-08T11:59:59Z",
  added_by: { kind: "agent" }, changed_by: { kind: "agent" }, last_used: null, ...over,
});

type Reader = { my_role: "owner" | "member" | null; emergency_reader?: boolean; administers?: boolean };

interface Call { method: string; path: string; body: unknown }
/** The mock API: `store.memory` is what the agent has now (`null` = no such memory). */
function mock(reader: Reader, store: { memory: ReturnType<typeof memory> | null }, over: Record<string, () => Response> = {}) {
  const calls: Call[] = [];
  const mem = "/v1/agents/a1/memories";
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input as Request;
      const url = new URL(req.url, "http://localhost");
      const text = req.method === "GET" || req.method === "DELETE" ? "" : await req.clone().text();
      const body = text ? (JSON.parse(text) as Record<string, unknown>) : undefined;
      calls.push({ method: req.method, path: url.pathname, body });
      const own = over[`${req.method} ${url.pathname}`];
      if (own) return own();
      if (url.pathname === "/v1/workspaces/w") return json(200, { id: "w", org_id: "o", emergency_reader: false, ...reader });
      const one = url.pathname.startsWith(`${mem}/`) ? url.pathname.slice(mem.length + 1) : null;
      if (req.method === "GET" && one) return store.memory && store.memory.key === one ? json(200, store.memory) : err(404, "not_found");
      if (req.method === "DELETE" && one) {
        store.memory = null;
        return json(204, null);
      }
      if (req.method === "POST" && one?.endsWith("/restore")) {
        store.memory = memory({ revision: (store.memory?.revision ?? 0) + 1 });
        return json(200, store.memory);
      }
      if (req.method === "POST" && url.pathname === mem) {
        store.memory = memory({ ...body, revision: 1, created_at: "2026-11-08T12:05:00Z" });
        return json(201, store.memory);
      }
      return err(404, "not_found");
    }),
  );
  return calls;
}

const writes = (calls: Call[]) => calls.filter((c) => c.method !== "GET").map((c) => `${c.method} ${c.path}`);

const pane = () =>
  wrap(
    <>
      <ChatPane agentId="a1" wsId="w" base="/o/x/w/y" convId="c1" direct={null} agent={{ id: "a1", name: "Daneel", handle: "daneel" }} />
      <UndoToastHost />
    </>,
  );

const card = () => screen.getByTestId("memory-card");
const button = (name: string) => within(card()).queryByRole("button", { name });
const view = () => within(card()).queryByRole("link", { name: "View" });

beforeEach(() => {
  st.events = [];
  nav.push.mockReset();
  clearMemoryDraft();
});
afterEach(() => dismissUndoToast());

describe("memory events reach the transcript", () => {
  it("the live stream listens for both", () => {
    expect(STREAM_EVENT_TYPES).toContain("memory.saved");
    expect(STREAM_EVENT_TYPES).toContain("memory.refused");
  });
});

describe("the saved card", () => {
  it("says who remembered what, and View opens that memory on the Memory page", async () => {
    st.events = [e(1, "memory.saved", savedPayload)];
    mock({ my_role: "owner" }, { memory: memory() });
    pane();
    expect(card().textContent).toContain("Daneel remembered");
    expect(card().textContent).toContain("Grafana service account");
    expect(card().textContent).toContain("Needs read access to the prod folder.");
    await waitFor(() => expect(view()).not.toBeNull());
    expect(view()!.getAttribute("href")).toBe("/o/x/w/y/agents/daneel?tab=memory&memory=grafana-service-account");
    await waitFor(() => expect(button("Undo")).not.toBeNull());
  });

  it("Undo deletes a created memory; Redo writes it again as it was", async () => {
    st.events = [e(1, "memory.saved", savedPayload)];
    const store = { memory: memory({ locked: true }) as ReturnType<typeof memory> | null };
    const calls = mock({ my_role: "member" }, store);
    pane();
    fireEvent.click(await waitFor(() => within(card()).getByRole("button", { name: "Undo" })));
    await waitFor(() => expect(card().textContent).toContain("Undone ·"));
    expect(writes(calls)).toEqual(["DELETE /v1/agents/a1/memories/grafana-service-account"]);
    expect(screen.getByTestId("undo-toast").textContent).toContain("Memory undone");
    expect(view()).toBeNull();

    fireEvent.click(within(card()).getByRole("button", { name: "Redo" }));
    await waitFor(() => expect(button("Undo")).not.toBeNull());
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({
      key: "grafana-service-account", title: "Grafana service account", description: "About Grafana",
      content: "Needs read access to the prod folder.", locked: true,
    });
    expect(screen.getByTestId("undo-toast").textContent).toContain("Memory saved again");
    expect(button("Changed since")).toBeNull();
    expect(view()).not.toBeNull();
  });

  it("Undo restores a changed memory's previous revision; Redo restores the saved one", async () => {
    st.events = [e(1, "memory.saved", changedPayload)];
    const store = { memory: memory({ revision: 3 }) as ReturnType<typeof memory> | null };
    const calls = mock({ my_role: "owner" }, store);
    pane();
    fireEvent.click(await waitFor(() => within(card()).getByRole("button", { name: "Undo" })));
    await waitFor(() => expect(button("Redo")).not.toBeNull());
    fireEvent.click(button("Redo")!);
    await waitFor(() => expect(button("Undo")).not.toBeNull());
    expect(calls.filter((c) => c.method === "POST").map((c) => [c.path, c.body])).toEqual([
      ["/v1/agents/a1/memories/grafana-service-account/restore", { revision: 2 }],
      ["/v1/agents/a1/memories/grafana-service-account/restore", { revision: 3 }],
    ]);
    // The restored text is revision 5 now, and still what this card saved: Undo works again.
    fireEvent.click(button("Undo")!);
    await waitFor(() => expect(button("Redo")).not.toBeNull());
    expect(calls.filter((c) => c.method === "POST").at(-1)?.body).toEqual({ revision: 2 });
  });

  it.each([
    ["a later revision", memory({ revision: 2 }), "This memory changed after this was saved."],
    ["a memory made again under the same key", memory({ created_at: "2026-11-08T12:30:00Z" }), "This memory changed after this was saved."],
    ["a deleted memory", null, "This memory was deleted after this was saved."],
  ])("reads Changed since, disabled, for %s", async (_name, now, reason) => {
    st.events = [e(1, "memory.saved", savedPayload)];
    const calls = mock({ my_role: "owner" }, { memory: now });
    pane();
    const changed = await waitFor(() => within(card()).getByRole("button", { name: "Changed since" }));
    expect(changed).toHaveProperty("disabled", true);
    expect(card().textContent).toContain(reason);
    expect(button("Undo")).toBeNull();
    // Nothing to open once the memory is gone.
    expect(view() === null).toBe(now === null);
    expect(writes(calls)).toEqual([]);
  });

  it("never undoes across an edit made after the card was shown", async () => {
    st.events = [e(1, "memory.saved", changedPayload)];
    const store = { memory: memory({ revision: 3 }) as ReturnType<typeof memory> | null };
    const calls = mock({ my_role: "owner" }, store);
    pane();
    const undo = await waitFor(() => within(card()).getByRole("button", { name: "Undo" }));
    store.memory = memory({ revision: 4 });
    fireEvent.click(undo);
    await waitFor(() => expect(button("Changed since")).not.toBeNull());
    expect(writes(calls)).toEqual([]);
  });

  it("does not Redo over a memory someone else wrote after the Undo", async () => {
    st.events = [e(1, "memory.saved", savedPayload)];
    const store = { memory: memory() as ReturnType<typeof memory> | null };
    const calls = mock({ my_role: "owner" }, store);
    pane();
    fireEvent.click(await waitFor(() => within(card()).getByRole("button", { name: "Undo" })));
    const redo = await waitFor(() => within(card()).getByRole("button", { name: "Redo" }));
    store.memory = memory({ content: "someone else's" });
    fireEvent.click(redo);
    await waitFor(() => expect(button("Changed since")).not.toBeNull());
    expect(writes(calls)).toEqual(["DELETE /v1/agents/a1/memories/grafana-service-account"]);
  });

  it("says so when the Undo fails, and keeps the button", async () => {
    st.events = [e(1, "memory.saved", savedPayload)];
    mock({ my_role: "owner" }, { memory: memory() }, { "DELETE /v1/agents/a1/memories/grafana-service-account": () => err(500, "internal", "Something broke.") });
    pane();
    fireEvent.click(await waitFor(() => within(card()).getByRole("button", { name: "Undo" })));
    expect(await screen.findByRole("alert")).toBeTruthy();
    expect(button("Undo")).not.toBeNull();
    expect(screen.queryByTestId("undo-toast")).toBeNull();
  });
});

describe("who may do what", () => {
  it("someone without access to the agent's memories gets neither View nor Undo", async () => {
    st.events = [e(1, "memory.saved", savedPayload), e(2, "memory.refused", refusedPayload)];
    const calls = mock({ my_role: null }, { memory: memory() });
    pane();
    await waitFor(() => expect(calls.some((c) => c.path === "/v1/workspaces/w")).toBe(true));
    const cards = screen.getAllByTestId("memory-card");
    for (const c of cards) {
      expect(within(c).queryByRole("link")).toBeNull();
      expect(within(c).queryByRole("button", { name: /Undo|Save it yourself|Changed since/ })).toBeNull();
    }
    // The card itself, and Dismiss, are for every reader of the conversation.
    expect(within(cards[1]).getByRole("button", { name: "Dismiss" })).toBeTruthy();
    expect(calls.some((c) => c.path.includes("/memories"))).toBe(false);
  });

  it("an emergency reader may View, and nothing else", async () => {
    st.events = [e(1, "memory.saved", savedPayload)];
    const calls = mock({ my_role: null, administers: true, emergency_reader: true }, { memory: memory() });
    pane();
    await waitFor(() => expect(view()).not.toBeNull());
    expect(button("Undo")).toBeNull();
    expect(button("Changed since")).toBeNull();
    expect(calls.some((c) => c.path.includes("/memories"))).toBe(false);
  });
});

describe("the refused card", () => {
  it("says why and what the agent wanted to add; Dismiss folds it to one line, and Show opens it again", async () => {
    st.events = [e(1, "memory.refused", refusedPayload)];
    mock({ my_role: "owner" }, { memory: memory({ key: "jira-project-keys" }) });
    pane();
    expect(card().textContent).toContain("Daneel tried to remember this but couldn't");
    expect(card().textContent).toContain("“Jira project keys” is locked, so Daneel can't change it. What it wanted to add:");
    expect(card().textContent).toContain("PLAT is now the key for the platform team's board.");
    expect(card().textContent).not.toMatch(/deny/i);
    fireEvent.click(button("Dismiss")!);
    expect(card().textContent).toContain("Daneel tried to remember this but couldn't");
    expect(card().textContent).not.toContain("PLAT is now");
    expect(button("Save it yourself")).toBeNull();
    fireEvent.click(button("Show")!);
    expect(card().textContent).toContain("PLAT is now");
  });

  it("Save it yourself on a locked memory opens it for editing with the text appended as a draft", async () => {
    st.events = [e(1, "memory.refused", refusedPayload)];
    const calls = mock({ my_role: "owner" }, { memory: memory({ key: "jira-project-keys", title: "Jira project keys", content: "OPS is the ops board.", locked: true }) });
    pane();
    fireEvent.click(await waitFor(() => within(card()).getByRole("button", { name: "Save it yourself" })));
    await waitFor(() => expect(nav.push).toHaveBeenCalledWith("/o/x/w/y/agents/daneel?tab=memory&memory=jira-project-keys"));
    expect(peekMemoryDraft("a1")).toEqual({
      kind: "edit", key: "jira-project-keys", content: "OPS is the ops board.\n\nPLAT is now the key for the platform team's board.",
    });
    expect(peekMemoryDraft("another-agent")).toBeNull();
    // Saved by the person: nothing was written from the chat.
    expect(writes(calls)).toEqual([]);
  });

  const newRefusal = { ...refusedPayload, code: "memory_limit_reached", message: "Daneel's memory is full, so it couldn't add this.", title: "Platform board" };

  it("Save it yourself prefills a new memory with the key when it is free", async () => {
    st.events = [e(1, "memory.refused", newRefusal)];
    mock({ my_role: "owner" }, { memory: null });
    pane();
    fireEvent.click(await waitFor(() => within(card()).getByRole("button", { name: "Save it yourself" })));
    await waitFor(() => expect(nav.push).toHaveBeenCalledWith("/o/x/w/y/agents/daneel?tab=memory"));
    expect(peekMemoryDraft("a1")).toEqual({ kind: "new", key: "jira-project-keys", title: "Platform board", content: "PLAT is now the key for the platform team's board." });
  });

  it("Save it yourself leaves the key to the form when it is taken", async () => {
    st.events = [e(1, "memory.refused", { ...newRefusal, code: "invalid" })];
    mock({ my_role: "owner" }, { memory: memory({ key: "jira-project-keys" }) });
    pane();
    fireEvent.click(await waitFor(() => within(card()).getByRole("button", { name: "Save it yourself" })));
    await waitFor(() => expect(nav.push).toHaveBeenCalled());
    expect(peekMemoryDraft("a1")).toMatchObject({ kind: "new", key: null });
  });
});
