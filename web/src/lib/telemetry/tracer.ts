// Browser tracing (FIX-146 Part E, D-232 (i)). Loaded only by dynamic import from ./index when
// `AURORA_TRACING_SAMPLE_RATIO` reached the page, so a deployment without tracing ships and runs none of it.
//
// A deliberately small tracer instead of the OpenTelemetry web SDK: what it needs is a trace/span id, the
// rule-2 sampler (which the SDK's TraceIdRatioBased differs from), a buffer and one same-origin POST of OTLP
// JSON to `/v1/telemetry/traces`. No new npm dependency, no zone.js, a few KB.

import { ruleSampled, sampleThreshold } from "./sampling";

export interface SpanContext {
  traceId: string;
  spanId: string;
  /** The rule-2 decision for this trace (also what `traceparent` flags say). */
  sampled: boolean;
}

type AttrValue = string | number;

export interface Span {
  readonly context: SpanContext;
  setAttribute(key: string, value: AttrValue | undefined): void;
  end(endMs?: number): void;
}

interface Finished {
  traceId: string;
  spanId: string;
  parentSpanId?: string;
  name: string;
  startMs: number;
  endMs: number;
  attributes: Record<string, AttrValue>;
  error: boolean;
}

export const TRACES_PATH = "/v1/telemetry/traces";
const FLUSH_MS = 5000;
const FLUSH_AT = 50;
// Stay under the server's 512 spans per request.
const MAX_BATCH = 200;

let threshold = 0n;
let buffer: Finished[] = [];
let timer: ReturnType<typeof setTimeout> | undefined;
let off = false;
let fetchImpl: typeof fetch | undefined;

function hex(bytes: number): string {
  const a = new Uint8Array(bytes);
  crypto.getRandomValues(a);
  // An all-zero id is invalid in W3C trace context.
  if (a.every((b) => b === 0)) a[bytes - 1] = 1;
  return Array.from(a, (b) => b.toString(16).padStart(2, "0")).join("");
}

export function configure(ratio: number, opts: { fetch?: typeof fetch } = {}): void {
  threshold = sampleThreshold(ratio);
  fetchImpl = opts.fetch;
  off = false;
  buffer = [];
  if (typeof document !== "undefined") {
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "hidden") void flush();
    });
  }
}

/** Start a span. Without `parent` it is the root of a new trace (its sampling is decided here, by rule 2). */
export function startSpan(name: string, parent?: SpanContext, startMs: number = nowMs()): Span {
  const traceId = parent?.traceId ?? hex(16);
  const context: SpanContext = {
    traceId,
    spanId: hex(8),
    sampled: parent ? parent.sampled : ruleSampled(traceId, threshold),
  };
  const attributes: Record<string, AttrValue> = {};
  let ended = false;
  return {
    context,
    setAttribute(key, value) {
      if (value !== undefined) attributes[key] = value;
    },
    end(endMs = nowMs()) {
      if (ended) return;
      ended = true;
      if (!context.sampled || off) return;
      buffer.push({
        traceId,
        spanId: context.spanId,
        parentSpanId: parent?.spanId,
        name,
        startMs,
        endMs: Math.max(endMs, startMs),
        attributes,
        error: name === "browser.error",
      });
      if (buffer.length >= FLUSH_AT) void flush();
      else timer ??= setTimeout(() => void flush(), FLUSH_MS);
    },
  };
}

export function nowMs(): number {
  return typeof performance !== "undefined" && performance.timeOrigin
    ? performance.timeOrigin + performance.now()
    : Date.now();
}

/** W3C `traceparent` for a context: the sampled flag is the rule-2 result (the server decides again). */
export function traceparent(c: SpanContext): string {
  return `00-${c.traceId}-${c.spanId}-${c.sampled ? "01" : "00"}`;
}

const nanos = (ms: number): string => (BigInt(Math.round(ms * 1000)) * 1000n).toString();

function attr(key: string, value: AttrValue) {
  return typeof value === "number"
    ? { key, value: { intValue: String(Math.trunc(value)) } }
    : { key, value: { stringValue: value } };
}

/** The OTLP/HTTP JSON body of `spans` (what `POST /v1/telemetry/traces` takes). */
export function otlpBody(spans: Finished[]) {
  return {
    resourceSpans: [
      {
        resource: { attributes: [attr("service.name", "aurora-web")] },
        scopeSpans: [
          {
            scope: { name: "aurora-web" },
            spans: spans.map((s) => ({
              traceId: s.traceId,
              spanId: s.spanId,
              ...(s.parentSpanId ? { parentSpanId: s.parentSpanId } : {}),
              name: s.name,
              kind: 1,
              startTimeUnixNano: nanos(s.startMs),
              endTimeUnixNano: nanos(s.endMs),
              attributes: Object.entries(s.attributes).map(([k, v]) => attr(k, v)),
              ...(s.error ? { status: { code: 2 } } : {}),
            })),
          },
        ],
      },
    ],
  };
}

/** Send what is buffered. A failure drops the batch (telemetry is best effort); a 404 (the deployment exports nothing) or a 401 stops tracing for this page. */
export async function flush(): Promise<void> {
  if (timer) clearTimeout(timer);
  timer = undefined;
  while (buffer.length > 0 && !off) {
    const batch = buffer.splice(0, MAX_BATCH);
    try {
      const f = fetchImpl ?? fetch;
      const res = await f(TRACES_PATH, {
        method: "POST",
        headers: { "content-type": "application/json" },
        credentials: "same-origin",
        keepalive: true,
        body: JSON.stringify(otlpBody(batch)),
      });
      if (res.status === 404 || res.status === 401 || res.status === 403) {
        off = true;
        buffer = [];
      }
    } catch {
      // dropped
    }
  }
}

/** Test hook: what is waiting to be sent. */
export function pending(): readonly Finished[] {
  return buffer;
}
