import { vi } from "vitest";

/** A JSON `Response`; a `null` body sends no body at all (204-style). */
export const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

/** The API's error envelope. */
export const err = (status: number, code: string, message = code) => json(status, { error: { code, message } });

export type Handler = (req: Request) => Response | Promise<Response>;

/** Stubs `fetch`, answering `METHOD /path` from `routes` and 404 for the rest. */
export function mockFetch(routes: Record<string, Handler>) {
  const fn = vi.fn(async (input: Request | string | URL) => {
    const req = input as Request;
    const url = new URL(req.url, "http://localhost");
    const h = routes[`${req.method} ${url.pathname}`];
    return h ? h(req) : json(404, { error: { code: "not_found", message: "nf" } });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}
