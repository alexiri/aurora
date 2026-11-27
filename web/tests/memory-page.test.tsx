import { act, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { MemoryTab } from "@/components/memory/memory-tab";
import { UndoToastHost, dismissUndoToast } from "@/components/ui/undo-toast";
import { formatAge } from "@/lib/format-date";
import { baseKey, keyProblem, memoryName, newMemoryKey, withSuffix } from "@/lib/memory-key";
import { err, json, mockFetch, type Handler } from "./support/api";
import { wrap } from "./support/render";

// (P5-MEMORY-2) The Memory page of the Skills & Memory review: the list, the reading view, the editor, search, the deep
// link, the narrow screens and the emergency reader's view. The words are the mockups' (mem-1, mem-2, mem-3, mem-6).

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  usePathname: () => "/o/acme/w/main/agents/daneel",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(window.location.search),
}));

const ME = "u-bob";
const daysAgo = (n: number) => new Date(Date.now() - n * 86_400_000).toISOString();

const bob = { id: ME, display_name: "Bob Builder" };
const byBob = { kind: "person", person: bob };

const mem = (over: Record<string, unknown> = {}) => ({
  key: "aurora-migration",
  title: "Aurora migration project",
  description: "Status, decisions and next steps",
  content: "# Aurora migration\n\n**Goal:** move the agents.\n\n- [x] Pilot workspace set up\n- [ ] Switch the old bot off",
  index_line: "aurora-migration: Status",
  revision: 3,
  created_at: daysAgo(2),
  updated_at: daysAgo(0),
  locked: false,
  locked_at: null,
  locked_by: null,
  added_by: byBob,
  changed_by: byBob,
  last_used: { at: daysAgo(1), conversation: { id: "c1", title: "Weekly report" } },
  ...over,
});

const learned = mem({
  key: "carol-tea",
  title: null,
  description: null,
  content: "Carol drinks green tea, no sugar.\nAlways.",
  revision: 1,
  created_at: daysAgo(0),
  updated_at: daysAgo(1),
  added_by: { kind: "agent", agent: { id: "a1", name: "Daneel" }, conversation: { id: "c2", title: "Release prep" } },
  last_used: null,
});

const oncall = mem({
  key: "on-call",
  title: "On-call rotation",
  description: "Who is on call and how to reach them",
  content: "# On-call rotation\n\nRotation changes every Monday at 09:00.\n\n- This week: Carol",
  revision: 2,
  created_at: daysAgo(21),
  updated_at: daysAgo(3),
  locked: true,
  locked_at: daysAgo(3),
  locked_by: bob,
  last_used: null,
});

const ALL = [mem(), learned, oncall];

const me = () => json(200, { user: { id: ME, email: "bob@example.test", display_name: "Bob Builder" }, orgs: [], is_instance_admin: false });

/** `fetch` for the routes; `useMe` fetches a relative path, so a plain string is made a request first. */
function stub(routes: Record<string, Handler>) {
  const answer = mockFetch({ "GET /v1/me": me, ...routes });
  vi.stubGlobal("fetch", (input: Request | string, init?: RequestInit) =>
    answer(typeof input === "string" ? new Request(new URL(input, "http://localhost"), { method: init?.method }) : input),
  );
}

function serve(items: unknown[] = ALL, extra: Record<string, Handler> = {}) {
  stub({ "GET /v1/agents/a1/memories": () => json(200, { items, next_cursor: null }), ...extra });
}

const page = (props: Partial<React.ComponentProps<typeof MemoryTab>> = {}) =>
  wrap(
    <>
      <MemoryTab agentId="a1" agentName="Daneel" {...props} />
      <UndoToastHost />
    </>,
  );

const list = () => screen.findByRole("list", { name: "Memories" });
const rows = async () => within(await list()).getAllByRole("button");
const reading = (name: string) => screen.findByRole("article", { name });

beforeEach(() => window.history.replaceState(null, "", "/o/acme/w/main/agents/daneel?tab=memory"));
afterEach(() => dismissUndoToast());

describe("memory keys", () => {
  it("comes from the title, else the first words of the text, in kebab-case ASCII", () => {
    expect(baseKey("Bob's tea", "ignored")).toBe("bobs-tea");
    expect(baseKey("", "Bob prefers green tea in the morning, always.")).toBe("bob-prefers-green-tea-in-the");
    expect(baseKey("Crème brûlée & Zoë", "")).toBe("creme-brulee-zoe");
    expect(baseKey("日本語", "")).toBe("note");
    expect(baseKey("", "   ")).toBe("note");
  });
  it("is at most 64 characters, never ends in a hyphen, and is unique by a numeric suffix", () => {
    const k = baseKey("a".repeat(63) + " bbb", "");
    expect(k.length).toBeLessThanOrEqual(64);
    expect(k.endsWith("-")).toBe(false);
    expect(withSuffix("a".repeat(64), 12)).toHaveLength(64);
    expect(newMemoryKey("Bob's tea", "", ["bobs-tea", "bobs-tea-2"])).toBe("bobs-tea-3");
    expect(newMemoryKey("Bob's tea", "", [], 1)).toBe("bobs-tea-2");
  });
  it("says what is wrong with a typed key as the server does", () => {
    expect(keyProblem("where-the-runbooks-live")).toBeNull();
    for (const bad of ["", "Upper", "two--dashes", "-start", "end-", "with space", "a".repeat(65)]) expect(keyProblem(bad), bad).not.toBeNull();
    for (const reserved of ["index", "search"]) expect(keyProblem(reserved)).toContain("reserved");
  });
  it("names an untitled memory by its first line", () => {
    expect(memoryName({ content: "\n# Heading line\nmore" })).toBe("Heading line");
    expect(memoryName({ content: "w".repeat(80) })).toHaveLength(60);
  });
  // (FIX-277) The first line as plain text: Markdown's marks are not part of a name.
  it.each([
    ["a list item", "- Carol drinks tea", "Carol drinks tea"],
    ["an ordered item", "2) Cut over on Monday", "Cut over on Monday"],
    ["a task item", "- [ ] Move the on-call agent", "Move the on-call agent"],
    ["a ticked task item", "* [x] Pilot set up", "Pilot set up"],
    ["a quote", "> > Never on a Friday", "Never on a Friday"],
    ["a list item in a quote, with a heading", "> - ## Rota", "Rota"],
    ["emphasis", "**Goal:** move *the* ~~old~~ __three__ _agents_ ***now***", "Goal: move the old three agents now"],
    ["inline code", "Run `kubectl get pods` first", "Run kubectl get pods first"],
    ["code keeps its own characters", "Set `__init__` and `a*b*c`", "Set __init__ and a*b*c"],
    ["a link", "See [the runbook](https://example.org/a_(b)) and ![a chart](x.png)", "See the runbook and a chart"],
    ["a reference link and an autolink", "[Docs][1] at <https://example.org/docs>", "Docs at https://example.org/docs"],
    ["underscores inside a word", "The key is memory_search_v2, not _this_", "The key is memory_search_v2, not this"],
    ["stars that mark nothing", "2 * 3 * 4 for *.rs and *.ts", "2 * 3 * 4 for *.rs and *.ts"],
    ["escaped marks", "\\*not emphasis\\*", "*not emphasis*"],
    ["a hash that is no heading", "#277 is fixed", "#277 is fixed"],
    ["a first line that is only marks", "```\nnpm ci\n```", "npm ci"],
    ["a rule before the text", "---\n- [ ]\nRota", "Rota"],
    ["the review's line", "- *API latency*: `platform/api-overview` — p50/p95 by route.", "API latency: platform/api-overview — p50/p95 by route."],
  ])("names an untitled memory in plain text: %s", (_what, content, name) => {
    expect(memoryName({ content })).toBe(name);
  });
  it("keeps a title as written, and falls back when the text has no words", () => {
    expect(memoryName({ title: "**As written**", content: "x" })).toBe("**As written**");
    expect(memoryName({ content: "---\n", index_line: "note" })).toBe("note");
  });
  it("says how long ago in calendar days", () => {
    const now = Date.parse("2026-11-27T12:00:00Z");
    const at = (d: number) => new Date(now - d * 86_400_000).toISOString();
    expect([0, 1, 2, 6, 7, 21, 29].map((d) => formatAge(at(d), now))).toEqual(["today", "yesterday", "2 days ago", "6 days ago", "1 week ago", "3 weeks ago", "4 weeks ago"]);
    expect(formatAge(at(90), now)).toBe("3 months ago");
    expect(formatAge(at(800), now)).toBe("2 years ago");
  });
});

describe("the list (mem-1)", () => {
  it("has the chips with their counts, rows recent first, and no banner or Select-a-memory column", async () => {
    serve();
    page();
    await waitFor(() => expect(screen.getByRole("radio", { name: "By you 2" })).toBeInTheDocument());
    for (const name of ["All 3", "By Daneel 1", "Locked 1"]) expect(screen.getByRole("radio", { name })).toBeInTheDocument();
    expect((await rows()).map((r) => r.querySelector("span span")?.textContent)).toEqual([
      "Aurora migration project",
      "Carol drinks green tea, no sugar.",
      "On-call rotation",
    ]);
    expect(screen.queryByText(/Select a memory/)).toBeNull();
    expect(screen.queryByText(/are written in/)).toBeNull();
    expect(screen.getByRole("searchbox", { name: "Search memories" })).toHaveAttribute("placeholder", "Search memories");
  });

  it("filters by chip and sorts by name", async () => {
    serve();
    page();
    fireEvent.click(await screen.findByRole("radio", { name: "Locked 1" }));
    expect(await rows()).toHaveLength(1);
    expect((await rows())[0]).toHaveTextContent("On-call rotation");
    fireEvent.click(screen.getByRole("radio", { name: "By Daneel 1" }));
    expect((await rows())[0]).toHaveTextContent("Carol drinks green tea");
    fireEvent.click(screen.getByRole("radio", { name: "All 3" }));
    fireEvent.keyDown(screen.getByRole("button", { name: "Sorted by Recently changed" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "Name" }));
    expect((await rows()).map((r) => r.textContent?.slice(0, 6))).toEqual(["Aurora", "Carol ", "On-cal"]);
  });

  it("rows say the description or No description, where the memory came from, and a lock; never the key or a link", async () => {
    serve();
    page();
    const l = await list();
    await within(l).findByText("Added by you · 2 days ago");
    expect(within(l).getByText("Status, decisions and next steps")).toBeInTheDocument();
    expect(within(l).getByText("No description")).toBeInTheDocument();
    expect(within(l).getByText("Learned by Daneel in “Release prep” · today")).toBeInTheDocument();
    expect(within(l).getAllByText("Locked")).toHaveLength(1);
    expect(within(l).queryAllByRole("link")).toHaveLength(0);
    expect(l.textContent).not.toContain("aurora-migration");
  });

  it.each([
    ["another person", { kind: "person", person: { id: "u2", display_name: "Carol Co" } }, "Added by Carol Co · 2 days ago"],
    ["my own API key", { kind: "api_key", person: bob, api_key: { id: "k1", name: "laptop" } }, "Added by you · 2 days ago"],
    ["another's API key", { kind: "api_key", person: { id: "u2", display_name: "Carol Co" }, api_key: { id: "k2", name: "deploy bot" } }, "Added through deploy bot · 2 days ago"],
    ["an unreadable conversation", { kind: "agent", agent: { id: "a1", name: "Daneel" }, conversation: null }, "Learned by Daneel in a conversation · 2 days ago"],
    ["an untitled conversation", { kind: "agent", agent: { id: "a1", name: "Daneel" }, conversation: { id: "c9", title: null } }, "Learned by Daneel in a conversation · 2 days ago"],
    ["a template", { kind: "template", template: { name: "Release helper", version: "2" } }, "From the template Release helper · 2 days ago"],
    ["an unknown source", { kind: "unknown" }, "Added · 2 days ago"],
  ])("the source line for %s", async (_what, added_by, line) => {
    serve([mem({ added_by })]);
    page();
    const l = await list();
    expect(await within(l).findByText(line)).toBeInTheDocument();
    expect(within(l).queryAllByRole("link")).toHaveLength(0);
  });
});

describe("the reading view (mem-1)", () => {
  it("shows the first memory rendered, with its key, actions and the line about its history", async () => {
    serve();
    page();
    const a = await reading("Aurora migration project");
    expect(within(a).getByRole("heading", { level: 1, name: "Aurora migration" })).toBeInTheDocument();
    expect(within(a).getByRole("checkbox", { name: "Pilot workspace set up" })).toBeChecked();
    expect(within(a).getByRole("checkbox", { name: "Switch the old bot off" })).not.toBeChecked();
    expect(within(a).getByTestId("copy-chip-text")).toHaveTextContent("aurora-migration");
    for (const name of ["Edit", "Lock", "More actions", "Copy Key"]) expect(within(a).getByRole("button", { name })).toBeInTheDocument();
    await waitFor(() =>
      expect(within(a).getByTestId("memory-meta")).toHaveTextContent(/^Added by Bob Builder on .+ · Changed twice · Daneel last used it in “Weekly report” yesterday$/),
    );
    expect(screen.queryByText(/Technical details/)).toBeNull();
    expect(screen.queryByRole("textbox", { name: "Title" })).toBeNull();
  });

  it("copies the key", async () => {
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    serve();
    page();
    fireEvent.click(await screen.findByRole("button", { name: "Copy Key" }));
    expect(writeText).toHaveBeenCalledWith("aurora-migration");
  });

  it.each([
    [{ at: daysAgo(1), conversation: null }, "Daneel last used it in a conversation yesterday"],
    [null, "Daneel hasn't used it yet"],
  ])("never names a conversation it was not given (%#)", async (last_used, words) => {
    serve([mem({ last_used, revision: 1 })]);
    page();
    const meta = await screen.findByTestId("memory-meta");
    expect(meta.textContent).toContain(words);
    expect(meta.textContent).not.toContain("Changed");
    expect(within(meta).queryAllByRole("link")).toHaveLength(0);
  });

  it("locks and unlocks from the actions, with a toast", async () => {
    const puts: unknown[] = [];
    serve([mem()], {
      "PUT /v1/agents/a1/memories/aurora-migration/locked": async (r) => {
        puts.push(await r.json());
        return json(200, mem({ locked: true }));
      },
    });
    page();
    fireEvent.click(await screen.findByRole("button", { name: "Lock" }));
    await waitFor(() => expect(puts).toEqual([{ locked: true }]));
    expect(await screen.findByTestId("undo-toast")).toHaveTextContent("Locked");
  });

  it("opens History and deletes from the … menu", async () => {
    let deleted = false;
    serve([mem()], {
      "GET /v1/agents/a1/memories/aurora-migration/revisions": () => json(200, { items: [], next_cursor: null }),
      "DELETE /v1/agents/a1/memories/aurora-migration": () => {
        deleted = true;
        return json(204, null);
      },
    });
    page();
    const menu = async () => fireEvent.keyDown(await screen.findByRole("button", { name: "More actions" }), { key: "Enter" });
    await menu();
    fireEvent.click(await screen.findByRole("menuitem", { name: "History" }));
    const history = await screen.findByRole("article", { name: "History" });
    expect(window.location.search).toBe("?tab=memory&memory=aurora-migration&history=1");
    fireEvent.click(within(history).getByRole("button", { name: "Back to the memory" }));
    await menu();
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const dialog = await screen.findByRole("dialog", { name: "Delete the memory “Aurora migration project”?" });
    expect(deleted).toBe(false);
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete memory" }));
    await waitFor(() => expect(deleted).toBe(true));
  });
});

describe("editing (mem-2)", () => {
  const openEdit = async () => {
    fireEvent.click(await screen.findByRole("button", { name: "Edit" }));
    return screen.findByRole("form", { name: "Edit Aurora migration project" });
  };

  it("has Title, the fixed key, Description, the Write/Preview editor, the Locked switch; Save sends them", async () => {
    const puts: unknown[] = [];
    serve([mem()], {
      "PUT /v1/agents/a1/memories/aurora-migration": async (r) => {
        puts.push(await r.json());
        return json(200, mem({ revision: 4 }));
      },
    });
    page();
    const form = await openEdit();
    expect(within(form).getByLabelText("Title")).toHaveValue("Aurora migration project");
    expect(within(form).getByTestId("copy-chip-text")).toHaveTextContent("aurora-migration");
    expect(within(form).getByText("Daneel finds the memory by this name. It can't be changed.")).toBeInTheDocument();
    expect(within(form).queryByRole("textbox", { name: "Key" })).toBeNull();
    expect(within(form).getByLabelText("Description")).toHaveValue("Status, decisions and next steps");
    expect(within(form).getByText("Daneel sees the title and this line first and opens the memory only when it fits.")).toBeInTheDocument();
    expect(within(form).getByRole("tab", { name: "Write", selected: true })).toBeInTheDocument();
    expect(within(form).getByText("Markdown works: # headings, - lists, - [ ] checklists, tables")).toBeInTheDocument();
    expect(within(form).getByText(`${mem().content.length} of 100,000 characters`)).toBeInTheDocument();
    expect(within(form).getByRole("switch", { name: "Locked" })).not.toBeChecked();
    expect(within(form).getByText("Daneel can read this memory but can't change or delete it. You can still edit it.")).toBeInTheDocument();
    // Nothing changed yet: no bar, and Done leaves.
    expect(screen.queryByText("Unsaved changes")).toBeNull();
    expect(within(form).getByRole("button", { name: "Done" })).toBeInTheDocument();

    fireEvent.change(within(form).getByLabelText("Description"), { target: { value: "  Next steps  " } });
    fireEvent.change(within(form).getByRole("textbox", { name: "What should Daneel remember?" }), { target: { value: "New text" } });
    expect(await screen.findByRole("region", { name: "Unsaved changes" })).toBeInTheDocument();
    expect(within(await list()).getByTestId("dirty-dot")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(puts).toEqual([{ title: "Aurora migration project", description: "Next steps", content: "New text" }]));
    // Back to reading, with the saved flash.
    expect(await screen.findByTestId("save-bar-saved")).toBeInTheDocument();
    expect(await reading("Aurora migration project")).toBeInTheDocument();
  });

  it("Discard drops the changes and shows the memory again", async () => {
    serve([mem()]);
    page();
    const form = await openEdit();
    fireEvent.change(within(form).getByLabelText("Title"), { target: { value: "Other" } });
    fireEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(await reading("Aurora migration project")).toBeInTheDocument();
    expect(screen.queryByRole("form")).toBeNull();
  });

  it("asks before leaving with unsaved changes: another memory, New memory", async () => {
    serve();
    page();
    const form = await openEdit();
    fireEvent.change(within(form).getByLabelText("Title"), { target: { value: "Changed title" } });
    fireEvent.click(within(await list()).getByText("On-call rotation"));
    let dialog = await screen.findByRole("dialog", { name: "Leave without saving?" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Keep editing" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.getByLabelText("Title")).toHaveValue("Changed title");
    fireEvent.click(screen.getByRole("button", { name: "New memory" }));
    dialog = await screen.findByRole("dialog", { name: "Leave without saving?" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Discard" }));
    expect(await screen.findByRole("form", { name: "New memory" })).toBeInTheDocument();
  });

  it("the switch of an existing memory saves on its own and says who locked it", async () => {
    let locked = false;
    const state = () => mem({ locked, locked_at: locked ? daysAgo(0) : null, locked_by: locked ? bob : null });
    const posts: unknown[] = [];
    stub({
      "GET /v1/agents/a1/memories": () => json(200, { items: [state()], next_cursor: null }),
      "PUT /v1/agents/a1/memories/aurora-migration/locked": async (r) => {
        const body = (await r.json()) as { locked: boolean };
        posts.push(body);
        locked = body.locked;
        return json(200, state());
      },
    });
    page();
    const form = await openEdit();
    fireEvent.click(within(form).getByRole("switch", { name: "Locked" }));
    await waitFor(() => expect(posts).toEqual([{ locked: true }]));
    expect(await screen.findByText(/Locked by Bob Builder on /)).toBeInTheDocument();
    expect(await screen.findByTestId("undo-toast")).toHaveTextContent("Locked");
  });
});

describe("a new memory (mem-2)", () => {
  const openNew = async (items: unknown[] = ALL, extra: Record<string, Handler> = {}) => {
    serve(items, extra);
    page();
    fireEvent.click(await screen.findByRole("button", { name: "New memory" }));
    return screen.findByRole("form", { name: "New memory" });
  };
  const type = (label: string, value: string) => fireEvent.change(screen.getByRole("textbox", { name: label }), { target: { value } });
  const posted = () => {
    const bodies: Record<string, unknown>[] = [];
    const handler: Handler = async (r) => {
      const body = (await r.json()) as Record<string, unknown>;
      bodies.push(body);
      return json(201, mem({ key: body.key, title: body.title }));
    };
    return { bodies, handler };
  };

  it("makes the key from the title until the person changes it, in the same form with the bar", async () => {
    const { bodies, handler } = posted();
    const form = await openNew(ALL, { "POST /v1/agents/a1/memories": handler });
    expect(screen.getByText("New memory · not saved yet")).toBeInTheDocument();
    expect(within(form).getByRole("switch", { name: "Locked" })).not.toBeChecked();
    expect(within(form).getByPlaceholderText("One line: when should Daneel use this?")).toBeInTheDocument();
    expect(within(form).getByPlaceholderText("Write it the way you would tell a colleague. Plain words are fine.")).toBeInTheDocument();
    type("Title", "Where the runbooks live");
    expect(screen.getByRole("textbox", { name: "Key" })).toHaveValue("where-the-runbooks-live");
    expect(screen.getByText("Made from the title")).toBeInTheDocument();
    expect(
      screen.getByText("Daneel finds the memory by this name. Change it now if you like: lowercase words joined by dashes. It can't be changed after saving."),
    ).toBeInTheDocument();
    type("Key", "runbooks");
    expect(screen.queryByText("Made from the title")).toBeNull();
    type("Title", "Another title");
    expect(screen.getByRole("textbox", { name: "Key" })).toHaveValue("runbooks");
    type("What should Daneel remember?", "In the wiki.");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(bodies).toEqual([{ key: "runbooks", title: "Another title", description: null, content: "In the wiki." }]));
    expect(window.location.search).toBe("?tab=memory&memory=runbooks");
  });

  it("sends locked: true only when the switch is on", async () => {
    const { bodies, handler } = posted();
    await openNew(ALL, { "POST /v1/agents/a1/memories": handler });
    fireEvent.click(screen.getByRole("switch", { name: "Locked" }));
    type("What should Daneel remember?", "Pinned.");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(bodies[0]).toMatchObject({ locked: true, key: "pinned" }));
  });

  it("refuses a bad key and empty or too long text before asking the server", async () => {
    const { bodies, handler } = posted();
    await openNew(ALL, { "POST /v1/agents/a1/memories": handler });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText("Write what Daneel should remember.")).toBeInTheDocument();
    type("Key", "Not A Key");
    type("What should Daneel remember?", "Text");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText("Use lowercase words joined by dashes, at most 64 characters.")).toBeInTheDocument();
    type("What should Daneel remember?", "x".repeat(100_001));
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    expect(bodies).toHaveLength(0);
  });

  it("a made key that exists moves to the next suffix once", async () => {
    const keys: string[] = [];
    await openNew(ALL, {
      "POST /v1/agents/a1/memories": async (r) => {
        const body = (await r.json()) as { key: string };
        keys.push(body.key);
        return keys.length === 1 ? err(409, "memory_exists", "A memory with this key already exists.") : json(201, mem({ key: body.key }));
      },
    });
    type("What should Daneel remember?", "Tea");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(keys).toEqual(["tea", "tea-2"]));
  });

  it("a key the person typed that exists is said on the field, never silently changed", async () => {
    const keys: string[] = [];
    await openNew(ALL, {
      "POST /v1/agents/a1/memories": async (r) => {
        keys.push(((await r.json()) as { key: string }).key);
        return err(409, "memory_exists", "A memory with this key already exists.");
      },
    });
    type("Key", "on-call");
    type("What should Daneel remember?", "Text");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    const field = screen.getByRole("textbox", { name: "Key" });
    await waitFor(() => expect(field).toHaveAccessibleDescription("A memory with this key already exists"));
    expect(field).toHaveAttribute("aria-invalid", "true");
    expect(keys).toEqual(["on-call"]);
  });

  it("an agent with no memory shows one empty state with its action", async () => {
    await openNew([]);
    expect(screen.queryByRole("heading", { name: "Daneel doesn't remember anything yet" })).toBeNull();
  });
});

describe("search (mem-3)", () => {
  const hit = {
    key: "on-call",
    title: "On-call rotation",
    snippet: "Rotation changes every Monday at 09:00. This week: Carol",
    highlights: [[0, 8]],
    title_highlights: [[8, 16]],
    locked: true,
    score: 1,
  };
  const searching = () => {
    const queries: string[] = [];
    serve(ALL, {
      "GET /v1/agents/a1/memories/search": (r) => {
        queries.push(new URL(r.url).searchParams.get("q") ?? "");
        return json(200, { items: [hit] });
      },
    });
    return queries;
  };

  it("waits for the typing to stop, counts, highlights from the ranges and opens the first match at its line", async () => {
    const queries = searching();
    page();
    await reading("Aurora migration project");
    const box = screen.getByRole("searchbox", { name: "Search memories" });
    for (const v of ["r", "rot", "rotation"]) fireEvent.change(box, { target: { value: v } });
    expect(await screen.findByText("1 memory matches “rotation”")).toBeInTheDocument();
    expect(queries).toEqual(["rotation"]);
    const results = screen.getByRole("list", { name: "Search results" });
    expect([...results.querySelectorAll("mark")].map((m) => m.textContent)).toEqual(["rotation", "Rotation"]);
    expect(within(results).getByText(/Added by you · 3 weeks ago/)).toBeInTheDocument();
    const a = await reading("On-call rotation");
    expect(within(a).getByTestId("match-line")).toHaveTextContent("Match 1 of 1in the text, from the top: “# On-call rotation …”");
    const line = a.querySelector(".md-match-line")!;
    expect(line.tagName).toBe("P");
    expect(line.textContent).toBe("Rotation changes every Monday at 09:00.");
    expect(line.querySelector("mark")?.textContent).toBe("Rotation");
    expect(document.body.textContent).not.toContain("<b>");
    expect(screen.queryByRole("article", { name: "Aurora migration project" })).toBeNull();
  });

  it("Clear restores the list and the memory that was open", async () => {
    searching();
    page();
    fireEvent.click(within(await list()).getByText("Carol drinks green tea, no sugar."));
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "rotation" } });
    fireEvent.click(within(await screen.findByRole("list", { name: "Search results" })).getByRole("button"));
    expect(window.location.search).toBe("?tab=memory&memory=on-call");
    fireEvent.click(screen.getByRole("button", { name: "Clear" }));
    expect(await list()).toBeInTheDocument();
    expect(await reading("Carol drinks green tea, no sugar.")).toBeInTheDocument();
    expect(screen.getByRole("searchbox")).toHaveValue("");
    expect(window.location.search).toBe("?tab=memory&memory=carol-tea");
  });

  it("with no result it says so and shows no unrelated memory", async () => {
    serve(ALL, { "GET /v1/agents/a1/memories/search": () => json(200, { items: [] }) });
    page();
    fireEvent.change(await screen.findByRole("searchbox"), { target: { value: "zzz" } });
    expect(await screen.findByText("No memories match “zzz”")).toBeInTheDocument();
    expect(screen.queryByRole("article")).toBeNull();
  });
});

describe("the deep link (C-1177)", () => {
  it("?memory=<key> opens that memory, and choosing a row writes it with replace", async () => {
    window.history.replaceState(null, "", "/o/acme/w/main/agents/daneel?tab=memory&memory=on-call");
    serve();
    page();
    expect(await reading("On-call rotation")).toBeInTheDocument();
    const entries = window.history.length;
    fireEvent.click(within(await list()).getByText("Aurora migration project"));
    expect(window.location.search).toBe("?tab=memory&memory=aurora-migration");
    expect(window.history.length).toBe(entries);
  });

  it("an unknown key falls back to the first memory", async () => {
    window.history.replaceState(null, "", "/o/acme/w/main/agents/daneel?tab=memory&memory=gone");
    serve();
    page();
    expect(await reading("Aurora migration project")).toBeInTheDocument();
  });
});

describe("the emergency reader's view", () => {
  it("reads memories, with no search, no actions and no history", async () => {
    window.history.replaceState(null, "", "/o/acme/w/main/agents/daneel?tab=memory&memory=on-call&history=1");
    serve();
    page({ readOnly: true });
    const a = await reading("On-call rotation");
    expect(within(a).getByTestId("memory-meta").textContent).toContain("Locked by Bob Builder on ");
    expect(screen.queryByRole("searchbox")).toBeNull();
    expect(screen.queryByRole("article", { name: "History" })).toBeNull();
    for (const name of ["Edit", "Lock", "Unlock", "More actions", "New memory"]) expect(screen.queryByRole("button", { name }), name).toBeNull();
  });
});

describe("narrow screens (mem-6)", () => {
  beforeEach(() => {
    vi.stubGlobal("matchMedia", (query: string) => ({ matches: query.includes("max-width"), media: query, addEventListener: () => undefined, removeEventListener: () => undefined }));
    vi.stubGlobal("scrollTo", () => undefined);
  });
  afterEach(() => vi.unstubAllGlobals());

  it("the list is one screen; a row opens the memory on its own, with back, Edit, and Lock and History below", async () => {
    serve();
    page();
    expect(await list()).toBeInTheDocument();
    expect(screen.queryByRole("article")).toBeNull();
    await act(async () => {
      fireEvent.click(within(await list()).getByText("Aurora migration project"));
    });
    expect(await reading("Aurora migration project")).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Memories" })).toBeNull();
    expect(screen.queryByRole("searchbox")).toBeNull();
    for (const name of ["Memory", "Edit", "More actions", "Lock", "History"]) expect(screen.getByRole("button", { name })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect(await screen.findByRole("form", { name: "Edit Aurora migration project" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Memory" }));
    expect(await list()).toBeInTheDocument();
    expect(window.location.search).toBe("?tab=memory");
  });
});
