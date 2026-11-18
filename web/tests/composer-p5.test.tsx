import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { Composer, pastedName } from "@/components/chat/composer";
import { clearDrafts, draftKey, loadDraft, saveDraft } from "@/components/chat/composer/drafts";
import type { TranscriptState } from "@/lib/chat/transcript";

// P5-CHAT-3: formatting, shortcuts, fence-aware Enter, list continuation, Preview, emoji, files-only send, drafts.

const json = (status: number, body: unknown) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
const state = { events: [], deltas: {}, activity: {}, purged: {}, lastSeq: 0 } as unknown as TranscriptState;
const ATT = {
  id: "att1",
  filename: "a.txt",
  size_bytes: 3,
  content_type: "text/plain",
  conversation_id: "c1",
  created_at: "2026-11-14T10:00:00Z",
  direction: "inbound",
  status: "pending",
};

let sent: Array<{ text: string; attachment_ids?: string[] }>;
function mock(opts: { uploads?: boolean } = {}) {
  sent = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const path = new URL(req.url, "http://localhost").pathname;
      if (path === "/v1/me") return json(200, { user: { id: "u1", display_name: "Ada", email: "a@x.test" } });
      if (req.method === "POST" && path === "/v1/conversations/c1/messages") {
        sent.push(JSON.parse(await req.clone().text()));
        return json(202, { turn: { id: "t1" }, followup: null });
      }
      if (req.method === "POST" && path === "/v1/conversations/c1/attachments" && opts.uploads !== false) return json(201, ATT);
      return json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
}

function mount(props: Partial<React.ComponentProps<typeof Composer>> = {}) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const ui = (
    <QueryClientProvider client={qc}>
      <Composer agentId="a1" convId="c1" base="/o/x/w/y" agentName="Daneel" activeTurn={null} state={state} {...props} />
    </QueryClientProvider>
  );
  const r = render(ui);
  return { box: screen.getByLabelText("Message") as HTMLTextAreaElement, ...r, ui };
}

function type(box: HTMLTextAreaElement, value: string, at = value.length, to = at) {
  fireEvent.change(box, { target: { value } });
  box.setSelectionRange(at, to);
  fireEvent.select(box);
}

beforeEach(() => {
  window.localStorage.clear();
  mock();
});
afterEach(() => {
  vi.restoreAllMocks();
});

describe("shortcuts and the formatting row", () => {
  it("Ctrl+B wraps the selection and keeps it selected (works with the row hidden)", () => {
    const { box } = mount();
    type(box, "say hello now", 4, 9);
    fireEvent.keyDown(box, { key: "b", ctrlKey: true });
    expect(box.value).toBe("say **hello** now");
    expect(box.value.slice(box.selectionStart, box.selectionEnd)).toBe("hello");
  });

  it("the toolbar buttons name their shortcut and apply the same edit", () => {
    const { box } = mount();
    type(box, "x", 1);
    const bold = screen.getByRole("button", { name: "Bold (Ctrl+B)" });
    expect(bold.getAttribute("aria-keyshortcuts")).toBe("Control+B");
    fireEvent.click(bold);
    expect(box.value).toBe("x**bold text**");
    expect(screen.getByRole("toolbar", { name: "Formatting" })).toBeTruthy();
  });

  it("`Aa` is a toggle that remembers its state, and storage failures are ignored", () => {
    const { box } = mount();
    const aa = screen.getByRole("button", { name: "Formatting" });
    // (jsdom has no media query: the unset default is "hidden", as on a phone)
    expect(aa.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(aa);
    expect(window.localStorage.getItem("aurora:chat-formatting")).toBe("1");
    expect(aa.getAttribute("aria-pressed")).toBe("true");
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("quota");
    });
    fireEvent.click(aa);
    expect(aa.getAttribute("aria-pressed")).toBe("false");
    type(box, "still works", 11);
    expect(box.value).toBe("still works");
  });

  it("the toolbar roves focus with the arrow keys", () => {
    mount();
    const bold = screen.getByRole("button", { name: "Bold (Ctrl+B)" });
    bold.focus();
    fireEvent.keyDown(bold, { key: "ArrowRight" });
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Italic (Ctrl+I)" }));
    expect(screen.getByRole("button", { name: "Italic (Ctrl+I)" }).tabIndex).toBe(0);
    expect(bold.tabIndex).toBe(-1);
  });
});

describe("Enter", () => {
  it("sends on Enter; Shift+Enter does not", async () => {
    const { box } = mount();
    type(box, "hello");
    fireEvent.keyDown(box, { key: "Enter", shiftKey: true });
    expect(sent).toHaveLength(0);
    fireEvent.keyDown(box, { key: "Enter" });
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0].text).toBe("hello");
  });

  it("inside an unclosed fence Enter is a new line (the hint says so) and Ctrl+Enter sends", async () => {
    const { box } = mount();
    type(box, "```\ncode");
    const ev = fireEvent.keyDown(box, { key: "Enter" });
    expect(ev).toBe(true); // not default-prevented: the browser inserts the line
    expect(sent).toHaveLength(0);
    expect(screen.getByTestId("composer-hint").textContent).toContain("Enter for a new line");
    fireEvent.keyDown(box, { key: "Enter", ctrlKey: true });
    await waitFor(() => expect(sent).toHaveLength(1));
  });

  it("Shift+Enter continues a list and ends it on an empty item", () => {
    const { box } = mount();
    type(box, "- one");
    fireEvent.keyDown(box, { key: "Enter", shiftKey: true });
    expect(box.value).toBe("- one\n- ");
    box.setSelectionRange(box.value.length, box.value.length);
    fireEvent.keyDown(box, { key: "Enter", shiftKey: true });
    expect(box.value).toBe("- one\n");
  });

  it("an IME composition keeps Enter", () => {
    const { box } = mount();
    type(box, "かな");
    fireEvent.keyDown(box, { key: "Enter", isComposing: true });
    expect(sent).toHaveLength(0);
  });
});

describe("Write / Preview", () => {
  it("Preview renders the draft with the chat renderer, disables the buttons, and Write returns", () => {
    const { box } = mount();
    type(box, "**bold** and `code`");
    fireEvent.click(screen.getByRole("tab", { name: "Preview" }));
    const panel = screen.getByRole("tabpanel", { name: "Preview" });
    expect(panel.querySelector("strong")?.textContent).toBe("bold");
    expect((screen.getByRole("button", { name: "Bold (Ctrl+B)" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Send" }) as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(screen.getByRole("tab", { name: "Write" }));
    expect(box.value).toBe("**bold** and `code`");
  });
});

describe("emoji", () => {
  it("`:` and letters open a listbox; the arrows move and Enter inserts the character", async () => {
    const { box } = mount();
    type(box, "hi :tad");
    const list = await screen.findByRole("listbox", { name: "Emoji suggestions" });
    expect(box.getAttribute("aria-controls")).toBe(list.id);
    expect(box.getAttribute("aria-activedescendant")).toBe(list.querySelector('[aria-selected="true"]')?.id);
    fireEvent.keyDown(box, { key: "Enter" });
    expect(box.value).toBe("hi 🎉");
    expect(sent).toHaveLength(0);
    await waitFor(() => expect(screen.queryByRole("listbox")).toBeNull());
  });

  it("Esc closes the list without touching the text", async () => {
    const { box } = mount();
    type(box, ":tad");
    await screen.findByRole("listbox");
    fireEvent.keyDown(box, { key: "Escape" });
    expect(screen.queryByRole("listbox")).toBeNull();
    expect(box.value).toBe(":tad");
  });
});

describe("files", () => {
  it("a file alone is enough to send (C-726) and the chip is removable", async () => {
    const { container } = mount();
    const send = screen.getByRole("button", { name: "Send" }) as HTMLButtonElement;
    expect(send.disabled).toBe(true);
    const input = container.querySelector('input[type="file"]') as HTMLInputElement;
    fireEvent.change(input, { target: { files: [new File(["abc"], "a.txt", { type: "text/plain" })] } });
    await screen.findByRole("list", { name: "Pending attachments" });
    await waitFor(() => expect(send.disabled).toBe(false));
    fireEvent.click(send);
    await waitFor(() => expect(sent).toHaveLength(1));
    expect(sent[0]).toMatchObject({ text: "", attachment_ids: ["att1"] });
  });

  it("the drop zone's handler uploads several files; a pasted image gets a dated name", async () => {
    const attachRef = { current: null } as React.MutableRefObject<((f: File[]) => void) | null>;
    const { box } = mount({ attachRef });
    attachRef.current?.([new File(["1"], "a.txt"), new File(["2"], "b.txt")]);
    await waitFor(() => expect(screen.getAllByRole("listitem")).toHaveLength(2));
    const png = new File(["x"], "image.png", { type: "image/png" });
    fireEvent.paste(box, { clipboardData: { files: [png], getData: () => "" } });
    await waitFor(() => expect(screen.getAllByRole("listitem")).toHaveLength(3));
    expect(pastedName(png, new Date(2026, 10, 14, 23, 37, 9))).toBe("Pasted image 2026-11-14 23.37.09.png");
  });

  it("smart paste: a URL over a selection becomes a link, spreadsheet rows a table", () => {
    const { box } = mount();
    type(box, "see docs", 4, 8);
    fireEvent.paste(box, { clipboardData: { files: [], getData: () => "https://example.test" } });
    expect(box.value).toBe("see [docs](https://example.test)");
    type(box, "", 0);
    fireEvent.paste(box, { clipboardData: { files: [], getData: () => "a\tb\n1\t2" } });
    expect(box.value).toBe("| a | b |\n| --- | --- |\n| 1 | 2 |");
  });
});

describe("drafts", () => {
  it("restores the text and attachments after a remount, keyed by user and conversation, and clears on send", async () => {
    const first = mount();
    type(first.box, "unfinished thought");
    await waitFor(() => expect(loadDraft("u1", "c1")?.text).toBe("unfinished thought"));
    first.unmount();
    expect(loadDraft("u2", "c1")).toBeNull();
    const second = mount();
    await waitFor(() => expect(second.box.value).toBe("unfinished thought"));
    fireEvent.keyDown(second.box, { key: "Enter" });
    await waitFor(() => expect(loadDraft("u1", "c1")).toBeNull());
  });

  it("clearDrafts() on sign-out removes every user's drafts but not other keys", () => {
    saveDraft("u1", "c1", { text: "a", reply: null, attachments: [] });
    saveDraft("u2", "c9", { text: "b", reply: null, attachments: [] });
    window.localStorage.setItem("other", "keep");
    clearDrafts();
    expect(window.localStorage.getItem(draftKey("u1", "c1"))).toBeNull();
    expect(window.localStorage.getItem(draftKey("u2", "c9"))).toBeNull();
    expect(window.localStorage.getItem("other")).toBe("keep");
  });

  it("works when storage throws", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(loadDraft("u1", "c1")).toBeNull();
    expect(() => saveDraft("u1", "c1", { text: "x", reply: null, attachments: [] })).not.toThrow();
    const { box } = mount();
    type(box, "typing anyway");
    expect(box.value).toBe("typing anyway");
  });

  it("restored attachments the API no longer knows are dropped with a notice, the text kept", async () => {
    saveDraft("u1", "c1", { text: "keep me", reply: null, attachments: [ATT] });
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: Request | string | URL, init?: RequestInit) => {
        const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
        const path = new URL(req.url, "http://localhost").pathname;
        if (path === "/v1/me") return json(200, { user: { id: "u1", display_name: "Ada", email: "a@x.test" } });
        return json(422, { error: { code: "validation_failed", message: "bad", details: { fields: [{ pointer: "/attachment_ids", message: "unknown" }] } } });
      }),
    );
    const { box } = mount();
    await waitFor(() => expect(box.value).toBe("keep me"));
    await screen.findByRole("list", { name: "Pending attachments" });
    fireEvent.keyDown(box, { key: "Enter" });
    await screen.findByText(/1 attached file expired/);
    expect(screen.queryByRole("list", { name: "Pending attachments" })).toBeNull();
    expect(box.value).toBe("keep me");
  });
});
