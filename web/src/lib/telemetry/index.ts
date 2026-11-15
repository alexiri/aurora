// Browser tracing entry (FIX-146 Part E, D-232 (i)). This file is the only part in the main bundle; it is
// tiny and does nothing until `configureTelemetry` receives the sample ratio the chart renders as
// `AURORA_TRACING_SAMPLE_RATIO`. The tracer itself (./tracer) is a dynamic import, so without the value no
// telemetry code is loaded and nothing is sent.

import type { Span, SpanContext } from "./tracer";

type Tracer = typeof import("./tracer");

let ratio: number | null = null;
let loaded: Promise<Tracer> | null = null;
let action: SpanContext | null = null;
let errorsHooked = false;

/** Parse the env value: a finite number in (0, 1]; anything else (absent, 0, garbage) means tracing is off. */
export function parseRatio(raw: string | undefined | null): number | null {
  if (raw === undefined || raw === null || raw.trim() === "") return null;
  const n = Number(raw);
  return Number.isFinite(n) && n > 0 && n <= 1 ? n : null;
}

export function configureTelemetry(r: number | null): void {
  ratio = r;
  if (r !== null && typeof window !== "undefined" && !errorsHooked) {
    errorsHooked = true;
    window.addEventListener("error", (e) => recordError(e.error ?? e.message));
    window.addEventListener("unhandledrejection", (e) => recordError(e.reason));
  }
}

export function tracingEnabled(): boolean {
  return ratio !== null;
}

function load(): Promise<Tracer> {
  loaded ??= import("./tracer").then((m) => {
    m.configure(ratio ?? 0);
    return m;
  });
  return loaded;
}

/** Test hook. */
export function resetTelemetryForTests(): void {
  ratio = null;
  loaded = null;
  action = null;
}

/** An id-free route template (`/agents/[agent]/chat`) from a pathname and the route's params. */
export function routeTemplate(pathname: string, params: Record<string, string | string[] | undefined>): string {
  let out = pathname;
  for (const [name, value] of Object.entries(params)) {
    if (value === undefined) continue;
    const raw = Array.isArray(value) ? value.join("/") : value;
    if (!raw) continue;
    // Params may arrive percent-decoded or not; replace whole segments only.
    for (const v of new Set([raw, encodeURIComponent(raw)])) {
      const re = new RegExp(`(^|/)${v.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(?=/|$)`);
      out = out.replace(re, `$1[${Array.isArray(value) ? "..." : ""}${name}]`);
    }
  }
  return out || "/";
}

/** Run `fn` inside an `action <name>` span: the root that makes "click → API → turn → gateway" one trace. */
export async function traceAction<T>(name: string, fn: () => Promise<T>): Promise<T> {
  if (ratio === null) return fn();
  const t = await load();
  const span = t.startSpan(`action ${name}`);
  const previous = action;
  action = span.context;
  try {
    return await fn();
  } catch (e) {
    span.setAttribute("aurora.error_code", errorCode(e));
    throw e;
  } finally {
    action = previous;
    span.end();
  }
}

function errorCode(e: unknown): string | undefined {
  const code = typeof e === "object" && e !== null ? (e as { code?: unknown }).code : undefined;
  return typeof code === "string" && /^[a-z][a-z0-9_.-]{0,63}$/.test(code) ? code : undefined;
}

export interface RequestTrace {
  traceparent: string;
  finish(status?: number): void;
}

/** One API call: a `{METHOD} {schemaPath}` span (a child of the running action, else a new root) and the `traceparent` to send. */
export async function traceRequest(method: string, schemaPath: string): Promise<RequestTrace | null> {
  if (ratio === null) return null;
  const t = await load();
  const span = t.startSpan(`${method.toUpperCase()} ${schemaPath}`, action ?? undefined);
  span.setAttribute("http.request.method", method.toUpperCase());
  span.setAttribute("http.route", schemaPath);
  return {
    traceparent: t.traceparent(span.context),
    finish(status) {
      span.setAttribute("http.response.status_code", status);
      span.end();
    },
  };
}

/** One transcript stream (re)connect; `end(true)` when it opened, `end(false)` when it did not. */
export function traceStreamConnect(template: string): { end(opened: boolean): void } {
  if (ratio === null) return { end() {} };
  let span: Span | undefined;
  const started = typeof performance !== "undefined" ? performance.timeOrigin + performance.now() : Date.now();
  const ready = load().then((t) => {
    span = t.startSpan(`stream connect ${template}`, action ?? undefined, started);
  });
  return {
    end(opened) {
      void ready.then(() => {
        if (!opened) span?.setAttribute("aurora.error_code", "stream_failed");
        span?.end();
      });
    },
  };
}

/** A `navigate <template>` span from `startMs` (the click, else the render) to now (the first commit). */
export function traceNavigation(template: string, startMs?: number): void {
  if (ratio === null) return;
  void load().then((t) => {
    const s = t.startSpan(`navigate ${template}`, undefined, startMs);
    s.setAttribute("aurora.page", template);
    s.end();
  });
}

/** Stack frames only: never the message, no query strings or fragments. */
export function sanitizeStack(stack: string | undefined): string {
  if (!stack) return "";
  return stack
    .split("\n")
    .filter((l) => /^\s*at\s/.test(l) || /@.+:\d+:\d+\s*$/.test(l))
    .map((l) => l.replace(/\?[^\s):]*/g, "").replace(/#[^\s):]*/g, ""))
    .join("\n");
}

/** An unhandled error, rejection or error-boundary catch as a `browser.error` span (no message). */
export function recordError(err: unknown): void {
  if (ratio === null) return;
  const e = err instanceof Error ? err : undefined;
  void load().then((t) => {
    const s = t.startSpan("browser.error", action ?? undefined);
    s.setAttribute("exception.type", e?.name && /^[A-Za-z_$][\w$.]*$/.test(e.name) ? e.name : "Error");
    s.setAttribute("exception.stacktrace", sanitizeStack(e?.stack));
    s.setAttribute("aurora.error_code", errorCode(err));
    s.end();
  });
}
