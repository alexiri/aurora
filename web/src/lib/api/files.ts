"use client";

// The agent's files (P5-FILES-2, D-316, C-898..C-904): list, upload, download, mkdir, rename and delete under the
// sandbox's `/workspace`, over the API's SFTP-backed routes (session cookie only). Downloads are plain links to the
// attachment route; agent bytes are never rendered here (SEC-126).
import { useQuery } from "@tanstack/react-query";

import { traceRequest } from "@/lib/telemetry";

import { ApiError, api } from "./client";
import { errorDetail } from "./errors";
import type { components } from "./schema";
import { unwrap } from "./unwrap";

type S = components["schemas"];
export type FileEntry = S["FileEntry"];
export type FileList = S["FileList"];
export type FileOp = S["FileOp"];
export type FileOpResult = S["FileOpResult"];
export type FileWriteResult = S["FileWriteResult"];
export type FileText = S["FileText"];

/** The scope of the page (C-899): nothing outside it is shown or requested. */
export const WORKSPACE_ROOT = "/workspace";
/** `FILES_LIST_MAX` (C-898), for the words only; the API enforces it. */
export const LIST_MAX = 5000;
/** C-621 / C-904: a parked sandbox is retried at most this many times, every `WAKE_RETRY_MS` (`retry-after: 2`, C-900). */
export const WAKE_ATTEMPTS = 12;
export const WAKE_RETRY_MS = 2000;
/** The live retry delay; tests shorten it (the production value is `WAKE_RETRY_MS`). */
export const filesTiming = { retryDelayMs: WAKE_RETRY_MS };

export const filesKeys = {
  list: (agent: string, path: string) => ["agents", agent, "files", "list", path] as const,
  all: (agent: string) => ["agents", agent, "files"] as const,
};

// ---------------------------------------------------------------- paths

/** `/workspace/a/b` → ["a","b"]. */
const segments = (path: string) => path.split("/").filter(Boolean).slice(1);

/**
 * A person's text → a request path (C-899's shape, checked again by the API): absolute under `/workspace`, or relative to
 * `base`. `.` is dropped, a trailing `/` is stripped; `..`, NUL and empty components are refused (null).
 */
export function normalizePath(input: string, base: string = WORKSPACE_ROOT): string | null {
  const text = input.trim();
  if (!text || text.includes("\0")) return null;
  const absolute = text.startsWith("/") ? text : `${base}/${text}`;
  const parts: string[] = [];
  for (const part of absolute.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") return null;
    parts.push(part);
  }
  if (parts[0] !== "workspace") return null;
  const path = `/${parts.join("/")}`;
  return new TextEncoder().encode(path).length > 4096 ? null : path;
}

export const joinPath = (dir: string, name: string) => `${dir === "/" ? "" : dir}/${name}`;

export function parentPath(path: string): string {
  const parts = segments(path);
  parts.pop();
  return parts.length === 0 ? WORKSPACE_ROOT : `${WORKSPACE_ROOT}/${parts.join("/")}`;
}

export const baseName = (path: string) => path.split("/").filter(Boolean).pop() ?? "";

/** The breadcrumb trail of a path: `workspace` first, then each segment with the path it opens. */
export function breadcrumbs(path: string): { label: string; path: string }[] {
  const trail = [{ label: "workspace", path: WORKSPACE_ROOT }];
  let at = WORKSPACE_ROOT;
  for (const s of segments(path)) {
    at = joinPath(at, s);
    trail.push({ label: s, path: at });
  }
  return trail;
}

/** The next free `name (1).ext` (`.env` has no extension; `a.tar.gz` → `a.tar (1).gz`), skipping `taken`. */
export function nextFreeName(name: string, taken: ReadonlySet<string>): string {
  const dot = name.lastIndexOf(".");
  const stem = dot > 0 ? name.slice(0, dot) : name;
  const ext = dot > 0 ? name.slice(dot) : "";
  for (let n = 1; ; n += 1) {
    const candidate = `${stem} (${n})${ext}`;
    if (!taken.has(candidate)) return candidate;
  }
}

/** A single entry name, not a path (rename, new folder, new file): non-empty, no `/`, NUL, `.` or `..`, ≤ 255 bytes. */
export function nameProblem(name: string): string | null {
  if (!name.trim()) return "Enter a name.";
  if (name === "." || name === "..") return "That is not a valid name.";
  if (name.includes("/") || name.includes("\0")) return "A name cannot contain a slash.";
  if (new TextEncoder().encode(name).length > 255) return "That name is too long (255 bytes at most).";
  return null;
}

export const downloadUrl = (agent: string, path: string) =>
  `/v1/agents/${encodeURIComponent(agent)}/files/content?path=${encodeURIComponent(path)}`;

// ---------------------------------------------------------------- display

/** Human units for sizes ("—" for a folder is the caller's). */
export function humanSize(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  return `${v >= 10 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

/** `/workspace/.aurora` and everything under it is platform-managed and read-only (C-899). */
export const isManaged = (path: string) => path === `${WORKSPACE_ROOT}/.aurora` || path.startsWith(`${WORKSPACE_ROOT}/.aurora/`);

/**
 * (P5-REFINE-15, computer-9) What Aurora itself writes into the workspace root for the agent: `AGENTS.md` and `CLAUDE.md`
 * (the instruction files, 0444, from `docs/12` §instruction_files and the supervisor's home sync) and `memory/` (the notes the
 * supervisor syncs). They are shown in their own group, read-only by default, with a Managed badge and the line below.
 */
export const AGENT_OWN_ENTRIES: Readonly<Record<string, string>> = {
  "AGENTS.md": "Instructions, kept by Aurora",
  "CLAUDE.md": "Instructions, kept by Aurora",
  memory: "Memory, kept by Aurora",
};

/** The one line under an agent-own entry's name, or null when the entry is not one (only the root's entries are). */
export function agentOwnNote(path: string): string | null {
  return parentPath(path) === WORKSPACE_ROOT ? (AGENT_OWN_ENTRIES[baseName(path)] ?? null) : null;
}

export const isAgentOwn = (path: string) => agentOwnNote(path) !== null;

export const isHidden =(e: Pick<FileEntry, "name">) => e.name.startsWith(".");

/** The kind a symlink opens as (its target's), a regular entry's own. */
export function effectiveKind(e: FileEntry): "file" | "dir" | "other" | "missing" {
  if (e.kind === "symlink") {
    const k = e.target?.kind ?? "missing";
    return k === "file" || k === "dir" || k === "other" ? k : "missing";
  }
  return e.kind === "file" || e.kind === "dir" ? e.kind : "other";
}

/** Why an entry cannot be opened, or null. */
export function unopenable(e: FileEntry): string | null {
  if (e.kind !== "symlink") return e.kind === "other" ? "Not a regular file" : null;
  if (e.target?.outside_workspace) return "Points outside the workspace";
  if (!e.target || e.target.kind === "missing") return "Broken link";
  return null;
}

// ---------------------------------------------------------------- errors

/** Every error code the Files routes answer (C-898..C-901) and the sentence the page shows for it. Never a raw code. */
export const FILES_ERROR_SENTENCES: Record<string, string> = {
  invalid_path: "That path is not valid.",
  path_outside_workspace: "That path leads outside the workspace, so it cannot be used here.",
  path_read_only: "That location is read-only (it is managed by the platform).",
  permission_denied: "The sandbox did not allow that (permission denied).",
  path_not_found: "That file or folder no longer exists.",
  files_operation_failed: "The sandbox refused the operation (for example, the disk is full).",
  is_directory: "That is a folder, not a file.",
  not_a_directory: "That is a file, not a folder.",
  file_not_editable: "This file cannot be edited here.",
  length_required: "The upload could not be sent (its size is unknown).",
  file_too_large: "Too large: the limit is 512 MiB.",
  file_exists: "Something with that name already exists.",
  file_changed: "This file changed in the sandbox after you opened it.",
  directory_not_empty: "That folder is not empty.",
  too_many_entries: "This folder has more than 10,000 items; delete it from the Terminal.",
  terminal_disabled: "Files are not available on this instance.",
  files_not_supported: "Files are not available for this agent.",
  frozen: "This agent is paused. Resume it to use its files.",
  sandbox_waking: "Waking the computer…",
  sandbox_unavailable: "The agent's computer is not running and cannot be started right now.",
  wake_limited: "The computer was woken too many times this hour. Try again later.",
  too_many_transfers: "Too many transfers at once. Wait for one to finish and try again.",
  files_unavailable: "The computer isn't answering; trying again…",
  rate_limited: "Too many requests. Please wait a moment and try again.",
  forbidden: "You do not have permission to use this agent's files.",
  not_found: "This agent's files are not available.",
  unauthorized: "Your session has expired. Please sign in again.",
};

/** C-898..C-901's error codes (the table above must answer each: guarded by a test). */
export const FILES_ERROR_CODES = Object.keys(FILES_ERROR_SENTENCES).filter(
  (c) => !["rate_limited", "forbidden", "not_found", "unauthorized"].includes(c),
);

const REASONS: Record<string, string> = {
  byo: "This agent runs on your own machine, so it has no sandbox files.",
  runner: "This agent's runner is too old for Files. Ask an administrator to update it.",
  image: "This agent's sandbox image has no SFTP server, so Files is not available.",
};

const NOT_EDITABLE: Record<string, string> = {
  too_large: "It is larger than 1 MiB.",
  binary: "It is a binary file.",
  not_utf8: "It is not UTF-8 text.",
};

const detail = (err: ApiError, key: string): string | undefined => {
  const v = errorDetail(err, key);
  return typeof v === "string" ? v : undefined;
};

/** Words for any error from the Files routes (falls back to the server's message, then a generic sentence). */
export function filesErrorMessage(err: unknown): string {
  if (!(err instanceof ApiError)) return "That did not work. Check your connection and try again.";
  if (err.code === "files_not_supported") return REASONS[detail(err, "reason") ?? ""] ?? FILES_ERROR_SENTENCES.files_not_supported!;
  if (err.code === "file_not_editable") return `${FILES_ERROR_SENTENCES.file_not_editable} ${NOT_EDITABLE[detail(err, "reason") ?? ""] ?? ""}`.trim();
  if (err.code === "sandbox_unavailable") {
    const state = detail(err, "state");
    return state ? `${FILES_ERROR_SENTENCES.sandbox_unavailable} (It is ${state}.)` : FILES_ERROR_SENTENCES.sandbox_unavailable!;
  }
  if (err.code === "files_operation_failed") {
    const done = errorDetail(err, "deleted");
    return typeof done === "number"
      ? `${FILES_ERROR_SENTENCES.files_operation_failed} ${done} ${done === 1 ? "item was" : "items were"} removed before it stopped.`
      : FILES_ERROR_SENTENCES.files_operation_failed!;
  }
  if (err.status === 413) return FILES_ERROR_SENTENCES.file_too_large!;
  return FILES_ERROR_SENTENCES[err.code] ?? (err.message || "That did not work. Please try again.");
}

/** The upload chip's reason (docs/14 §Files: "Too large: the limit is 512 MB"). */
export function uploadFailure(err: unknown): string {
  if (err instanceof ApiError) {
    if (err.status === 413 || err.code === "file_too_large") return "Too large: the limit is 512 MiB.";
    if (err.code === "path_read_only" || (err.status === 403 && err.code !== "path_outside_workspace")) {
      return err.code === "path_read_only" ? "Read-only: this folder is managed by the platform." : "Not allowed: the sandbox refused to write here.";
    }
  }
  return filesErrorMessage(err);
}

/** (computer-2) A computer that does not answer (`files_unavailable`) gets as many tries as the terminal's reconnect: 4. */
export const UNAVAILABLE_ATTEMPTS = 4;

/**
 * The retry rule of every Files read: a parked sandbox that is waking (`sandbox_waking`) is retried for the wake budget;
 * a runner that does not answer (`files_unavailable`) stops after `UNAVAILABLE_ATTEMPTS` tries in all, like the terminal.
 * `failures` is the number of failed tries so far.
 */
export function shouldRetry(failures: number, err: unknown): boolean {
  if (!isTransient(err)) return false;
  if (err instanceof ApiError && err.code === "files_unavailable") return failures + 1 < UNAVAILABLE_ATTEMPTS;
  return failures < WAKE_ATTEMPTS;
}

/** A parked or restarting sandbox: the page retries (C-900, C-901). */
export function isTransient(err: unknown): boolean {
  return err instanceof ApiError && err.status === 503 && (err.code === "sandbox_waking" || err.code === "files_unavailable");
}

// ---------------------------------------------------------------- requests

/**
 * The folder listing. A parked sandbox answers 503 `sandbox_waking` at once with `retry-after: 2`, so the query retries
 * (up to `WAKE_ATTEMPTS`) and reports `failureReason`, which the page shows as "Waking the computer…".
 */
export function useFileList(agent: string, path: string, opts: { enabled?: boolean; retryDelayMs?: number } = {}) {
  return useQuery({
    queryKey: filesKeys.list(agent, path),
    queryFn: () => unwrap(api.GET("/v1/agents/{agent}/files", { params: { path: { agent }, query: { path } } })),
    enabled: opts.enabled ?? true,
    retry: shouldRetry,
    retryDelay: opts.retryDelayMs ?? filesTiming.retryDelayMs,
    staleTime: 0,
    refetchOnWindowFocus: false,
  });
}

/**
 * The editor's read (C-898 `GET …/files/text`, P5-FILES-3): the text, its `etag` and line ending; 422 `file_not_editable`
 * (`details.reason`: too_large, binary, not_utf8) rejects with an `ApiError`.
 */
export function getFileText(agent: string, path: string, opts: { preview?: boolean } = {}): Promise<FileText> {
  return unwrap(api.GET("/v1/agents/{agent}/files/text", { params: { path: { agent }, query: { path, ...(opts.preview ? { preview: "1" as const } : {}) } } }));
}

/** The 10 MiB cap of the image preview (C-905). */
export const IMAGE_PREVIEW_MAX = 10 * 1024 * 1024;

/**
 * The raw bytes of a file for the image preview (`GET …/files/content`): refuses before reading the body when the
 * advertised length is over `max` (`null`), rejects with an `ApiError` for an error body. The bytes are only ever
 * sniffed and shown through a `data:` URL, never as HTML (SEC-126).
 */
export async function fetchContent(agent: string, path: string, max: number = IMAGE_PREVIEW_MAX): Promise<Uint8Array | null> {
  const res = await fetch(downloadUrl(agent, path), { credentials: "same-origin" });
  if (!res.ok) {
    let parsed: unknown = null;
    try {
      parsed = await res.json();
    } catch {
      parsed = null;
    }
    throw ApiError.fromBody(parsed, res.status);
  }
  const advertised = Number(res.headers.get("content-length") ?? "0");
  if (advertised > max) {
    await res.body?.cancel();
    return null;
  }
  const bytes = new Uint8Array(await res.arrayBuffer());
  return bytes.length > max ? null : bytes;
}

export function fileOp(agent: string, body: FileOp): Promise<FileOpResult> {
  return unwrap(api.POST("/v1/agents/{agent}/files/ops", { params: { path: { agent } }, body }));
}

export const mkdirOp = (agent: string, path: string, parents = false) => fileOp(agent, { op: "mkdir", path, parents });
export const renameOp = (agent: string, path: string, to: string) => fileOp(agent, { op: "rename", path, to });
export const deleteOp = (agent: string, path: string, recursive: boolean) => fileOp(agent, { op: "delete", path, recursive });

/** Thrown (as a rejection) by `putFile` when its signal aborts. */
export class UploadAborted extends Error {
  constructor() {
    super("Upload cancelled");
    this.name = "UploadAborted";
  }
}

export interface PutFileOptions {
  /** `true` replaces an existing file; otherwise the request carries `If-None-Match: *` (create only, 409 `file_exists`). */
  replace?: boolean;
  /** Extra preconditions for the editor (P5-FILES-3): `If-Match` with an etag. */
  ifMatch?: string;
  onProgress?: (sent: number, total: number) => void;
  signal?: AbortSignal;
}

/**
 * `PUT …/files/content` with a raw body and upload progress (fetch has none): an XHR with the dashboard's `traceparent`
 * (`14` API client). `Content-Length` is the browser's, from the body. Rejects with `ApiError` for an error body,
 * `UploadAborted` on cancel.
 */
export async function putFile(agent: string, path: string, body: Blob | string, opts: PutFileOptions = {}): Promise<FileWriteResult> {
  const url = `/v1/agents/${encodeURIComponent(agent)}/files/content?path=${encodeURIComponent(path)}`;
  const trace = await traceRequest("PUT", "/v1/agents/{agent}/files/content");
  return new Promise<FileWriteResult>((resolve, reject) => {
    if (opts.signal?.aborted) {
      trace?.finish();
      reject(new UploadAborted());
      return;
    }
    const xhr = new XMLHttpRequest();
    xhr.open("PUT", url);
    xhr.withCredentials = true;
    xhr.setRequestHeader("Content-Type", "application/octet-stream");
    if (opts.ifMatch) xhr.setRequestHeader("If-Match", opts.ifMatch);
    else if (!opts.replace) xhr.setRequestHeader("If-None-Match", "*");
    if (trace) xhr.setRequestHeader("traceparent", trace.traceparent);
    xhr.upload.onprogress = (e) => {
      if (e.lengthComputable) opts.onProgress?.(e.loaded, e.total);
    };
    const onAbort = () => xhr.abort();
    opts.signal?.addEventListener("abort", onAbort, { once: true });
    const done = (status?: number) => {
      opts.signal?.removeEventListener("abort", onAbort);
      trace?.finish(status);
    };
    xhr.onload = () => {
      done(xhr.status);
      let parsed: unknown = null;
      try {
        parsed = xhr.responseText ? JSON.parse(xhr.responseText) : null;
      } catch {
        parsed = null;
      }
      if (xhr.status >= 200 && xhr.status < 300) resolve(parsed as FileWriteResult);
      else reject(ApiError.fromBody(parsed, xhr.status));
    };
    xhr.onerror = () => {
      done();
      reject(new ApiError("network_error", "The upload could not reach the server.", null, 0));
    };
    xhr.onabort = () => {
      done();
      reject(new UploadAborted());
    };
    xhr.send(body);
  });
}
