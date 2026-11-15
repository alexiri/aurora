import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page, type Route } from "@playwright/test";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { mockApi } from "./support/mock-api";
import { overflow, type Scheme } from "./support/open";

// P5-FILES-2 (docs/14 §Files, C-904): the agent Files page against the shared mock plus an in-memory `/workspace`
// behind the C-898 routes. The response headers of the real download route (`content-disposition: attachment`,
// C-903) are the API suite's and the kind journey's (P5-FILES-4); here the page must use the attachment link and
// never open or render the bytes.

type Node = { kind: "file" | "dir"; content: string; mtime: string };

class Fs {
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

  entry(path: string) {
    const n = this.nodes.get(path)!;
    const readOnly = path === "/workspace/.aurora" || path.startsWith("/workspace/.aurora/");
    return {
      name: path.split("/").pop()!,
      path,
      kind: n.kind,
      size: n.kind === "dir" ? 4096 : Buffer.byteLength(n.content),
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

const err = (route: Route, status: number, code: string) =>
  route.fulfill({ status, contentType: "application/json", body: JSON.stringify({ error: { code, message: code } }) });
const ok = (route: Route, status: number, body: unknown) => route.fulfill({ status, contentType: "application/json", body: JSON.stringify(body) });

async function mockFiles(page: Page, fs: Fs) {
  await mockApi(page);
  await page.route(/\/v1\/agents\/[^/]+\/files(\/content|\/ops|\/text)?(\?.*)?$/, async (route) => {
    const req = route.request();
    const url = new URL(req.url());
    const path = url.searchParams.get("path") ?? "/workspace";
    const kind = url.pathname.endsWith("/content") ? "content" : url.pathname.endsWith("/ops") ? "ops" : "list";
    if (kind === "list") {
      const node = fs.nodes.get(path);
      if (!node) return err(route, 404, "path_not_found");
      if (node.kind !== "dir") return err(route, 422, "not_a_directory");
      return ok(route, 200, { path, entries: fs.children(path).map((p) => fs.entry(p)), truncated: false });
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
        body: node.content,
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
      fs.nodes.set(path, { kind: "file", content: body.toString("utf8"), mtime: new Date().toISOString() });
      return ok(route, exists ? 200 : 201, { entry: fs.entry(path), etag: null });
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

async function openFiles(page: Page, scheme: Scheme = "light", query = "") {
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto(`/o/acme/w/main/agents/daneel?tab=files${query}`);
  await expect.poll(() => page.locator("html").getAttribute("data-theme"), { timeout: 20_000 }).toBe(scheme);
  await expect(page.getByRole("navigation", { name: "Folder path" })).toBeVisible();
}

const serious = async (page: Page) => {
  const { violations } = await new AxeBuilder({ page }).analyze();
  return violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id} (${v.impact}): ${v.nodes.map((n) => n.target.join(" ")).slice(0, 5).join(" | ")}`);
};

const rowMenu = (page: Page, name: string) => page.getByRole("button", { name: `Actions for ${name}` });

test.describe("agent Files page", () => {
  test.use({ viewport: { width: 1280, height: 900 } });

  test("lists /workspace, folders first, dot-names hidden until Show hidden", async ({ page }) => {
    const fs = new Fs();
    await mockFiles(page, fs);
    await openFiles(page);
    await expect(page.getByRole("tablist", { name: "Computer pages" }).getByRole("tab", { name: "Files" })).toBeVisible();
    const rows = page.getByRole("region", { name: "Files in this folder" }).locator("tbody tr");
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(0)).toContainText("src");
    await expect(rows.nth(1)).toContainText("notes.md");
    await page.getByRole("checkbox", { name: "Show hidden" }).check();
    await expect(rows).toHaveCount(4);
    await expect(page.getByText("Read-only")).toBeVisible();
    // The choice survives a reload.
    await page.reload();
    await expect(page.getByRole("checkbox", { name: "Show hidden" })).toBeChecked();
  });

  test("keeps the folder in the URL and follows Back", async ({ page }) => {
    const fs = new Fs();
    await mockFiles(page, fs);
    await openFiles(page);
    await page.getByRole("link", { name: "src", exact: true }).click();
    await expect(page.getByText("main.rs")).toBeVisible();
    expect(new URL(page.url()).searchParams.get("path")).toBe("/workspace/src");
    await page.reload();
    await expect(page.getByText("main.rs")).toBeVisible();
    await page.goBack();
    await expect(page.getByRole("link", { name: "src", exact: true })).toBeVisible();
    expect(new URL(page.url()).searchParams.has("path")).toBe(false);
  });

  test("uploads files by drag and drop, with If-None-Match, and lists them", async ({ page }) => {
    const fs = new Fs();
    await mockFiles(page, fs);
    await openFiles(page);
    const zone = page.getByRole("region", { name: "Files in this folder" }).locator("xpath=ancestor::div[contains(@class,'min-h-32')][1]");
    await zone.evaluate((el) => {
      const dt = new DataTransfer();
      dt.items.add(new File(["dropped bytes"], "dropped.txt", { type: "text/plain" }));
      el.dispatchEvent(new DragEvent("dragenter", { dataTransfer: dt, bubbles: true }));
      el.dispatchEvent(new DragEvent("dragover", { dataTransfer: dt, bubbles: true, cancelable: true }));
      el.dispatchEvent(new DragEvent("drop", { dataTransfer: dt, bubbles: true, cancelable: true }));
    });
    await expect(page.getByRole("region", { name: "Uploads" }).getByText("Done")).toBeVisible();
    await expect(page.getByRole("button", { name: "dropped.txt" })).toBeVisible();
    expect(fs.puts.map((p) => [p.path, p.headers["if-none-match"], p.bytes])).toEqual([["/workspace/dropped.txt", "*", 13]]);
  });

  test("uploads a folder: mkdir with parents first, then each file", async ({ page }, info) => {
    const fs = new Fs();
    await mockFiles(page, fs);
    await openFiles(page);
    const dir = info.outputPath("pics");
    mkdirSync(join(dir, "sub"), { recursive: true });
    writeFileSync(join(dir, "a.txt"), "a");
    writeFileSync(join(dir, "sub", "b.txt"), "bb");
    await page.getByTestId("folder-input").setInputFiles(dir);
    await expect(page.getByRole("region", { name: "Uploads" }).getByText("Done")).toHaveCount(2);
    expect(fs.ops).toEqual(expect.arrayContaining([{ op: "mkdir", path: "/workspace/pics", parents: true }, { op: "mkdir", path: "/workspace/pics/sub", parents: true }]));
    expect(fs.puts.map((p) => p.path).sort()).toEqual(["/workspace/pics/a.txt", "/workspace/pics/sub/b.txt"]);
    await page.getByRole("link", { name: "pics", exact: true }).click();
    await expect(page.getByRole("link", { name: "sub", exact: true })).toBeVisible();
  });

  test("a name that exists asks Replace / Keep both / Skip and keeps both as `name (1)`", async ({ page }, info) => {
    const fs = new Fs();
    await mockFiles(page, fs);
    await openFiles(page);
    const file = info.outputPath("notes.md");
    mkdirSync(info.outputDir, { recursive: true });
    writeFileSync(file, "second");
    await page.getByTestId("files-input").setInputFiles(file);
    await expect(page.getByRole("dialog", { name: "“notes.md” already exists" })).toBeVisible();
    await page.getByRole("button", { name: "Keep both" }).click();
    await expect(page.getByRole("button", { name: "notes (1).md", exact: true })).toBeVisible();
    expect(fs.nodes.get("/workspace/notes.md")!.content).toBe("# notes\n");
    // (The request body of a file-backed upload is not visible to Playwright's `postData`, so the path and the
    // preconditions are what is checked here: the first try 409'd on the original, the second created `notes (1).md`.)
    expect(fs.puts.map((p) => [p.path, p.headers["if-none-match"]])).toEqual([
      ["/workspace/notes.md", "*"],
      ["/workspace/notes (1).md", "*"],
    ]);
    expect(fs.nodes.has("/workspace/notes (1).md")).toBe(true);
  });

  test("renames, and deletes a folder recursively after a danger confirmation", async ({ page }) => {
    const fs = new Fs();
    await mockFiles(page, fs);
    await openFiles(page);
    await rowMenu(page, "notes.md").click();
    await page.getByRole("menuitem", { name: "Rename" }).click();
    await page.getByLabel("New name").fill("README.md");
    await page.getByRole("button", { name: "Rename" }).click();
    await expect(page.getByRole("button", { name: "README.md", exact: true })).toBeVisible();

    await rowMenu(page, "src").click();
    await page.getByRole("menuitem", { name: "Delete" }).click();
    await expect(page.getByRole("dialog", { name: "Delete src?" })).toContainText("everything inside the folder");
    await page.getByRole("dialog").getByRole("button", { name: "Delete" }).click();
    await expect(page.getByRole("link", { name: "src", exact: true })).toHaveCount(0);
    expect(fs.ops).toContainEqual({ op: "delete", path: "/workspace/src", recursive: true });
    expect([...fs.nodes.keys()].some((p) => p.startsWith("/workspace/src"))).toBe(false);
  });

  test("downloads through the attachment link, never a new tab", async ({ page }) => {
    const fs = new Fs();
    await mockFiles(page, fs);
    await openFiles(page);
    await rowMenu(page, "notes.md").click();
    const link = page.getByRole("menuitem", { name: "Download" });
    await expect(link).toHaveAttribute("href", /\/v1\/agents\/[^/]+\/files\/content\?path=%2Fworkspace%2Fnotes\.md$/);
    const popups: unknown[] = [];
    page.context().on("page", (p) => popups.push(p));
    const [download] = await Promise.all([page.waitForEvent("download"), link.click()]);
    expect(download.suggestedFilename()).toBe("notes.md");
    expect(popups).toHaveLength(0);
  });

  for (const scheme of ["light", "dark"] as const) {
    test(`${scheme} theme: no serious axe violations on the listing, a dialog and the tray`, async ({ page }, info) => {
      const fs = new Fs();
      await mockFiles(page, fs);
      await openFiles(page, scheme);
      expect(await serious(page)).toEqual([]);
      await rowMenu(page, "notes.md").click();
      await page.getByRole("menuitem", { name: "Rename" }).click();
      await expect(page.getByRole("dialog", { name: "Rename" })).toBeVisible();
      expect(await serious(page)).toEqual([]);
      await page.keyboard.press("Escape");
      const file = info.outputPath("axe.txt");
      mkdirSync(info.outputDir, { recursive: true });
      writeFileSync(file, "x");
      await page.getByTestId("files-input").setInputFiles(file);
      await expect(page.getByRole("region", { name: "Uploads" })).toBeVisible();
      expect(await serious(page)).toEqual([]);
    });
  }
});

test.describe("agent Files page at 390 px", () => {
  test.use({ viewport: { width: 390, height: 800 } });

  for (const scheme of ["light", "dark"] as const) {
    test(`${scheme} theme: rows become cards and a 120-character name causes no horizontal scroll`, async ({ page }) => {
      const fs = new Fs();
      const long = `${"long-file-name-".repeat(8)}.txt`.slice(0, 120);
      expect(long).toHaveLength(120);
      fs.nodes.set(`/workspace/${long}`, { kind: "file", content: "x", mtime: "2026-11-15T10:00:00Z" });
      await mockFiles(page, fs);
      await openFiles(page, scheme as Scheme);
      await expect(page.getByRole("button", { name: long, exact: true })).toBeVisible();
      expect(await overflow(page)).toBeLessThanOrEqual(0);
      // The table region itself does not scroll sideways either: cards, not columns.
      const region = page.getByRole("region", { name: "Files in this folder" });
      expect(await region.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(0);
      expect(await serious(page)).toEqual([]);
    });
  }
});
