import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { EditorView } from "@codemirror/view";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { FilesTab } from "@/components/agents/files-tab";
import { FileViewer } from "@/components/agents/files/viewer/file-viewer";
import { imageDataUrl, sniffImage, typeGuess } from "@/components/agents/files/viewer/image";
import { filesTiming, type FileEntry } from "@/lib/api/files";
import { json } from "./support/api";

// P5-FILES-3 (docs/14 §Files item 6, C-905, SEC-126): the viewer against a mocked API — the dirty marker and guard, Save
// with If-Match, the conflict dialog's three actions, CRLF kept, New file with If-None-Match: *, the image magic-byte
// check, `file_not_editable` in words, and read-only mode. The editor is the real CodeMirror (driven through its view).

const err = (status: number, code: string, details?: unknown) => json(status, { error: { code, message: "server words", details } });

const PNG = Uint8Array.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13]);
const bytesResponse = (b: Uint8Array) => new Response(b as BlobPart, { status: 200, headers: { "content-type": "application/octet-stream", "content-length": String(b.length) } });
const enc = (s: string) => new TextEncoder().encode(s);

const textBody = (content: string, over: Record<string, unknown> = {}) => ({
  path: "/workspace/a.rs",
  content,
  etag: "sha256:aaa",
  size: content.length,
  modified_at: null,
  line_ending: "lf",
  ...over,
});

interface Seen {
  method: string;
  path: string;
  query: URLSearchParams;
}
function mockApi(handler: (req: Seen) => Response | undefined) {
  const seen: Seen[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
      const url = new URL(req.url);
      const s: Seen = { method: req.method, path: url.pathname, query: url.searchParams };
      seen.push(s);
      return handler(s) ?? err(404, "not_found");
    }),
  );
  return seen;
}

interface XhrRecord {
  method: string;
  url: string;
  headers: Record<string, string>;
  body: unknown;
  respond: (status: number, body: unknown) => void;
}
function stubXhr() {
  const all: XhrRecord[] = [];
  class FakeXhr {
    upload = { onprogress: null };
    onload: (() => void) | null = null;
    onerror: (() => void) | null = null;
    onabort: (() => void) | null = null;
    status = 0;
    responseText = "";
    withCredentials = false;
    private rec: XhrRecord = { method: "", url: "", headers: {}, body: null, respond: () => undefined };
    open(method: string, url: string) {
      this.rec.method = method;
      this.rec.url = url;
    }
    setRequestHeader(k: string, v: string) {
      this.rec.headers[k.toLowerCase()] = v;
    }
    send(body: unknown) {
      this.rec.body = body;
      this.rec.respond = (status, b) => {
        this.status = status;
        this.responseText = JSON.stringify(b);
        this.onload?.();
      };
      all.push(this.rec);
    }
    abort() {
      this.onabort?.();
    }
  }
  vi.stubGlobal("XMLHttpRequest", FakeXhr);
  return all;
}

const entry = (name: string, over: Partial<FileEntry> = {}): FileEntry => ({
  name,
  path: `/workspace/${name}`,
  kind: "file",
  size: 20,
  mode: "0644",
  modified_at: "2026-11-15T10:00:00Z",
  writable: true,
  ...over,
});

function qc() {
  return new QueryClient({ defaultOptions: { queries: { retry: false } } });
}
function renderViewer(props: Partial<React.ComponentProps<typeof FileViewer>> = {}) {
  const onBack = vi.fn();
  const onDirtyChange = vi.fn();
  render(
    <QueryClientProvider client={qc()}>
      <FileViewer agent="a1" path="/workspace/a.rs" onBack={onBack} onDirtyChange={onDirtyChange} {...props} />
    </QueryClientProvider>,
  );
  return { onBack, onDirtyChange };
}

/** The CodeMirror view behind the editor, once the lazy chunk has loaded. */
async function editor(): Promise<EditorView> {
  const dom = await screen.findByTestId("code-editor");
  const content = await waitFor(() => {
    const el = dom.querySelector<HTMLElement>(".cm-content");
    if (!el) throw new Error("editor not ready");
    return el;
  });
  const view = EditorView.findFromDOM(dom);
  expect(view).not.toBeNull();
  expect(content).toBeTruthy();
  return view!;
}
// Appends text through the view; `act` lets React take the editor's onChange before the test goes on.
const type = (view: EditorView, text: string) => act(async () => view.dispatch({ changes: { from: view.state.doc.length, insert: text } }));
const docText = (view: EditorView) => view.state.doc.sliceString(0, view.state.doc.length, view.state.lineBreak);

// jsdom has no layout: CodeMirror measures text through Range rects.
beforeAll(() => {
  const none = { length: 0, item: () => null, [Symbol.iterator]: function* () {} } as unknown as DOMRectList;
  Range.prototype.getClientRects = () => none;
  Range.prototype.getBoundingClientRect = () => new DOMRect();
});
beforeEach(() => {
  window.history.replaceState(null, "", "/o/acme/w/main/agents/scout?tab=files");
  filesTiming.retryDelayMs = 5;
});
afterEach(() => {
  filesTiming.retryDelayMs = 2000;
});

const TEXT_ROUTE = "/v1/agents/a1/files/text";

describe("editor", () => {
  it("opens the text, marks unsaved changes and saves with If-Match, keeping the new etag", async () => {
    const xhrs = stubXhr();
    const seen = mockApi((r) => (r.path === TEXT_ROUTE ? json(200, textBody("fn main() {}\n")) : undefined));
    const { onDirtyChange } = renderViewer();
    const view = await editor();
    expect(seen[0]!.query.get("path")).toBe("/workspace/a.rs");
    expect(docText(view)).toBe("fn main() {}\n");
    const save = screen.getByRole("button", { name: "Save" });
    expect(save).toBeDisabled();
    expect(screen.queryByText("Unsaved changes")).toBeNull();

    await type(view, "// more\n");
    expect(await screen.findByText("Unsaved changes")).toBeInTheDocument();
    expect(onDirtyChange).toHaveBeenLastCalledWith(true);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(xhrs).toHaveLength(1));
    expect(xhrs[0]).toMatchObject({ method: "PUT", url: "/v1/agents/a1/files/content?path=%2Fworkspace%2Fa.rs", body: "fn main() {}\n// more\n" });
    expect(xhrs[0]!.headers["if-match"]).toBe('"sha256:aaa"');
    expect(xhrs[0]!.headers["if-none-match"]).toBeUndefined();
    xhrs[0]!.respond(200, { entry: entry("a.rs"), etag: "sha256:bbb" });
    await waitFor(() => expect(screen.queryByText("Unsaved changes")).toBeNull());
    expect(onDirtyChange).toHaveBeenLastCalledWith(false);

    await type(view, "x");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(xhrs).toHaveLength(2));
    expect(xhrs[1]!.headers["if-match"]).toBe('"sha256:bbb"');
  });

  it("Ctrl+S saves", async () => {
    const xhrs = stubXhr();
    mockApi((r) => (r.path === TEXT_ROUTE ? json(200, textBody("a")) : undefined));
    renderViewer();
    const view = await editor();
    await type(view, "b");
    fireEvent.keyDown(view.contentDOM, { key: "s", ctrlKey: true });
    await waitFor(() => expect(xhrs).toHaveLength(1));
    expect(xhrs[0]!.body).toBe("ab");
  });

  it("keeps CRLF line endings on save", async () => {
    const xhrs = stubXhr();
    mockApi((r) => (r.path === TEXT_ROUTE ? json(200, textBody("one\r\ntwo\r\n", { line_ending: "crlf" })) : undefined));
    renderViewer();
    const view = await editor();
    expect(view.state.doc.lines).toBe(3);
    await type(view, "three");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(xhrs).toHaveLength(1));
    expect(xhrs[0]!.body).toBe("one\r\ntwo\r\nthree");
  });

  it("a read-only file has a badge and no Save", async () => {
    mockApi((r) => (r.path === TEXT_ROUTE ? json(200, textBody("x")) : undefined));
    renderViewer({ readOnly: true });
    await editor();
    expect(screen.getByText("Read-only")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
  });

  it("Esc then Tab leaves the editor for the next control and says so", async () => {
    mockApi((r) => (r.path === TEXT_ROUTE ? json(200, textBody("x")) : undefined));
    renderViewer();
    const view = await editor();
    fireEvent.keyDown(view.contentDOM, { key: "Escape" });
    fireEvent.keyDown(view.contentDOM, { key: "Tab" });
    expect(screen.getByText("Left the editor.")).toBeInTheDocument();
    expect(document.activeElement).toBe(screen.getAllByRole("button", { name: "Back to folder" })[0]);
  });
});

describe("conflict", () => {
  async function conflicted() {
    const xhrs = stubXhr();
    mockApi((r) => (r.path === TEXT_ROUTE ? json(200, textBody("a")) : undefined));
    renderViewer();
    const view = await editor();
    await type(view, "b");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(xhrs).toHaveLength(1));
    xhrs[0]!.respond(409, { error: { code: "file_changed", message: "x", details: { current_etag: "sha256:zzz" } } });
    await screen.findByText("This file changed in the sandbox after you opened it.");
    return { xhrs, view };
  }

  it("Overwrite sends the same text without If-Match", async () => {
    const { xhrs } = await conflicted();
    fireEvent.click(screen.getByRole("button", { name: "Overwrite" }));
    await waitFor(() => expect(xhrs).toHaveLength(2));
    expect(xhrs[1]!.body).toBe("ab");
    expect(xhrs[1]!.headers["if-match"]).toBeUndefined();
    expect(xhrs[1]!.headers["if-none-match"]).toBeUndefined();
    xhrs[1]!.respond(200, { entry: entry("a.rs"), etag: "sha256:new" });
    await waitFor(() => expect(screen.queryByText("This file changed in the sandbox after you opened it.")).toBeNull());
    expect(screen.queryByText("Unsaved changes")).toBeNull();
  });

  it("Copy my text puts it on the clipboard and stays", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const { xhrs } = await conflicted();
    fireEvent.click(screen.getByRole("button", { name: "Copy my text" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("ab"));
    expect(await screen.findByText("Your text is on the clipboard.")).toBeInTheDocument();
    expect(xhrs).toHaveLength(1);
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();
  });

  it("Reload discards my changes and shows the file as it is now", async () => {
    let reads = 0;
    const xhrs = stubXhr();
    mockApi((r) => {
      if (r.path !== TEXT_ROUTE) return undefined;
      reads += 1;
      return json(200, reads === 1 ? textBody("a") : textBody("changed in the sandbox", { etag: "sha256:new" }));
    });
    renderViewer();
    const view = await editor();
    await type(view, "b");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(xhrs).toHaveLength(1));
    xhrs[0]!.respond(409, { error: { code: "file_changed", message: "x" } });
    fireEvent.click(await screen.findByRole("button", { name: "Reload" }));
    await waitFor(() => expect(reads).toBe(2));
    await waitFor(async () => expect(docText(await editor())).toBe("changed in the sandbox"));
    expect(screen.queryByText("Unsaved changes")).toBeNull();
    expect(xhrs).toHaveLength(1);
  });
});

describe("new file", () => {
  it("opens an empty editor and the first Save creates with If-None-Match: *", async () => {
    const xhrs = stubXhr();
    const seen = mockApi(() => undefined);
    renderViewer({ path: "/workspace/new.txt", isNew: true });
    const view = await editor();
    expect(seen).toHaveLength(0);
    expect(docText(view)).toBe("");
    await type(view, "hello");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(xhrs).toHaveLength(1));
    expect(xhrs[0]!.headers["if-none-match"]).toBe("*");
    expect(xhrs[0]!.headers["if-match"]).toBeUndefined();
    xhrs[0]!.respond(201, { entry: entry("new.txt"), etag: "sha256:n1" });
    await waitFor(() => expect(screen.queryByText("Unsaved changes")).toBeNull());
    await type(view, "!");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(xhrs).toHaveLength(2));
    expect(xhrs[1]!.headers["if-match"]).toBe('"sha256:n1"');
  });

  it("an existing name is an inline message", async () => {
    const xhrs = stubXhr();
    mockApi(() => undefined);
    renderViewer({ path: "/workspace/new.txt", isNew: true });
    const view = await editor();
    await type(view, "x");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(xhrs).toHaveLength(1));
    xhrs[0]!.respond(409, { error: { code: "file_exists", message: "x" } });
    expect(await screen.findByText(/A file with that name already exists/)).toBeInTheDocument();
  });
});

describe("not editable and images", () => {
  const notEditable = (reason: string) => err(422, "file_not_editable", { reason });

  it("says why a file cannot be edited, with Download, and never renders it", async () => {
    mockApi((r) => (r.path === TEXT_ROUTE ? notEditable("not_utf8") : r.path.endsWith("/content") ? bytesResponse(enc("<script>alert(1)</script>")) : undefined));
    renderViewer({ path: "/workspace/data.bin" });
    expect(await screen.findByText("No preview for this file")).toBeInTheDocument();
    expect(screen.getByText(/It is not UTF-8 text\./)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Download/ })).toHaveAttribute("href", "/v1/agents/a1/files/content?path=%2Fworkspace%2Fdata.bin");
    expect(document.querySelector("script")).toBeNull();
    expect(document.querySelector("img")).toBeNull();
  });

  it.each([
    ["too_large", /larger than 1 MiB/],
    ["binary", /binary file/],
  ])("the %s reason is in words", async (reason, words) => {
    mockApi((r) => (r.path === TEXT_ROUTE ? notEditable(reason) : r.path.endsWith("/content") ? bytesResponse(enc("zz")) : undefined));
    renderViewer({ path: "/workspace/x.dat" });
    expect(await screen.findByText(words)).toBeInTheDocument();
  });

  it("previews a PNG as a data: URL by its magic bytes (whatever its name)", async () => {
    mockApi((r) => (r.path === TEXT_ROUTE ? notEditable("binary") : r.path.endsWith("/content") ? bytesResponse(PNG) : undefined));
    renderViewer({ path: "/workspace/picture.txt" });
    const img = await screen.findByRole("img", { name: "picture.txt" });
    expect(img.getAttribute("src")).toMatch(/^data:image\/png;base64,/);
    expect(screen.queryByText("No preview for this file")).toBeNull();
  });

  it("refuses an SVG (markup, not a raster image) and an image over 10 MiB", async () => {
    mockApi((r) => (r.path === TEXT_ROUTE ? notEditable("binary") : r.path.endsWith("/content") ? bytesResponse(enc("<svg xmlns='http://www.w3.org/2000/svg'/>")) : undefined));
    renderViewer({ path: "/workspace/logo.svg" });
    expect(await screen.findByText("No preview for this file")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
    cleanup();
    const seen = mockApi((r) => (r.path === TEXT_ROUTE ? notEditable("too_large") : undefined));
    renderViewer({ path: "/workspace/huge.png", size: 11 * 1024 * 1024 });
    expect(await screen.findByText(/Images are previewed up to 10 MiB/)).toBeInTheDocument();
    expect(seen.some((s) => s.path.endsWith("/content"))).toBe(false);
  });

  it("sniffs only PNG, JPEG, GIF and WebP", () => {
    expect(sniffImage(PNG)).toBe("image/png");
    expect(sniffImage(Uint8Array.from([0xff, 0xd8, 0xff, 0xe0]))).toBe("image/jpeg");
    expect(sniffImage(enc("GIF89a....."))).toBe("image/gif");
    expect(sniffImage(enc("GIF87a....."))).toBe("image/gif");
    expect(sniffImage(enc("RIFF\0\0\0\0WEBPVP8 "))).toBe("image/webp");
    expect(sniffImage(enc("RIFF\0\0\0\0WAVEfmt "))).toBeNull();
    expect(sniffImage(enc("<svg xmlns='http://www.w3.org/2000/svg'/>"))).toBeNull();
    expect(sniffImage(enc("<html><body>x</body></html>"))).toBeNull();
    expect(imageDataUrl(PNG, "image/png")).toMatch(/^data:image\/png;base64,iVBORw0KGgo/);
    expect(typeGuess("a.svg")).toMatch(/SVG/);
    expect(typeGuess("Makefile")).toBe("File");
  });
});

describe("in the Files page", () => {
  const ROOT = [entry("a.rs"), entry("logo.svg"), entry("locked.txt", { writable: false })];
  const api = (handler?: (r: Seen) => Response | undefined) =>
    mockApi((r) => handler?.(r) ?? (r.path === "/v1/agents/a1/files" ? json(200, { path: "/workspace", entries: ROOT, truncated: false }) : undefined));
  function page() {
    return render(
      <QueryClientProvider client={qc()}>
        <FilesTab agentId="a1" frozen={false} />
      </QueryClientProvider>,
    );
  }

  it("asks before leaving a file with unsaved changes; Cancel keeps it, Discard leaves", async () => {
    stubXhr();
    api((r) => (r.path === TEXT_ROUTE ? json(200, textBody("a")) : undefined));
    page();
    fireEvent.click(await screen.findByRole("button", { name: "a.rs" }));
    const view = await editor();
    await type(view, "b");
    await screen.findByText("Unsaved changes");
    // The breadcrumb's "workspace" link goes back to the folder.
    fireEvent.click(screen.getByRole("link", { name: "workspace" }));
    expect(await screen.findByText("Discard your changes?")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByTestId("code-editor")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("link", { name: "workspace" }));
    fireEvent.click(await screen.findByRole("button", { name: "Discard changes" }));
    await screen.findByRole("button", { name: "logo.svg" });
    expect(screen.queryByTestId("code-editor")).toBeNull();
  });

  it("warns the browser before unloading with unsaved changes, and only then", async () => {
    stubXhr();
    api((r) => (r.path === TEXT_ROUTE ? json(200, textBody("a")) : undefined));
    page();
    fireEvent.click(await screen.findByRole("button", { name: "a.rs" }));
    const view = await editor();
    const clean = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(clean);
    expect(clean.defaultPrevented).toBe(false);
    await type(view, "b");
    await screen.findByText("Unsaved changes");
    const dirty = new Event("beforeunload", { cancelable: true });
    window.dispatchEvent(dirty);
    expect(dirty.defaultPrevented).toBe(true);
  });

  it("opens a non-writable entry read-only", async () => {
    api((r) => (r.path === TEXT_ROUTE ? json(200, textBody("x")) : undefined));
    page();
    fireEvent.click(await screen.findByRole("button", { name: "locked.txt" }));
    await editor();
    expect(screen.getByText("Read-only")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
  });

  it("New file opens an empty editor without creating anything first", async () => {
    const xhrs = stubXhr();
    const seen = api();
    page();
    await screen.findByRole("button", { name: "a.rs" });
    fireEvent.click(screen.getByRole("button", { name: "New file" }));
    fireEvent.change(await screen.findByLabelText("File name"), { target: { value: "notes.md" } });
    fireEvent.click(screen.getByRole("button", { name: "Create file" }));
    const view = await editor();
    expect(docText(view)).toBe("");
    expect(xhrs).toHaveLength(0);
    expect(seen.some((s) => s.method === "PUT" || s.path.endsWith("/text"))).toBe(false);
    await type(view, "# hi");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(xhrs).toHaveLength(1));
    expect(xhrs[0]!.url).toContain(encodeURIComponent("/workspace/notes.md"));
    expect(xhrs[0]!.headers["if-none-match"]).toBe("*");
  });
});

describe("bundle", () => {
  it("only the lazy editor module imports CodeMirror, so the Files listing's chunk does not carry it", () => {
    const root = join(process.cwd(), "src");
    const offenders: string[] = [];
    const walk = (dir: string) => {
      for (const f of readdirSync(dir)) {
        const p = join(dir, f);
        if (statSync(p).isDirectory()) walk(p);
        else if (/\.(tsx?|mts)$/.test(f) && /from\s+["'](@codemirror|@lezer)\//.test(readFileSync(p, "utf8")) && !p.endsWith(join("viewer", "editor.tsx"))) offenders.push(p);
      }
    };
    walk(root);
    expect(offenders).toEqual([]);
    expect(readFileSync(join(root, "components/agents/files/viewer/file-viewer.tsx"), "utf8")).toMatch(/dynamic\(\(\) => import\("\.\/editor"\)/);
  });
});
