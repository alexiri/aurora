import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { FilesTab } from "@/components/agents/files-tab";
import { ClashDialog } from "@/components/agents/files/dialogs";
import type { UploadJob } from "@/components/agents/files/upload-queue";
import { filesTiming, type FileEntry } from "@/lib/api/files";

// P5-FILES-2 (docs/14 §Files): the page against a mocked API — listing, the hidden filter, the URL's `&path=`, the
// waking and unsupported states, row actions (rename, recursive delete), uploads by picker and by dropped folder.

const json = (status: number, body: unknown, headers: Record<string, string> = {}) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json", ...headers } });
const err = (status: number, code: string, details?: unknown) => json(status, { error: { code, message: "server words", details } });

const entry = (name: string, kind: FileEntry["kind"], over: Partial<FileEntry> = {}, dir = "/workspace"): FileEntry => ({
  name,
  path: `${dir}/${name}`,
  kind,
  size: kind === "dir" ? 4096 : 1234,
  mode: kind === "dir" ? "0755" : "0644",
  modified_at: "2026-11-15T10:00:00Z",
  writable: true,
  ...over,
});

interface Seen {
  method: string;
  path: string;
  query: URLSearchParams;
  body: unknown;
}

function mockApi(handler: (req: Seen) => Response | undefined) {
  const seen: Seen[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request | string | URL) => {
      const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
      const url = new URL(req.url);
      const text = req.method === "GET" || req.method === "HEAD" ? "" : await req.text();
      const s: Seen = { method: req.method, path: url.pathname, query: url.searchParams, body: text ? JSON.parse(text) : null };
      seen.push(s);
      return handler(s) ?? err(404, "not_found");
    }),
  );
  return seen;
}

const listing = (path: string, entries: FileEntry[], truncated = false) => json(200, { path, entries, truncated });

function renderTab(frozen = false) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <FilesTab agentId="a1" frozen={frozen} />
    </QueryClientProvider>,
  );
}

const openRowMenu = async (name: string) => {
  const trigger = await screen.findByRole("button", { name: `Actions for ${name}` });
  fireEvent.pointerDown(trigger, { button: 0, ctrlKey: false });
  fireEvent.keyDown(trigger, { key: "Enter" });
};

beforeEach(() => {
  window.history.replaceState(null, "", "/o/acme/w/main/agents/scout?tab=files");
  window.localStorage.clear();
  filesTiming.retryDelayMs = 5;
});
afterEach(() => {
  filesTiming.retryDelayMs = 2000;
});

const ROOT = [
  entry("src", "dir"),
  entry(".aurora", "dir", { writable: false }),
  entry("notes.md", "file"),
  entry(".env", "file"),
  entry("evil", "symlink", { target: { path: "/etc", kind: "dir", outside_workspace: true } }),
  entry("gone", "symlink", { target: { path: "/workspace/nowhere", kind: "missing", outside_workspace: false } }),
];

describe("FilesTab listing", () => {
  it("lists the folder with sizes, symlink targets, a Read-only badge and a download link", async () => {
    const seen = mockApi((r) => (r.path === "/v1/agents/a1/files" ? listing("/workspace", ROOT) : undefined));
    renderTab();
    expect(await screen.findByRole("link", { name: "src" })).toBeInTheDocument();
    expect(seen[0]!.query.get("path")).toBe("/workspace");
    // Hidden names are filtered by default; `.aurora` is a dot-name too.
    expect(screen.queryByText(".env")).toBeNull();
    expect(screen.getByText("1.2 KiB")).toBeInTheDocument();
    expect(screen.getByText("→ /etc · Points outside the workspace")).toBeInTheDocument();
    expect(screen.getByText(/Broken link/)).toBeInTheDocument();
    // A link that leaves the workspace is shown but not openable: no menu "Open" for it.
    await openRowMenu("notes.md");
    const download = await screen.findByRole("menuitem", { name: "Download" });
    expect(download).toHaveAttribute("href", "/v1/agents/a1/files/content?path=%2Fworkspace%2Fnotes.md");
    expect(download).toHaveAttribute("download");
  });

  it("Show hidden reveals dot-names, the read-only badge and drops write actions; the choice is remembered", async () => {
    mockApi((r) => (r.path === "/v1/agents/a1/files" ? listing("/workspace", ROOT) : undefined));
    renderTab();
    await screen.findByRole("link", { name: "src" });
    fireEvent.click(screen.getByRole("checkbox", { name: "Show hidden" }));
    expect(await screen.findByText(".env")).toBeInTheDocument();
    expect(screen.getByText("Read-only")).toBeInTheDocument();
    expect(window.localStorage.getItem("aurora.files.hidden")).toBe("1");
    await openRowMenu(".aurora");
    await screen.findByRole("menuitem", { name: "Copy path" });
    expect(screen.queryByRole("menuitem", { name: "Rename" })).toBeNull();
    expect(screen.queryByRole("menuitem", { name: "Delete" })).toBeNull();
  });

  it("starts with Show hidden on when the browser remembered it", async () => {
    window.localStorage.setItem("aurora.files.hidden", "1");
    mockApi((r) => (r.path === "/v1/agents/a1/files" ? listing("/workspace", ROOT) : undefined));
    renderTab();
    expect(await screen.findByText(".env")).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Show hidden" })).toBeChecked();
  });

  it("shows the empty state, and the truncated notice", async () => {
    mockApi((r) => (r.path === "/v1/agents/a1/files" ? listing("/workspace", []) : undefined));
    renderTab();
    expect(await screen.findByText("This folder is empty")).toBeInTheDocument();
    cleanup();
    mockApi((r) => (r.path === "/v1/agents/a1/files" ? listing("/workspace", [entry("a", "file")], true) : undefined));
    renderTab();
    expect(await screen.findByText(/Showing the first 5,000 items\. Use the Terminal to see the rest\./)).toBeInTheDocument();
  });
});

describe("path in the URL", () => {
  it("reads &path= on load, navigates into folders and back with the breadcrumb, and keeps the URL in step", async () => {
    window.history.replaceState(null, "", "/o/acme/w/main/agents/scout?tab=files&path=%2Fworkspace%2Fsrc");
    const seen = mockApi((r) => {
      if (r.path !== "/v1/agents/a1/files") return undefined;
      const p = r.query.get("path");
      if (p === "/workspace/src") return listing(p, [entry("app", "dir", {}, "/workspace/src")]);
      if (p === "/workspace/src/app") return listing(p, [entry("page.tsx", "file", {}, "/workspace/src/app")]);
      return listing("/workspace", [entry("src", "dir")]);
    });
    renderTab();
    const crumbs = await screen.findByRole("navigation", { name: "Folder path" });
    expect(within(crumbs).getByRole("link", { name: "workspace" })).toBeInTheDocument();
    expect(within(crumbs).getByText("src")).toHaveAttribute("aria-current", "page");
    expect(seen[0]!.query.get("path")).toBe("/workspace/src");

    fireEvent.click(await screen.findByRole("link", { name: "app" }));
    expect(await screen.findByText("page.tsx")).toBeInTheDocument();
    expect(new URLSearchParams(window.location.search).get("path")).toBe("/workspace/src/app");
    expect(new URLSearchParams(window.location.search).get("tab")).toBe("files");

    fireEvent.click(within(screen.getByRole("navigation", { name: "Folder path" })).getByRole("link", { name: "workspace" }));
    expect(await screen.findByRole("link", { name: "src" })).toBeInTheDocument();
    expect(new URLSearchParams(window.location.search).has("path")).toBe(false);
  });

  it("follows the browser's Back (popstate)", async () => {
    mockApi((r) => {
      if (r.path !== "/v1/agents/a1/files") return undefined;
      const p = r.query.get("path");
      return p === "/workspace/src" ? listing(p, [entry("inner.txt", "file", {}, p)]) : listing("/workspace", [entry("src", "dir")]);
    });
    renderTab();
    fireEvent.click(await screen.findByRole("link", { name: "src" }));
    await screen.findByText("inner.txt");
    window.history.replaceState(null, "", "/o/acme/w/main/agents/scout?tab=files");
    fireEvent(window, new PopStateEvent("popstate"));
    expect(await screen.findByRole("link", { name: "src" })).toBeInTheDocument();
  });

  it("Go to path takes a path under /workspace and refuses others", async () => {
    mockApi((r) => (r.path === "/v1/agents/a1/files" ? listing(r.query.get("path") ?? "/workspace", [entry("x.txt", "file", {}, r.query.get("path") ?? "/workspace")]) : undefined));
    renderTab();
    await screen.findByText("x.txt");
    fireEvent.click(screen.getByRole("button", { name: "Go to path" }));
    const input = await screen.findByLabelText("Path");
    fireEvent.change(input, { target: { value: "/etc" } });
    fireEvent.click(screen.getByRole("button", { name: "Go" }));
    expect(await screen.findByText(/Enter a path under \/workspace/)).toBeInTheDocument();
    fireEvent.change(input, { target: { value: "/workspace/deep/er" } });
    fireEvent.click(screen.getByRole("button", { name: "Go" }));
    await waitFor(() => expect(new URLSearchParams(window.location.search).get("path")).toBe("/workspace/deep/er"));
  });
});

describe("states", () => {
  it("retries a parked sandbox with 'Waking the computer…' and then lists", async () => {
    let calls = 0;
    const seen = mockApi((r) => {
      if (r.path !== "/v1/agents/a1/files") return undefined;
      calls += 1;
      return calls < 3 ? err(503, "sandbox_waking") : listing("/workspace", [entry("a.txt", "file")]);
    });
    renderTab();
    expect(await screen.findByText("Waking the computer…")).toBeInTheDocument();
    expect(await screen.findByText("a.txt")).toBeInTheDocument();
    expect(seen.filter((s) => s.path === "/v1/agents/a1/files")).toHaveLength(3);
  });

  it("says the computer restarted while files_unavailable retries", async () => {
    let calls = 0;
    mockApi((r) => {
      if (r.path !== "/v1/agents/a1/files") return undefined;
      calls += 1;
      return calls < 2 ? err(503, "files_unavailable") : listing("/workspace", [entry("a.txt", "file")]);
    });
    renderTab();
    expect(await screen.findByText("The computer restarted; retrying…")).toBeInTheDocument();
    expect(await screen.findByText("a.txt")).toBeInTheDocument();
  });

  it("shows the reason for files_not_supported as a notice, never a raw code", async () => {
    mockApi((r) => (r.path === "/v1/agents/a1/files" ? err(422, "files_not_supported", { reason: "image" }) : undefined));
    renderTab();
    expect(await screen.findByText(/no SFTP server/)).toBeInTheDocument();
    expect(screen.queryByText(/files_not_supported/)).toBeNull();
  });

  it("answers any other error in words with Try again", async () => {
    mockApi((r) => (r.path === "/v1/agents/a1/files" ? err(403, "path_outside_workspace") : undefined));
    renderTab();
    expect(await screen.findByText(/leads outside the workspace/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Try again" })).toBeInTheDocument();
  });

  it("a frozen agent shows the notice and makes no request", () => {
    const seen = mockApi(() => undefined);
    renderTab(true);
    // (P5-REFINE-1) "frozen" is "paused" in the product's words.
    expect(screen.getByText(/This agent is paused/)).toBeInTheDocument();
    expect(seen).toHaveLength(0);
    expect(screen.queryByRole("button", { name: "Upload" })).toBeNull();
  });

  it("opening a file shows the viewer (here: No preview, as the API says it is binary) with Download and Back to folder", async () => {
    mockApi((r) => {
      if (r.path === "/v1/agents/a1/files/text") return err(422, "file_not_editable", { reason: "binary" });
      if (r.path === "/v1/agents/a1/files/content") return new Response(new Uint8Array([0, 1, 2]), { status: 200, headers: { "content-length": "3" } });
      return r.path === "/v1/agents/a1/files" ? listing("/workspace", [entry("notes.md", "file")]) : undefined;
    });
    renderTab();
    fireEvent.click(await screen.findByRole("button", { name: "notes.md" }));
    const dl = await screen.findByRole("link", { name: /Download/ });
    expect(dl).toHaveAttribute("href", "/v1/agents/a1/files/content?path=%2Fworkspace%2Fnotes.md");
    fireEvent.click(screen.getByRole("button", { name: /Back to folder/ }));
    expect(await screen.findByRole("button", { name: "notes.md" })).toBeInTheDocument();
  });
});

describe("row actions", () => {
  it("Rename refuses an existing name inline, then sends the rename op", async () => {
    const seen = mockApi((r) => {
      if (r.path === "/v1/agents/a1/files/ops") return json(200, { entry: entry("renamed.md", "file") });
      if (r.path === "/v1/agents/a1/files") return listing("/workspace", [entry("notes.md", "file"), entry("other.md", "file")]);
      return undefined;
    });
    renderTab();
    await openRowMenu("notes.md");
    fireEvent.click(await screen.findByRole("menuitem", { name: "Rename" }));
    const input = await screen.findByLabelText("New name");
    fireEvent.change(input, { target: { value: "other.md" } });
    fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    expect(await screen.findByText("An item with that name already exists.")).toBeInTheDocument();
    expect(seen.some((s) => s.path.endsWith("/ops"))).toBe(false);
    fireEvent.change(input, { target: { value: "renamed.md" } });
    fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    await waitFor(() => expect(seen.find((s) => s.path.endsWith("/ops"))?.body).toEqual({ op: "rename", path: "/workspace/notes.md", to: "/workspace/renamed.md" }));
  });

  it("Rename shows the API's file_exists inline too", async () => {
    mockApi((r) => {
      if (r.path === "/v1/agents/a1/files/ops") return err(409, "file_exists");
      if (r.path === "/v1/agents/a1/files") return listing("/workspace", [entry("notes.md", "file")]);
      return undefined;
    });
    renderTab();
    await openRowMenu("notes.md");
    fireEvent.click(await screen.findByRole("menuitem", { name: "Rename" }));
    fireEvent.change(await screen.findByLabelText("New name"), { target: { value: "taken.md" } });
    fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    expect(await screen.findByText("Something with that name already exists.")).toBeInTheDocument();
  });

  it("Delete of a folder asks first (danger) and deletes recursively; too_many_entries is a sentence", async () => {
    let fail = true;
    const seen = mockApi((r) => {
      if (r.path === "/v1/agents/a1/files/ops") return fail ? err(422, "too_many_entries") : json(200, { deleted: 12 });
      if (r.path === "/v1/agents/a1/files") return listing("/workspace", [entry("src", "dir")]);
      return undefined;
    });
    renderTab();
    await openRowMenu("src");
    fireEvent.click(await screen.findByRole("menuitem", { name: "Delete" }));
    expect(await screen.findByText("Delete src?")).toBeInTheDocument();
    expect(screen.getByText(/everything inside the folder/)).toBeInTheDocument();
    expect(seen.some((s) => s.path.endsWith("/ops"))).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    expect(await screen.findByText("This folder has more than 10,000 items; delete it from the Terminal.")).toBeInTheDocument();
    fail = false;
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.queryByText("Delete src?")).toBeNull());
    expect(seen.filter((s) => s.path.endsWith("/ops")).map((s) => s.body)).toEqual([
      { op: "delete", path: "/workspace/src", recursive: true },
      { op: "delete", path: "/workspace/src", recursive: true },
    ]);
  });

  it("New folder posts mkdir; New file creates an empty file with If-None-Match", async () => {
    const seen = mockApi((r) => {
      if (r.path === "/v1/agents/a1/files/ops") return json(201, { entry: entry("docs", "dir") });
      if (r.path === "/v1/agents/a1/files") return listing("/workspace", []);
      return undefined;
    });
    renderTab();
    fireEvent.click(await screen.findByRole("button", { name: /New folder/ }));
    fireEvent.change(await screen.findByLabelText("Folder name"), { target: { value: "docs" } });
    fireEvent.click(screen.getByRole("button", { name: "Create folder" }));
    await waitFor(() => expect(seen.find((s) => s.path.endsWith("/ops"))?.body).toEqual({ op: "mkdir", path: "/workspace/docs", parents: false }));
  });
});

// ---------------------------------------------------------------------------------------------- uploads

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
    upload: { onprogress: ((e: { lengthComputable: boolean; loaded: number; total: number }) => void) | null } = { onprogress: null };
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

describe("uploads", () => {
  it("uploads picked files with If-None-Match: * and shows done in the tray", async () => {
    const xhrs = stubXhr();
    mockApi((r) => (r.path === "/v1/agents/a1/files" ? listing("/workspace", []) : undefined));
    renderTab();
    await screen.findByText("This folder is empty");
    fireEvent.change(screen.getByTestId("files-input"), { target: { files: [new File(["hello"], "hello.txt")] } });
    await waitFor(() => expect(xhrs).toHaveLength(1));
    expect(xhrs[0]).toMatchObject({ method: "PUT", url: "/v1/agents/a1/files/content?path=%2Fworkspace%2Fhello.txt" });
    expect(xhrs[0]!.headers["if-none-match"]).toBe("*");
    expect(xhrs[0]!.headers["content-type"]).toBe("application/octet-stream");
    xhrs[0]!.respond(201, { entry: entry("hello.txt", "file"), etag: null });
    const tray = await screen.findByRole("region", { name: "Uploads" });
    await waitFor(() => expect(within(tray).getByText("Done")).toBeInTheDocument());
  });

  it("shows a failed upload as a danger chip with the reason", async () => {
    const xhrs = stubXhr();
    mockApi((r) => (r.path === "/v1/agents/a1/files" ? listing("/workspace", []) : undefined));
    renderTab();
    await screen.findByText("This folder is empty");
    fireEvent.change(screen.getByTestId("files-input"), { target: { files: [new File(["x"], "big.bin")] } });
    await waitFor(() => expect(xhrs).toHaveLength(1));
    xhrs[0]!.respond(413, { error: { code: "file_too_large", message: "x" } });
    expect(await screen.findByText("Too large: the limit is 512 MiB.")).toBeInTheDocument();
  });

  it("a clash opens the dialog; Skip leaves the tray saying Skipped", async () => {
    const xhrs = stubXhr();
    mockApi((r) => (r.path === "/v1/agents/a1/files" ? listing("/workspace", [entry("a.txt", "file")]) : undefined));
    renderTab();
    await screen.findByText("a.txt");
    fireEvent.change(screen.getByTestId("files-input"), { target: { files: [new File(["x"], "a.txt")] } });
    await waitFor(() => expect(xhrs).toHaveLength(1));
    xhrs[0]!.respond(409, { error: { code: "file_exists", message: "x" } });
    expect(await screen.findByText("“a.txt” already exists")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Skip" }));
    const tray = await screen.findByRole("region", { name: "Uploads" });
    await waitFor(() => expect(within(tray).getByText("Skipped")).toBeInTheDocument());
    expect(xhrs).toHaveLength(1);
  });

  it("a dropped folder is recreated (mkdir parents) before its files are sent", async () => {
    const xhrs = stubXhr();
    const seen = mockApi((r) => {
      if (r.path === "/v1/agents/a1/files/ops") return json(200, { entry: entry("pics", "dir") });
      if (r.path === "/v1/agents/a1/files") return listing("/workspace", []);
      return undefined;
    });
    renderTab();
    await screen.findByText("This folder is empty");
    const f = new File(["png"], "a.png");
    const fileEntry = { isFile: true, isDirectory: false, name: "a.png", file: (ok: (f: File) => void) => ok(f) };
    const dirEntry = {
      isFile: false,
      isDirectory: true,
      name: "pics",
      createReader: () => {
        let done = false;
        return { readEntries: (ok: (e: unknown[]) => void) => ok(done ? [] : ((done = true), [fileEntry])) };
      },
    };
    const zone = screen.getByText("This folder is empty").closest(".min-h-32")!;
    const dataTransfer = { types: ["Files"], files: [f], items: [{ kind: "file", webkitGetAsEntry: () => dirEntry }], dropEffect: "" };
    fireEvent.drop(zone, { dataTransfer });
    await waitFor(() => expect(xhrs).toHaveLength(1));
    expect(seen.find((s) => s.path.endsWith("/ops"))?.body).toEqual({ op: "mkdir", path: "/workspace/pics", parents: true });
    expect(xhrs[0]!.url).toBe("/v1/agents/a1/files/content?path=%2Fworkspace%2Fpics%2Fa.png");
  });
});

describe("ClashDialog", () => {
  const job = { id: "u1", name: "report.txt", dir: "/workspace/up", label: "report.txt", state: "waiting" } as UploadJob;

  it("offers Replace, Keep both and Skip", () => {
    const onChoose = vi.fn();
    render(<ClashDialog job={job} remaining={0} onChoose={onChoose} />);
    expect(screen.queryByLabelText(/Do this for the remaining/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Replace" }));
    fireEvent.click(screen.getByRole("button", { name: "Keep both" }));
    fireEvent.click(screen.getByRole("button", { name: "Skip" }));
    expect(onChoose.mock.calls).toEqual([["replace", false], ["keep", false], ["skip", false]]);
  });

  it("'Do this for the remaining N' applies the choice to the rest", () => {
    const onChoose = vi.fn();
    render(<ClashDialog job={job} remaining={4} onChoose={onChoose} />);
    fireEvent.click(screen.getByLabelText("Do this for the remaining 4"));
    fireEvent.click(screen.getByRole("button", { name: "Keep both" }));
    expect(onChoose).toHaveBeenCalledWith("keep", true);
  });

  it("closing it never overwrites: it skips", () => {
    const onChoose = vi.fn();
    render(<ClashDialog job={job} remaining={0} onChoose={onChoose} />);
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    expect(onChoose).toHaveBeenCalledWith("skip", false);
  });
});
