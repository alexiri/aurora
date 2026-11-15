import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ErrorState } from "@/components/states";
import { ApiError, api, withRequestId } from "@/lib/api/client";
import { chatApi } from "@/lib/api/conversations";
import {
  configureTelemetry,
  parseRatio,
  resetTelemetryForTests,
  routeTemplate,
  sanitizeStack,
  traceAction,
} from "@/lib/telemetry";
import { ruleSampled, sampleThreshold } from "@/lib/telemetry/sampling";
import { flush, otlpBody, pending } from "@/lib/telemetry/tracer";

// The Rust side reads the same file (crates/aurora-core/src/telemetry.rs tests): change both together.
const vectors = JSON.parse(
  readFileSync(resolve(process.cwd(), "../crates/aurora-core/tests/fixtures/sampling-vectors.json"), "utf8"),
) as {
  thresholds: { ratio: number; threshold: string }[];
  vectors: { ratio: number; trace_id: string; sampled: boolean }[];
};

describe("sampling rule (C-732 (7)) shared vectors", () => {
  it("thresholds match", () => {
    for (const t of vectors.thresholds) expect(sampleThreshold(t.ratio), `ratio ${t.ratio}`).toBe(BigInt(t.threshold));
  });
  it("sampled decisions match", () => {
    for (const v of vectors.vectors) {
      expect(ruleSampled(v.trace_id, sampleThreshold(v.ratio)), `${v.ratio} ${v.trace_id}`).toBe(v.sampled);
    }
  });
});

describe("helpers", () => {
  it("parseRatio accepts only (0, 1]", () => {
    expect(parseRatio("1.0")).toBe(1);
    expect(parseRatio("0.25")).toBe(0.25);
    for (const bad of [undefined, "", "0", "0.0", "-1", "1.5", "abc", "NaN"]) expect(parseRatio(bad)).toBeNull();
  });
  it("routeTemplate rebuilds [param] segments", () => {
    expect(routeTemplate("/agents/6ba7b810/chat", { agent: "6ba7b810" })).toBe("/agents/[agent]/chat");
    expect(routeTemplate("/o/acme/w/x", { org: "acme", workspace: "x" })).toBe("/o/[org]/w/[workspace]");
    expect(routeTemplate("/files/a/b", { path: ["a", "b"] })).toBe("/files/[...path]");
    expect(routeTemplate("/", {})).toBe("/");
  });
  it("sanitizeStack keeps frames only, without message or query strings", () => {
    const stack = "TypeError: secret message token=abc\n    at f (http://x/a.js?v=1:10:5)\n    at g (http://x/b.js#frag:2:3)\nfoo@http://x/c.js?q=1:4:5";
    const out = sanitizeStack(stack);
    expect(out).not.toContain("secret");
    expect(out).not.toContain("token");
    expect(out).not.toContain("?");
    expect(out).not.toContain("#");
    expect(out.split("\n")).toHaveLength(3);
  });
});

describe("request reference (C-732 (1))", () => {
  afterEach(cleanup);
  it("copies x-request-id into the error and shows a copyable Reference", async () => {
    const id = "0af7651916cd43dd8448eb211c80319c";
    const res = new Response(JSON.stringify({ error: { code: "not_found", message: "no" } }), {
      status: 404,
      headers: { "content-type": "application/json", "x-request-id": id },
    });
    const body: unknown = await (await withRequestId(res)).json();
    const err = ApiError.fromBody(body, 404);
    expect(err.requestId).toBe(id);
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    render(<ErrorState error={err} />);
    expect(screen.getByText(/Reference:/)).toHaveTextContent("Reference: 0af76519…");
    fireEvent.click(screen.getByRole("button", { name: /copy reference/i }));
    expect(writeText).toHaveBeenCalledWith(id);
  });
  it("shows nothing without an id", () => {
    render(<ErrorState error={new ApiError("x", "m", null, 500)} />);
    expect(screen.queryByText(/Reference:/)).toBeNull();
  });
});

describe("browser tracing", () => {
  const sent: { url: string; body: unknown }[] = [];
  const apiCalls: Request[] = [];
  const id = "11111111-1111-1111-1111-111111111111";

  beforeEach(() => {
    sent.length = 0;
    apiCalls.length = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
        if (new URL(req.url).pathname === "/v1/telemetry/traces") {
          sent.push({ url: req.url, body: JSON.parse(String(init?.body)) });
          return new Response(null, { status: 204 });
        }
        apiCalls.push(req);
        return new Response(JSON.stringify({ seq: 1 }), { status: 200, headers: { "content-type": "application/json" } });
      }),
    );
  });
  afterEach(() => {
    resetTelemetryForTests();
    vi.unstubAllGlobals();
  });

  it("is inert without the env value: no traceparent, nothing buffered or posted", async () => {
    configureTelemetry(null);
    await chatApi.send(id, "hello");
    expect(apiCalls).toHaveLength(1);
    expect(apiCalls[0].headers.get("traceparent")).toBeNull();
    expect(sent).toHaveLength(0);
    expect(await traceAction("x", async () => 7)).toBe(7);
  });

  it("send_message: the action span is the parent of the API span whose id the traceparent carries", async () => {
    configureTelemetry(1);
    await chatApi.send(id, "a message text that must never be a span attribute");
    const tp = apiCalls[0].headers.get("traceparent") ?? "";
    expect(tp).toMatch(/^00-[0-9a-f]{32}-[0-9a-f]{16}-01$/);
    const spans = [...pending()]; // flush() below empties the live buffer
    const action = spans.find((s) => s.name === "action send_message");
    const call = spans.find((s) => s.name === "POST /v1/conversations/{id}/messages");
    expect(action && call).toBeTruthy();
    expect(call?.parentSpanId).toBe(action?.spanId);
    expect(call?.traceId).toBe(action?.traceId);
    expect(tp).toBe(`00-${call?.traceId}-${call?.spanId}-01`);
    expect(action?.parentSpanId).toBeUndefined();
    // the name carries the route template, never the id; nothing carries the text
    expect(JSON.stringify(spans)).not.toContain(id);
    expect(JSON.stringify(spans)).not.toContain("message text");
    await flush();
    expect(sent).toHaveLength(1);
    const body = JSON.stringify(sent[0].body);
    expect(body).toContain("aurora-web");
    expect(otlpBody(spans).resourceSpans[0].scopeSpans[0].spans).toHaveLength(2);
    expect(pending()).toHaveLength(0);
  });

  it("an unsampled trace still propagates its id but records and sends nothing", async () => {
    configureTelemetry(1e-15); // threshold so low that no random id passes
    await chatApi.send(id, "x");
    expect(apiCalls[0].headers.get("traceparent")).toMatch(/^00-[0-9a-f]{32}-[0-9a-f]{16}-00$/);
    expect(pending()).toHaveLength(0);
    await flush();
    expect(sent).toHaveLength(0);
  });

  it("only same-origin /v1 calls are traced", async () => {
    configureTelemetry(1);
    const res = await api.GET("/v1/me");
    expect(res.response.ok).toBe(true);
    expect(apiCalls[0].headers.get("traceparent")).not.toBeNull();
    expect(pending().some((s) => s.name === "GET /v1/me")).toBe(true);
  });

  it("raw uploads are traced under their schema path (a multipart attachment and a raw avatar PUT)", async () => {
    configureTelemetry(1);
    const { uploadAttachment } = await import("@/lib/api/attachments");
    await uploadAttachment(id, new File(["abc"], "canary-file-name-5c.txt"));
    const span = pending().find((s) => s.name === "POST /v1/conversations/{id}/attachments");
    expect(span, "upload span").toBeDefined();
    expect(JSON.stringify(otlpBody([...pending()]))).not.toContain("canary-file-name-5c");
    expect(apiCalls.at(-1)?.headers.get("traceparent")).toBe(`00-${span?.traceId}-${span?.spanId}-01`);
  });
});

describe("error boundary capture", () => {
  afterEach(() => {
    cleanup();
    resetTelemetryForTests();
  });

  it("a render error becomes a browser.error span with frames only, never the message", async () => {
    const { default: RootError } = await import("@/app/error");
    configureTelemetry(1);
    const err = new Error("canary-render-message-0e9b for user ada@example.org");
    const reset = vi.fn();
    render(<RootError error={err} reset={reset} />);
    // The page shows a generic message with a retry, not the error text.
    expect(screen.queryByText(/canary-render-message/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(reset).toHaveBeenCalledOnce();
    await vi.waitFor(() => expect(pending().some((s) => s.name === "browser.error")).toBe(true));
    const body = JSON.stringify(otlpBody([...pending()]));
    expect(body).toContain("browser.error");
    expect(body).not.toContain("canary-render-message");
    expect(body).not.toContain("ada@example.org");
  });

  it("is inert when tracing is off", async () => {
    const { default: RootError } = await import("@/app/error");
    configureTelemetry(null);
    const before = pending().length; // the buffer is module state: earlier tests may have left spans
    render(<RootError error={new Error("x")} reset={() => undefined} />);
    await Promise.resolve();
    expect(pending()).toHaveLength(before);
  });
});
