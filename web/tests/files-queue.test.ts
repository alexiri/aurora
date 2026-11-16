import { describe, expect, it } from "vitest";

import { ApiError } from "@/lib/api/client";
import { UploadAborted } from "@/lib/api/files";
import { UploadQueue, type QueueItem, type QueueIo } from "@/components/agents/files/upload-queue";
import { fromFileList } from "@/components/agents/files/collect";

// P5-FILES-2 (docs/14 §Files item 4): at most three uploads at once, folders recreated first, a name that exists asks
// Replace / Keep both / Skip (uploads send If-None-Match: *, so a clash is the API's 409 file_exists), failures in words.

const file = (name: string, size = 10, rel?: string) => {
  const f = new File([new Uint8Array(size)], name);
  if (rel) Object.defineProperty(f, "webkitRelativePath", { value: rel });
  return f;
};
const item = (name: string, dir = "/workspace"): QueueItem => ({ file: file(name), dir, label: name, ensureDir: false });

interface Call {
  path: string;
  replace: boolean;
  signal: AbortSignal;
  resolve: () => void;
  reject: (e: unknown) => void;
  onProgress: (s: number, t: number) => void;
}

/** A fake API: every put waits until the test settles it; mkdir resolves at once and is recorded. */
function harness(opts: { existing?: Set<string> } = {}) {
  const calls: Call[] = [];
  const order: string[] = [];
  const existing = opts.existing ?? new Set<string>();
  const io: QueueIo = {
    mkdirParents: async (p) => {
      order.push(`mkdir ${p}`);
    },
    put: (path, _body, o) =>
      new Promise((resolve, reject) => {
        order.push(`put ${path}${o.replace ? " (replace)" : ""}`);
        if (!o.replace && existing.has(path)) {
          reject(new ApiError("file_exists", "exists", null, 409));
          return;
        }
        o.signal.addEventListener("abort", () => reject(new UploadAborted()));
        calls.push({ path, replace: o.replace, signal: o.signal, onProgress: o.onProgress, resolve: () => resolve({ entry: null } as never), reject });
      }),
  };
  let drained = 0;
  const queue = new UploadQueue(io, { onDrained: () => (drained += 1) });
  const settle = () => new Promise((r) => setTimeout(r, 0));
  return { queue, calls, order, drained: () => drained, settle, existing };
}

describe("UploadQueue", () => {
  it("runs at most three uploads at once and starts the next as one finishes", async () => {
    const h = harness();
    h.queue.enqueue(["a", "b", "c", "d", "e"].map((n) => item(n)));
    await h.settle();
    expect(h.calls.map((c) => c.path)).toEqual(["/workspace/a", "/workspace/b", "/workspace/c"]);
    expect(h.queue.getSnapshot().jobs.filter((j) => j.state === "uploading")).toHaveLength(3);
    h.calls[0]!.resolve();
    await h.settle();
    expect(h.calls.map((c) => c.path)).toEqual(["/workspace/a", "/workspace/b", "/workspace/c", "/workspace/d"]);
    h.calls[1]!.resolve();
    h.calls[2]!.resolve();
    await h.settle();
    h.calls[3]!.resolve();
    h.calls[4]!.resolve();
    await h.settle();
    expect(h.queue.getSnapshot().jobs.map((j) => j.state)).toEqual(["done", "done", "done", "done", "done"]);
    expect(h.drained()).toBeGreaterThan(0);
  });

  it("cancels a running upload (aborts its request) and a queued one (never starts it)", async () => {
    const h = harness();
    h.queue.enqueue(["a", "b", "c", "d"].map((n) => item(n)));
    await h.settle();
    const [, , , d] = h.queue.getSnapshot().jobs;
    h.queue.cancel(d!.id);
    h.queue.cancel(h.queue.getSnapshot().jobs[0]!.id);
    await h.settle();
    expect(h.calls[0]!.signal.aborted).toBe(true);
    const states = h.queue.getSnapshot().jobs.map((j) => j.state);
    expect(states).toEqual(["cancelled", "uploading", "uploading", "cancelled"]);
    // The freed slot does not start the cancelled one.
    expect(h.calls.map((c) => c.path)).not.toContain("/workspace/d");
  });

  it("shows progress", async () => {
    const h = harness();
    h.queue.enqueue([item("a")]);
    await h.settle();
    h.calls[0]!.onProgress(5, 10);
    const job = h.queue.getSnapshot().jobs[0]!;
    expect([job.sent, job.total]).toEqual([5, 10]);
  });

  it("words failures: 413 too large, 403 read-only, other 409", async () => {
    const h = harness();
    h.queue.enqueue([item("big"), item("ro"), item("net")]);
    await h.settle();
    h.calls[0]!.reject(new ApiError("file_too_large", "x", null, 413));
    h.calls[1]!.reject(new ApiError("path_read_only", "x", null, 403));
    h.calls[2]!.reject(new ApiError("files_operation_failed", "x", null, 422));
    await h.settle();
    const [big, ro, net] = h.queue.getSnapshot().jobs;
    expect(big).toMatchObject({ state: "failed", error: "Too large: the limit is 512 MiB." });
    expect(ro).toMatchObject({ state: "failed" });
    expect(ro!.error).toMatch(/Read-only/);
    expect(net!.error).toMatch(/disk is full/);
  });

  it("recreates a dropped folder once before its files and never for the target folder itself", async () => {
    const h = harness();
    h.queue.enqueue([
      { file: file("a.txt"), dir: "/workspace/up/docs", label: "docs/a.txt", ensureDir: true },
      { file: file("b.txt"), dir: "/workspace/up/docs", label: "docs/b.txt", ensureDir: true },
      { file: null, dir: "/workspace/up/empty", label: "empty/", ensureDir: true },
      { file: file("c.txt"), dir: "/workspace/up", label: "c.txt", ensureDir: false },
    ]);
    await h.settle();
    expect(h.order.filter((o) => o.startsWith("mkdir"))).toEqual(["mkdir /workspace/up/docs", "mkdir /workspace/up/empty"]);
    expect(h.order.indexOf("mkdir /workspace/up/docs")).toBeLessThan(h.order.indexOf("put /workspace/up/docs/a.txt"));
    expect(h.order).not.toContain("mkdir /workspace/up");
  });

  it("a mkdir failure fails the file in words and does not upload it", async () => {
    const h = harness();
    const q = new UploadQueue({ ...{ put: async () => ({}) as never }, mkdirParents: async () => { throw new ApiError("path_read_only", "x", null, 403); } });
    q.enqueue([{ file: file("a"), dir: "/workspace/.aurora/x", label: "x/a", ensureDir: true }]);
    await h.settle();
    expect(q.getSnapshot().jobs[0]).toMatchObject({ state: "failed" });
    expect(q.getSnapshot().jobs[0]!.error).toMatch(/Read-only/);
  });

  describe("name clashes", () => {
    it("asks, and Replace uploads again with replace (no If-None-Match)", async () => {
      const h = harness({ existing: new Set(["/workspace/a"]) });
      h.queue.enqueue([item("a")]);
      await h.settle();
      expect(h.queue.getSnapshot().clash?.name).toBe("a");
      expect(h.queue.getSnapshot().jobs[0]!.state).toBe("waiting");
      h.queue.resolveClash("replace", false);
      await h.settle();
      expect(h.calls.map((c) => [c.path, c.replace])).toEqual([["/workspace/a", true]]);
      h.calls[0]!.resolve();
      await h.settle();
      expect(h.queue.getSnapshot().jobs[0]!.state).toBe("done");
    });

    it("Keep both uploads to the next free `name (1).ext`", async () => {
      const h = harness({ existing: new Set(["/workspace/r.txt", "/workspace/r (1).txt"]) });
      h.queue.enqueue([item("r.txt")]);
      await h.settle();
      h.queue.resolveClash("keep", false);
      await h.settle();
      expect(h.calls.map((c) => c.path)).toEqual(["/workspace/r (2).txt"]);
      h.calls[0]!.resolve();
      await h.settle();
      expect(h.queue.getSnapshot().jobs[0]).toMatchObject({ state: "done", note: "kept as r (2).txt" });
    });

    it("Skip ends the job without a write", async () => {
      const h = harness({ existing: new Set(["/workspace/a"]) });
      h.queue.enqueue([item("a")]);
      await h.settle();
      h.queue.resolveClash("skip", false);
      await h.settle();
      expect(h.queue.getSnapshot().jobs[0]!.state).toBe("skipped");
      expect(h.calls).toHaveLength(0);
      expect(h.queue.getSnapshot().clash).toBeNull();
    });

    it("counts the remaining jobs and applies \"for the remaining N\" to later clashes without asking", async () => {
      const h = harness({ existing: new Set(["/workspace/a", "/workspace/b", "/workspace/c", "/workspace/d"]) });
      h.queue.enqueue(["a", "b", "c", "d"].map((n) => item(n)));
      await h.settle();
      // a, b, c clash first (three slots); d is still queued: the answer to `a` can also cover b, c and d.
      expect(h.queue.getSnapshot().clash?.name).toBe("a");
      expect(h.queue.getSnapshot().remaining).toBeGreaterThanOrEqual(2);
      h.queue.resolveClash("skip", true);
      await h.settle();
      expect(h.queue.getSnapshot().clash).toBeNull();
      expect(h.queue.getSnapshot().jobs.map((j) => j.state)).toEqual(["skipped", "skipped", "skipped", "skipped"]);
      expect(h.calls).toHaveLength(0);
    });

    it("without \"for the remaining\" the next clash asks again", async () => {
      const h = harness({ existing: new Set(["/workspace/a", "/workspace/b"]) });
      h.queue.enqueue([item("a"), item("b")]);
      await h.settle();
      h.queue.resolveClash("skip", false);
      expect(h.queue.getSnapshot().clash?.name).toBe("b");
    });
  });

  it("dismiss and clearFinished remove only finished jobs", async () => {
    const h = harness();
    h.queue.enqueue([item("a"), item("b")]);
    await h.settle();
    h.calls[0]!.resolve();
    await h.settle();
    const [a, b] = h.queue.getSnapshot().jobs;
    h.queue.dismiss(b!.id);
    expect(h.queue.getSnapshot().jobs).toHaveLength(2);
    h.queue.dismiss(a!.id);
    expect(h.queue.getSnapshot().jobs.map((j) => j.name)).toEqual(["b"]);
    h.queue.clearFinished();
    expect(h.queue.getSnapshot().jobs).toHaveLength(1);
  });
});

describe("fromFileList", () => {
  it("keeps a picked folder's relative paths and marks its files for mkdir", () => {
    const items = fromFileList([file("x.txt", 1, "proj/src/x.txt"), file("r.md", 1, "proj/r.md"), file("loose.txt")], "/workspace/up");
    expect(items.map((i) => [i.dir, i.label, i.ensureDir])).toEqual([
      ["/workspace/up/proj/src", "proj/src/x.txt", true],
      ["/workspace/up/proj", "proj/r.md", true],
      ["/workspace/up", "loose.txt", false],
    ]);
    expect(items.map((i) => i.file?.name)).toEqual(["x.txt", "r.md", "loose.txt"]);
  });
});
