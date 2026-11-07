import { describe, expect, it, vi } from "vitest";

import { MAX_RETRIES, retryDelayMs, retryingFetch, type RateNotice } from "@/lib/api/rate-limit";

const limited = (retryAfter = "2", code = "rate_limited") =>
  new Response(JSON.stringify({ error: { code, message: "x" } }), {
    status: 429,
    headers: { "retry-after": retryAfter },
  });
const ok = () => new Response("{}", { status: 200 });

function deps(responses: Response[]) {
  const calls: string[] = [];
  const notices: RateNotice[] = [];
  const sleeps: number[] = [];
  return {
    calls,
    notices,
    sleeps,
    d: {
      fetchImpl: vi.fn(async (r: Request) => {
        calls.push(r.method);
        return responses.shift() ?? ok();
      }),
      sleep: async (ms: number) => void sleeps.push(ms),
      random: () => 0.5,
      notice: (n: RateNotice) => void notices.push(n),
    },
  };
}

describe("429 handling (RFE-017)", () => {
  it("retries idempotent reads after retry-after and shows a notice", async () => {
    const t = deps([limited("2"), ok()]);
    const res = await retryingFetch(new Request("http://x/v1/me"), t.d);
    expect(res.status).toBe(200);
    expect(t.calls).toEqual(["GET", "GET"]);
    expect(t.sleeps).toEqual([2000]);
    expect(t.notices[0]).toEqual({ kind: "retrying", seconds: 2 });
    expect(t.notices.at(-1)).toBeNull();
  });

  it("never auto-retries writes", async () => {
    const t = deps([limited("1"), ok()]);
    const res = await retryingFetch(new Request("http://x/v1/agents", { method: "POST", body: "{}" }), t.d);
    expect(res.status).toBe(429);
    expect(t.calls).toEqual(["POST"]);
    expect(t.sleeps).toEqual([]);
  });

  it("does not retry too_many_streams and says so", async () => {
    const t = deps([limited("1", "too_many_streams"), ok()]);
    const res = await retryingFetch(new Request("http://x/v1/conversations/1/events?wait=5"), t.d);
    expect(res.status).toBe(429);
    expect(t.calls).toHaveLength(1);
    expect(t.notices).toEqual([{ kind: "streams" }]);
  });

  it("gives up after the retry budget", async () => {
    const t = deps(Array.from({ length: 10 }, () => limited("1")));
    const res = await retryingFetch(new Request("http://x/v1/me"), t.d);
    expect(res.status).toBe(429);
    expect(t.calls).toHaveLength(MAX_RETRIES + 1);
  });

  it("caps and jitters the wait", () => {
    expect(retryDelayMs("600", () => 1)).toBe(30_000);
    expect(retryDelayMs("10", () => 0)).toBe(8000);
    expect(retryDelayMs("10", () => 1)).toBe(12_000);
    expect(retryDelayMs(null, () => 0.5)).toBe(1000);
  });
});
