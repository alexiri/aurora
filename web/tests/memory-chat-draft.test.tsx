import { fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { MemoryTab } from "@/components/memory/memory-tab";
import { clearMemoryDraft, peekMemoryDraft, setMemoryDraft } from "@/lib/chat/memory-draft";

import { json, mockFetch, type Handler } from "./support/api";
import { wrap } from "./support/render";

// (P5-MEMORY-4, mem-5) What the Memory page does with the draft the chat's "Save it yourself" hands over: it opens the
// form with the text, unsaved. Without a draft the page is as it was.

vi.mock("next/navigation", () => ({
  useParams: () => ({ org: "acme", ws: "main" }),
  usePathname: () => "/o/acme/w/main/agents/daneel",
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useSearchParams: () => new URLSearchParams(window.location.search),
}));

const at = "2026-11-08T12:00:00Z";
const jira = {
  key: "jira-project-keys", title: "Jira project keys", description: null, content: "OPS is the ops board.", index_line: "", revision: 1,
  created_at: at, updated_at: at, locked: true, locked_at: at, locked_by: null, added_by: { kind: "unknown" }, changed_by: { kind: "unknown" }, last_used: null,
};
const ADDED = "PLAT is now the key for the platform team's board.";

let sent: { method: string; path: string; body: unknown }[] = [];
function serve(items: unknown[]) {
  sent = [];
  const note: Handler = async (req) => {
    sent.push({ method: req.method, path: new URL(req.url).pathname, body: await req.json() });
    return json(200, jira);
  };
  const answer = mockFetch({
    "GET /v1/me": () => json(200, { user: { id: "u", email: "bob@example.test", display_name: "Bob" }, orgs: [], is_instance_admin: false }),
    "GET /v1/agents/a1/memories": () => json(200, { items, next_cursor: null }),
    "POST /v1/agents/a1/memories": note,
    "PUT /v1/agents/a1/memories/jira-project-keys": note,
  });
  vi.stubGlobal("fetch", (input: Request | string, init?: RequestInit) =>
    answer(typeof input === "string" ? new Request(new URL(input, "http://localhost"), { method: init?.method }) : input),
  );
}

const page = () => wrap(<MemoryTab agentId="a1" agentName="Daneel" />);
const field = async (label: string | RegExp) => (await screen.findByLabelText(label)) as HTMLInputElement | HTMLTextAreaElement;
const text = () => field("What should Daneel remember?");

beforeEach(() => {
  clearMemoryDraft();
  window.history.replaceState(null, "", "/o/acme/w/main/agents/daneel?tab=memory");
  window.localStorage.clear();
  window.sessionStorage.clear();
});
afterEach(() => clearMemoryDraft());

describe("a draft from the chat", () => {
  it("opens the new-memory form with the free key, the title and the text, not saved yet", async () => {
    setMemoryDraft("a1", { kind: "new", key: "platform-board", title: "Platform board", content: ADDED });
    serve([jira]);
    page();
    expect((await field("Title")).value).toBe("Platform board");
    expect((await field("Key")).value).toBe("platform-board");
    expect((await text()).value).toBe(ADDED);
    expect(screen.getByText("New memory · not saved yet")).toBeTruthy();
    // Used once; it is in the page now, and never in the address or in storage.
    expect(peekMemoryDraft("a1")).toBeNull();
    expect(window.location.href).not.toContain("PLAT");
    expect(window.localStorage.length + window.sessionStorage.length).toBe(0);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0]!.body).toMatchObject({ key: "platform-board", title: "Platform board", content: ADDED });
  });

  it("makes a key from the title when the draft's key is taken, or has none", async () => {
    setMemoryDraft("a1", { kind: "new", key: "jira-project-keys", title: "Platform board", content: ADDED });
    serve([jira]);
    page();
    expect((await field("Key")).value).toBe("platform-board");
  });

  it("opens the locked memory in edit mode with the text appended as an unsaved change", async () => {
    window.history.replaceState(null, "", "/o/acme/w/main/agents/daneel?tab=memory&memory=jira-project-keys");
    setMemoryDraft("a1", { kind: "edit", key: "jira-project-keys", content: `${jira.content}\n\n${ADDED}` });
    serve([jira]);
    page();
    expect(await screen.findByRole("form", { name: "Edit Jira project keys" })).toBeTruthy();
    expect((await text()).value).toBe(`OPS is the ops board.\n\n${ADDED}`);
    expect(screen.getAllByText("Unsaved changes").length).toBeGreaterThan(0);
    expect(screen.getByRole("button", { name: "Save" })).toBeTruthy();
    expect(sent).toEqual([]);
  });

  it("opens the new form when the memory to edit is gone", async () => {
    setMemoryDraft("a1", { kind: "edit", key: "jira-project-keys", content: ADDED });
    serve([]);
    page();
    expect(await screen.findByRole("form", { name: "New memory" })).toBeTruthy();
    expect((await field("Key")).value).toBe("jira-project-keys");
    expect((await text()).value).toBe(ADDED);
  });

  it("is left alone by another agent's page, which is as it always was", async () => {
    setMemoryDraft("another-agent", { kind: "new", key: "k", title: "T", content: ADDED });
    serve([jira]);
    page();
    expect(await screen.findByRole("article", { name: "Jira project keys" })).toBeTruthy();
    expect(screen.queryByRole("form")).toBeNull();
    expect(peekMemoryDraft("another-agent")).not.toBeNull();
  });
});
