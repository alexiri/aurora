// The Files page's upload queue (P5-FILES-2, docs/14 §Files item 4): at most three uploads at once per tab, folders
// recreated (`mkdir` with `parents`) before their files, and a name that already exists asks Replace / Keep both / Skip
// (uploads send `If-None-Match: *`, so a clash is the API's 409 `file_exists` and nothing is ever silently overwritten).
// Plain TypeScript with injected I/O: the page subscribes through `useSyncExternalStore`, tests drive it directly.
import { ApiError } from "@/lib/api/client";
import { type FileWriteResult, UploadAborted, joinPath, nextFreeName, uploadFailure } from "@/lib/api/files";

export type ClashChoice = "replace" | "keep" | "skip";
export type JobState = "queued" | "uploading" | "waiting" | "done" | "failed" | "cancelled" | "skipped";

export interface UploadJob {
  id: string;
  /** What the person sees: the path relative to the folder the upload went to. */
  label: string;
  /** The destination directory (absolute, under `/workspace`). */
  dir: string;
  name: string;
  file: File | null;
  state: JobState;
  sent: number;
  total: number;
  error?: string;
  note?: string;
  replace: boolean;
  /** Needs `mkdir -p dir` first (a folder's files); the folder the upload targets already exists. */
  ensureDir: boolean;
}

export interface QueueSnapshot {
  jobs: UploadJob[];
  /** The job whose name clashes and awaits an answer (the first of those waiting), or null. */
  clash: UploadJob | null;
  /** Other jobs the answer can also cover: queued ones and further clashes ("Do this for the remaining N"). */
  remaining: number;
}

export interface QueueIo {
  put: (path: string, body: Blob, opts: { replace: boolean; signal: AbortSignal; onProgress: (sent: number, total: number) => void }) => Promise<FileWriteResult>;
  mkdirParents: (path: string) => Promise<unknown>;
}

export interface QueueItem {
  /** `null` creates only the directory (an empty folder of a dropped tree). */
  file: File | null;
  dir: string;
  label: string;
  ensureDir: boolean;
}

export const UPLOAD_CONCURRENCY = 3;
const KEEP_BOTH_TRIES = 200;

const isClash = (e: unknown) => e instanceof ApiError && e.code === "file_exists";

export class UploadQueue {
  private jobs: UploadJob[] = [];
  private readonly aborts = new Map<string, AbortController>();
  private readonly dirs = new Map<string, Promise<unknown>>();
  private readonly listeners = new Set<() => void>();
  private policy: ClashChoice | null = null;
  private seq = 0;
  private snapshot: QueueSnapshot = { jobs: [], clash: null, remaining: 0 };
  private active = 0;

  constructor(
    private readonly io: QueueIo,
    private readonly opts: { concurrency?: number; onDrained?: () => void } = {},
  ) {}

  subscribe = (fn: () => void) => {
    this.listeners.add(fn);
    return () => {
      this.listeners.delete(fn);
    };
  };

  getSnapshot = () => this.snapshot;

  private emit() {
    const waiting = this.jobs.filter((j) => j.state === "waiting");
    const queued = this.jobs.filter((j) => j.state === "queued").length;
    this.snapshot = { jobs: this.jobs, clash: waiting[0] ?? null, remaining: queued + Math.max(0, waiting.length - 1) };
    for (const l of this.listeners) l();
  }

  private set(id: string, patch: Partial<UploadJob>) {
    this.jobs = this.jobs.map((j) => (j.id === id ? { ...j, ...patch } : j));
  }

  private get(id: string) {
    return this.jobs.find((j) => j.id === id);
  }

  enqueue(items: QueueItem[]) {
    if (items.length === 0) return;
    // A new batch starts with no remembered answer: "Do this for the remaining N" covers one batch, not the tab's life.
    if (!this.jobs.some((j) => j.state === "queued" || j.state === "uploading" || j.state === "waiting")) this.policy = null;
    const added: UploadJob[] = items.map((it) => ({
      id: `u${(this.seq += 1)}`,
      label: it.label,
      dir: it.dir,
      name: it.file?.name ?? "",
      file: it.file,
      state: "queued",
      sent: 0,
      total: it.file?.size ?? 0,
      replace: false,
      ensureDir: it.ensureDir,
    }));
    this.jobs = [...this.jobs, ...added];
    this.emit();
    this.pump();
  }

  cancel(id: string) {
    const job = this.get(id);
    if (!job) return;
    if (job.state === "uploading") {
      this.aborts.get(id)?.abort();
      return;
    }
    if (job.state === "queued" || job.state === "waiting") {
      this.set(id, { state: "cancelled" });
      this.emit();
      this.settleIfDrained();
    }
  }

  dismiss(id: string) {
    const job = this.get(id);
    if (!job || job.state === "queued" || job.state === "uploading" || job.state === "waiting") return;
    this.jobs = this.jobs.filter((j) => j.id !== id);
    this.emit();
  }

  clearFinished() {
    this.jobs = this.jobs.filter((j) => j.state === "queued" || j.state === "uploading" || j.state === "waiting");
    this.emit();
  }

  /** The person's answer to the clash dialog; `applyToRest` makes it the answer for every later clash of the batch too. */
  resolveClash(choice: ClashChoice, applyToRest: boolean) {
    const first = this.jobs.find((j) => j.state === "waiting");
    if (!first) return;
    if (applyToRest) this.policy = choice;
    const targets = applyToRest ? this.jobs.filter((j) => j.state === "waiting") : [first];
    for (const job of targets) this.apply(job.id, choice);
    this.emit();
    this.pump();
    this.settleIfDrained();
  }

  private apply(id: string, choice: ClashChoice) {
    if (choice === "skip") this.set(id, { state: "skipped" });
    else if (choice === "replace") this.set(id, { state: "queued", replace: true, sent: 0 });
    else this.set(id, { state: "queued", sent: 0, note: "keep both" });
  }

  private pump() {
    const limit = this.opts.concurrency ?? UPLOAD_CONCURRENCY;
    while (this.active < limit) {
      const next = this.jobs.find((j) => j.state === "queued");
      if (!next) break;
      this.active += 1;
      this.set(next.id, { state: "uploading" });
      this.emit();
      void this.run(next.id).finally(() => {
        this.active -= 1;
        this.pump();
        this.settleIfDrained();
      });
    }
  }

  private ensure(dir: string): Promise<unknown> {
    let p = this.dirs.get(dir);
    if (!p) {
      p = this.io.mkdirParents(dir);
      this.dirs.set(dir, p);
      // A failed mkdir must not poison a retry in a later batch.
      p.catch(() => this.dirs.delete(dir));
    }
    return p;
  }

  private async run(id: string) {
    const job = this.get(id)!;
    const ctl = new AbortController();
    this.aborts.set(id, ctl);
    try {
      if (job.ensureDir) await this.ensure(job.dir);
      if (!job.file) {
        this.set(id, { state: "done" });
        return;
      }
      const file = job.file;
      const onProgress = (sent: number, total: number) => {
        this.set(id, { sent, total });
        this.emit();
      };
      if (job.note === "keep both") {
        const name = await this.keepBoth(job, file, ctl.signal, onProgress);
        this.set(id, { state: "done", note: `kept as ${name}`, sent: file.size, total: file.size });
        return;
      }
      try {
        await this.io.put(joinPath(job.dir, job.name), file, { replace: job.replace, signal: ctl.signal, onProgress });
        this.set(id, { state: "done", sent: file.size, total: file.size });
      } catch (e) {
        if (!isClash(e) || job.replace) throw e;
        if (this.policy) {
          this.apply(id, this.policy);
          // Applied remembered answer: replace and keep both re-queue the job; skip ends it.
          return;
        }
        this.set(id, { state: "waiting", sent: 0 });
      }
    } catch (e) {
      if (e instanceof UploadAborted) this.set(id, { state: "cancelled" });
      else this.set(id, { state: "failed", error: uploadFailure(e) });
    } finally {
      this.aborts.delete(id);
      this.emit();
    }
  }

  /** `name (1).ext`, `name (2).ext`, … — the first free one, found by asking the API (each try fails before any byte is written). */
  private async keepBoth(job: UploadJob, file: File, signal: AbortSignal, onProgress: (s: number, t: number) => void): Promise<string> {
    const tried = new Set<string>([job.name]);
    for (let i = 0; i < KEEP_BOTH_TRIES; i += 1) {
      const name = nextFreeName(job.name, tried);
      tried.add(name);
      try {
        await this.io.put(joinPath(job.dir, name), file, { replace: false, signal, onProgress });
        return name;
      } catch (e) {
        if (!isClash(e)) throw e;
      }
    }
    throw new ApiError("file_exists", "No free name was found.", null, 409);
  }

  private settleIfDrained() {
    if (this.jobs.some((j) => j.state === "queued" || j.state === "uploading" || j.state === "waiting")) return;
    if (this.jobs.length > 0) this.opts.onDrained?.();
  }
}
