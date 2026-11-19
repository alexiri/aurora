import { describe, expect, it } from "vitest";

import { ApiError } from "@/lib/api/client";
import {
  FILES_ERROR_CODES,
  FILES_ERROR_SENTENCES,
  WORKSPACE_ROOT,
  breadcrumbs,
  downloadUrl,
  filesErrorMessage,
  humanSize,
  isTransient,
  joinPath,
  nameProblem,
  nextFreeName,
  normalizePath,
  parentPath,
  unopenable,
  uploadFailure,
  type FileEntry,
} from "@/lib/api/files";

// P5-FILES-2 (docs/14 §Files, C-898..C-904): the page's pure helpers — paths, names, error sentences.

const err = (code: string, status: number, details?: unknown) => new ApiError(code, "server words", details, status);

describe("paths and breadcrumbs", () => {
  it("normalizes to an absolute path under /workspace and refuses anything else", () => {
    expect(normalizePath("/workspace")).toBe("/workspace");
    expect(normalizePath("/workspace/a//b/")).toBe("/workspace/a/b");
    expect(normalizePath("a/b", "/workspace/x")).toBe("/workspace/x/a/b");
    expect(normalizePath("./a", "/workspace")).toBe("/workspace/a");
    for (const bad of ["", "  ", "/etc", "/workspacex", "/workspace/../etc", "../x", "/workspace/a\0b"]) {
      expect(normalizePath(bad), bad).toBeNull();
    }
    expect(normalizePath(`/workspace/${"a".repeat(4100)}`)).toBeNull();
  });

  it("round-trips a path through breadcrumbs: each crumb opens its own prefix", () => {
    const trail = breadcrumbs("/workspace/src/app");
    expect(trail).toEqual([
      { label: "workspace", path: "/workspace" },
      { label: "src", path: "/workspace/src" },
      { label: "app", path: "/workspace/src/app" },
    ]);
    expect(breadcrumbs(WORKSPACE_ROOT)).toEqual([{ label: "workspace", path: "/workspace" }]);
    for (const c of trail) expect(normalizePath(c.path)).toBe(c.path);
    expect(parentPath("/workspace/src/app")).toBe("/workspace/src");
    expect(parentPath("/workspace/src")).toBe("/workspace");
    expect(parentPath("/workspace")).toBe("/workspace");
    expect(joinPath("/workspace/src", "a b.txt")).toBe("/workspace/src/a b.txt");
  });

  it("builds the download link as the attachment route with the path encoded", () => {
    expect(downloadUrl("a1", "/workspace/a b/ü.txt")).toBe("/v1/agents/a1/files/content?path=%2Fworkspace%2Fa%20b%2F%C3%BC.txt");
  });
});

describe("names", () => {
  it("picks the next free `name (1).ext`", () => {
    expect(nextFreeName("report.txt", new Set(["report.txt"]))).toBe("report (1).txt");
    expect(nextFreeName("report.txt", new Set(["report.txt", "report (1).txt"]))).toBe("report (2).txt");
    expect(nextFreeName(".env", new Set([".env"]))).toBe(".env (1)");
    expect(nextFreeName("a.tar.gz", new Set())).toBe("a.tar (1).gz");
  });

  it("refuses names that are paths or empty", () => {
    expect(nameProblem("ok.txt")).toBeNull();
    expect(nameProblem("")).not.toBeNull();
    expect(nameProblem("a/b")).not.toBeNull();
    expect(nameProblem("..")).not.toBeNull();
    expect(nameProblem("x".repeat(256))).not.toBeNull();
  });
});

describe("display helpers", () => {
  it("uses human units", () => {
    expect(humanSize(0)).toBe("0 B");
    expect(humanSize(1536)).toBe("1.5 KiB");
    expect(humanSize(5 * 1024 * 1024)).toBe("5.0 MiB");
  });

  it("marks links that cannot be opened", () => {
    const base = { name: "l", path: "/workspace/l", size: 0, mode: "0777", writable: true, kind: "symlink" } as FileEntry;
    expect(unopenable({ ...base, target: { path: "/etc", kind: "dir", outside_workspace: true } })).toBe("Points outside the workspace");
    expect(unopenable({ ...base, target: { path: "/workspace/x", kind: "missing", outside_workspace: false } })).toBe("Broken link");
    expect(unopenable({ ...base, target: { path: "/workspace/x", kind: "file", outside_workspace: false } })).toBeNull();
  });
});

describe("error sentences (C-898..C-901)", () => {
  it("has a sentence for every code the routes answer, never the raw code", () => {
    expect(FILES_ERROR_CODES.length).toBeGreaterThanOrEqual(23);
    for (const code of FILES_ERROR_CODES) {
      const words = FILES_ERROR_SENTENCES[code];
      expect(words, code).toBeTruthy();
      expect(words).not.toContain("_");
      expect(filesErrorMessage(err(code, 422))).not.toBe("server words");
    }
  });

  it("covers each C-898..C-901 code by name", () => {
    const required = [
      "invalid_path",
      "path_outside_workspace",
      "path_read_only",
      "permission_denied",
      "path_not_found",
      "files_operation_failed",
      "is_directory",
      "not_a_directory",
      "file_not_editable",
      "length_required",
      "file_too_large",
      "file_exists",
      "file_changed",
      "directory_not_empty",
      "too_many_entries",
      "terminal_disabled",
      "files_not_supported",
      "frozen",
      "sandbox_waking",
      "sandbox_unavailable",
      "wake_limited",
      "too_many_transfers",
      "files_unavailable",
    ];
    for (const code of required) expect(FILES_ERROR_CODES, code).toContain(code);
  });

  it("uses the details: reason for files_not_supported and file_not_editable, state for sandbox_unavailable", () => {
    expect(filesErrorMessage(err("files_not_supported", 422, { reason: "runner" }))).toMatch(/runner is too old/);
    expect(filesErrorMessage(err("files_not_supported", 422, { reason: "image" }))).toMatch(/no SFTP server/);
    expect(filesErrorMessage(err("files_not_supported", 422, { reason: "byo" }))).toMatch(/own machine/);
    expect(filesErrorMessage(err("file_not_editable", 422, { reason: "binary" }))).toMatch(/binary/);
    expect(filesErrorMessage(err("sandbox_unavailable", 409, { state: "stopped" }))).toMatch(/stopped/);
    expect(filesErrorMessage(err("files_operation_failed", 422, { deleted: 3 }))).toMatch(/3 items were removed/);
    expect(filesErrorMessage(err("too_many_entries", 422))).toBe("This folder has more than 10,000 items; delete it from the Terminal.");
    expect(filesErrorMessage(new Error("boom"))).toMatch(/connection/);
  });

  it("words an upload failure for 413, 409 and 403", () => {
    expect(uploadFailure(err("file_too_large", 413))).toBe("Too large: the limit is 512 MiB.");
    expect(uploadFailure(err("payload_too_large", 413))).toBe("Too large: the limit is 512 MiB.");
    expect(uploadFailure(err("file_exists", 409))).toBe("Something with that name already exists.");
    expect(uploadFailure(err("path_read_only", 403))).toMatch(/Read-only/);
    expect(uploadFailure(err("permission_denied", 403))).toMatch(/Not allowed/);
    expect(uploadFailure(err("path_outside_workspace", 403))).toMatch(/outside the workspace/);
  });

  it("retries only a parked or restarting sandbox", () => {
    expect(isTransient(err("sandbox_waking", 503))).toBe(true);
    expect(isTransient(err("files_unavailable", 503))).toBe(true);
    expect(isTransient(err("sandbox_unavailable", 409))).toBe(false);
    expect(isTransient(err("frozen", 423))).toBe(false);
  });
});
