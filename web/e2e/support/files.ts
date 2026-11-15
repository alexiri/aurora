import AxeBuilder from "@axe-core/playwright";
import { expect, type Page, type Route } from "@playwright/test";
import { createHash } from "node:crypto";

import { mockApi } from "./mock-api";
import type { Scheme } from "./open";

// The Files page's mocked API (P5-FILES-2/3, C-898): an in-memory `/workspace` behind the list, content, text and ops
// routes, shared by `files.spec.ts` (dev server) and `files.prod.spec.ts` (the production build, the only place the real
// CSP applies). The real response headers (C-903) are the API suite's and the kind journey's (P5-FILES-4).

export type Node = { kind: "file" | "dir"; content: string; mtime: string; /** Raw bytes for a binary file (no text view). */ bytes?: Buffer };

const sha = (b: Buffer) => `sha256:${createHash("sha256").update(b).digest("hex")}`;

export class Fs {
  nodes = new Map<string, Node>([
    ["/workspace", { kind: "dir", content: "", mtime: "2026-11-15T10:00:00Z" }],
    ["/workspace/src", { kind: "dir", content: "", mtime: "2026-11-15T10:00:00Z" }],
    ["/workspace/src/main.rs", { kind: "file", content: "fn main() {}\n", mtime: "2026-11-15T10:00:00Z" }],
    ["/workspace/notes.md", { kind: "file", content: "# notes\n", mtime: "2026-11-15T10:00:00Z" }],
    ["/workspace/.aurora", { kind: "dir", content: "", mtime: "2026-11-15T10:00:00Z" }],
    ["/workspace/.env", { kind: "file", content: "A=1\n", mtime: "2026-11-15T10:00:00Z" }],
  ]);
  puts: { path: string; headers: Record<string, string>; bytes: number }[] = [];
  ops: unknown[] = [];

  bytesOf(path: string): Buffer {
    const n = this.nodes.get(path)!;
    return n.bytes ?? Buffer.from(n.content, "utf8");
  }

  etag(path: string) {
    return sha(this.bytesOf(path));
  }

  entry(path: string) {
    const n = this.nodes.get(path)!;
    const readOnly = path === "/workspace/.aurora" || path.startsWith("/workspace/.aurora/");
    return {
      name: path.split("/").pop()!,
      path,
      kind: n.kind,
      size: n.kind === "dir" ? 4096 : this.bytesOf(path).length,
      mode: n.kind === "dir" ? "0755" : "0644",
      modified_at: n.mtime,
      writable: !readOnly,
    };
  }

  children(dir: string) {
    return [...this.nodes.keys()]
      .filter((p) => p !== dir && p.startsWith(`${dir}/`) && !p.slice(dir.length + 1).includes("/"))
      .sort((a, b) => {
        const ka = this.nodes.get(a)!.kind === "dir" ? 0 : 1;
        const kb = this.nodes.get(b)!.kind === "dir" ? 0 : 1;
        return ka - kb || a.localeCompare(b);
      });
  }
}

export const err = (route: Route, status: number, code: string, details?: unknown) =>
  route.fulfill({ status, contentType: "application/json", body: JSON.stringify({ error: { code, message: code, details } }) });
export const ok = (route: Route, status: number, body: unknown) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

export async function mockFiles(page: Page, fs: Fs) {
  await mockApi(page);
  await page.route(/\/v1\/agents\/[^/]+\/files(\/content|\/ops|\/text)?(\?.*)?$/, async (route) => {
    const req = route.request();
    const url = new URL(req.url());
    const path = url.searchParams.get("path") ?? "/workspace";
    const kind = url.pathname.endsWith("/content") ? "content" : url.pathname.endsWith("/ops") ? "ops" : url.pathname.endsWith("/text") ? "text" : "list";
    if (kind === "list") {
      const node = fs.nodes.get(path);
      if (!node) return err(route, 404, "path_not_found");
      if (node.kind !== "dir") return err(route, 422, "not_a_directory");
      return ok(route, 200, { path, entries: fs.children(path).map((p) => fs.entry(p)), truncated: false });
    }
    if (kind === "text") {
      const node = fs.nodes.get(path);
      if (!node || node.kind !== "file") return err(route, 404, "path_not_found");
      const bytes = fs.bytesOf(path);
      if (bytes.length > 1024 * 1024) return err(route, 422, "file_not_editable", { reason: "too_large" });
      if (bytes.includes(0)) return err(route, 422, "file_not_editable", { reason: "binary" });
      return ok(route, 200, {
        path,
        content: bytes.toString("utf8"),
        etag: fs.etag(path),
        size: bytes.length,
        modified_at: node.mtime,
        line_ending: bytes.toString("utf8").includes("\r\n") ? "crlf" : "lf",
      });
    }
    if (kind === "content" && req.method() === "GET") {
      const node = fs.nodes.get(path);
      if (!node || node.kind !== "file") return err(route, 404, "path_not_found");
      return route.fulfill({
        status: 200,
        headers: {
          "content-type": "application/octet-stream",
          "content-disposition": `attachment; filename="${path.split("/").pop()}"`,
          "x-content-type-options": "nosniff",
        },
        body: fs.bytesOf(path),
      });
    }
    if (kind === "content" && req.method() === "PUT") {
      const headers = req.headers();
      const body = req.postDataBuffer() ?? Buffer.alloc(0);
      fs.puts.push({ path, headers, bytes: body.length });
      const parent = path.slice(0, path.lastIndexOf("/"));
      if (!fs.nodes.has(parent)) return err(route, 404, "path_not_found");
      const exists = fs.nodes.has(path);
      if (headers["if-none-match"] === "*" && exists) return err(route, 409, "file_exists");
      const want = headers["if-match"]?.replace(/^"|"$/g, "");
      if (want !== undefined && (!exists || fs.etag(path) !== want)) return err(route, 409, "file_changed", exists ? { current_etag: fs.etag(path) } : undefined);
      fs.nodes.set(path, { kind: "file", content: body.toString("utf8"), mtime: new Date().toISOString() });
      return ok(route, exists ? 200 : 201, { entry: fs.entry(path), etag: fs.etag(path) });
    }
    if (kind === "ops") {
      const body = req.postDataJSON() as { op: string; path: string; to?: string; parents?: boolean; recursive?: boolean };
      fs.ops.push(body);
      if (body.op === "mkdir") {
        if (fs.nodes.has(body.path) && !body.parents) return err(route, 409, "file_exists");
        const parts = body.path.split("/").filter(Boolean);
        for (let i = 2; i <= parts.length; i += 1) {
          const p = `/${parts.slice(0, i).join("/")}`;
          if (!fs.nodes.has(p)) fs.nodes.set(p, { kind: "dir", content: "", mtime: new Date().toISOString() });
        }
        return ok(route, 201, { entry: fs.entry(body.path) });
      }
      if (body.op === "rename") {
        if (fs.nodes.has(body.to!)) return err(route, 409, "file_exists");
        for (const p of [...fs.nodes.keys()]) {
          if (p === body.path || p.startsWith(`${body.path}/`)) {
            const n = fs.nodes.get(p)!;
            fs.nodes.delete(p);
            fs.nodes.set(body.to! + p.slice(body.path.length), n);
          }
        }
        return ok(route, 200, { entry: fs.entry(body.to!) });
      }
      let deleted = 0;
      for (const p of [...fs.nodes.keys()]) {
        if (p === body.path || p.startsWith(`${body.path}/`)) {
          fs.nodes.delete(p);
          deleted += 1;
        }
      }
      return ok(route, 200, { deleted });
    }
    return route.fallback();
  });
}

export async function openFiles(page: Page, scheme: Scheme = "light", query = "") {
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(`/o/acme/w/main/agents/daneel?tab=files${query}`);
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  await expect(page.getByRole("navigation", { name: "Folder path" })).toBeVisible();
}

export const serious = async (page: Page) => {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
};

export const rowMenu = (page: Page, name: string) => page.getByRole("button", { name: `Actions for ${name}` });

/** 1×1 transparent PNG. */
export const PNG_1X1 = Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGD4DwABBAEAHnOcFQAAAABJRU5ErkJggg==", "base64");

export const SVG_WITH_SCRIPT = '<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><script>window.__svg_ran = true</script><rect width="40" height="40" fill="red"/></svg>';
export const HTML_WITH_SCRIPT = "<!doctype html><html><body><h1>hi</h1><script>window.__html_ran = true</script></body></html>\n";
