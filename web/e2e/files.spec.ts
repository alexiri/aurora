import { expect, test, type Page } from "@playwright/test";
import { createHash } from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { Fs, HTML_WITH_SCRIPT, PNG_1X1, SVG_WITH_SCRIPT, mockFiles, openFiles, rowMenu, serious } from "./support/files";
import { overflow, type Scheme } from "./support/open";

// P5-FILES-2/3 (docs/14 §Files, C-904, C-905): the agent Files page against the shared mock plus an in-memory
// `/workspace` behind the C-898 routes (support/files.ts). The page must use the attachment link and never open or
// render the bytes (SEC-126).

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
    await expect(page.getByRole("button", { name: "dropped.txt", exact: true })).toBeVisible();
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

// P5-FILES-3 (C-905): the viewer. CSP violations are collected from the page (`securitypolicyviolation`); the dev server's
// policy is looser than production's, so the strict check against the real policy is `files.prod.spec.ts`.
const watchCsp = (page: Page) =>
  page.addInitScript(() => {
    (window as unknown as { __csp: string[] }).__csp = [];
    document.addEventListener("securitypolicyviolation", (e) => (window as unknown as { __csp: string[] }).__csp.push(`${e.violatedDirective} ${e.blockedURI}`));
  });
const cspViolations = (page: Page) => page.evaluate(() => (window as unknown as { __csp: string[] }).__csp);
const editorText = (page: Page) => page.locator(".cm-content").innerText();

test.describe("Files viewer", () => {
  test.use({ viewport: { width: 1280, height: 900 } });

  test("edits a .rs file, saves with Ctrl+S (If-Match) and a reload shows the change", async ({ page }) => {
    const fs = new Fs();
    await mockFiles(page, fs);
    await openFiles(page);
    await page.getByRole("link", { name: "src", exact: true }).click();
    await page.getByRole("button", { name: "main.rs", exact: true }).click();
    const before = fs.etag("/workspace/src/main.rs");
    await expect(page.locator(".cm-content")).toContainText("fn main()");
    await page.locator(".cm-content").click();
    await page.keyboard.press("Control+End");
    await page.keyboard.type("// edited");
    await expect(page.getByText("Unsaved changes")).toBeVisible();
    await page.keyboard.press("Control+s");
    await expect(page.getByText("Unsaved changes")).toHaveCount(0);
    expect(fs.puts.at(-1)).toMatchObject({ path: "/workspace/src/main.rs" });
    expect(fs.puts.at(-1)!.headers["if-match"]).toBe(`"${before}"`);
    expect(fs.nodes.get("/workspace/src/main.rs")!.content).toBe("fn main() {}\n// edited");
    await page.reload();
    await page.getByRole("button", { name: "main.rs", exact: true }).click();
    await expect(page.locator(".cm-content")).toContainText("// edited");
  });

  test("a change made behind the editor opens the conflict dialog; Overwrite replaces it", async ({ page }) => {
    const fs = new Fs();
    await mockFiles(page, fs);
    await openFiles(page);
    await page.getByRole("button", { name: "notes.md", exact: true }).click();
    await expect(page.locator(".cm-content")).toContainText("# notes");
    await page.locator(".cm-content").click();
    await page.keyboard.press("Control+End");
    await page.keyboard.type("mine");
    // Another session (the terminal, a turn) changes the file in between.
    fs.nodes.set("/workspace/notes.md", { kind: "file", content: "# changed elsewhere\n", mtime: new Date().toISOString() });
    await page.getByRole("button", { name: "Save", exact: true }).click();
    const dialog = page.getByRole("dialog", { name: "This file changed in the sandbox after you opened it." });
    await expect(dialog).toBeVisible();
    await dialog.getByRole("button", { name: "Overwrite" }).click();
    await expect(dialog).toHaveCount(0);
    await expect.poll(() => fs.nodes.get("/workspace/notes.md")!.content).toBe("# notes\nmine");
    expect(fs.puts.at(-1)!.headers["if-match"]).toBeUndefined();
  });

  test("leaving a file with unsaved changes asks first", async ({ page }) => {
    const fs = new Fs();
    await mockFiles(page, fs);
    await openFiles(page);
    await page.getByRole("button", { name: "notes.md", exact: true }).click();
    await page.locator(".cm-content").click();
    await page.keyboard.type("x");
    await page.getByRole("navigation", { name: "Folder path" }).getByRole("link", { name: "workspace" }).click();
    await expect(page.getByRole("dialog", { name: "Discard your changes?" })).toBeVisible();
    await page.getByRole("button", { name: "Discard changes" }).click();
    await expect(page.getByRole("button", { name: "notes.md", exact: true })).toBeVisible();
    expect(fs.puts).toHaveLength(0);
  });

  test("Esc then Tab leaves the editor and says so", async ({ page }) => {
    const fs = new Fs();
    await mockFiles(page, fs);
    await openFiles(page);
    await page.getByRole("button", { name: "notes.md", exact: true }).click();
    await page.locator(".cm-content").click();
    await page.keyboard.press("Tab");
    await expect(page.locator(".cm-content")).toContainText("\t# notes");
    await page.keyboard.press("Escape");
    await page.keyboard.press("Tab");
    await expect(page.getByText("Left the editor.")).toBeVisible();
    await expect(page.getByRole("button", { name: "Back to folder" }).first()).toBeFocused();
  });

  test("a PNG is previewed from its bytes; a text file named .png is not an image", async ({ page }) => {
    const fs = new Fs();
    fs.nodes.set("/workspace/pic.dat", { kind: "file", content: "", bytes: PNG_1X1, mtime: "2026-11-15T10:00:00Z" });
    fs.nodes.set("/workspace/fake.png", { kind: "file", content: "just text, not an image\n", mtime: "2026-11-15T10:00:00Z" });
    await mockFiles(page, fs);
    await openFiles(page);
    await page.getByRole("button", { name: "pic.dat", exact: true }).click();
    await expect(page.getByRole("img", { name: "pic.dat" })).toHaveAttribute("src", /^data:image\/png;base64,/);
    await page.getByRole("button", { name: "Back to folder" }).click();
    await page.getByRole("button", { name: "fake.png", exact: true }).click();
    await expect(page.locator(".cm-content")).toContainText("just text");
    await expect(page.locator("img")).toHaveCount(0);
  });

  for (const scheme of ["light", "dark"] as const) {
    test(`${scheme} theme: an SVG and an HTML file are never rendered, with no CSP violation, and axe is clean`, async ({ page }) => {
      const fs = new Fs();
      fs.nodes.set("/workspace/logo.svg", { kind: "file", content: SVG_WITH_SCRIPT, bytes: Buffer.concat([Buffer.from(SVG_WITH_SCRIPT), Buffer.from([0])]), mtime: "2026-11-15T10:00:00Z" });
      fs.nodes.set("/workspace/page.html", { kind: "file", content: HTML_WITH_SCRIPT, mtime: "2026-11-15T10:00:00Z" });
      await watchCsp(page);
      await mockFiles(page, fs);
      await openFiles(page, scheme);
      await page.getByRole("button", { name: "logo.svg", exact: true }).click();
      await expect(page.getByText("No preview for this file")).toBeVisible();
      await expect(page.locator("img, svg image, object, iframe, embed")).toHaveCount(0);
      expect(await serious(page)).toEqual([]);
      await page.getByRole("button", { name: "Back to folder" }).click();

      await page.getByRole("button", { name: "page.html", exact: true }).click();
      await expect(page.locator(".cm-content")).toContainText("<script>window.__html_ran = true</script>");
      expect(await editorText(page)).toContain("<h1>hi</h1>");
      expect(await serious(page)).toEqual([]);

      expect(await page.evaluate(() => [(window as unknown as Record<string, unknown>).__svg_ran, (window as unknown as Record<string, unknown>).__html_ran])).toEqual([undefined, undefined]);
      // (Axe's own helper pages are closed by now: nothing the viewer did opened a tab.)
      expect(page.context().pages()).toHaveLength(1);
      expect(await cspViolations(page)).toEqual([]);
    });
  }
});

// P5-FILES-4 (C-907): the whole journey in one flow, in both themes — upload a folder, edit and save, conflict, download with
// a matching sha256, rename, recursive delete, `.aurora` read-only, a link out of /workspace refused, a parked sandbox
// waking, and (separately) a frozen agent. The kind journey (scripts/e2e/files.sh) proves the same against real SFTP.
// (A file picked with `setInputFiles` has a body Playwright cannot read back, so its bytes are written in the editor.)
test.describe("Files journey (C-907)", () => {
  test.use({ viewport: { width: 1280, height: 900 } });

  for (const scheme of ["light", "dark"] as const) {
    test(`${scheme} theme: upload, edit, conflict, download, rename, delete, read-only, outside link, waking`, async ({ page }, info) => {
      const fs = new Fs();
      fs.nodes.set("/workspace/evil", { kind: "dir", content: "", mtime: "2026-11-15T10:00:00Z", link: { path: "/etc", kind: "dir", outside: true } });
      await mockFiles(page, fs);
      await openFiles(page, scheme);

      // Upload a folder of three files and list it.
      const dir = info.outputPath("proj");
      mkdirSync(dir, { recursive: true });
      for (const n of ["a.txt", "b.txt", "c.txt"]) writeFileSync(join(dir, n), n);
      await page.getByTestId("folder-input").setInputFiles(dir);
      await expect(page.getByRole("region", { name: "Uploads" }).getByText("Done")).toHaveCount(3);
      await page.getByRole("link", { name: "proj", exact: true }).click();
      await expect(page.getByRole("region", { name: "Files in this folder" }).locator("tbody tr")).toHaveCount(3);

      // Edit and save; download it and compare the digests.
      await page.getByRole("button", { name: "a.txt", exact: true }).click();
      await page.locator(".cm-content").click();
      await page.keyboard.type("alpha");
      await page.keyboard.press("Control+s");
      await expect(page.getByText("Unsaved changes")).toHaveCount(0);
      await page.getByRole("button", { name: "Back to folder" }).first().click();
      await rowMenu(page, "a.txt").click();
      // (A browser download bypasses `page.route`, so the bytes the link serves are fetched in the page, through the mock.)
      const href = await page.getByRole("menuitem", { name: "Download" }).getAttribute("href");
      const digest = await page.evaluate(async (url) => {
        const bytes = await (await fetch(url!)).arrayBuffer();
        return [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))].map((b) => b.toString(16).padStart(2, "0")).join("");
      }, href);
      expect(digest).toBe(createHash("sha256").update("alpha").digest("hex"));
      const [download] = await Promise.all([page.waitForEvent("download"), page.getByRole("menuitem", { name: "Download" }).click()]);
      expect(download.suggestedFilename()).toBe("a.txt");

      // A change behind the editor: conflict, Overwrite.
      await page.getByRole("button", { name: "a.txt", exact: true }).click();
      await page.locator(".cm-content").click();
      await page.keyboard.press("Control+End");
      await page.keyboard.type("!");
      fs.nodes.set("/workspace/proj/a.txt", { kind: "file", content: "from the terminal", mtime: new Date().toISOString() });
      await page.getByRole("button", { name: "Save", exact: true }).click();
      const dialog = page.getByRole("dialog", { name: "This file changed in the sandbox after you opened it." });
      await dialog.getByRole("button", { name: "Overwrite" }).click();
      await expect.poll(() => fs.nodes.get("/workspace/proj/a.txt")!.content).toBe("alpha!");
      await page.getByRole("button", { name: "Back to folder" }).first().click();

      // Rename, then delete the folder recursively.
      await rowMenu(page, "b.txt").click();
      await page.getByRole("menuitem", { name: "Rename" }).click();
      await page.getByLabel("New name").fill("b2.txt");
      await page.getByRole("button", { name: "Rename" }).click();
      await expect(page.getByRole("button", { name: "b2.txt", exact: true })).toBeVisible();
      await page.getByRole("navigation", { name: "Folder path" }).getByRole("link", { name: "workspace" }).click();
      await rowMenu(page, "proj").click();
      await page.getByRole("menuitem", { name: "Delete" }).click();
      await page.getByRole("dialog").getByRole("button", { name: "Delete" }).click();
      await expect(page.getByRole("link", { name: "proj", exact: true })).toHaveCount(0);
      expect([...fs.nodes.keys()].some((p) => p.startsWith("/workspace/proj"))).toBe(false);

      // `.aurora` is read-only: a badge, and no Rename or Delete.
      await page.getByRole("checkbox", { name: "Show hidden" }).check();
      await expect(page.getByRole("link", { name: ".aurora", exact: true })).toBeVisible();
      await expect(page.getByText("Read-only").first()).toBeVisible();
      await rowMenu(page, ".aurora").click();
      await expect(page.getByRole("menuitem", { name: "Open" })).toBeVisible();
      await expect(page.getByRole("menuitem", { name: "Rename" })).toHaveCount(0);
      await expect(page.getByRole("menuitem", { name: "Delete" })).toHaveCount(0);
      await page.keyboard.press("Escape");

      // A link that leaves /workspace is shown but cannot be opened.
      await expect(page.getByText("Points outside the workspace")).toBeVisible();
      await rowMenu(page, "evil").click();
      await expect(page.getByRole("menuitem", { name: "Open" })).toHaveCount(0);
      await page.keyboard.press("Escape");

      // A parked sandbox: the page says it is waking, retries, then shows the listing.
      fs.waking = 2;
      await page.reload();
      await expect(page.getByText("Waking the computer…")).toBeVisible();
      await expect(page.getByRole("button", { name: "notes.md", exact: true })).toBeVisible({ timeout: 20_000 });
      expect(await serious(page)).toEqual([]);
    });
  }

  test("a frozen agent shows the freeze notice and sends no files request", async ({ page }) => {
    const fs = new Fs();
    const requests: string[] = [];
    page.on("request", (r) => {
      if (/\/v1\/agents\/[^/]+\/files/.test(r.url())) requests.push(r.url());
    });
    await mockFiles(page, fs, { frozen: true });
    await page.goto("/o/acme/w/main/agents/daneel?tab=files");
    await expect(page.getByText(/Files are unavailable while it is paused/)).toBeVisible();
    expect(requests).toEqual([]);
  });
});

test.describe("Files viewer at 390 px", () => {
  test.use({ viewport: { width: 390, height: 800 } });

  for (const scheme of ["light", "dark"] as const) {
    test(`${scheme} theme: a long line scrolls inside the editor, not the page`, async ({ page }) => {
      const fs = new Fs();
      fs.nodes.set("/workspace/wide.txt", { kind: "file", content: `${"w".repeat(400)}\nshort\n`, mtime: "2026-11-15T10:00:00Z" });
      await mockFiles(page, fs);
      await openFiles(page, scheme as Scheme);
      await page.getByRole("button", { name: "wide.txt", exact: true }).click();
      await expect(page.locator(".cm-content")).toContainText("short");
      expect(await overflow(page)).toBeLessThanOrEqual(0);
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
