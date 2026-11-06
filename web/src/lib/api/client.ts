import createClient from "openapi-fetch";

import type { paths } from "./schema";

export const api = createClient<paths>({
  // Same-origin; an absolute base keeps `Request` construction valid outside browsers (tests).
  baseUrl: typeof window === "undefined" ? "" : window.location.origin,
  credentials: "same-origin",
  // Resolve lazily so tests can stub `fetch`.
  fetch: (req) => globalThis.fetch(req),
});

export interface ApiErrorEnvelope {
  error: { code: string; message: string; details?: unknown };
}

export class ApiError extends Error {
  readonly code: string;
  readonly details: unknown;
  readonly status?: number;

  constructor(code: string, message: string, details?: unknown, status?: number) {
    super(message);
    this.name = "ApiError";
    this.code = code;
    this.details = details ?? null;
    this.status = status;
  }

  /** Parse a `{error:{code,message,details}}` body; falls back to a generic error. */
  static fromBody(body: unknown, status?: number): ApiError {
    if (typeof body === "object" && body !== null && "error" in body) {
      const e = (body as { error: unknown }).error;
      if (typeof e === "object" && e !== null) {
        const { code, message, details } = e as Record<string, unknown>;
        if (typeof code === "string" && typeof message === "string") {
          return new ApiError(code, message, details, status);
        }
      }
    }
    return new ApiError("unknown", "Unexpected error response", body, status);
  }
}
