// RFE-017 (docs/14 §Networking): the generated client treats 429 as retryable for idempotent
// reads; non-idempotent writes are never auto-retried; streams refused with
// `too_many_streams` are not retried either.
import { useSyncExternalStore } from "react";

export const MAX_RETRY_WAIT_MS = 30_000;
export const MAX_RETRIES = 3;

export type RateNotice = { kind: "retrying"; seconds: number } | { kind: "streams" } | null;

let current: RateNotice = null;
const listeners = new Set<() => void>();

export function setRateNotice(n: RateNotice): void {
  current = n;
  listeners.forEach((l) => l());
}

export function useRateNotice(): RateNotice {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => current,
    () => null,
  );
}

/** Wait before retrying: `retry-after` capped at 30 s with ±20 % jitter (never above the cap). */
export function retryDelayMs(retryAfter: string | null, random: () => number = Math.random): number {
  const secs = Number(retryAfter);
  const base = Number.isFinite(secs) && secs > 0 ? secs * 1000 : 1000;
  return Math.min(MAX_RETRY_WAIT_MS, Math.round(Math.min(base, MAX_RETRY_WAIT_MS) * (0.8 + 0.4 * random())));
}

export interface RetryDeps {
  fetchImpl?: (req: Request) => Promise<Response>;
  sleep?: (ms: number) => Promise<void>;
  random?: () => number;
  notice?: (n: RateNotice) => void;
}

export async function retryingFetch(req: Request, deps: RetryDeps = {}): Promise<Response> {
  const f = deps.fetchImpl ?? ((r: Request) => globalThis.fetch(r));
  const sleep = deps.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  const notice = deps.notice ?? setRateNotice;
  const idempotent = req.method === "GET" || req.method === "HEAD";
  if (!idempotent) return f(req);
  for (let attempt = 0; ; attempt++) {
    const res = await f(req.clone());
    if (res.status !== 429) {
      if (attempt > 0) notice(null);
      return res;
    }
    const body = (await res
      .clone()
      .json()
      .catch(() => null)) as { error?: { code?: string } } | null;
    if (body?.error?.code === "too_many_streams") {
      notice({ kind: "streams" });
      return res;
    }
    if (attempt >= MAX_RETRIES) {
      notice(null);
      return res;
    }
    const wait = retryDelayMs(res.headers.get("retry-after"), deps.random);
    notice({ kind: "retrying", seconds: Math.ceil(wait / 1000) });
    await sleep(wait);
  }
}
