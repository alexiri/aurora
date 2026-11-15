import createClient, { type Middleware } from "openapi-fetch";

import { traceRequest, type RequestTrace } from "@/lib/telemetry";

import { retryingFetch } from "./rate-limit";
import type { paths } from "./schema";

/**
 * (FIX-146, C-732 (1)) A failed response's `x-request-id` is copied into the error envelope as
 * `error.request_id` (client side only; the server's body has none), so `ApiError.requestId` can show it as
 * the "Reference" on error states without every call site passing the response along.
 */
export async function withRequestId(res: Response): Promise<Response> {
  const id = res.headers.get("x-request-id");
  if (res.ok || !id || !(res.headers.get("content-type") ?? "").includes("json")) return res;
  try {
    const body: unknown = await res.clone().json();
    if (typeof body !== "object" || body === null || typeof (body as { error?: unknown }).error !== "object") return res;
    const err = (body as { error: Record<string, unknown> }).error;
    err.request_id = id;
    return new Response(JSON.stringify(body), { status: res.status, statusText: res.statusText, headers: res.headers });
  } catch {
    return res;
  }
}

export const api = createClient<paths>({
  // Same-origin; an absolute base keeps `Request` construction valid outside browsers (tests).
  baseUrl: typeof window === "undefined" ? "" : window.location.origin,
  credentials: "same-origin",
  // Resolve lazily so tests can stub `fetch`.
  // 429 is retried for idempotent reads only (RFE-017).
  fetch: (req) => retryingFetch(req).then(withRequestId),
});

export interface ApiErrorEnvelope {
  error: { code: string; message: string; details?: unknown; request_id?: string };
}

export class ApiError extends Error {
  readonly code: string;
  readonly details: unknown;
  readonly status?: number;
  /** (FIX-146, C-732 (1)) The response's `x-request-id`: shown as the "Reference" on error states. */
  readonly requestId?: string;

  constructor(code: string, message: string, details?: unknown, status?: number, requestId?: string) {
    super(message);
    this.name = "ApiError";
    this.code = code;
    this.details = details ?? null;
    this.status = status;
    this.requestId = requestId;
  }

  /** Parse a `{error:{code,message,details}}` body; falls back to a generic error. */
  static fromBody(body: unknown, status?: number): ApiError {
    if (typeof body === "object" && body !== null && "error" in body) {
      const e = (body as { error: unknown }).error;
      if (typeof e === "object" && e !== null) {
        const { code, message, details, request_id } = e as Record<string, unknown>;
        if (typeof code === "string" && typeof message === "string") {
          return new ApiError(code, message, details, status, typeof request_id === "string" ? request_id : undefined);
        }
      }
    }
    return new ApiError("unknown", "Unexpected error response", body, status);
  }
}

// (FIX-146, D-232 (i)) One span per same-origin `/v1` call, named by the route template, with `traceparent`
// injected. A no-op (no code loaded) unless the dashboard has tracing enabled.
const inflight = new WeakMap<Request, RequestTrace>();
const tracing: Middleware = {
  async onRequest({ request, schemaPath }) {
    const url = new URL(request.url);
    if (typeof window !== "undefined" && url.origin !== window.location.origin) return undefined;
    if (!url.pathname.startsWith("/v1/")) return undefined;
    const t = await traceRequest(request.method, schemaPath);
    if (!t) return undefined;
    request.headers.set("traceparent", t.traceparent);
    inflight.set(request, t);
    return request;
  },
  onResponse({ request, response }) {
    inflight.get(request)?.finish(response.status);
    inflight.delete(request);
    return undefined;
  },
  onError({ request }) {
    inflight.get(request)?.finish();
    inflight.delete(request);
    return undefined;
  },
};
api.use(tracing);

/**
 * `fetch` for the few raw same-origin calls (uploads, streams re-asks): the same span and `traceparent`
 * as the typed client. Pass the route template, never the concrete path.
 */
export async function tracedFetch(template: string, input: RequestInfo | URL, init: RequestInit = {}): Promise<Response> {
  const method = init.method ?? (input instanceof Request ? input.method : "GET");
  const t = await traceRequest(method, template);
  if (!t) return fetch(input, init);
  const headers = new Headers(init.headers ?? (input instanceof Request ? input.headers : undefined));
  headers.set("traceparent", t.traceparent);
  try {
    const res = await fetch(input, { ...init, headers });
    t.finish(res.status);
    return res;
  } catch (e) {
    t.finish();
    throw e;
  }
}
