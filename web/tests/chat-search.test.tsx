import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ChatTab } from "@/components/agents/chat-tab";
import { ResultRow } from "@/components/chat/search/search-results";
import { HighlightedText } from "@/components/ui/highlighted-text";
import { addChip, extractFilters, inTokenAtEnd, localDayStart, queryOf, tokenize, toSearchParams, type FilterChip } from "@/lib/chat/search-syntax";

// P5-CHAT-4 part B (D-229, C-727): the syntax parser, the highlight and result row, and the search box in the chat tab.

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  window.history.replaceState(null, "", "/");
});

describe("search syntax", () => {
  it("turns finished filter words into chips and leaves the rest as q", () => {
    const r = extractFilters("release from:me has:file notes ", []);
    expect(r.chips).toEqual([{ kind: "from", value: "me" }, { kind: "has", value: "file" }]);
    expect(queryOf(r.rest)).toBe("release notes");
  });
  it("leaves the last word alone until it is finished (Enter finishes it)", () => {
    expect(extractFilters("deploy from:me", []).chips).toEqual([]);
    expect(extractFilters("deploy from:me", [], true).chips).toEqual([{ kind: "from", value: "me" }]);
  });
  it("keeps unknown filters, bad values and impossible days as words", () => {
    for (const w of ["foo:bar", "from:nobody", "has:video", "after:2026-02-30", "after:yesterday", "in:x"]) {
      const r = extractFilters(`${w} `, []);
      expect(r.chips).toEqual([]);
      expect(r.rest).toBe(`${w} `);
    }
  });
  it("never splits a quoted phrase", () => {
    const r = extractFilters('"from:me is a phrase" from:agent ', []);
    expect(r.chips).toEqual([{ kind: "from", value: "agent" }]);
    expect(r.rest.trim()).toBe('"from:me is a phrase"');
    expect(tokenize('a "b c" d').map((t) => t.text)).toEqual(["a", '"b c"', "d"]);
  });
  it("a chip replaces one of its kind, and on: excludes a range", () => {
    let c: FilterChip[] = [];
    c = addChip(c, { kind: "from", value: "me" });
    c = addChip(c, { kind: "from", value: "agent" });
    c = addChip(c, { kind: "after", value: "2026-11-01" });
    c = addChip(c, { kind: "on", value: "2026-11-05" });
    expect(c).toEqual([{ kind: "from", value: "agent" }, { kind: "on", value: "2026-11-05" }]);
  });
  it("finds the partial in: token at the end", () => {
    expect(inTokenAtEnd("deploy in:Wee")).toBe("Wee");
    expect(inTokenAtEnd("deploy in:Weekly ")).toBeNull();
    expect(inTokenAtEnd("deploy")).toBeNull();
  });
  it("sends parameters: local days become RFC 3339 instants, on: is both bounds, the scope is agent_id", () => {
    const p = toSearchParams("ship it", [{ kind: "from", value: "me" }, { kind: "has", value: "file" }, { kind: "on", value: "2026-11-05" }], { agentId: "a1", sort: "relevance" });
    expect(p).toMatchObject({ q: "ship it", agent_id: "a1", author: "me", has: "file", sort: "relevance" });
    expect(p.after).toBe(localDayStart("2026-11-05")!.toISOString());
    expect(p.before).toBe(new Date(2026, 10, 6).toISOString());
    expect(toSearchParams("x", [], { sort: "recent" }).agent_id).toBeUndefined();
    const range = toSearchParams("x", [{ kind: "after", value: "2026-11-01" }, { kind: "before", value: "2026-11-09" }, { kind: "in", value: "c9", label: "T" }], { sort: "recent" });
    expect(range).toMatchObject({ conversation_id: "c9", after: new Date(2026, 10, 1).toISOString(), before: new Date(2026, 10, 9).toISOString() });
  });
});

describe("highlighted text", () => {
  it("marks UTF-16 ranges as text and never as markup", () => {
    const { container } = render(<HighlightedText text="a <b>release</b> 😀 ok" highlights={[[5, 12]]} />);
    expect(container.querySelector("b")).toBeNull();
    expect(container.querySelector("mark")!.textContent).toBe("release");
    expect(container.textContent).toBe("a <b>release</b> 😀 ok");
  });
  it("clamps bad and overlapping ranges", () => {
    const { container } = render(<HighlightedText text="abcdef" highlights={[[4, 99], [-3, 2], [1, 3], [5, 5]]} />);
    expect([...container.querySelectorAll("mark")].map((m) => m.textContent)).toEqual(["ab", "ef"]);
  });
});

const item = (over: Record<string, unknown> = {}) =>
  ({
    conversation: { id: "g1", title: "Team room", kind: "group", archived: false },
    agent: { id: "a1", name: "Daneel" },
    seq: 4,
    type: "text",
    author: { kind: "agent", label: "Daneel" },
    at: "2026-11-08T10:00:00Z",
    snippet: "the release is ready",
    highlights: [[4, 11]],
    ...over,
  }) as never;

describe("result row", () => {
  it("shows title, author, snippet with the highlight and an Archived badge", () => {
    render(<ResultRow item={item({ conversation: { id: "g1", title: "Team room", kind: "group", archived: true }, type: "attachment" })} now={new Date("2026-11-09T10:00:00Z")} showAgent={false} selected={false} href="?c=g1&m=4" local onOpen={() => {}} />);
    const link = screen.getByRole("link");
    expect(within(link).getByText("Team room")).toBeTruthy();
    expect(within(link).getByText("Archived")).toBeTruthy();
    expect(link.textContent).toContain("Daneel · File");
    expect(link.querySelector("mark")!.textContent).toBe("release");
    expect(link.getAttribute("aria-current")).toBeNull();
  });
  it("opens in place on a plain click, keeps the link for modified clicks, and names the agent in All agents", () => {
    const open = vi.fn();
    render(<ResultRow item={item()} now={new Date("2026-11-09T10:00:00Z")} showAgent selected href="?c=g1&m=4" local onOpen={open} />);
    const link = screen.getByRole("link");
    expect(link.getAttribute("aria-current")).toBe("true");
    expect(link.getAttribute("href")).toBe("?c=g1&m=4");
    fireEvent.click(link, { ctrlKey: true });
    expect(open).not.toHaveBeenCalled();
    fireEvent.click(link);
    expect(open).toHaveBeenCalledTimes(1);
  });
  it("does not intercept a result of another agent", () => {
    const open = vi.fn();
    render(<ResultRow item={item()} now={new Date()} showAgent selected={false} href="/o/x/w/y/agents/other?c=g1&m=4" local={false} onOpen={open} />);
    fireEvent.click(screen.getByRole("link"));
    expect(open).not.toHaveBeenCalled();
  });
});

describe("search in the chat tab", () => {
  const conv = (id: string, kind: string, over: Record<string, unknown> = {}) => ({
    id, agent_id: "a1", kind, external_ref: id, title: null, direct_user_id: null, direct_user: null, origin_conversation_id: null,
    last_seq: 0, app_turn_streak: 0, app_turn_capped: false, active_turn: null, last_activity_at: "2026-11-08T10:00:00Z", archived_at: null, created_at: "2026-11-01T00:00:00Z", ...over,
  });
  const list = [conv("mine", "direct", { direct_user_id: "me" }), conv("g1", "group", { title: "Team room" })];
  let searches: URL[] = [];
  let reply: () => Response;
  beforeEach(() => {
    searches = [];
    reply = () => json(200, { items: [item()], next_cursor: null, index_complete: false });
    vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url, "http://localhost");
      if (url.pathname === "/v1/agents/a1/conversations/direct") return json(200, list[0]);
      if (url.pathname === "/v1/agents/a1/conversations") return json(200, { items: list, next_cursor: null });
      if (url.pathname === "/v1/workspaces/w/messages/search") {
        searches.push(url);
        return reply();
      }
      return json(404, { error: { code: "not_found", message: "x" } });
    }));
  });
  function mount() {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    return render(
      <QueryClientProvider client={qc}>
        <ChatTab agentId="a1" wsId="w" base="/o/x/w/y" />
      </QueryClientProvider>,
    );
  }
  const box = () => screen.getByRole("combobox", { name: "Search conversations and messages" });

  it("Enter searches, sends parameters (not the syntax), shows results and the indexing line", async () => {
    mount();
    await screen.findByText("Team room");
    fireEvent.change(box(), { target: { value: "release from:me " } });
    fireEvent.keyDown(box(), { key: "Enter" });
    const nav = screen.getByRole("navigation", { name: "Conversations" });
    await within(nav).findByRole("link", { name: /Team room/ });
    const q = searches.at(-1)!.searchParams;
    expect(q.get("q")).toBe("release");
    expect(q.get("author")).toBe("me");
    expect(q.get("agent_id")).toBe("a1");
    expect(q.get("sort")).toBe("recent");
    expect(within(nav).getByRole("status").textContent).toBe("1 result");
    expect(within(nav).getByText("Older messages are still being indexed")).toBeTruthy();
  });

  it("a typed filter becomes a removable chip; Backspace in an empty box removes it", async () => {
    mount();
    await screen.findByText("Team room");
    fireEvent.change(box(), { target: { value: "has:file " } });
    expect(screen.getByRole("group", { name: "Search filters" }).textContent).toContain("Has: file");
    expect((box() as HTMLInputElement).value).toBe("");
    fireEvent.keyDown(box(), { key: "Backspace" });
    expect(screen.queryByRole("group", { name: "Search filters" })).toBeNull();
  });

  it("opening a result of this agent selects the conversation and sets c and m", async () => {
    mount();
    await screen.findByText("Team room");
    fireEvent.change(box(), { target: { value: "release" } });
    fireEvent.keyDown(box(), { key: "Enter" });
    fireEvent.click(await screen.findByRole("link", { name: /Team room/ }));
    expect(window.location.search).toBe("?c=g1&m=4");
    expect(screen.getByRole("link", { name: /Team room/ }).getAttribute("aria-current")).toBe("true");
  });

  it("Esc clears the search and returns to the conversation list", async () => {
    mount();
    await screen.findByText("Team room");
    fireEvent.change(box(), { target: { value: "release" } });
    fireEvent.keyDown(box(), { key: "Enter" });
    await screen.findByRole("link", { name: /Team room/ });
    fireEvent.keyDown(box(), { key: "Escape" }); // closes the suggestions first, if open
    fireEvent.keyDown(box(), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("link", { name: /Team room/ })).toBeNull());
    expect((box() as HTMLInputElement).value).toBe("");
    expect(screen.getByRole("tab", { name: "Active" })).toBeTruthy();
  });

  it("a 422 shows its field error under the box", async () => {
    reply = () => json(422, { error: { code: "search_query_invalid", message: "bad", details: { reason: "too_long" } } });
    mount();
    await screen.findByText("Team room");
    fireEvent.change(box(), { target: { value: "x".repeat(250) } });
    fireEvent.keyDown(box(), { key: "Enter" });
    const err = await screen.findByText("Searches are limited to 200 characters.");
    expect(err.getAttribute("role")).toBe("alert");
    expect(box().getAttribute("aria-invalid")).toBe("true");
  });

  it("no results: says so and how to widen", async () => {
    reply = () => json(200, { items: [], next_cursor: null, index_complete: true });
    mount();
    await screen.findByText("Team room");
    fireEvent.change(box(), { target: { value: "nothing" } });
    fireEvent.keyDown(box(), { key: "Enter" });
    expect(await screen.findByText("No messages match “nothing”.")).toBeTruthy();
    expect(screen.getByText(/search All agents/)).toBeTruthy();
  });

  it("the scope switch searches the workspace (no agent_id) and the sort sends relevance", async () => {
    mount();
    await screen.findByText("Team room");
    fireEvent.change(box(), { target: { value: "release" } });
    fireEvent.keyDown(box(), { key: "Enter" });
    await screen.findByRole("link", { name: /Team room/ });
    fireEvent.click(screen.getByRole("radio", { name: "All agents" }));
    await waitFor(() => expect(searches.at(-1)!.searchParams.has("agent_id")).toBe(false));
    fireEvent.change(await screen.findByRole("combobox", { name: "Sort results" }), { target: { value: "relevance" } });
    await waitFor(() => expect(searches.at(-1)!.searchParams.get("sort")).toBe("relevance"));
  });
});
