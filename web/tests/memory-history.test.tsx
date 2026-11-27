import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { MemoryTab } from "@/components/memory/memory-tab";
import { UndoToastHost, dismissUndoToast } from "@/components/ui/undo-toast";
import type { MemoryRevision } from "@/lib/api/memory";
import { whenInSentence } from "@/lib/format-date";
import { err, json, mockFetch, type Handler } from "./support/api";
import { wrap } from "./support/render";

// (P5-MEMORY-3, mem-4) A memory's History: it replaces the reading view, names people and context, compares each
// version with the one before in the shared DiffView, restores with an undo and removes a version's text.

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  usePathname: () => "/o/acme/w/main/agents/daneel",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(window.location.search),
}));

const ago = (ms: number) => new Date(Date.now() - ms).toISOString();
const MIN = 60_000;
const DAY = 86_400_000;
const bob = { id: "u-bob", display_name: "Bob Builder" };

const rev = (n: number, over: Partial<MemoryRevision> = {}): MemoryRevision => ({
  revision: n,
  author_kind: "user",
  author_label: "Bob Builder",
  provenance: "dashboard",
  author: { kind: "person", person: bob },
  via: "dashboard",
  redacted: false,
  content: `content ${n}`,
  created_at: ago((10 - n) * DAY),
  ...over,
});

const V1 = "# Aurora\n- [ ] Move the on-call agent\n- [ ] Switch the old bot off by 1 December";
const V2 = `${V1}\n- Decided on Monday`;
const V3 = "# Aurora\n- [x] Move the on-call agent\n- [ ] Switch the old bot off\n- Decided on Monday\n- Never on a Friday";

const REVS = [
  rev(3, { content: V3, via: "api", author: { kind: "api_key", person: bob, api_key: { id: "k1", name: "laptop" } }, created_at: ago(2 * MIN) }),
  rev(2, {
    content: V2,
    via: "conversation",
    author_kind: "agent",
    author: { kind: "agent", agent: { id: "a1", name: "Daneel" }, conversation: { id: "c2", title: "Release prep" } },
    created_at: ago(DAY),
  }),
  rev(1, { content: V1 }),
];

const memory = {
  key: "aurora-migration",
  title: "Aurora migration project",
  description: null,
  content: V3,
  index_line: "aurora-migration: Aurora",
  revision: 3,
  created_at: ago(9 * DAY),
  updated_at: ago(2 * MIN),
  locked: false,
  locked_at: null,
  locked_by: null,
  added_by: { kind: "person", person: bob },
  changed_by: { kind: "person", person: bob },
  last_used: null,
};

const BASE = "/v1/agents/a1/memories/aurora-migration";

function serve(revs: MemoryRevision[] = REVS, extra: Record<string, Handler> = {}, current: Record<string, unknown> = {}) {
  const answer = mockFetch({
    "GET /v1/me": () => json(200, { user: { id: bob.id, email: "bob@example.test", display_name: bob.display_name } }),
    "GET /v1/agents/a1/memories": () => json(200, { items: [{ ...memory, revision: revs[0]!.revision, ...current }], next_cursor: null }),
    [`GET ${BASE}/revisions`]: () => json(200, { items: revs, next_cursor: null }),
    ...extra,
  });
  vi.stubGlobal("fetch", (input: Request | string, init?: RequestInit) =>
    answer(typeof input === "string" ? new Request(new URL(input, "http://localhost"), { method: init?.method }) : input),
  );
}

const page = (props: { readOnly?: boolean } = {}) =>
  wrap(
    <>
      <MemoryTab agentId="a1" agentName="Daneel" {...props} />
      <UndoToastHost />
    </>,
  );

const history = () => screen.findByRole("article", { name: "History" });
const versions = async () => within(await within(await history()).findByRole("list", { name: "Versions" })).getAllByRole("button");
const open = async () => {
  page();
  return history();
};
const menu = async () => fireEvent.keyDown(within(await history()).getByRole("button", { name: "More actions" }), { key: "Enter" });

beforeEach(() => window.history.replaceState(null, "", "/o/acme/w/main/agents/daneel?tab=memory&memory=aurora-migration&history=1"));
afterEach(() => dismissUndoToast());

describe("History replaces the reading view", () => {
  it("opens from the … menu in place of the memory, and goes back; the address says so", async () => {
    window.history.replaceState(null, "", "/o/acme/w/main/agents/daneel?tab=memory");
    serve();
    page();
    fireEvent.keyDown(await screen.findByRole("button", { name: "More actions" }), { key: "Enter" });
    fireEvent.click(await screen.findByRole("menuitem", { name: "History" }));
    const panel = await history();
    expect(screen.queryByRole("article", { name: "Aurora migration project" })).toBeNull();
    expect(window.location.search).toBe("?tab=memory&memory=aurora-migration&history=1");
    expect(await within(panel).findByText("Aurora migration project · 3 versions")).toBeInTheDocument();
    fireEvent.click(within(panel).getByRole("button", { name: "Back to the memory" }));
    expect(await screen.findByRole("article", { name: "Aurora migration project" })).toBeInTheDocument();
    expect(screen.queryByRole("article", { name: "History" })).toBeNull();
    expect(window.location.search).toBe("?tab=memory&memory=aurora-migration");
  });

  it("names people and context newest first, never an e-mail, with Current on the newest, which is selected", async () => {
    serve();
    const panel = await open();
    const rows = await versions();
    expect(rows).toHaveLength(3);
    expect(rows[0]).toHaveTextContent(/Bob Builder changed it · 2 min ago.*Current.*through an API key/);
    expect(rows[1]).toHaveTextContent(/Daneel changed it · .*in “Release prep”/);
    expect(rows[2]).toHaveTextContent(/Bob Builder created it · /);
    expect(rows[0]).toHaveAttribute("aria-current", "true");
    expect(within(panel).getAllByText("Current")).toHaveLength(1);
    expect(panel.textContent).not.toContain("@");
    expect(within(panel).queryAllByRole("link")).toHaveLength(0);
  });

  it.each<[string, Partial<MemoryRevision>, RegExp]>([
    ["an unreadable conversation", { via: "conversation", author: { kind: "agent", agent: { id: "a1", name: "Daneel" }, conversation: null } }, /Daneel changed it · .*in a conversation$/],
    ["an untitled conversation", { via: "conversation", author: { kind: "agent", agent: { id: "a1", name: "Daneel" }, conversation: { id: "c", title: null } } }, /in a conversation$/],
    ["a file the agent wrote", { via: "harvest", author: { kind: "agent", agent: { id: "a1", name: "Daneel" } } }, /^D?Daneel changed it · [^a]*(ago|\d\d:\d\d)$/],
    ["a key without a person", { via: "api", author: { kind: "api_key", api_key: { id: "k", name: "deploy bot" } } }, /deploy bot changed it · .*through an API key$/],
    ["a template", { via: "template", author: { kind: "template", person: bob, template: { name: "Release helper", version: "2" } } }, /Bob Builder changed it · .*from the template Release helper 2$/],
    ["a template with no person", { via: "template", author: { kind: "template", template: { name: "Release helper", version: "2" } } }, /Release helper changed it · .*from the template Release helper 2$/],
    ["a restore", { via: "restore", restored_from: 1 }, /Bob Builder restored version 1 · /],
    ["an unknown writer", { author: { kind: "unknown" } }, /Someone changed it · /],
    ["removed text", { redacted: true, content: null }, /Bob Builder changed it · .*its text was removed$/],
  ])("words a version for %s", async (_what, over, words) => {
    serve([rev(5), rev(4, over)]);
    await open();
    const rows = await versions();
    expect(rows[1]!.textContent!.slice(1)).toMatch(words);
  });

  it("only revision 1 is 'created it': an older version that is the oldest kept 'changed it'", async () => {
    serve([rev(9), rev(8)]);
    const panel = await open();
    const rows = await versions();
    expect(rows[1]).toHaveTextContent("changed it");
    fireEvent.click(rows[1]!);
    expect(await within(panel).findByText("The version before is no longer kept, so there is nothing to compare with.")).toBeInTheDocument();
    expect(within(panel).queryByTestId("diff-view")).toBeNull();
  });
});

describe("What changed", () => {
  it("compares the newest with the one before in the shared DiffView, with the counts", async () => {
    serve();
    const panel = await open();
    expect(await within(panel).findByText(`Compared with the version before (Daneel, ${whenInSentence(REVS[1]!.created_at)})`)).toBeInTheDocument();
    const diff = within(panel).getByTestId("diff-view");
    expect(diff.querySelectorAll('[data-diff="add"]')).toHaveLength(3);
    expect(diff.querySelectorAll('[data-diff="del"]')).toHaveLength(2);
    expect(within(panel).getByTestId("diff-summary")).toHaveTextContent("+3 added · −2 removed in this version");
  });

  it("compares a middle version with the one before, and the first with nothing", async () => {
    serve();
    const panel = await open();
    fireEvent.click((await versions())[1]!);
    await waitFor(() => expect(within(panel).getByTestId("diff-summary")).toHaveTextContent("+1 added · −0 removed in this version"));
    expect(within(panel).getByText(`Compared with the version before (Bob Builder, ${whenInSentence(REVS[2]!.created_at)})`)).toBeInTheDocument();
    fireEvent.click((await versions())[2]!);
    await waitFor(() => expect(within(panel).getByTestId("diff-summary")).toHaveTextContent("+3 added · −0 removed in this version"));
    expect(within(panel).getByText("The first version")).toBeInTheDocument();
  });

  it.each([
    ["the selected version", 1],
    ["the version before", 0],
  ])("shows no diff when the text of %s was removed", async (_which, selected) => {
    serve([REVS[0]!, { ...REVS[1]!, redacted: true, content: null }, REVS[2]!]);
    const panel = await open();
    fireEvent.click((await versions())[selected]!);
    expect(await within(panel).findByText("This version's text was removed")).toBeInTheDocument();
    expect(within(panel).queryByTestId("diff-view")).toBeNull();
    expect(within(panel).queryByTestId("diff-summary")).toBeNull();
  });
});

describe("Restore this version", () => {
  it("is not offered on the current version, or on a version whose text was removed", async () => {
    serve([REVS[0]!, { ...REVS[1]!, redacted: true, content: null }, REVS[2]!]);
    const panel = await open();
    await versions();
    expect(within(panel).queryByRole("button", { name: "Restore this version" })).toBeNull();
    fireEvent.click((await versions())[1]!);
    expect(within(panel).queryByRole("button", { name: "Restore this version" })).toBeNull();
    expect(within(panel).queryByRole("button", { name: "More actions" })).toBeNull();
  });

  it("restores, returns to the memory, and Undo restores the one that was current", async () => {
    const restored: unknown[] = [];
    serve(REVS, {
      [`POST ${BASE}/restore`]: async (r) => {
        restored.push(await r.json());
        return json(200, memory);
      },
    });
    const panel = await open();
    fireEvent.click((await versions())[1]!);
    fireEvent.click(within(panel).getByRole("button", { name: "Restore this version" }));
    expect(await screen.findByRole("article", { name: "Aurora migration project" })).toBeInTheDocument();
    expect(screen.queryByRole("article", { name: "History" })).toBeNull();
    expect(window.location.search).toBe("?tab=memory&memory=aurora-migration");
    const toast = await screen.findByTestId("undo-toast");
    expect(toast).toHaveTextContent("Restored version 2");
    expect(restored).toEqual([{ revision: 2 }]);
    fireEvent.click(within(toast).getByRole("button", { name: "Undo" }));
    await waitFor(() => expect(restored).toEqual([{ revision: 2 }, { revision: 3 }]));
  });

  it("stays on History and says why when the restore fails; a failed Undo says so too", async () => {
    let fail = true;
    serve(REVS, { [`POST ${BASE}/restore`]: () => (fail ? err(404, "not_found", "Revision not found.") : json(200, memory)) });
    const panel = await open();
    fireEvent.click((await versions())[1]!);
    fireEvent.click(within(panel).getByRole("button", { name: "Restore this version" }));
    expect(await within(panel).findByRole("alert")).toBeInTheDocument();
    expect(screen.queryByTestId("undo-toast")).toBeNull();
    fail = false;
    fireEvent.click(within(panel).getByRole("button", { name: "Restore this version" }));
    const toast = await screen.findByTestId("undo-toast");
    fail = true;
    fireEvent.click(within(toast).getByRole("button", { name: "Undo" }));
    await waitFor(() => expect(screen.getByTestId("undo-toast")).toHaveTextContent("Couldn't undo the restore"));
  });
});

describe("Remove this version's text", () => {
  it("is disabled on the current version, with the reason", async () => {
    serve();
    await open();
    await versions();
    await menu();
    const item = await screen.findByRole("menuitem", { name: /Remove this version's text…/ });
    expect(item).toHaveAttribute("aria-disabled", "true");
    expect(item).toHaveTextContent("The current version can't be removed: change the memory first.");
  });

  it("asks twice, then removes the text of that version", async () => {
    let redacted = 0;
    serve(REVS, {
      [`POST ${BASE}/revisions/2/redact`]: () => {
        redacted += 1;
        return json(204, null);
      },
    });
    await open();
    fireEvent.click((await versions())[1]!);
    await menu();
    fireEvent.click(await screen.findByRole("menuitem", { name: "Remove this version's text…" }));
    const dialog = await screen.findByRole("dialog", { name: "Remove the text of version 2?" });
    expect(dialog).toHaveTextContent("This can't be undone.");
    expect(redacted).toBe(0);
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove text" }));
    await waitFor(() => expect(redacted).toBe(1));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });
});

describe("who sees History", () => {
  it("an emergency reader's deep link shows the memory, and no history is asked for", async () => {
    const asked = vi.fn();
    serve(REVS, { [`GET ${BASE}/revisions`]: () => { asked(); return json(403, {}); } });
    page({ readOnly: true });
    expect(await screen.findByRole("article", { name: "Aurora migration project" })).toBeInTheDocument();
    expect(screen.queryByRole("article", { name: "History" })).toBeNull();
    expect(asked).not.toHaveBeenCalled();
  });

  it("on a narrow screen it is its own screen, with the way back to the memory", async () => {
    vi.stubGlobal("matchMedia", (query: string) => ({ matches: query.includes("max-width"), media: query, addEventListener: () => undefined, removeEventListener: () => undefined }));
    vi.stubGlobal("scrollTo", () => undefined);
    serve();
    await open();
    expect(screen.queryByRole("list", { name: "Memories" })).toBeNull();
    // The way back is the sub-screen's own header, above the panel.
    fireEvent.click(screen.getByRole("button", { name: "Back to the memory" }));
    expect(await screen.findByRole("article", { name: "Aurora migration project" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "History" }));
    expect(await history()).toBeInTheDocument();
    vi.unstubAllGlobals();
  });
});
